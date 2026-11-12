#!/usr/bin/env python3
"""uv for the desktop app's installer (docs/plans/speech-servers.md, stage 5).

The app sets up Python for its own voice and the voice lab with uv, so a new computer needs nothing installed by hand.
Tauri takes uv as an external binary (bundle.externalBin "binaries/uv" in desktop/src-tauri/tauri.conf.json) and
wants it at desktop/src-tauri/binaries/uv-<target triple>[.exe] when the app builds — `cargo check` of the app too.
This script puts it there: the pinned uv release for the target from GitHub, checked against its SHA-256 below.

    python3 scripts/release/fetch-uv.py [target triple]     # default: rustc's host

Only the standard library: it runs on the build machines of all three systems before anything is installed.
"""

import hashlib
import io
import os
import subprocess
import sys
import tarfile
import urllib.request
import zipfile
from pathlib import Path

VERSION = "0.13.5"
# The release's own .sha256 files, taken 12.11.2026.
SHA256 = {
    "aarch64-apple-darwin": "743f85035a1996ecb350659e01cd9faea9709829be09b1b4b03c73eaf3c9cd73",
    "x86_64-apple-darwin": "b0f52b0e3945b4107d89024ea38e625683011fbfc05a0d98f9fa2d6dcee28c66",
    "x86_64-pc-windows-msvc": "a38b8d1902b7cfbc76b423ba1bb456f623f42064cdc72628b6075b89afd1019a",
    "aarch64-pc-windows-msvc": "09f47edf90726084a70f9426f6e3ca4f46e9bf7cbf23b1716ec64d60b055fae3",
    "x86_64-unknown-linux-gnu": "f68d291c75783122dad58ab77def0e13f3233f358731db5d6d7e8715603b928f",
    "aarch64-unknown-linux-gnu": "4bb69cf95ed13f23713bb7bc09f241896ea0c75b7c557e615a20604b82f9c403",
}

ROOT = Path(__file__).resolve().parents[2]
BINARIES = ROOT / "desktop" / "src-tauri" / "binaries"


def host_target() -> str:
    output = subprocess.run(["rustc", "-vV"], capture_output=True, text=True, check=True).stdout
    for line in output.splitlines():
        if line.startswith("host:"):
            return line.split(":", 1)[1].strip()
    sys.exit("fetch-uv: rustc -vV names no host")


def main() -> None:
    target = sys.argv[1] if len(sys.argv) > 1 and sys.argv[1] else host_target()
    if target not in SHA256:
        sys.exit(f"fetch-uv: no uv {VERSION} for {target} (known: {', '.join(sorted(SHA256))})")
    windows = "windows" in target
    destination = BINARIES / (f"uv-{target}.exe" if windows else f"uv-{target}")
    stamp = destination.with_name(destination.name + ".version")
    if destination.is_file() and stamp.is_file() and stamp.read_text().strip() == f"{VERSION} {SHA256[target]}":
        print(f"fetch-uv: {destination.relative_to(ROOT)} is uv {VERSION} already")
        return
    archive = f"uv-{target}.zip" if windows else f"uv-{target}.tar.gz"
    url = f"https://github.com/astral-sh/uv/releases/download/{VERSION}/{archive}"
    print(f"fetch-uv: {url}")
    with urllib.request.urlopen(url, timeout=120) as response:
        data = response.read()
    digest = hashlib.sha256(data).hexdigest()
    if digest != SHA256[target]:
        sys.exit(f"fetch-uv: {archive} has SHA-256 {digest}, expected {SHA256[target]} — not used")
    name = "uv.exe" if windows else "uv"
    program = None
    if windows:
        with zipfile.ZipFile(io.BytesIO(data)) as zipped:
            for member in zipped.namelist():
                if member.split("/")[-1] == name:
                    program = zipped.read(member)
                    break
    else:
        with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as tarred:
            for member in tarred.getmembers():
                if member.isfile() and member.name.split("/")[-1] == name:
                    extracted = tarred.extractfile(member)
                    program = extracted.read() if extracted else None
                    break
    if not program:
        sys.exit(f"fetch-uv: no {name} inside {archive}")
    BINARIES.mkdir(parents=True, exist_ok=True)
    partial = destination.with_name(destination.name + ".part")
    partial.write_bytes(program)
    os.chmod(partial, 0o755)
    os.replace(partial, destination)
    stamp.write_text(f"{VERSION} {SHA256[target]}\n")
    print(f"fetch-uv: {destination.relative_to(ROOT)} — uv {VERSION}, {len(program) // 1_000_000} MB")


if __name__ == "__main__":
    main()
