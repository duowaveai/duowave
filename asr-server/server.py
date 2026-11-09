"""Speech recognition behind an OpenAI-compatible HTTP API: GigaAM, and Parakeet and Whisper beside it.

The voice gateway sends one utterance (16 kHz mono 16-bit WAV) after the end of speech to
POST /v1/audio/transcriptions, the same call it makes to OpenAI, so either can be used.

    python server.py --model v3_e2e_rnnt --device auto --port 8090

Endpoints:
    POST /v1/audio/transcriptions   multipart form (file, model, language, response_format) or a raw WAV body
    POST /v1/models/load            JSON {"model": "v3_ctc"}: loaded beside the serving model, it takes over once it
                                    has passed its warm-up; until then the old one serves, and it stays if the new
                                    one fails. One load at a time (409 otherwise); the loaded model is remembered for
                                    the next start. 200 when it serves; with "wait": false, 202 at once and /health
                                    tells the end ("loading" gone: "model" if it took, "error" if not).
    GET  /v1/models                 the loaded model
    GET  /health                    model, device, requests served, the load under way and the last one's error

The model at start: --model or GIGAAM_MODEL; without them the one last loaded over HTTP (--model-file, by default
chosen-model next to this file), else v3_e2e_rnnt; one that does not load gives way to the default. `none` starts without a
model and waits for a load.

Only the standard library besides gigaam/torch/numpy: no web framework to install or keep updated.
"""
from __future__ import annotations

import argparse
import email.parser
import email.policy
import gc
import hmac
import importlib.util
import io
import json
import logging
import os
import re
import signal
import tempfile
import threading
import time
import wave
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit

import numpy as np
import torch

import gigaam

SAMPLE_RATE = 16000
# GigaAM's short-form transcribe() accepts up to 25 s; the gateway caps utterances well below that.
MAX_SECONDS = 24
DEFAULT_MODEL = "v3_e2e_rnnt"
PARAKEET_REPO = "nvidia/parakeet-tdt-0.6b-v3"
HERE = os.path.dirname(os.path.abspath(__file__))
# A name that comes over the network or from the remembered file is a name, never a path: gigaam.load_model also
# takes a path to a checkpoint, and a checkpoint is a pickle.
SAFE_NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$")

log = logging.getLogger("asr")


class Recognizer:
    def __init__(self, model_name: str, device: str, cache_dir: str | None = None) -> None:
        if device == "auto":
            device = "cuda" if torch.cuda.is_available() else "mps" if torch.backends.mps.is_available() else "cpu"
        self.model_name = model_name
        self.device = device
        started = time.monotonic()
        self.whisper = None
        self.parakeet = None
        # Hugging Face's hub cache (Parakeet, Whisper) beside GigaAM's: one folder to mount as a volume.
        hub = os.path.join(cache_dir, "huggingface", "hub") if cache_dir else None
        if model_name.startswith("parakeet-"):
            # NVIDIA Parakeet TDT v3 through transformers: multilingual, fast on Apple silicon (MPS).
            from transformers import AutoProcessor, ParakeetForTDT

            self.processor = AutoProcessor.from_pretrained(PARAKEET_REPO, cache_dir=hub)
            self.parakeet = ParakeetForTDT.from_pretrained(PARAKEET_REPO, cache_dir=hub).to(device).eval()
            self.model = None
        elif model_name.startswith("whisper-"):
            # faster-whisper (CTranslate2): tiny…large-v3-turbo, downloaded from HuggingFace on first use.
            from faster_whisper import WhisperModel

            size = model_name.removeprefix("whisper-")
            # CTranslate2 has no MPS: on Apple silicon it runs on CPU, and int8 keeps it fast there.
            self.whisper = WhisperModel(size, device="cpu", compute_type="int8", download_root=hub)
            self.model = None
        else:
            root = os.path.join(cache_dir, "gigaam") if cache_dir else None
            self.model = gigaam.load_model(model_name, device=device, download_root=root)
        self.lock = threading.Lock()  # one inference at a time: a single model, a single GPU queue
        self.transcribe(np.zeros(SAMPLE_RATE, dtype=np.float32))  # warm-up: first MPS/CUDA call is slow
        log.info("loaded %s in %.1f s", model_name, time.monotonic() - started)

    def transcribe(self, samples: np.ndarray) -> str:
        samples = samples[: MAX_SECONDS * SAMPLE_RATE]
        if self.parakeet is not None:
            with self.lock, torch.inference_mode():
                inputs = self.processor(samples, sampling_rate=SAMPLE_RATE, return_tensors="pt").to(self.device)
                ids = self.parakeet.generate(**inputs, max_new_tokens=224)
                row = ids[0].tolist() if hasattr(ids[0], "tolist") else list(ids[0])
                text = self.processor.tokenizer.decode(row, skip_special_tokens=True)
                if isinstance(text, list):  # newer tokenizers answer with one string per input
                    text = text[0]
                return text.strip()
        if self.whisper is not None:
            with self.lock:
                fd, path = tempfile.mkstemp(suffix=".wav")
                os.close(fd)
                try:
                    write_wav(path, samples)
                    segments, _ = self.whisper.transcribe(path, language="ru", vad_filter=False)
                    return " ".join(segment.text.strip() for segment in segments).strip()
                finally:
                    os.unlink(path)
        with self.lock, torch.inference_mode():
            try:
                return self._transcribe_tensor(samples)
            except (AttributeError, TypeError) as error:
                # Internal API changed in another gigaam version: fall back to the public file-based call.
                log.warning("in-memory path failed (%s), using a temporary file", error)
                return self._transcribe_file(samples)

    def _transcribe_tensor(self, samples: np.ndarray) -> str:
        model = self.model
        wav = torch.from_numpy(np.ascontiguousarray(samples, dtype=np.float32))
        wav = wav.to(model._device).to(model._dtype).unsqueeze(0)
        length = torch.full([1], wav.shape[-1], device=model._device)
        encoded, encoded_len = model.forward(wav, length)
        text, _ = model._decode(encoded, encoded_len, length)[0]
        return text

    def _transcribe_file(self, samples: np.ndarray) -> str:
        fd, path = tempfile.mkstemp(suffix=".wav")
        os.close(fd)
        try:
            write_wav(path, samples)
            result = self.model.transcribe(path)
            return getattr(result, "text", result)
        finally:
            os.unlink(path)


