"""The owner's own voices behind the same OpenAI-compatible speech API as tts-server: one engine per process.

    runtime/omnivoice/env/bin/python server.py --engine omnivoice --runtime runtime/omnivoice \
        --voices data/voice-lab/voices --port 8093 --instance <token>

A voice is a folder of data/voice-lab/voices whose meta.json names this engine, with the imprint in voice.pt (taken by
imprint.py, never here). The gateway asks POST /v1/audio/speech {"model", "voice": "<id>", "input"} and gets 16-bit mono
WAV; GET /health tells it whose server answers ({"server": "clone-server", "instance"}), so it never stops a stranger,
and how much memory it holds ({"memoryMb", "peakMb"}): the gateway judges the voice's next start by that peak.
One synthesis at a time (one model); the gateway queues phrases itself. The text is never logged, only its length.
"""
from __future__ import annotations

import argparse
import io
import json
import logging
import os
import sys
import threading
import time
import wave
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from voicelab.audio import to_int16
from voicelab.engines import ENGINES, load_engine, pick_device, prepare_environment
from voicelab.limits import MB, Guard, cap_allocator, windows_job

log = logging.getLogger("clone-server")


class Voices:
    """The engine's voices on disk, loaded on first use and again when their imprint changes."""

    def __init__(self, engine, folder: Path):
        self.engine = engine
        self.folder = folder
        self.loaded: dict[str, tuple[float, object]] = {}

    def names(self) -> list[str]:
        found = []
        if self.folder.is_dir():
            for voice in sorted(self.folder.iterdir()):
                if voice.name.startswith(".") or not (voice / "voice.pt").is_file():
                    continue
                try:
                    meta = json.loads((voice / "meta.json").read_text(encoding="utf-8"))
                except (OSError, ValueError):
                    continue
                if meta.get("engine") == self.engine.name:
                    found.append(voice.name)
        return found

    def get(self, name: str):
        if not name or "/" in name or "\\" in name or name.startswith(".") or name not in self.names():
            raise LookupError(f"voice '{name}' of '{self.engine.name}' is not here (have {', '.join(self.names()) or 'none'})")
        imprint = self.folder / name / "voice.pt"
        stamp = imprint.stat().st_mtime
        cached = self.loaded.get(name)
        if not cached or cached[0] != stamp:
            cached = (stamp, self.engine.load_voice(self.folder / name))
            self.loaded[name] = cached
        return cached[1]


def wav(pcm, rate: int) -> bytes:
    buffer = io.BytesIO()
    with wave.open(buffer, "wb") as target:
        target.setnchannels(1)
        target.setsampwidth(2)
        target.setframerate(rate)
        target.writeframes(pcm.tobytes())
    return buffer.getvalue()


class Handler(BaseHTTPRequestHandler):
    engine = None
    voices: Voices
    lock = threading.Lock()
    info: dict = {}
    requests = 0
    guard: Guard

    def log_message(self, fmt, *args):  # one line per synthesis instead
        pass

    def handle_one_request(self):
        """A phrase the gateway cancelled drops its connection: routine, not a traceback."""
        try:
            super().handle_one_request()
        except (BrokenPipeError, ConnectionResetError):
            log.info("client went away mid-request (a cancelled phrase)")

    def _json(self, code: int, body: dict) -> None:
        data = json.dumps(body, ensure_ascii=False).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        if self.path == "/health":
            memory = {"memoryMb": Handler.guard.current // MB, "peakMb": Handler.guard.peak // MB}
            self._json(200, {**Handler.info, **memory, "status": "ok", "voices": Handler.voices.names(), "requests": Handler.requests})
        elif self.path == "/v1/models":
            self._json(200, {"data": [{"id": Handler.engine.name, "object": "model", "voices": Handler.voices.names()}]})
        else:
            self._json(404, {"error": f"no {self.path}"})

    def do_POST(self):
        if self.path != "/v1/audio/speech":
            return self._json(404, {"error": f"no {self.path}"})
        try:
            length = int(self.headers.get("Content-Length") or 0)
            request = json.loads(self.rfile.read(length) or b"{}")
            text = str(request.get("input") or "").strip()
            name = str(request.get("voice") or "")
            if not text:
                return self._json(400, {"error": "input is empty"})
            voice = Handler.voices.get(name)
            began = time.time()
            with Handler.lock:
                audio = Handler.engine.synthesize(text, voice)
            pcm = to_int16(audio)
            if pcm.size == 0:
                raise ValueError("the engine returned no audio")
            rate = Handler.engine.rate
            log.info("%s: %d chars → %.2fs audio in %.2fs", name, len(text), pcm.size / rate, time.time() - began)
        except LookupError as error:  # strict: an unknown voice is an error, never another voice
            log.warning("%s", error)
            return self._json(404, {"error": str(error)})
        except Exception as error:
            log.exception("synthesis failed")
            return self._json(500, {"error": f"{type(error).__name__}: {error}"})
        Handler.requests += 1
        if request.get("response_format") == "pcm":
            body, kind = pcm.tobytes(), "audio/pcm"
        else:
            body, kind = wav(pcm, rate), "audio/wav"
        self.send_response(200)
        self.send_header("Content-Type", kind)
        self.send_header("X-Sample-Rate", str(rate))
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def main() -> int:
    parser = argparse.ArgumentParser(description="Own cloned voices over the OpenAI speech API")
    parser.add_argument("--engine", required=True, choices=ENGINES)
    parser.add_argument("--runtime", required=True)
    parser.add_argument("--voices", required=True)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--device", default="auto")
    parser.add_argument("--threads", type=int, default=4)
    parser.add_argument("--instance", default="")
    parser.add_argument("--mem-limit-mb", type=int, default=0)
    args = parser.parse_args()

    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s", stream=sys.stdout)
    limit = args.mem_limit_mb * MB if args.mem_limit_mb > 0 else None
    windows_job(limit)
    guard = Guard(limit)
    runtime = Path(args.runtime).resolve()
    prepare_environment(runtime)
    import torch

    guard.attach_torch(torch)
    device = pick_device(torch, args.engine, args.device)
    cap_allocator(torch, device, limit)
    began = time.time()
    log.info("loading %s on %s…", args.engine, device)
    engine = load_engine(args.engine, runtime, device, args.threads)
    voices = Voices(engine, Path(args.voices).resolve())
    names = voices.names()
    if names:  # the first synthesis on MPS or CUDA is slow: pay it now, not on the household's first answer
        try:
            engine.synthesize("Да.", voices.get(names[0]))
        except Exception as error:
            log.warning("warm-up failed: %s", error)
    log.info("ready in %.1fs on %s, voices: %s, %d MB", time.time() - began, device, ", ".join(names) or "none", guard.measure() // MB)

    Handler.engine = engine
    Handler.voices = voices
    Handler.guard = guard
    Handler.info = {"server": "clone-server", "engine": args.engine, "device": device, "pid": os.getpid(), "instance": args.instance}
    server = ThreadingHTTPServer((args.host, args.port), Handler)
    log.info("clone-server %s on http://%s:%d", args.engine, args.host, args.port)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
