#!/usr/bin/env bash
# The station emulator KUYASE (github.com/vypivshiy/kuyase), built on this machine's libc (the upstream Linux binary
# wants a newer glibc than CI runners have): its CLI from a pinned source commit, its ARM guest bridge and kernel from
# the pinned portable release. Both archives are checked by SHA-256. The pins are the ones Quasar Bridge tests with
# (its scripts/kuyase/build-emulator.sh, Apache-2.0).
#
#   scripts/emulator/build.sh <folder>     # needs Rust (cargo +stable) and curl; Linux only
set -euo pipefail
mkdir -p "$1"
out=$(cd "$1" && pwd)
src="$out/source"
mkdir -p "$src"

curl --fail --location --retry 3 --max-time 180 \
    https://codeload.github.com/vypivshiy/kuyase/tar.gz/0cd9adac36e167fc0dbc934ce9302590a4318856 --output "$out/source.tar.gz"
echo "4207c594f4c34b7c5da6c6f29e2e4d76749713fdefec61ef35e0852fd023412e  $out/source.tar.gz" | sha256sum --check --strict
tar -xzf "$out/source.tar.gz" --strip-components=1 -C "$src"

curl --fail --location --retry 3 --max-time 180 \
    https://github.com/vypivshiy/kuyase/releases/download/1.4.0/kuyase-linux64-portable.tar.gz --output "$out/portable.tar.gz"
echo "87292ceb4722db3710cbfc5457570bbbdc62b60950e76d64cae98589348800c9  $out/portable.tar.gz" | sha256sum --check --strict
tar -xzf "$out/portable.tar.gz" -C "$out" qemu-hw-bridge kuyase-kernel-arm64.img
mkdir -p "$src/guest-init/bin"
cp "$out/qemu-hw-bridge" "$src/guest-init/bin/qemu-hw-bridge"

(cd "$src" && CARGO_TARGET_DIR="$out/target" cargo +stable build --locked --release -p kuyase)
cp "$out/target/release/kuyase" "$out/kuyase"
chmod 755 "$out/kuyase" "$out/qemu-hw-bridge"
"$out/kuyase" --help >/dev/null
echo "kuyase: $out/kuyase"