def check_name(model: str) -> None:
    """Refuses, before anything downloads, a name this server cannot load: the ValueError says why."""
    if not SAFE_NAME.match(model) or os.path.exists(model):
        raise ValueError(f"not a model name: {model!r}")
    if model.startswith("whisper-"):
        if importlib.util.find_spec("faster_whisper") is None:
            raise ValueError("Whisper needs faster-whisper, not installed here (the Docker image: --build-arg WHISPER=1)")
        from faster_whisper import available_models

        if model.removeprefix("whisper-") not in available_models():
            raise ValueError(f"no Whisper model {model}: {', '.join('whisper-' + name for name in available_models())}")
    elif model.startswith("parakeet-"):
        if importlib.util.find_spec("transformers") is None or importlib.util.find_spec("librosa") is None:
            raise ValueError("Parakeet needs transformers and librosa, not installed here")
    else:
        # Full names only: gigaam also takes "ctc" for v3_ctc, and the gateway would then wait for a model that
        # never reports by its own name. SSL and emotion checkpoints do not transcribe.
        names = [name for name in getattr(gigaam, "_MODEL_HASHES", {}) if "ssl" not in name and name != "emo"]
        if names and model not in names:
            raise ValueError(f"no GigaAM model {model}: {', '.join(names)}")


class Busy(Exception):
    """Another model is loading: one load at a time."""


