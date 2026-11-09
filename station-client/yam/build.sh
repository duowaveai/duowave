#!/bin/sh
# Builds the YAM 0.2 package duowave/gateway (repo.yse.wtf): a .yampkg per target in yam.yaml — the `duowave` command,
# the postinstall hook that turns the gateway on, the call client and its service, voice.sh with the boot script
# S95duowave (where the voice goes: our copy of quasar.cfg mounted over the firmware's — `duowave restore` works with
# them alone), and the voice gateway built for the station (aarch64 musl). The version is the Cargo workspace's.
#
#   cargo zigbuild -p duowave --profile station --target aarch64-unknown-linux-musl --no-default-features --features station
#   station-client/yam/build.sh [out-dir]     # default: dist-station/
#
# Needs yam-publish from YAM (https://github.com/nyakokitsu/yam: cargo build --release --bin yam-publish) on PATH or
# in YAM_PUBLISH, and YAM's platform profiles in YAM_PROFILES (its platforms/ folder). GATEWAY_BIN names another build
# of the program; YAM_STAGE keeps the package's source folder there (yam.yaml with the version and the files), which
# `yam-publish publish --source` reads. Publishing is release.yml's (by hand: station-client/README.md).
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
OUT=${1:-$ROOT/dist-station}
YAM_PUBLISH=${YAM_PUBLISH:-yam-publish}
: "${YAM_PROFILES:?YAM_PROFILES: the platforms folder of the YAM repository}"
VERSION=$(sed -n 's/^version *= *"\([^"]*\)".*/\1/p' "$ROOT/Cargo.toml" | head -n 1)
[ -n "$VERSION" ] || { echo "no version in $ROOT/Cargo.toml" >&2; exit 1; }
GATEWAY_BIN=${GATEWAY_BIN:-$ROOT/target/aarch64-unknown-linux-musl/station/duowave}
[ -x "$GATEWAY_BIN" ] || { echo "no gateway program at $GATEWAY_BIN: build it for the station first (above)" >&2; exit 1; }
TARGETS=$(sed -n 's/^targets: *\[\(.*\)\].*/\1/p' "$HERE/yam.yaml" | tr ',' ' ')
[ -n "$TARGETS" ] || { echo "no targets in $HERE/yam.yaml" >&2; exit 1; }

if [ -n "${YAM_STAGE:-}" ]; then
    rm -rf "$YAM_STAGE" && mkdir -p "$YAM_STAGE"
    STAGE=$(cd "$YAM_STAGE" && pwd)
else
    STAGE=$(mktemp -d)
    trap 'rm -rf "$STAGE"' EXIT
fi
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
sed "s/^version: .*/version: $VERSION/" "$HERE/yam.yaml" > "$STAGE/yam.yaml"
cp "$HERE/duowave" "$HERE/duowave-postinstall" "$ROOT/station-client/voice.sh" "$ROOT/station-client/S95duowave" \
    "$ROOT/station-client/call-client.sh" "$ROOT/station-client/S99zduowave" "$STAGE/"
cp "$GATEWAY_BIN" "$STAGE/gateway"
# tar keeps the modes, and YAM installs a file with an executable bit as 0755.
chmod 755 "$STAGE/duowave" "$STAGE/duowave-postinstall" "$STAGE/voice.sh" "$STAGE/S95duowave" "$STAGE/call-client.sh" \
    "$STAGE/S99zduowave" "$STAGE/gateway"

for target in $TARGETS; do
    package=$OUT/duowave-gateway-$VERSION-$target.yampkg
    "$YAM_PUBLISH" pack --source "$STAGE" --platform "$target" --profiles "$YAM_PROFILES" --output "$package" >&2
    sha=$( (sha256sum "$package" 2>/dev/null || shasum -a 256 "$package") | cut -d' ' -f1)
    echo "$package ($sha)"
done
