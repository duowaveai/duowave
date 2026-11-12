"""Our own speech synthesis behind an OpenAI-compatible HTTP API, for the voice gateway's own voice.

The gateway sends a phrase to POST /v1/audio/speech, the same call it would make to OpenAI, and gets a 16-bit
mono WAV back; it encodes it to Ogg/Opus for the station itself.

    ../asr-server/.venv/bin/python server.py --port 8091   # every model in models/: Silero and Piper voices at once
    python server.py --engine tone --port 8091              # no model: a tone per phrase, to check the plumbing

The request picks the voice: "model" names the engine (silero, piper), "voice" its voice (baya, xenia, denis…), so
the gateway's TTS_OWN_MODEL and TTS_OWN_VOICE choose it and this server stays as it is. GET /v1/models lists what
there is.

Endpoints:
    POST /v1/audio/speech    JSON {"model", "input", "voice", "response_format": "wav" | "pcm"}
    POST /v1/models/rescan   reads the models folder again: voices put there since the start speak, deleted ones are
                             forgotten, no restart. Answers like /health, with what came ("added") and went ("removed")
    GET  /v1/models          the engines and their voices
    GET  /health             engines, voices, requests served

Engines, found in --models (TTS_MODELS or TTS_MODELS_DIR, the folder the gateway downloads into; else
$DATA_DIR/voices when DATA_DIR is set; else models/ next to this file). A voice asked for and not loaded is looked
for in the folder once more before the request fails.
    silero  Silero TTS from a local model file, any *.pt (torch.package), in this process; Russian voices aidar,
            baya, kseniya, xenia, eugene. Needs torch. The models are CC BY-NC 4.0: non-commercial use, with
            attribution.
    piper   Piper (ONNX, quick on a CPU), any *.onnx with its .onnx.json next to it, a voice each, named by its
            file (ru_RU-denis-medium → denis). Piper is GPL-3.0 and never runs in this process: its own command line
            (piper-tts in an environment of its own — --piper, PIPER_BIN, .venv-piper next to this file, or `piper` on
            PATH) speaks in a process per voice that stays up between phrases: the voice loads once (~1 s on an
            M-series Mac), a phrase then takes tens of milliseconds. At most --piper-workers such processes; the voice
            used least recently gives its process back first. Each voice has its own license: denis and dmitri are
            CC0, ruslan CC BY-NC-SA 4.0, irina unknown (02.11).
    tone    No model at all.

Only the standard library besides numpy (and torch for silero): no web framework to install or keep updated.
"""
from __future__ import annotations

import argparse
import collections
import glob
import hmac
import io
import json
import logging
import math
import os
import queue
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
import wave
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Callable
from urllib.parse import urlsplit

import numpy as np

log = logging.getLogger("tts")
HERE = os.path.dirname(os.path.abspath(__file__))
# Piper's command line is Python: its stdin and its log stay UTF-8 whatever the locale (a Russian Windows says cp1251).
UTF8 = {"PYTHONUTF8": "1", "PYTHONIOENCODING": "utf-8"}
PHRASE_TIMEOUT = 60


def stamp(*paths: str) -> tuple:
    """What tells a file changed on disk from the one loaded: size and time of every file the model needs."""
    return tuple((info.st_size, info.st_mtime_ns) for info in map(os.stat, paths))


class ToneEngine:
    """No model: a short tone per phrase, as long as the phrase would roughly take to say."""

    name = "tone"
    voices = ["tone"]

    def synthesize(self, text: str, voice: str) -> tuple[np.ndarray, int]:
        rate = 16000
        seconds = max(0.3, min(10.0, len(text) * 0.06))
        t = np.arange(int(rate * seconds)) / rate
        return (np.sin(2 * math.pi * 440 * t) * 0.2 * 32767).astype("<i2"), rate