class Models:
    """The serving recognizer and the loads that replace it. A new model loads beside the serving one and takes
    over only after its warm-up: the household keeps its recognition through a slow download and keeps the old model
    when the new one fails. Memory holds both for that moment."""

    def __init__(self, device: str, cache_dir: str | None, model_file: str | None) -> None:
        self.device = device
        self.cache_dir = cache_dir
        self.model_file = model_file
        self.current: Recognizer | None = None
        self.loading: str | None = None
        self.error: str | None = None
        self.requests = 0
        self.changed = threading.Condition()

    def load(self, model: str, remember: bool = False) -> bool:
        """Loads a model and puts it to work; False when it serves already. Busy while another one loads."""
        with self.changed:
            while self.loading == model:  # asked twice: the second waits for the first and sees its outcome
                self.changed.wait()
            claimed = self._claim(model)
        if claimed:
            self._load(model)
        if remember:
            self.remember(model)
        return claimed

    def begin(self, model: str, remember: bool = False) -> bool:
        """The same in the background: True while it loads (watch /health), False when it serves already."""
        with self.changed:
            if self.loading == model:
                return True
            claimed = self._claim(model)
        if not claimed:
            if remember:
                self.remember(model)
            return False

        def work() -> None:
            try:
                self._load(model)
            except Exception:  # noqa: BLE001 - /health carries the error to whoever waits
                log.exception("%s did not load", model)
                return
            if remember:
                self.remember(model)

        threading.Thread(target=work, name=f"load {model}", daemon=True).start()
        return True

    def _claim(self, model: str) -> bool:
        """Under the lock: whether this call loads the model (False: it serves already)."""
        if self.loading is not None:
            raise Busy(self.loading)
        if self.current is not None and self.current.model_name == model:
            return False
        self.loading = model
        return True

    def _load(self, model: str) -> None:
        try:
            recognizer = Recognizer(model, self.device, self.cache_dir)
        except BaseException as error:
            with self.changed:
                self.loading = None
                self.error = f"{model}: {error}"
                self.changed.notify_all()
            raise
        with self.changed:
            previous, self.current = self.current, recognizer
            self.loading, self.error = None, None
            self.changed.notify_all()
        # Requests under way keep the old model until they finish; then it goes with its memory.
        del previous
        free_memory()

    def remember(self, model: str) -> None:
        """The model for the next start, written whole or not at all."""
        if not self.model_file:
            return
        part = None
        try:
            folder = os.path.dirname(os.path.abspath(self.model_file))
            os.makedirs(folder, exist_ok=True)
            fd, part = tempfile.mkstemp(dir=folder, prefix=".chosen-model-")
            with os.fdopen(fd, "w", encoding="utf-8") as file:
                file.write(f"{model}\n")
            os.chmod(part, 0o644)
            os.replace(part, self.model_file)
        except OSError as error:
            if part and os.path.exists(part):
                os.unlink(part)
            log.warning("%s is not remembered in %s (%s): the next start takes another", model, self.model_file, error)


def free_memory() -> None:
    gc.collect()
    if torch.cuda.is_available():
        torch.cuda.empty_cache()
    elif torch.backends.mps.is_available():
        torch.mps.empty_cache()


def remembered(path: str | None) -> str | None:
    if not path:
        return None
    try:
        with open(path, encoding="utf-8") as file:
            return file.readline().strip() or None
    except OSError:
        return None


def start(models: Models, pinned: str | None) -> None:
    """The first model. A pinned one (--model, GIGAAM_MODEL) that fails stops the server, as it always did; a
    remembered one that fails gives way to the default, so a stale choice cannot keep recognition down."""
    if pinned == "none":
        log.info("no model yet: POST /v1/models/load {\"model\": …}")
        return
    if pinned:
        models.load(pinned)
        return
    choice = remembered(models.model_file)
    if choice and choice not in ("none", DEFAULT_MODEL):
        try:
            check_name(choice)
            models.load(choice)
            return
        except Exception as error:  # noqa: BLE001 - whatever it was, the default still hears
            log.error("remembered model %s did not load (%s); starting with %s", choice, error, DEFAULT_MODEL)
    models.load(DEFAULT_MODEL)


def write_wav(path: str, samples: np.ndarray) -> None:
    with wave.open(path, "wb") as out:
        out.setnchannels(1)
        out.setsampwidth(2)
        out.setframerate(SAMPLE_RATE)
        out.writeframes((np.clip(samples, -1, 1) * 32767).astype("<i2").tobytes())


def read_wav(data: bytes) -> np.ndarray:
    with wave.open(io.BytesIO(data), "rb") as wav:
        if wav.getsampwidth() != 2:
            raise ValueError(f"expected 16-bit PCM, got {8 * wav.getsampwidth()}-bit")
        rate, channels = wav.getframerate(), wav.getnchannels()
        pcm = np.frombuffer(wav.readframes(wav.getnframes()), dtype="<i2").astype(np.float32) / 32768
    if channels > 1:
        pcm = pcm.reshape(-1, channels).mean(axis=1)
    if rate != SAMPLE_RATE:
        # Linear resampling is enough for speech; the gateway always sends 16 kHz anyway.
        positions = np.arange(0, len(pcm), rate / SAMPLE_RATE)
        pcm = np.interp(positions, np.arange(len(pcm)), pcm).astype(np.float32)
    return pcm


