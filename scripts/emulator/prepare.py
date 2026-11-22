#!/usr/bin/env python3
"""A station's firmware for the emulator, from a public OTA pinned by SHA-256: its root file system read out of the
OTA (an SWUpdate CPIO with a UBIFS image in it) with the files' modes kept, handed to `kuyase prepare`. The OTA's own
installer never runs. The Станция 2 and Мини images are the ones Quasar Bridge tests on (its scripts/kuyase/prepare.py,
Apache-2.0); the Лайт 1 and Мини 2 links are the official ones of the emulator author's catalog
(huggingface.co/datasets/vypivshiy/iot-quasar-dataset), downloaded from Yandex itself. The Лайт 2 has no public OTA
link. The board names are the emulator's own.

    python3 scripts/emulator/prepare.py --board station2 --emulator <kuyase folder> --output <folder>

Needs ubi-reader (pip install ubi-reader==0.8.16) and curl."""
import argparse
import hashlib
import io
import shutil
import subprocess
import sys
import tarfile
from pathlib import Path

# board: (OTA, its SHA-256, the SHA-256 of the UBIFS root file system inside it)
FIRMWARE = {
    "station2": (
        "https://quasar.s3.yandex.net/yandexmidi/ota/user/"
        "949c532f-f0fe-4363-95c8-5479c410dfd1/0.373.1.13.4819000379.20261030.299.zip",
        "22e6783c88b6947c8a7a2243e9511dede07f12df083edae89b95a2ee65f0fc2e",
        "ec7857bd96eed02785596e31b795b2a2154343e1cfbbb2124b157cd42bec4aee",
    ),
    "yandexmini": (
        "https://quasar.s3.yandex.net/yandexmini/ota/release/"
        "d165ea71-76cd-4df5-84f8-0441526d1124/1.373.4.13.4819025044.20261030.zip",
        "80ca52832f78403335faa45e050c5828d63facdd89dba9f456de5d82fd20c98f",
        "c057fbafcd960f0f0329400040cf974a749b74f8261be2e858d164ee7087c1c4",
    ),
    "yandexmicro": (  # Лайт 1
        "https://quasar.s3.yandex.net/yandexmicro/ota/user/"
        "dbb517ff-eac2-4ba7-acb6-da7f21c3de80/0.372.1.8.4790672876.20261019.300.zip",
        "713b0d7110bf87201d3d2eca61785fed9c6305fa98d7f459eb0db72624b0a8ce",
        "227b37c5d771415bdfdb4dcfe978f537b6bbb52ba1f20293670a0bb12cde5601",
    ),
    "yandexmini2": (  # Мини 2 (the firmware is yandexmini_2, the same with and without the clock)
        "https://quasar.s3.yandex.net/yandexmini_2/ota/user/"
        "07756f40-2d2c-4d08-83be-5c35372c5e7c/0.372.1.8.4790679165.20261019.302.zip",
        "7255b82c4bd71d925f634a06247c80464667d03ebe65af5b9491e7f7f847c95f",
        "78aee6f52d865609049a56ce4d8dcf0ea4882dd62f526495893835390c34785e",
    ),
}


def checked(data, digest, what):
    if hashlib.sha256(data).hexdigest() != digest:
        raise SystemExit(f"{what}: SHA-256 is not the pinned one")
    return data


def member(data, wanted):
    """One member of an SWUpdate newc CPIO, read in memory: nothing is extracted to a path."""
    at = 0
    while at + 110 <= len(data):
        head = data[at:at + 110]
        if head[:6] not in (b"070701", b"070702"):
            raise SystemExit("not an SWUpdate CPIO")
        size, name_size = int(head[54:62], 16), int(head[94:102], 16)
        name = data[at + 110:at + 110 + name_size].rstrip(b"\0").decode()
        start = (at + 110 + name_size + 3) & ~3
        end = start + size
        if not name_size or end > len(data):
            raise SystemExit("a truncated CPIO member")
        if name == wanted:
            return data[start:end]
        if name == "TRAILER!!!":
            break
        at = (end + 3) & ~3
    raise SystemExit(f"no {wanted} in the OTA")


def root_tar(image, folder, board):
    """The UBIFS image's files into a tar, each with its mode and owner as the image has them (no root needed)."""
    from ubireader.scripts import ubireader_extract_files
    from ubireader.ubifs import output

    kept = {}

    def keep(path, inode):
        info = inode["ino"]
        kept[str(Path(path).relative_to(folder))] = (info.mode & 0o7777, info.uid, info.gid)

    output._set_file_perms = keep
    sys.argv = ["ubireader_extract_files", "-k", "-o", str(folder), str(image)]
    ubireader_extract_files.main()
    if board == "yandexmini":
        # The Mini keeps its stock adbd under another name: the emulator reaches the guest by adb.
        shutil.copyfile(folder / "usr/bin/adbd_backup", folder / "usr/bin/adbd")
        kept["usr/bin/adbd"] = (0o755, 0, 0)

    def as_kept(info):
        info.mode, info.uid, info.gid = kept.get(info.name, (info.mode, 0, 0))
        info.uname = info.gname = ""
        info.pax_headers = {}
        return info

    archive = folder.with_suffix(".tar")
    with tarfile.open(archive, "w", format=tarfile.PAX_FORMAT) as tar:
        for path in sorted(folder.iterdir()):
            tar.add(path, arcname=path.name, filter=as_kept)
    return archive


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--board", choices=FIRMWARE, required=True)
    parser.add_argument("--emulator", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    url, ota_sha, root_sha = FIRMWARE[args.board]
    ota = out / "ota.zip"
    subprocess.run(["curl", "--fail", "--location", "--retry", "3", "--max-time", "300", url, "--output", str(ota)],
                   check=True)
    data = checked(ota.read_bytes(), ota_sha, "the OTA")
    if args.board == "yandexmini":
        with tarfile.open(fileobj=io.BytesIO(data), mode="r:xz") as archive:
            data = archive.extractfile("./yandex_io_ota_enc.swu").read()
    image = out / "rootfs.ubifs"
    image.write_bytes(checked(member(data, "rootfs.ubifs"), root_sha, "the root file system"))
    archive = root_tar(image, out / "rootfs", args.board)
    emulator = args.emulator.resolve()
    (emulator / "work").mkdir(exist_ok=True)
    log = out / "prepare.log"
    try:
        with log.open("w") as sink:
            subprocess.run([str(emulator / "kuyase"), "prepare", "--board", args.board, "--dump", str(archive)],
                           cwd=emulator, stdout=sink, stderr=subprocess.STDOUT, check=True, timeout=300)
    finally:
        print(log.read_text(errors="replace"), end="", flush=True)
    if not (emulator / "work" / f"{args.board}-system.ext4").is_file():
        raise SystemExit("kuyase made no system image")


if __name__ == "__main__":
    main()
