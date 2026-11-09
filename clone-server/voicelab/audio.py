"""WAV in and out, and the choice of the reference: the best 5-8 seconds of the owner's takes.

Only the standard library and numpy — the same in every engine's environment. The takes arrive as 16-bit mono WAV at
24 kHz (the gateway converts them); the zero-shot engines take a few seconds of speech as the voice, and a short clean
reference is both closer and faster to speak with (bench: OmniVoice RTF 0.96 with a short one, 1.53 with 14 s).
"""
from __future__ import annotations

import wave
from dataclasses import dataclass
from pathlib import Path

import numpy as np

FRAME_S = 0.02
#: Pauses shorter than this keep a phrase whole.
JOIN_GAP_S = 0.25
#: Speech runs shorter than this are clicks and breaths.
MIN_RUN_S = 0.3


def read_wav(path: str | Path) -> tuple[np.ndarray, int]:
    """A 16-bit PCM WAV as mono float32 in [-1, 1] and its rate."""
    with wave.open(str(path), "rb") as source:
        if source.getsampwidth() != 2:
            raise ValueError(f"{Path(path).name}: only 16-bit PCM WAV is expected")
        rate = source.getframerate()
        channels = source.getnchannels()
        data = np.frombuffer(source.readframes(source.getnframes()), dtype="<i2").astype(np.float32) / 32768.0
    if channels > 1:
        data = data.reshape(-1, channels).mean(axis=1)
    return data, rate


def write_wav(path: str | Path, samples: np.ndarray, rate: int) -> None:
    pcm = np.clip(samples, -1.0, 1.0)
    with wave.open(str(path), "wb") as target:
        target.setnchannels(1)
        target.setsampwidth(2)
        target.setframerate(rate)
        target.writeframes((pcm * 32767.0).astype("<i2").tobytes())


def to_int16(samples: np.ndarray) -> np.ndarray:
    """An engine's float output as 16-bit PCM, peak-normalised just below full scale."""
    if samples.dtype == np.int16:
        return samples
    peak = float(np.max(np.abs(samples))) if samples.size else 0.0
    scaled = samples / peak * 0.95 if peak > 0 else samples
    return (np.clip(scaled, -1.0, 1.0) * 32767.0).astype(np.int16)


def resample(samples: np.ndarray, rate: int, target: int) -> np.ndarray:
    """Linear resampling: enough for recognition of a reference clip (the voice itself never goes through it)."""
    if rate == target or samples.size == 0:
        return samples
    count = int(round(samples.size * target / rate))
    positions = np.linspace(0, samples.size - 1, count)
    return np.interp(positions, np.arange(samples.size), samples).astype(np.float32)


def frame_db(samples: np.ndarray, rate: int) -> np.ndarray:
    size = max(1, int(rate * FRAME_S))
    count = samples.size // size
    if count == 0:
        return np.zeros(0, dtype=np.float32)
    frames = samples[: count * size].reshape(count, size)
    rms = np.sqrt(np.mean(frames * frames, axis=1)) + 1e-9
    return 20.0 * np.log10(rms)


@dataclass
class Run:
    """A stretch of speech between pauses, in seconds within its take."""

    start: float
    end: float


def speech_runs(samples: np.ndarray, rate: int) -> tuple[list[Run], float]:
    """Speech runs by energy — frames louder than the noise floor by 10 dB — and the floor itself in dBFS."""
    db = frame_db(samples, rate)
    if db.size == 0:
        return [], -90.0
    floor = float(np.percentile(db, 10))
    threshold = max(floor + 10.0, -55.0)
    speech = db > threshold
    runs: list[Run] = []
    start = None
    for index, on in enumerate(speech):
        if on and start is None:
            start = index
        elif not on and start is not None:
            runs.append(Run(start * FRAME_S, index * FRAME_S))
            start = None
    if start is not None:
        runs.append(Run(start * FRAME_S, speech.size * FRAME_S))
    merged: list[Run] = []
    for run in runs:
        if merged and run.start - merged[-1].end < JOIN_GAP_S:
            merged[-1].end = run.end
        else:
            merged.append(run)
    return [run for run in merged if run.end - run.start >= MIN_RUN_S], floor


@dataclass
class Reference:
    take: int
    start: float
    end: float
    score: float
    snr_db: float
    level_db: float

    @property
    def seconds(self) -> float:
        return self.end - self.start


def pick_reference(
    takes: list[np.ndarray],
    rate: int,
    shortest: float = 5.0,
    longest: float = 8.0,
    widest_pause: float = 0.7,
) -> Reference:
    """
    The best continuous stretch of speech: whole phrases, shortest..longest seconds, no pause inside wider than
    widest_pause. Score: signal over the take's noise floor, minus clipping, minus a level far from normal speech, plus
    a little for ending at a real pause (a finished sentence) and for a length near the middle of the range.
    """
    best: Reference | None = None
    middle = (shortest + longest) / 2
    for take_index, samples in enumerate(takes):
        runs, floor = speech_runs(samples, rate)
        for first in range(len(runs)):
            for last in range(first, len(runs)):
                if last > first and runs[last].start - runs[last - 1].end > widest_pause:
                    break
                start, end = runs[first].start, runs[last].end
                length = end - start
                if length > longest:
                    break
                if length < shortest:
                    continue
                piece = samples[int(start * rate) : int(end * rate)]
                speech_db = frame_db(piece, rate)
                level = float(np.percentile(speech_db, 75)) if speech_db.size else -90.0
                clipping = float(np.mean(np.abs(piece) >= 0.99)) if piece.size else 0.0
                following = runs[last + 1].start - end if last + 1 < len(runs) else 1.0
                score = (
                    (level - floor)
                    - clipping * 2000.0
                    - max(0.0, abs(level + 20.0) - 10.0) * 0.5
                    - abs(length - middle) * 0.5
                    + (2.0 if following >= 0.5 else 0.0)
                )
                if best is None or score > best.score:
                    best = Reference(take_index, start, end, score, level - floor, level)
    if best is None:
        raise ValueError(
            f"в записях не нашлось связного куска речи на {shortest:.0f}–{longest:.0f} секунд — "
            "запишите дубль спокойнее, без долгих пауз"
        )
    return best


def render_reference(samples: np.ndarray, rate: int, reference: Reference) -> np.ndarray:
    """The reference with 100 ms of air on both sides, speech level about -20 dBFS (peaks below -1 dBFS), soft edges."""
    start = max(0, int((reference.start - 0.1) * rate))
    end = min(samples.size, int((reference.end + 0.1) * rate))
    piece = samples[start:end].astype(np.float32).copy()
    speech_db = frame_db(piece, rate)
    level = float(np.percentile(speech_db, 75)) if speech_db.size else -20.0
    gain = 10 ** ((-20.0 - level) / 20.0)
    peak = float(np.max(np.abs(piece))) or 1.0
    gain = min(gain, 0.89 / peak)
    piece *= gain
    fade = min(piece.size // 2, int(0.02 * rate))
    if fade > 0:
        ramp = np.linspace(0.0, 1.0, fade, dtype=np.float32)
        piece[:fade] *= ramp
        piece[-fade:] *= ramp[::-1]
    return piece
