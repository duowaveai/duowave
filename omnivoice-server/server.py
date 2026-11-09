"""Our own cloned-voice synthesis (OmniVoice: code Apache-2.0, weights CC BY-NC 4.0 — at home only) behind the same
OpenAI-compatible API as tts-server.

The gateway sends a phrase to POST /v1/audio/speech exactly as it would to tts-server or OpenAI, and gets a
16-bit mono WAV back. A voice is a "voice prompt" file, models/<name>/voice_prompt.pt: the zero-shot clone of
a person's timbre made from a few seconds of their speech (the voice lab, clone-server/, makes them). No training runs here:
creating a voice is one call (POST /v1/voices), meant for the panel later — record a clip, get a voice.

    env/bin/python server.py --port 8092                    # every voice in models/
    curl -s localhost:8092/health

The request picks the voice: "model" is the engine name (omnivoice), "voice" the cloned voice (its folder in models/), so
the gateway's TTS_OWN_MODEL and TTS_OWN_VOICE choose it and this server stays as it is. Separate process on
purpose: omnivoice needs its own torch, and a process can load only one (tts-server keeps silero/piper).

Endpoints:
    POST /v1/audio/speech   JSON {"model", "input", "voice", "response_format": "wav" | "pcm"} → 24 kHz
    GET  /v1/models         {"data": [{"id": "omnivoice", "voices": [...]}]}
    GET  /health            {"status", "loaded": {"omnivoice": [names]}, "requests"}
    POST /v1/voices?name=X&ref_text=Y   body: raw audio clip (wav preferred, 5-10 s) → new voice_prompt.pt,
                            picked up live, no restart. This is the panel's future "клонировать мой голос".

Model weights (k2-fsa/OmniVoice, ~2 GB) download from HuggingFace on first start into ./hf-cache.
The repository holds no voice and names none: models/ is the household's own, kept out of git (.gitignore).
Only the standard library besides numpy/torch: no web framework to install or keep updated.
"""
from __future__ import annotations

import argparse
import io
import json
import logging
import os
import re
import threading
import time
import wave
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import numpy as np

log = logging.getLogger("omnivoice")
HERE = os.path.dirname(os.path.abspath(__file__))
SAFE_NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$")


class OmniVoiceEngine:
    """One OmniVoice model, many cloned voices: a voice_prompt.pt each, loaded from disk (cheap)."""

    name = "omnivoice"
    rate = 24000

    def __init__(self, models_dir: str, device: str):
        import torch
        from omnivoice import OmniVoice, VoiceClonePrompt

        self.torch = torch
        self.VoiceClonePrompt = VoiceClonePrompt
        self.lock = threading.Lock()  # one model instance → one synthesis at a time
        self.models_dir = Path(models_dir)
        self.prompts: dict[str, Path] = {}  # voice name → voice_prompt.pt on disk

        if device == "auto":
            device = "mps" if torch.backends.mps.is_available() else "cpu"
        log.info("loading OmniVoice on %s (first run downloads ~2 GB from HuggingFace)…", device)
        t0 = time.time()
        # базовая HF-модель, или локальный чекпоинт после файнтюна: OMNIVOICE_MODEL=/путь/к/merged
        model_id = os.environ.get("OMNIVOICE_MODEL", "k2-fsa/OmniVoice")
        self.model = OmniVoice.from_pretrained(
            model_id, device_map=device,
            dtype=torch.float16 if device == "mps" else torch.float32, asr_device="cpu")
        log.info("model ready in %.1fs on %s", time.time() - t0, device)
        self.rescan()

    def rescan(self) -> list[str]:
        """Voices are the models/<name>/voice_prompt.pt folders; also drops deleted ones."""
        found = {p.parent.name: p for p in self.models_dir.glob("*/voice_prompt.pt")}
        self.prompts = found
        log.info("voices: %s", ", ".join(sorted(found)) or "none")
        return sorted(found)

    def create_voice(self, name: str, clip_path: str, ref_text: str | None = None) -> str:
        """A new cloned voice from a speech clip (5-10 s); transcription is Whisper's job when ref_text is empty."""
        if not SAFE_NAME.match(name):
            raise ValueError(f"bad voice name {name!r}")
        prompt = self.model.create_voice_clone_prompt(ref_audio=clip_path, ref_text=ref_text or None)
        out = self.models_dir / name
        out.mkdir(parents=True, exist_ok=True)
        prompt.save(str(out / "voice_prompt.pt"))
        self.rescan()
        return name

    def synthesize(self, text: str, voice: str) -> tuple[np.ndarray, int]:
        if voice not in self.prompts:
            raise ValueError(f"voice '{voice}' of 'omnivoice' is not loaded (have {', '.join(self.prompts) or 'none'})")
        prompt = self.VoiceClonePrompt.load(str(self.prompts[voice]))
        with self.lock:
            audio = self.model.generate(text=text, voice_clone_prompt=prompt)
        wav = audio[0]
        if wav.dtype != np.int16:
            peak = float(np.max(np.abs(wav))) or 1.0
            wav = np.clip(wav / peak * 0.95, -1.0, 1.0)
            wav = (wav * 32767).astype(np.int16)
        return wav, self.rate


