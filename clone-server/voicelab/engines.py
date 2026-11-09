"""The zero-shot engines a voice imprint is taken with: OmniVoice (k2-fsa; code Apache-2.0, weights CC BY-NC 4.0),
CosyVoice 3 (FunAudioLLM; Apache-2.0, its model page says academic use only) and VoxCPM (OpenBMB; Apache-2.0 whole —
the code and the weights). No training: a few seconds of the owner's speech and its words become the voice.

Each engine lives in its own environment under clone-server/runtime/<engine>/ (env, hf-cache; CosyVoice also its repo,
weights and the wetext mirror): their torch versions differ, and a process loads one torch. This module is imported
inside that environment only.
"""
from __future__ import annotations

import os
import sys
from pathlib import Path

import numpy as np

ENGINES = ("omnivoice", "cosyvoice3", "voxcpm")


def prepare_environment(runtime: Path) -> None:
    """Weights come from the runtime's own cache and never from the network while serving or imprinting."""
    os.environ.setdefault("HF_HOME", str(runtime / "hf-cache"))
    os.environ.setdefault("HF_HUB_OFFLINE", "1")
    os.environ.setdefault("TRANSFORMERS_OFFLINE", "1")
    os.environ.setdefault("TOKENIZERS_PARALLELISM", "false")


def pick_device(torch_module, engine: str, wanted: str = "auto") -> str:
    """cuda → mps → cpu; CosyVoice has no MPS path (it runs on the CPU of a Mac)."""
    if wanted != "auto":
        return wanted
    if torch_module.cuda.is_available():
        return "cuda"
    if engine in ("omnivoice", "voxcpm") and torch_module.backends.mps.is_available():
        return "mps"
    return "cpu"


class OmniVoiceEngine:
    name = "omnivoice"
    rate = 24000

    def __init__(self, runtime: Path, device: str, threads: int):
        import torch
        from omnivoice import OmniVoice, VoiceClonePrompt

        torch.set_num_threads(max(1, threads))
        self.torch = torch
        self.device = device
        self.prompt_class = VoiceClonePrompt
        dtype = torch.float16 if device in ("mps", "cuda") else torch.float32
        # No asr_device: with the reference's words given, OmniVoice never loads its Whisper (1.6 GB).
        self.model = OmniVoice.from_pretrained("k2-fsa/OmniVoice", device_map=device, dtype=dtype)

    def imprint(self, reference: Path, words: str, out: Path) -> None:
        prompt = self.model.create_voice_clone_prompt(ref_audio=str(reference), ref_text=words)
        prompt.save(str(out / "voice.pt"))

    def load_voice(self, folder: Path):
        return self.prompt_class.load(str(folder / "voice.pt"))

    def synthesize(self, text: str, voice) -> np.ndarray:
        audio = self.model.generate(text=text, voice_clone_prompt=voice)
        return np.asarray(audio[0], dtype=np.float32).reshape(-1)


class CosyVoice3Engine:
    name = "cosyvoice3"
    #: CosyVoice 3 wants this before the reference's words (its example.py).
    PROMPT_PREFIX = "You are a helpful assistant.<|endofprompt|>"
    WEIGHTS = "Fun-CosyVoice3-0.5B-2512"

    def __init__(self, runtime: Path, device: str, threads: int):
        sys.path[:0] = [str(runtime / "repo"), str(runtime / "repo" / "third_party" / "Matcha-TTS")]
        mirror = runtime / "wetext-mirror"
        if (mirror / "en" / "tn" / "tagger.fst").exists():
            # The text normaliser's grammars from the local mirror: modelscope refuses or rate-limits downloads.
            import wetext.wetext as wetext_module

            wetext_module.snapshot_download = lambda *args, **kwargs: str(mirror)
        import torch
        from cosyvoice.cli.cosyvoice import CosyVoice3

        torch.set_num_threads(max(1, threads))
        self.torch = torch
        self.device = device
        self.model = CosyVoice3(str(runtime / "pretrained" / self.WEIGHTS))
        self.rate = int(self.model.sample_rate)

    def imprint(self, reference: Path, words: str, out: Path) -> None:
        key = "imprint"
        self.model.add_zero_shot_spk(self.PROMPT_PREFIX + words, str(reference), key)
        self.torch.save(self.model.frontend.spk2info.pop(key), str(out / "voice.pt"))

    def load_voice(self, folder: Path):
        key = f"own-{folder.name}"
        where = getattr(self.model.frontend, "device", "cpu")
        self.model.frontend.spk2info[key] = self.torch.load(str(folder / "voice.pt"), map_location=where, weights_only=True)
        return key

    def synthesize(self, text: str, voice) -> np.ndarray:
        parts = [
            output["tts_speech"].squeeze().detach().cpu().numpy().astype(np.float32)
            for output in self.model.inference_zero_shot(text, "", "", zero_shot_spk_id=voice, stream=False)
        ]
        return np.concatenate(parts) if parts else np.zeros(0, dtype=np.float32)


class VoxCPMEngine:
    """VoxCPM2 (OpenBMB, 2.3B) — 48 kHz, 30 languages with Russian among them. The model itself decides the dtype:
    on MPS it must run in float32 (half precision glitches into retries), so it takes about twice OmniVoice's memory
    there — the gateway's memory guard learns the real peak from the first run."""

    name = "voxcpm"

    def __init__(self, runtime: Path, device: str, threads: int):
        import torch
        from voxcpm import VoxCPM

        torch.set_num_threads(max(1, threads))
        self.torch = torch
        self.device = device
        # The denoiser is a ModelScope model the reference does not need, and torch.compile of a two-billion model
        # costs minutes of start; the server warms up with a real phrase instead (server.py).
        self.model = VoxCPM.from_pretrained(
            "openbmb/VoxCPM2", load_denoiser=False, optimize=False, local_files_only=True, device=device
        )
        self.rate = int(self.model.tts_model.sample_rate)

    def imprint(self, reference: Path, words: str, out: Path) -> None:
        # No separate speaker artifact: every synthesis is given the reference and its words again. voice.pt keeps
        # the words; ref.wav, picked by the imprint, is already next to them.
        self.torch.save({"words": words}, str(out / "voice.pt"))

    def load_voice(self, folder: Path):
        words = self.torch.load(str(folder / "voice.pt"), weights_only=True)["words"]
        return str(folder / "ref.wav"), str(words)

    def synthesize(self, text: str, voice) -> np.ndarray:
        reference, words = voice
        # The demo's «ultimate cloning»: the reference for the timbre, its exact words for the manner.
        audio = self.model.generate(
            text=text, reference_wav_path=reference, prompt_wav_path=reference, prompt_text=words
        )
        return np.asarray(audio, dtype=np.float32).reshape(-1)


def load_engine(name: str, runtime: Path, device: str, threads: int):
    if name == "omnivoice":
        return OmniVoiceEngine(runtime, device, threads)
    if name == "cosyvoice3":
        return CosyVoice3Engine(runtime, device, threads)
    if name == "voxcpm":
        return VoxCPMEngine(runtime, device, threads)
    raise ValueError(f"unknown engine {name!r} (have {', '.join(ENGINES)})")