class SileroEngine:
    """Silero TTS loaded from a local file (the "standalone" way of its README); no code is fetched at run time."""

    name = "silero"

    def __init__(self, model_path: str, device: str, sample_rate: int) -> None:
        import torch

        if device == "auto":
            device = "cuda" if torch.cuda.is_available() else "cpu"
        self.torch = torch
        self.path = model_path
        self.stamp = stamp(model_path)
        self.file = os.path.basename(model_path)  # v4_ru.pt or v5_ru.pt: which generation is loaded
        self.model = torch.package.PackageImporter(model_path).load_pickle("tts_models", "model")
        self.model.to(torch.device(device))
        self.sample_rate = sample_rate
        self.voices = [voice for voice in getattr(self.model, "speakers", ["aidar", "baya", "kseniya", "xenia", "eugene"]) if voice != "random"]
        self.lock = threading.Lock()

    def synthesize(self, text: str, voice: str) -> tuple[np.ndarray, int]:
        speaker = voice if voice in self.voices else "xenia"
        with self.lock, self.torch.inference_mode():
            audio = self.model.apply_tts(text=text, speaker=speaker, sample_rate=self.sample_rate)
        return (audio.clamp(-1, 1).cpu().numpy() * 32767).astype("<i2"), self.sample_rate


def hint(said: str) -> str:
    # espeak-ng keeps its data path in 160 bytes: from a deeper folder it falls back to its build machine's path.
    if "espeak-ng-data" in said:
        return " (Piper's environment is too deep a folder for espeak-ng: its path must stay under ~150 characters)"
    return ""


class PiperWorker:
    """Piper's command line kept running for one voice: a line of text in, a WAV in its own folder out, and the
    file's name in its log (piper-tts writes «Wrote …» to stderr; the older C++ piper prints the path to stdout)."""

    def __init__(self, binary: str, model: str) -> None:
        self.folder = tempfile.mkdtemp(prefix="piper-")
        try:
            self.process = subprocess.Popen(
                [binary, "--model", model, "--output_dir", self.folder],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                cwd=self.folder,
                env={**os.environ, **UTF8},
            )
        except OSError:
            shutil.rmtree(self.folder, ignore_errors=True)
            raise
        self.written: queue.Queue[bool] = queue.Queue()
        self.said: collections.deque[str] = collections.deque(maxlen=5)  # its last words, for the log when it fails
        for stream in (self.process.stdout, self.process.stderr):
            threading.Thread(target=self._listen, args=(stream,), daemon=True).start()

    def _listen(self, stream) -> None:
        for raw in iter(stream.readline, b""):
            line = raw.decode("utf-8", "replace").strip()
            if line.endswith(".wav"):
                self.written.put(True)
            elif line:
                self.said.append(line)
        self.written.put(False)  # the stream closed: piper is gone

    @property
    def alive(self) -> bool:
        return self.process.poll() is None

    def say(self, line: str) -> tuple[np.ndarray, int]:
        while not self.written.empty():
            self.written.get_nowait()
        self.process.stdin.write(f"{line}\n".encode("utf-8"))
        self.process.stdin.flush()
        deadline = time.monotonic() + PHRASE_TIMEOUT
        names: list[str] = []
        while not names:  # a word about a file that is not there yet is not this phrase's
            try:
                wrote = self.written.get(timeout=max(0.0, deadline - time.monotonic()))
            except queue.Empty:
                raise RuntimeError(f"no answer in {PHRASE_TIMEOUT} s") from None
            if not wrote:
                last = self.said[-1] if self.said else "without a word"
                raise RuntimeError(f"piper exited: {last}{hint(last)}")
            names = sorted(name for name in os.listdir(self.folder) if name.endswith(".wav"))
        try:
            with wave.open(os.path.join(self.folder, names[0]), "rb") as wav:
                if wav.getsampwidth() != 2 or wav.getnchannels() != 1:
                    raise RuntimeError(f"piper wrote {wav.getnchannels()} channels of {8 * wav.getsampwidth()} bits")
                return np.frombuffer(wav.readframes(wav.getnframes()), dtype="<i2"), wav.getframerate()
        finally:
            for name in names:
                os.unlink(os.path.join(self.folder, name))

    def close(self) -> None:
        try:
            self.process.stdin.close()  # the end of its input: piper finishes the line it has and exits
            self.process.wait(timeout=3)
        except (OSError, subprocess.TimeoutExpired):
            self.process.kill()
            self.process.wait()
        shutil.rmtree(self.folder, ignore_errors=True)


