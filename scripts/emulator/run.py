#!/usr/bin/env python3
"""The station's scripts in the emulator: a throwaway station VM (KUYASE, stub mode, a snapshot — every change goes
with it) gets station-client/ by adb and runs station-client/tests/station-check.sh in it: voice.sh's bind mount over
the firmware's own quasar.cfg on the firmware's busybox and a real kernel, the boot script, gateway.sh's pid files.

    python3 scripts/emulator/run.py --board station2 --emulator <kuyase folder> --output <folder>

Needs adb and qemu-system-aarch64 (with ipxe-qemu for its network card), the emulator built (build.sh) and the
firmware prepared (prepare.py). Ports 127.0.0.1:15555, 18090, 18091 must be free; nothing else is touched."""
import argparse
import os
import re
import signal
import socket
import subprocess
import time
from pathlib import Path

ADB = ["adb", "-s", "127.0.0.1:15555"]
GUEST = "/data/duowave-ci"
HERE = Path(__file__).resolve().parents[2]


def shell(command, timeout=120):
    # An old station adbd does not hand back the exit code: it comes as the last line.
    wrapped = "( " + command + " ); printf '\\n__RC=%s\\n' \"$?\""
    output = subprocess.check_output(ADB + ["shell", wrapped], timeout=timeout).decode(errors="replace")
    output = output.replace("\r", "")
    found = re.search(r"\n__RC=(\d+)\s*$", output)
    if not found:
        raise SystemExit(f"no exit code from the guest:\n{output}")
    return int(found[1]), output[:found.start()].strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--board", choices=("station2", "yandexmini"), required=True)
    parser.add_argument("--emulator", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    emulator, out = args.emulator.resolve(), args.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    for port in (15555, 18090, 18091):
        with socket.socket() as probe:
            probe.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            probe.bind(("127.0.0.1", port))
    log = (out / "vm.log").open("wb")
    vm = subprocess.Popen([str(emulator / "kuyase"), "run", "--headless", "--board", args.board, "--mode", "stub",
                           "--snapshot", "--keep-alive"], cwd=emulator, stdin=subprocess.PIPE, stdout=log,
                          stderr=subprocess.STDOUT, start_new_session=True)
    code = 1
    try:
        deadline = time.monotonic() + 240
        while f"Headless VM '{args.board}' READY" not in (out / "vm.log").read_text(errors="replace"):
            if vm.poll() is not None or time.monotonic() >= deadline:
                raise SystemExit("the VM did not come up; see vm.log")
            time.sleep(1)
        subprocess.run(["adb", "connect", "127.0.0.1:15555"], check=True, timeout=15)
        _, name = shell("hostname")
        if not name.startswith("qemu-"):
            raise SystemExit(f"connected to {name!r}, not the emulator: stopping")
        (out / "guest.txt").write_text(shell("uname -a; busybox 2>&1 | head -n 1")[1] + "\n")
        shell(f"rm -rf {GUEST}; mkdir -p {GUEST}/tests")
        for name in ("voice.sh", "S95duowave", "gateway.sh", "tests/station-check.sh"):
            subprocess.run(ADB + ["push", str(HERE / "station-client" / name), f"{GUEST}/{name}"], check=True,
                           timeout=60)
        code, said = shell(f"STATION_CHECK_SANDBOX=1 sh {GUEST}/tests/station-check.sh {GUEST}", timeout=300)
        (out / "station-check.txt").write_text(said + "\n")
        print(said, flush=True)
    finally:
        try:
            os.killpg(vm.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            vm.wait(timeout=15)
        except subprocess.TimeoutExpired:
            vm.kill()
        log.close()
        serial = emulator / "work" / f"{args.board}-serial.log"
        if serial.is_file():
            (out / "qemu-serial.log").write_text(serial.read_text(errors="replace"))
    raise SystemExit(code)


if __name__ == "__main__":
    main()