def parse_form(content_type: str, body: bytes) -> dict[str, tuple[bytes, str | None]]:
    """multipart/form-data → {field: (bytes, filename)} with the standard library's MIME parser."""
    message = email.parser.BytesParser(policy=email.policy.HTTP).parsebytes(
        b"Content-Type: " + content_type.encode() + b"\r\n\r\n" + body
    )
    fields = {}
    for part in message.iter_parts():
        name = part.get_param("name", header="content-disposition")
        if name:
            fields[name] = (part.get_payload(decode=True) or b"", part.get_filename())
    return fields


class Handler(BaseHTTPRequestHandler):
    models: Models
    api_key: str | None = None
    protocol_version = "HTTP/1.1"

    def do_GET(self) -> None:
        path = urlsplit(self.path).path.rstrip("/")
        if path in ("/health", ""):
            # Open without the key: Docker's health check and the gateway's probes ask it.
            models, current = self.models, self.models.current
            self.reply(
                200,
                {
                    "status": "ok",
                    "model": current.model_name if current else None,
                    "device": current.device if current else models.device,
                    "requests": models.requests,
                    "loading": models.loading,
                    "error": models.error,
                    # What the gateway may ask beyond OpenAI's API: a model switch without a restart.
                    "features": ["load"],
                },
            )
        elif not self.allowed():
            return
        elif path == "/v1/models":
            current = self.models.current
            self.reply(200, {"object": "list", "data": [{"id": current.model_name, "object": "model"}] if current else []})
        else:
            self.reply(404, {"error": {"message": f"no route {self.path}"}})

    def do_POST(self) -> None:
        path = urlsplit(self.path).path.rstrip("/")
        if not self.allowed():
            return
        if path == "/v1/models/load":
            self.load()
        elif path == "/v1/audio/transcriptions":
            self.transcribe()
        else:
            self.reply(404, {"error": {"message": f"no route {self.path}"}}, close=True)

    def load(self) -> None:
        try:
            length = int(self.headers.get("Content-Length") or 0)
            if not 0 <= length <= 4096:
                raise ValueError("the body is a model name, not this much")
            body = json.loads(self.rfile.read(length) or b"{}")
            model = body.get("model") if isinstance(body, dict) else None
            if not isinstance(model, str) or not model.strip():
                raise ValueError('body: {"model": "v3_e2e_rnnt"}')
            model = model.strip()
            check_name(model)
        except ValueError as error:
            self.reply(400, {"error": {"message": f"bad request: {error}"}}, close=True)
            return
        if body.get("wait") is False:
            # A download can outlast any client's patience (and Node's fetch gives up on headers after 5 min):
            # 202 at once, and /health says when "loading" is over — "model" if it took, "error" if not.
            try:
                loading = self.models.begin(model, remember=True)
            except Busy as busy:
                self.reply(409, {"error": {"message": f"{busy} is loading now: one load at a time"}})
                return
            if loading:
                self.reply(202, {"status": "loading", "model": model})
            else:
                current = self.models.current
                self.reply(200, {"status": "ok", "model": current.model_name, "device": current.device, "changed": False})
            return
        started = time.monotonic()
        try:
            changed = self.models.load(model, remember=True)
        except Busy as busy:
            self.reply(409, {"error": {"message": f"{busy} is loading now: one load at a time"}})
            return
        except Exception as error:  # noqa: BLE001 - a download, a checksum, memory: the old model serves on
            log.exception("%s did not load", model)
            serving = self.models.current.model_name if self.models.current else "no model"
            self.reply(500, {"error": {"message": f"{model} did not load: {error}; {serving} serves on"}})
            return
        current = self.models.current
        log.info("%s serves now%s", model, "" if changed else " (it did already)")
        self.reply(
            200,
            {
                "status": "ok",
                "model": current.model_name,
                "device": current.device,
                "changed": changed,
                "seconds": round(time.monotonic() - started, 1),
            },
        )

    def transcribe(self) -> None:
        started = time.monotonic()
        try:
            body = self.rfile.read(max(0, int(self.headers.get("Content-Length") or 0)))
            content_type = self.headers.get("Content-Type", "")
            fields = parse_form(content_type, body) if content_type.startswith("multipart/") else {"file": (body, None)}
            if "file" not in fields:
                raise ValueError("no 'file' field in the form")
            response_format = fields.get("response_format", (b"json", None))[0].decode() or "json"
            samples = read_wav(fields["file"][0])
        except Exception as error:  # noqa: BLE001 - any bad input is the client's problem, report it
            self.reply(400, {"error": {"message": f"bad request: {error}"}})
            return
        recognizer = self.models.current  # this request's model, whatever a load swaps in meanwhile
        if recognizer is None:
            self.reply(503, {"error": {"message": 'no model loaded yet: POST /v1/models/load {"model": "v3_e2e_rnnt"}'}})
            return
        try:
            text = recognizer.transcribe(samples).strip()
        except Exception as error:  # noqa: BLE001
            log.exception("recognition failed")
            self.reply(500, {"error": {"message": f"recognition failed: {error}"}})
            return
        self.models.requests += 1
        duration = len(samples) / SAMPLE_RATE
        log.info("%.2f s audio → %r in %d ms", duration, text, (time.monotonic() - started) * 1000)
        if response_format == "text":
            self.reply_text(text)
        elif response_format == "verbose_json":
            self.reply(200, {"text": text, "language": "ru", "duration": duration})
        else:
            self.reply(200, {"text": text})

    def allowed(self) -> bool:
        """With a key set (ASR_API_KEY, the one the gateway sends), every call but /health needs it."""
        if not self.api_key:
            return True
        given = self.headers.get("Authorization", "").encode()
        if hmac.compare_digest(given, f"Bearer {self.api_key}".encode()):
            return True
        # The body stays unread: the connection closes instead of reading it as the next request.
        self.reply(401, {"error": {"message": "unauthorized: Authorization: Bearer <ASR_API_KEY>"}}, close=True)
        return False

    def reply(self, status: int, payload: dict, close: bool = False) -> None:
        data = json.dumps(payload, ensure_ascii=False).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(data)))
        if close:
            self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(data)

    def reply_text(self, text: str) -> None:
        data = text.encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/plain; charset=utf-8")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, format: str, *args) -> None:  # noqa: A002 - signature of the base class
        log.debug("%s %s", self.address_string(), format % args)