class PiperPool:
    """The Piper processes alive: at most `size`, the voice used least recently gives its own back first."""

    def __init__(self, size: int) -> None:
        self.size = size
        self.lock = threading.Lock()
        self.recent: collections.OrderedDict[PiperEngine, None] = collections.OrderedDict()

    def used(self, engine: PiperEngine) -> None:
        with self.lock:
            self.recent[engine] = None
            self.recent.move_to_end(engine)
            extra = [other for other in self.recent if other is not engine][: max(0, len(self.recent) - self.size)]
        for other in extra:
            other.rest()

    def forget(self, engine: PiperEngine) -> None:
        with self.lock:
            self.recent.pop(engine, None)


class PiperEngine:
    """One Piper voice, said by Piper's own command line in a process of its own (see PiperWorker)."""

    name = "piper"

    def __init__(self, binary: str, model: str, pool: PiperPool) -> None:
        with open(f"{model}.json", encoding="utf-8") as file:
            self.sample_rate = int(json.load(file)["audio"]["sample_rate"])
        self.binary = binary
        self.model = model
        self.pool = pool
        self.stamp = stamp(model, f"{model}.json")
        self.full = os.path.basename(model).removesuffix(".onnx")
        parts = self.full.split("-")
        # ru_RU-denis-medium: «denis», and the file's name too.
        self.voices = [parts[1], self.full] if len(parts) == 3 else [self.full]
        self.worker: PiperWorker | None = None
        self.lock = threading.Lock()

    def synthesize(self, text: str, voice: str) -> tuple[np.ndarray, int]:
        line = " ".join(text.split())  # piper reads a line per phrase: a newline inside would make two
        if not any(char.isalnum() for char in line):
            # Nothing to pronounce («…»): piper would write no audio, and its process dies on the empty file.
            return np.zeros(0, dtype="<i2"), self.sample_rate
        with self.lock:
            if self.pool.size > 0:
                try:
                    if self.worker is None or not self.worker.alive:
                        self._stop()
                        self.worker = PiperWorker(self.binary, self.model)
                    self.pool.used(self)
                    return self.worker.say(line)
                except Exception as error:  # noqa: BLE001 - whatever broke it, a fresh piper says this phrase
                    log.warning("%s: piper's process failed (%s); this phrase goes to a piper of its own", self.full, error)
                    self._stop()
            return self._once(line)

    def _once(self, line: str) -> tuple[np.ndarray, int]:
        """A piper for this phrase alone: it loads the voice first (~1 s), and nothing is left behind."""
        result = subprocess.run(
            [self.binary, "--model", self.model, "--output_raw"],
            input=f"{line}\n".encode("utf-8"),
            capture_output=True,
            timeout=PHRASE_TIMEOUT,
            env={**os.environ, **UTF8},
        )
        if result.returncode != 0:
            why = result.stderr.decode("utf-8", "replace").strip().splitlines()
            raise RuntimeError(f"piper failed: {why[-1] if why else result.returncode}{hint(why[-1] if why else '')}")
        audio = result.stdout[: len(result.stdout) // 2 * 2]
        return np.frombuffer(audio, dtype="<i2"), self.sample_rate

    def rest(self) -> None:
        """Gives its process back when it is not speaking; a voice speaking now is left for the next time."""
        if self.lock.acquire(blocking=False):
            try:
                self._stop()
            finally:
                self.lock.release()

    def close(self) -> None:
        with self.lock:
            self._stop()

    def _stop(self) -> None:
        if self.worker is not None:
            self.worker.close()
            self.worker = None
        self.pool.forget(self)


def label(engine: object) -> str:
    return getattr(engine, "file", None) or getattr(engine, "full", None) or engine.name


class Engines:
    """Every engine loaded, and which one says a request: the engine of the asked model that has the asked
    voice, or any engine that has it. A voice nobody has is an error, not another voice in disguise: a quiet
    substitution would speak the wrong person and nobody would notice. With a folder to read (`scan`), the list is
    built again from the files on a rescan, and an engine whose files have not changed stays as it is."""

    def __init__(self, engines: list, scan: Callable[[list], tuple[list, list[str]]] | None = None) -> None:
        self.engines = engines
        self.scan = scan
        self.errors: list[str] = []
        self.lock = threading.Lock()  # one rescan at a time

    def pick(self, model: str, voice: str) -> tuple[object, str]:
        for attempt in range(2):
            engines = self.engines
            named = [engine for engine in engines if engine.name == model]
            for engine in named + engines:
                if voice in engine.voices:
                    return engine, voice
            if attempt or self.scan is None:
                break
            # A voice downloaded a moment ago speaks even when nobody asked for a rescan.
            self.rescan()
        have = ", ".join(f"{engine.name}:{'/'.join(engine.voices)}" for engine in self.engines) or "nothing"
        raise ValueError(f"voice '{voice}' of '{model}' is not loaded (have {have}); is its file in the models folder?")

    def rescan(self, quiet: bool = False) -> dict:
        """Reads the folder again; what came and what went, by file."""
        if self.scan is None:
            return {"added": [], "removed": []}
        with self.lock:
            before = self.engines
            try:
                engines, errors = self.scan(before)
            except Exception as error:  # noqa: BLE001 - an unreadable folder keeps the voices there are
                log.exception("the models folder is not read")
                return {"added": [], "removed": [], "errors": [str(error)]}
            self.engines, self.errors = engines, errors
        came = [engine for engine in engines if not any(engine is old for old in before)]
        gone = [engine for engine in before if not any(engine is new for new in engines)]
        for engine in gone:
            if hasattr(engine, "close"):
                engine.close()
        if (came or gone) and not quiet:
            log.info("models: %s", ", ".join([f"+{label(engine)}" for engine in came] + [f"-{label(engine)}" for engine in gone]))
        return {"added": [label(engine) for engine in came], "removed": [label(engine) for engine in gone]}

    def close(self) -> None:
        for engine in self.engines:
            if hasattr(engine, "close"):
                engine.close()

    def describe(self) -> dict[str, list[str]]:
        voices: dict[str, list[str]] = {}
        for engine in self.engines:
            # A Piper voice also answers to its file's name; the short one is enough here.
            voices.setdefault(engine.name, []).extend(engine.voices[:1] if engine.name == "piper" else engine.voices)
        return voices

    def loaded(self) -> dict:
        """What answered with which model file: the panel compares this with the disk to know a rescan is due."""
        out: dict = {"piper": [], "piperFiles": []} if self.scan is not None else {}
        for engine in self.engines:
            if engine.name == "piper":
                out.setdefault("piper", []).extend(engine.voices[:1])
                # The file's stem too: short names repeat across qualities (amy low and medium are both "amy").
                out.setdefault("piperFiles", []).append(engine.full)
            else:
                out[engine.name] = getattr(engine, "file", engine.voices[0])
        return out


def find_piper(given: str | None) -> str | None:
    """Piper's command line: the one given, else the environment next to this file (.venv-piper), else PATH."""
    if given:
        return shutil.which(given)
    for path in (os.path.join(HERE, ".venv-piper", "bin", "piper"), os.path.join(HERE, ".venv-piper", "Scripts", "piper.exe")):
        if os.path.isfile(path):
            return path
    return shutil.which("piper")


def scanner(folder: str, piper: str | None, pool: PiperPool, device: str, sample_rate: int) -> Callable[[list], tuple[list, list[str]]]:
    """The models folder as engines, keeping every engine whose files are as they were when it loaded."""
    reported: set[str] = set()
    # A Silero file that would not load, by its stamp: tried again only once it changes — a rescan on every request
    # for a missing voice must not spend seconds on the same broken file each time.
    broken: dict[str, tuple[tuple, str]] = {}

    def scan(current: list) -> tuple[list, list[str]]:
        kept = {getattr(engine, "path", None) or getattr(engine, "model", None): engine for engine in current}
        engines: list = []
        errors: list[str] = []
        for path in sorted(glob.glob(os.path.join(folder, "**", "*.pt"), recursive=True)):
            try:
                now = stamp(path)
            except OSError:
                continue  # gone since the listing
            if path in broken and broken[path][0] == now:
                errors.append(broken[path][1])
                continue
            old = kept.get(path)
            try:
                engines.append(old if isinstance(old, SileroEngine) and old.stamp == now else SileroEngine(path, device, sample_rate))
            except ImportError as error:
                errors.append(f"{path}: {error} in this Python, Silero is left out")
            except Exception as error:  # noqa: BLE001 - one broken file must not take every voice down
                errors.append(f"{path}: not loaded ({error}); the other voices still speak")
                broken[path] = (now, errors[-1])
                continue
            break  # one Silero model is all the voices it has
        binary = find_piper(piper)
        for path in sorted(glob.glob(os.path.join(folder, "**", "*.onnx"), recursive=True)):
            if not os.path.exists(f"{path}.json"):
                continue
            if binary is None:
                errors.append(f"{path}: no Piper command line here (piper-tts): --piper or PIPER_BIN")
                continue
            old = kept.get(path)
            try:
                same = isinstance(old, PiperEngine) and old.binary == binary and old.stamp == stamp(path, f"{path}.json")
                engines.append(old if same else PiperEngine(binary, path, pool))
            except Exception as error:  # noqa: BLE001 - as above
                errors.append(f"{path}: not loaded ({error}); the other voices still speak")
        for error in errors:
            if error not in reported:  # each problem once, not at every rescan
                log.warning(error)
        reported.clear()
        reported.update(errors)
        return engines, errors

    return scan


def models_folder() -> str:
    """The voices' folder: the gateway's own setting first (TTS_MODELS_DIR, $DATA_DIR/voices under DATA_DIR), so one
    environment serves both; models/ next to this file otherwise."""
    for name in ("TTS_MODELS", "TTS_MODELS_DIR"):
        if os.environ.get(name, "").strip():
            return os.environ[name].strip()
    data = os.environ.get("DATA_DIR", "").strip()
    return os.path.join(data, "voices") if data else os.path.join(HERE, "models")


def to_wav(pcm: np.ndarray, rate: int) -> bytes:
    buffer = io.BytesIO()
    with wave.open(buffer, "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(rate)
        wav.writeframes(pcm.astype("<i2").tobytes())
    return buffer.getvalue()


class Handler(BaseHTTPRequestHandler):
    engines: Engines
    api_key: str | None = None
    requests = 0

    def do_GET(self) -> None:
        path = urlsplit(self.path).path.rstrip("/")
        if path == "/health":
            # Open without the key: Docker's health check and the gateway's probes ask it.
            return self._json(200, self.state())
        if not self.allowed():
            return
        if path == "/v1/models":
            voices = self.engines.describe()
            return self._json(200, {"data": [{"id": name, "object": "model", "voices": names} for name, names in voices.items()]})
        self._json(404, {"error": "not found"})

    def do_POST(self) -> None:
        path = urlsplit(self.path).path.rstrip("/")
        if not self.allowed():
            return
        if path == "/v1/models/rescan":
            changes = self.engines.rescan()
            return self._json(200, {**self.state(), **changes})
        if path != "/v1/audio/speech":
            return self._json(404, {"error": "not found"})
        try:
            body = json.loads(self.rfile.read(max(0, int(self.headers.get("Content-Length", "0")))) or b"{}")
            text = str(body.get("input", "")).strip()
            if not text:
                return self._json(400, {"error": "input is empty"})
            started = time.monotonic()
            engine, voice = self.engines.pick(str(body.get("model", "")), str(body.get("voice", "")))
            pcm, rate = engine.synthesize(text, voice)
            Handler.requests += 1
            log.info("%s %s: %d chars → %.1f s of audio in %.2f s", engine.name, voice, len(text), len(pcm) / rate, time.monotonic() - started)
            if body.get("response_format") == "pcm":
                return self._send(200, "audio/pcm", pcm.astype("<i2").tobytes(), {"X-Sample-Rate": str(rate)})
            self._send(200, "audio/wav", to_wav(pcm, rate))
        except Exception as error:  # the gateway falls back or says nothing; the reason goes to both logs
            log.exception("synthesis failed")
            self._json(500, {"error": str(error)})

    def state(self) -> dict:
        return {
            "status": "ok",
            "engines": self.engines.describe(),
            "loaded": self.engines.loaded(),
            "requests": Handler.requests,
            # What the gateway may ask beyond OpenAI's API: new voices without a restart.
            "features": ["rescan"],
            "errors": self.engines.errors,
        }

    def allowed(self) -> bool:
        """With a key set (TTS_API_KEY, or the gateway's own TTS_OWN_API_KEY), every call but /health needs it."""
        if not self.api_key:
            return True
        if hmac.compare_digest(self.headers.get("Authorization", "").encode(), f"Bearer {self.api_key}".encode()):
            return True
        self._json(401, {"error": "unauthorized: Authorization: Bearer <TTS_API_KEY>"})
        return False

    def _json(self, status: int, payload: dict) -> None:
        self._send(status, "application/json", json.dumps(payload, ensure_ascii=False).encode("utf-8"))

    def _send(self, status: int, content_type: str, data: bytes, headers: dict | None = None) -> None:
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(data)))
        for name, value in (headers or {}).items():
            self.send_header(name, value)
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, format: str, *args) -> None:  # noqa: A002 - the base class's name
        log.debug(format, *args)


