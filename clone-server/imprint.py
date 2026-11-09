"""Takes a voice imprint: the owner's takes in, a voice folder out — the gateway's «Снять отпечаток».

    runtime/<engine>/env/bin/python imprint.py --engine omnivoice --runtime runtime/omnivoice \
        --takes t1.wav t2.wav --out data/voice-lab/voices/<id> --asr-url http://127.0.0.1:8090/v1 \
        --mem-limit-mb 5000 --samples '["Привет!"]' --die-with-stdin

Steps: the best 5-8 s of speech in the takes becomes the reference (ref.wav); our recognition gives its words; the
engine turns both into voice.pt; three sample phrases let the owner hear it. With --ref-text the words are known (a
character's reference from its package, a voice taken again for another engine): the one take is the reference as it
is, and nothing is recognised. Everything is written to a .partial folder
first and renamed only when whole. Progress goes to stdout as JSON lines ({"stage", "percent"}, {"memoryMb"},
{"error", "reason"} and finally {"stage": "done", ...}); the gateway shows them and guards the memory from outside too.
"""
from __future__ import annotations

import argparse
import io
import json
import shutil
import sys
import time
import urllib.request
import uuid
import wave
from datetime import datetime, timezone
from pathlib import Path

import numpy as np

from voicelab.audio import pick_reference, read_wav, render_reference, resample, to_int16, write_wav
from voicelab.engines import ENGINES, load_engine, pick_device, prepare_environment
from voicelab.limits import MB, Guard, cap_allocator, die_with_stdin, windows_job

RATE = 24000


def emit(**fields) -> None:
    print(json.dumps(fields, ensure_ascii=False), flush=True)


def wav_bytes(samples: np.ndarray, rate: int) -> bytes:
    buffer = io.BytesIO()
    with wave.open(buffer, "wb") as target:
        target.setnchannels(1)
        target.setsampwidth(2)
        target.setframerate(rate)
        target.writeframes((np.clip(samples, -1.0, 1.0) * 32767.0).astype("<i2").tobytes())
    return buffer.getvalue()


def recognise(base_url: str, model: str, audio: bytes) -> str:
    """The reference's words from our recognition server (OpenAI-compatible): exact words make a closer voice."""
    boundary = uuid.uuid4().hex
    body = io.BytesIO()
    for name, value in (("model", model or "default"), ("language", "ru"), ("response_format", "json")):
        body.write(f'--{boundary}\r\nContent-Disposition: form-data; name="{name}"\r\n\r\n{value}\r\n'.encode())
    body.write(
        f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="reference.wav"\r\n'
        "Content-Type: audio/wav\r\n\r\n".encode()
    )
    body.write(audio)
    body.write(f"\r\n--{boundary}--\r\n".encode())
    request = urllib.request.Request(
        base_url.rstrip("/") + "/audio/transcriptions",
        data=body.getvalue(),
        headers={"Content-Type": f"multipart/form-data; boundary={boundary}"},
    )
    try:
        with urllib.request.urlopen(request, timeout=60) as response:
            words = str(json.loads(response.read()).get("text") or "").strip()
    except Exception as error:
        raise RuntimeError(f"распознавание не ответило ({error}) — без слов образца отпечаток не снять") from error
    if not words:
        raise RuntimeError("распознавание не услышало слов в образце — запишите дубль громче и чище")
    return words


