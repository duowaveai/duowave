#!/bin/sh
# The «VPN» app's program for a Yandex station (linux-aarch64 as the gateway names it), shoes inside.
#
#   sh apps/own-server/build.sh [<catalog package folder>]
#
# 1. A static build for aarch64 musl with cargo-zigbuild (`cargo install cargo-zigbuild`, zig on PATH), small over fast:
#    opt-level "z", one codegen unit, fat LTO, stripped (Cargo.toml, [profile.release]).
# 2. Given a catalog package folder (duowave-packages/packages/own-server), the file goes to its bin/ and the
#    manifest's version and targets.linux-aarch64.sha256 are set to what was built.
#
# Needs the network once (shoes and its pinned forks come from GitHub, Cargo.lock pins them). The result is
# dist/duowave-own-server-linux-aarch64.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
cd "$here"
version=$(sed -n 's/^version=//p' VERSION)
target=aarch64-unknown-linux-musl
out="$here/dist/duowave-own-server-linux-aarch64"
mkdir -p "$here/dist"
rm -f "$out"

grep -q "^version = \"$version\"$" Cargo.toml || { echo "VERSION ($version) and Cargo.toml differ" >&2; exit 1; }
echo "duowave-own-server $version, shoes $(sed -n 's/.*cfal\/shoes", rev = "\([0-9a-f]\{7\}\).*/\1/p' Cargo.toml)"
cargo zigbuild --locked --release --target "$target"
cp "target/$target/release/duowave-own-server" "$out"
sha=$(shasum -a 256 "$out" | cut -d' ' -f1)
size=$(wc -c < "$out" | tr -d ' ')
echo "$out: $size bytes, gzip $(gzip -9 -c "$out" | wc -c | tr -d ' '), sha256 $sha"

if [ $# -ge 1 ]; then
  package=$(cd "$1" && pwd)
  mkdir -p "$package/bin"
  cp "$out" "$package/bin/duowave-own-server-linux-aarch64"
  node -e '
    const fs = require("fs");
    const [file, version, sha] = process.argv.slice(1);
    const manifest = JSON.parse(fs.readFileSync(file, "utf8"));
    manifest.version = version;
    manifest.targets["linux-aarch64"].sha256 = sha;
    fs.writeFileSync(file, JSON.stringify(manifest, null, 2) + "\n");
  ' "$package/manifest.json" "$version" "$sha"
  echo "$package: bin/duowave-own-server-linux-aarch64 and manifest.json ($version) updated"
fi
