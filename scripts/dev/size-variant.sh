#!/bin/bash
# One build variant from scratch, for the size measurements of docs/plans/core-diet-measurements.md («Диета ядра»): the
# release build in its own target folder, its size and the time of a clean build, then the time of a rebuild after
# touching the top crate. Build settings come through the environment (CARGO_PROFILE_*), so no profile in the
# repository changes.
#
# Usage: DIET_TARGETS=<folder for target dirs> scripts/dev/size-variant.sh <name> <station|linux|desktop> [ENV=VAL ...]
#   scripts/dev/size-variant.sh D2-desktop-cu1 desktop CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
#   scripts/dev/size-variant.sh S1-station-z station CARGO_PROFILE_STATION_OPT_LEVEL=z
#   DIET_CONFIG='profile.station.package.ring.opt-level="s"' — extra `cargo --config` values, separated by ";".
# Needs: panel/dist built (it goes into the program), and for desktop desktop/src-tauri/binaries/uv-<target>.
# Results: a line in logs/diet/results.tsv (name, kind, exit codes, bytes, clean s, rebuild s, settings, time, load),
# the program in logs/diet/<name>.bin, the build log in logs/diet/<name>.log. The target folder is removed after.
# Run variants one after another: parallel builds spoil the times. macOS (stat -f, sysctl).
set -u
cd "$(dirname "$0")/../.."
ROOT=$(pwd)
name=$1; kind=$2; shift 2
OUT="$ROOT/logs/diet"
mkdir -p "$OUT"
TD="${DIET_TARGETS:?set DIET_TARGETS to a folder for the target dirs}/$name"
rm -rf "$TD"
export CARGO_INCREMENTAL=0 CARGO_TARGET_DIR="$TD"
for kv in "$@"; do export "$kv"; done
case $kind in
  station) dir=$ROOT; leaf=gateway/src/main.rs; bin=$TD/aarch64-unknown-linux-musl/station/duowave
           cmd=(cargo zigbuild -p duowave --profile station --target aarch64-unknown-linux-musl --no-default-features --features station) ;;
  linux)   dir=$ROOT; leaf=gateway/src/main.rs; bin=$TD/aarch64-unknown-linux-musl/release/duowave
           cmd=(cargo zigbuild -p duowave --release --locked --target aarch64-unknown-linux-musl) ;;
  desktop) dir=$ROOT/desktop/src-tauri; leaf=src/main.rs; bin=$TD/release/duowave-desktop
           cmd=(cargo build --release) ;;
  *) echo "kind: station, linux or desktop" >&2; exit 2 ;;
esac
cd "$dir"
if [ -n "${DIET_CONFIG:-}" ]; then IFS=";" read -ra cfgs <<< "$DIET_CONFIG"; for c in "${cfgs[@]}"; do cmd+=(--config "$c"); done; fi
log="$OUT/$name.log"
la0=$(sysctl -n vm.loadavg | awk '{print $2}')
s=$(date +%s); "${cmd[@]}" > "$log" 2>&1; rc=$?; clean=$(( $(date +%s) - s ))
size=$(stat -f %z "$bin" 2>/dev/null || echo 0)
cp "$bin" "$OUT/$name.bin" 2>/dev/null
touch "$leaf"
s=$(date +%s); "${cmd[@]}" >> "$log" 2>&1; rc2=$?; leafs=$(( $(date +%s) - s ))
la1=$(sysctl -n vm.loadavg | awk '{print $2}')
printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\tload %s→%s\n' "$name" "$kind" "$rc/$rc2" "$size" "$clean" "$leafs" "$*" \
  "$(date '+%H:%M')" "$la0" "$la1" >> "$OUT/results.tsv"
echo "$name: rc=$rc/$rc2 size=$size clean=${clean}s rebuild=${leafs}s [$*] load $la0→$la1"
rm -rf "$TD"
df -h / | tail -1