def main() -> int:
    parser = argparse.ArgumentParser(description="Take a voice imprint from the owner's takes")
    parser.add_argument("--engine", required=True, choices=ENGINES)
    parser.add_argument("--runtime", required=True, help="clone-server/runtime/<engine>")
    parser.add_argument("--takes", nargs="+", required=True, help="16-bit mono WAV, 24 kHz")
    parser.add_argument("--out", required=True, help="the voice's folder; must not exist yet")
    parser.add_argument("--name", default="Мой голос")
    parser.add_argument("--asr-url", default="", help="our recognition; not needed with --ref-text")
    parser.add_argument("--ref-text", default="", help="the reference's words: the take is the reference, whole")
    parser.add_argument("--asr-model", default="")
    parser.add_argument("--device", default="auto")
    parser.add_argument("--threads", type=int, default=4)
    parser.add_argument("--mem-limit-mb", type=int, default=0)
    parser.add_argument("--samples", default="[]", help="JSON list of phrases to say with the new voice")
    parser.add_argument("--shortest", type=float, default=5.0)
    parser.add_argument("--longest", type=float, default=8.0)
    parser.add_argument("--die-with-stdin", action="store_true")
    args = parser.parse_args()
    known_words = args.ref_text.strip()
    if not known_words and not args.asr_url:
        parser.error("--asr-url is needed unless --ref-text gives the words")

    if args.die_with_stdin:
        die_with_stdin()
    limit = args.mem_limit_mb * MB if args.mem_limit_mb > 0 else None
    windows_job(limit)
    guard = Guard(limit, report=lambda used: emit(memoryMb=used // MB))

    out = Path(args.out)
    partial = out.parent / f".partial-{out.name}"
    started = time.time()
    try:
        if out.exists():
            raise RuntimeError(f"голос {out.name} уже есть")
        shutil.rmtree(partial, ignore_errors=True)
        (partial / "samples").mkdir(parents=True)

        emit(stage="prepare", percent=5)
        takes = []
        for path in args.takes:
            samples, rate = read_wav(path)
            takes.append(samples if rate == RATE else resample(samples, rate, RATE))

        emit(stage="select", percent=12)
        if known_words:
            # A reference already chosen, its words known: taken whole, nothing recognised.
            clip = takes[0]
            seconds = clip.size / RATE
            reference_meta = {"take": 1, "start": 0.0, "end": round(seconds, 2), "seconds": round(seconds, 2)}
            write_wav(partial / "ref.wav", clip, RATE)
            emit(stage="words", percent=20)
            words = known_words
        else:
            reference = pick_reference(takes, RATE, args.shortest, args.longest)
            clip = render_reference(takes[reference.take], RATE, reference)
            reference_meta = {
                "take": reference.take + 1,
                "start": round(reference.start, 2),
                "end": round(reference.end, 2),
                "seconds": round(reference.seconds, 2),
                "snrDb": round(reference.snr_db, 1),
            }
            write_wav(partial / "ref.wav", clip, RATE)

            emit(stage="words", percent=20)
            words = recognise(args.asr_url, args.asr_model, wav_bytes(resample(clip, RATE, 16000), 16000))

        emit(stage="load", percent=28)
        runtime = Path(args.runtime).resolve()
        prepare_environment(runtime)
        import torch

        guard.attach_torch(torch)
        device = pick_device(torch, args.engine, args.device)
        cap_allocator(torch, device, limit)
        engine = load_engine(args.engine, runtime, device, args.threads)

        emit(stage="imprint", percent=55)
        engine.imprint(partial / "ref.wav", words, partial)
        voice = engine.load_voice(partial)

        phrases = [phrase for phrase in json.loads(args.samples) if isinstance(phrase, str) and phrase.strip()]
        samples_meta = []
        spoken, spent = 0.0, 0.0
        for index, phrase in enumerate(phrases, start=1):
            emit(stage="samples", percent=60 + int(35 * (index - 1) / max(1, len(phrases))))
            began = time.time()
            audio = engine.synthesize(phrase, voice)
            took = time.time() - began
            seconds = audio.size / engine.rate
            spoken += seconds
            spent += took
            pcm = to_int16(audio)
            with wave.open(str(partial / "samples" / f"{index}.wav"), "wb") as target:
                target.setnchannels(1)
                target.setsampwidth(2)
                target.setframerate(engine.rate)
                target.writeframes(pcm.tobytes())
            samples_meta.append({"n": index, "text": phrase, "seconds": round(seconds, 2)})

        guard.measure()
        meta = {
            "id": out.name,
            "name": args.name.strip()[:60] or "Мой голос",
            "engine": args.engine,
            "device": device,
            "createdAt": datetime.now(timezone.utc).isoformat(timespec="seconds"),
            "reference": {**reference_meta, "words": words},
            "samples": samples_meta,
            "rtf": round(spent / spoken, 2) if spoken else None,
            "peakMb": guard.peak // MB,
            "seconds": round(time.time() - started, 1),
        }
        (partial / "meta.json").write_text(json.dumps(meta, ensure_ascii=False, indent=2), encoding="utf-8")
        partial.rename(out)
        emit(
            stage="done",
            percent=100,
            peakMb=meta["peakMb"],
            rtf=meta["rtf"],
            refSeconds=meta["reference"]["seconds"],
            device=device,
        )
        return 0
    except Exception as error:
        shutil.rmtree(partial, ignore_errors=True)
        emit(error=str(error) or type(error).__name__, reason="error", peakMb=guard.peak // MB)
        return 1


if __name__ == "__main__":
    sys.exit(main())