def stop(signum: int, frame: object) -> None:
    # Docker stops with SIGTERM, and as PID 1 the server gets no default action for it: end serve_forever instead.
    raise KeyboardInterrupt


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--host", default=os.environ.get("ASR_HOST", "127.0.0.1"),
                        help="0.0.0.0 to answer beyond this computer (the Docker image does); there is no other guard than --api-key")
    parser.add_argument("--port", type=int, default=int(os.environ.get("ASR_PORT", "8090")))
    parser.add_argument("--model", default=os.environ.get("GIGAAM_MODEL") or None,
                        help="v3_e2e_rnnt (punctuation and casing), v3_rnnt, v3_ctc, v2_rnnt, …, parakeet-0.6b-v3, "
                             "whisper-large-v3-turbo; none: start without one. Unset: the remembered model, else v3_e2e_rnnt")
    parser.add_argument("--device", default=os.environ.get("GIGAAM_DEVICE", "auto"), help="auto | cuda | mps | cpu")
    parser.add_argument("--cache-dir", default=os.environ.get("ASR_CACHE_DIR") or None,
                        help="where the weights go: <dir>/gigaam and <dir>/huggingface (default ~/.cache/gigaam and "
                             "Hugging Face's own cache)")
    parser.add_argument("--model-file", default=os.environ.get("ASR_MODEL_FILE") or os.path.join(HERE, "chosen-model"),
                        help="where a model loaded over HTTP is remembered for the next start")
    parser.add_argument("--api-key", default=os.environ.get("ASR_API_KEY") or None,
                        help="require Authorization: Bearer <key> on everything but /health")
    args = parser.parse_args()

    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(name)s: %(message)s")
    if args.cache_dir:
        # Anything else that reaches for Hugging Face's cache lands beside the weights too.
        os.environ.setdefault("HF_HOME", os.path.join(args.cache_dir, "huggingface"))
    models = Models(args.device, args.cache_dir, args.model_file)
    start(models, args.model)
    Handler.models = models
    Handler.api_key = args.api_key
    server = ThreadingHTTPServer((args.host, args.port), Handler)
    signal.signal(signal.SIGTERM, stop)
    log.info("listening on http://%s:%d/v1/audio/transcriptions", args.host, args.port)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