def exit_with_parent(engines: Engines) -> None:
    """Reads stdin until it ends — the app that started the server is gone — then ends the server and its Piper
    processes at once, whatever it was doing."""
    try:
        while sys.stdin.buffer.read(65536):
            pass
    except (OSError, ValueError):
        pass
    log.info("the app is gone: ending")
    try:
        engines.close()
    finally:
        os._exit(0)


def stop(signum: int, frame: object) -> None:
    # Docker stops with SIGTERM, and as PID 1 the server gets no default action for it: end serve_forever instead.
    raise KeyboardInterrupt


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--host", default=os.environ.get("TTS_HOST", "127.0.0.1"),
                        help="0.0.0.0 to answer beyond this computer (the Docker image does); there is no other guard than --api-key")
    parser.add_argument("--port", type=int, default=int(os.environ.get("TTS_PORT", "8091")))
    parser.add_argument("--engine", default=os.environ.get("TTS_ENGINE", "models"), choices=["models", "tone"], help="models: every model found")
    parser.add_argument("--models", default=models_folder(), help="where the models are")
    parser.add_argument("--model", default=os.environ.get("TTS_MODEL", ""), help="just this one: a Silero .pt or a Piper .onnx")
    parser.add_argument("--piper", default=os.environ.get("PIPER_BIN") or None,
                        help="Piper's command line (piper-tts); default: .venv-piper next to this file, else piper on PATH")
    parser.add_argument("--piper-workers", type=int, default=int(os.environ.get("PIPER_WORKERS", "2")),
                        help="Piper processes kept up between phrases, a voice each (~200 MB apiece); 0: one per phrase")
    parser.add_argument("--device", default=os.environ.get("TTS_DEVICE", "auto"), help="silero: auto | cuda | cpu")
    parser.add_argument("--sample-rate", type=int, default=48000, help="silero: 8000 | 24000 | 48000")
    parser.add_argument("--api-key", default=os.environ.get("TTS_API_KEY") or os.environ.get("TTS_OWN_API_KEY") or None,
                        help="require Authorization: Bearer <key> on everything but /health")
    parser.add_argument("--exit-with-parent", action="store_true",
                        help="end when stdin closes: the desktop app starts the server with a pipe, and its end (a crash too) ends the server")
    args = parser.parse_args()
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(name)s: %(message)s")

    pool = PiperPool(max(0, args.piper_workers))
    if args.engine == "tone":
        engines = Engines([ToneEngine()])
    elif args.model:
        # One model given: the engine its file says, and nothing to rescan.
        if args.model.endswith(".pt"):
            engines = Engines([SileroEngine(args.model, args.device, args.sample_rate)])
        else:
            binary = find_piper(args.piper)
            if binary is None:
                parser.error("no Piper command line (piper-tts): --piper or PIPER_BIN")
            engines = Engines([PiperEngine(binary, args.model, pool)])
    else:
        engines = Engines([], scanner(args.models, args.piper, pool, args.device, args.sample_rate))
        engines.rescan(quiet=True)
        if not engines.engines:
            # Up anyway: a voice the gateway downloads later speaks after a rescan (or its first request).
            log.warning("no voices in %s yet: a Silero .pt or a Piper .onnx with its .onnx.json", args.models)
    Handler.engines = engines
    Handler.api_key = args.api_key
    server = ThreadingHTTPServer((args.host, args.port), Handler)
    if args.exit_with_parent:
        threading.Thread(target=exit_with_parent, args=(engines,), name="parent", daemon=True).start()
    signal.signal(signal.SIGTERM, stop)
    voices = "; ".join(f"{name}: {', '.join(names)}" for name, names in engines.describe().items()) or "none yet"
    log.info("voices %s at http://%s:%d/v1", voices, args.host, args.port)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
        engines.close()  # the Piper processes go with the server


if __name__ == "__main__":
    main()
