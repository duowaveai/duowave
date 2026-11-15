#!/bin/sh
# The AirPlay app's program for a Yandex station (linux-aarch64 as the gateway names it), with its sound output inside.
#
#   sh apps/airplay/build.sh [<catalog package folder>]
#
# 1. A static build: CGO_ENABLED=0, GOOS=linux, GOARCH=arm64.
# 2. station-pcm-package of the same station-airplay as go.mod pins (a tool of this module) appends the PCM output modules, ARMv7 and ARM64:
#    on a Lite 2 the firmware's ALSA is 32-bit, the receiver picks the module by it. Without this step there is no sound.
# 3. Given a catalog package folder (duowave-packages/packages/airplay), the file goes to its bin/ and the manifest's
#    version and targets.linux-aarch64.sha256 are set to what was built.
#
# Needs Go 1.26 or newer and the network once (Go modules). The result is dist/duowave-airplay-linux-aarch64.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
cd "$here"
version=$(sed -n 's/^version=//p' VERSION)
library=$(go list -m -f '{{.Version}}' github.com/skrashevich/station-airplay)
out="$here/dist/duowave-airplay-linux-aarch64"
mkdir -p "$here/dist"
rm -f "$out"

echo "duowave-airplay $version, station-airplay $library"
CGO_ENABLED=0 GOOS=linux GOARCH=arm64 go build -trimpath -buildvcs=false -ldflags "-s -w -X main.version=$version" -o "$out" .
# The packager and the module it builds (station-pcm) are tools of this module (go.mod `tool`): the same version as
# the library, its dependencies pinned in go.sum. It runs on this machine and builds the modules for the station itself.
CGO_ENABLED=0 go tool station-pcm-package -arch arm64 -binary "$out"
sha=$(shasum -a 256 "$out" | cut -d' ' -f1)
size=$(wc -c < "$out" | tr -d ' ')
echo "$out: $size bytes, sha256 $sha"

if [ $# -ge 1 ]; then
  package=$(cd "$1" && pwd)
  mkdir -p "$package/bin"
  cp "$out" "$package/bin/duowave-airplay-linux-aarch64"
  node -e '
    const fs = require("fs");
    const [file, version, sha] = process.argv.slice(1);
    const manifest = JSON.parse(fs.readFileSync(file, "utf8"));
    manifest.version = version;
    manifest.targets["linux-aarch64"].sha256 = sha;
    fs.writeFileSync(file, JSON.stringify(manifest, null, 2) + "\n");
  ' "$package/manifest.json" "$version" "$sha"
  echo "$package: bin/duowave-airplay-linux-aarch64 and manifest.json ($version) updated"
fi