def to_wav(pcm: np.ndarray, rate: int) -> bytes:
    raw = io.BytesIO()
    with wave.open(raw, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(pcm.tobytes())
    return raw.getvalue()


class Handler(BaseHTTPRequestHandler):
    engine: OmniVoiceEngine
    requests = 0

    def log_message(self, fmt, *args):
        log.info("%s %s", self.address_string(), fmt % args)

    def handle_one_request(self):
        """A dropped client (the gateway aborting a cancelled phrase) is routine, not a crash: no traceback."""
        try:
            super().handle_one_request()
        except (BrokenPipeError, ConnectionResetError):
            log.info("client %s went away mid-request (cancelled phrase)", self.address_string())

    def _json(self, code: int, body: dict):
        data = json.dumps(body, ensure_ascii=False).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        if self.path == "/health":
            self._json(200, {"status": "ok", "loaded": {Handler.engine.name: Handler.engine.rescan()},
                             "requests": Handler.requests})
        elif self.path == "/v1/models":
            voices = Handler.engine.rescan()
            self._json(200, {"data": [{"id": Handler.engine.name, "object": "model", "voices": voices}]})
        else:
            self._json(404, {"error": f"no {self.path}"})

    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(length) if length else b""

        if self.path == "/v1/audio/speech":
            try:
                req = json.loads(body)
                text = (req.get("input") or "").strip()
                if not text:
                    return self._json(400, {"error": "input is empty"})
                voice = req.get("voice")
                if not voice:
                    # No default voice in the code: a single one in models/ is the one, otherwise the request names it.
                    voices = Handler.engine.rescan()
                    if len(voices) != 1:
                        return self._json(400, {"error": "voice is required: " + (", ".join(voices) or "no voices in models/ yet")})
                    voice = voices[0]
                t0 = time.time()
                pcm, rate = Handler.engine.synthesize(text, voice)
                log.info("%s: %d chars → %.2fs audio in %.2fs", voice, len(text), len(pcm) / rate, time.time() - t0)
            except ValueError as e:  # unknown voice or empty audio: strict, no silent substitution
                log.warning("speech failed for %r: %s", voice, e)
                return self._json(500, {"error": str(e)})
            except Exception as e:
                log.exception("synthesis failed")
                return self._json(500, {"error": f"{type(e).__name__}: {e}"})
            Handler.requests += 1
            if req.get("response_format") == "pcm":
                self.send_response(200)
                self.send_header("Content-Type", "audio/pcm")
                self.send_header("X-Sample-Rate", str(rate))
                self.send_header("Content-Length", str(len(pcm.tobytes())))
                self.end_headers()
                self.wfile.write(pcm.tobytes())
            else:
                wav = to_wav(pcm, rate)
                self.send_response(200)
                self.send_header("Content-Type", "audio/wav")
                self.send_header("Content-Length", str(len(wav)))
                self.end_headers()
                self.wfile.write(wav)

        elif self.path.startswith("/v1/voices"):
            # ?name=<voice>&ref_text=... ; body: the clip's bytes (wav preferred)
            from urllib.parse import urlparse, parse_qs
            q = parse_qs(urlparse(self.path).query)
            name = (q.get("name") or [""])[0]
            if not name:
                return self._json(400, {"error": "?name= is required"})
            if not body:
                return self._json(400, {"error": "clip bytes expected in the body"})
            clip = Handler.engine.models_dir / name / "ref-in.wav"
            clip.parent.mkdir(parents=True, exist_ok=True)
            clip.write_bytes(body)
            try:
                Handler.engine.create_voice(name, str(clip), (q.get("ref_text") or [""])[0])
            except Exception as e:
                log.exception("voice creation failed")
                clip.unlink(missing_ok=True)
                return self._json(500, {"error": f"{type(e).__name__}: {e}"})
            Handler.requests += 1
            self._json(200, {"ok": True, "voice": name, "loaded": Handler.engine.rescan()})
        else:
            self._json(404, {"error": f"no {self.path}"})


def main():
    ap = argparse.ArgumentParser(description="OmniVoice cloned-voice TTS sidecar")
    ap.add_argument("--port", type=int, default=int(os.environ.get("OMNIVOICE_PORT", 8092)))
    ap.add_argument("--host", default=os.environ.get("OMNIVOICE_HOST", "127.0.0.1"))
    ap.add_argument("--models", default=os.environ.get("OMNIVOICE_MODELS", os.path.join(HERE, "models")))
    ap.add_argument("--device", default=os.environ.get("OMNIVOICE_DEVICE", "auto"),
                    help="auto (mps→cpu), mps or cpu")
    args = ap.parse_args()

    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
    os.environ.setdefault("HF_HOME", os.path.join(HERE, "hf-cache"))
    Handler.engine = OmniVoiceEngine(args.models, args.device)
    httpd = ThreadingHTTPServer((args.host, args.port), Handler)
    log.info("omnivoice-server on http://%s:%d (voices in %s)", args.host, args.port, args.models)
    try:
        httpd.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
