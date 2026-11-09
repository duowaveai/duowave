#!/bin/sh
# One version for everything DuoWave ships: the gateway (Cargo.toml's workspace), the desktop app (its Cargo.toml and
# tauri.conf.json), the panel and the root package.json, both Cargo.lock files, and the Home Assistant add-on (its
# version and the gateway image it is built on). The gateway compares the feed with its own version, so a tag without
# the version raised would offer the same release forever: release.yml runs --check.
#
#   scripts/release/version.sh                 the version now
#   scripts/release/version.sh 0.2.0           set it everywhere (then commit; the tag is v0.2.0)
#   scripts/release/version.sh --check v0.2.0  fail unless every place says 0.2.0
set -eu
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
cd "$ROOT"

# Every place, as "file|perl expression that prints the version found there".
places() {
    cat <<'EOF'
Cargo.toml|/\[workspace\.package\]\s*\nversion = "([^"]+)"/ and print $1
desktop/src-tauri/Cargo.toml|/\[package\]\s*\nname = "duowave-desktop"\s*\nversion = "([^"]+)"/ and print $1
desktop/src-tauri/tauri.conf.json|/"version":\s*"([^"]+)"/ and print $1
package.json|/"version":\s*"([^"]+)"/ and print $1
panel/package.json|/"version":\s*"([^"]+)"/ and print $1
Cargo.lock|/name = "duowave"\nversion = "([^"]+)"/ and print $1
desktop/src-tauri/Cargo.lock|/name = "duowave"\nversion = "([^"]+)"/ and print $1
desktop/src-tauri/Cargo.lock|/name = "duowave-desktop"\nversion = "([^"]+)"/ and print $1
deploy/home-assistant/duowave/config.yaml|/^version: "([^"]+)"/m and print $1
deploy/home-assistant/duowave/build.yaml|/aarch64: "[^"]+:([^"]+)"/ and print $1
deploy/home-assistant/duowave/build.yaml|/amd64: "[^"]+:([^"]+)"/ and print $1
EOF
}

found() { perl -0ne "$2" "$1"; }

case "${1:-}" in
"")
    found Cargo.toml '/\[workspace\.package\]\s*\nversion = "([^"]+)"/ and print $1'
    echo
    ;;
--check)
    want=${2:?usage: version.sh --check vX.Y.Z}
    want=${want#v}
    problems=$(places | while IFS='|' read -r file expr; do
        have=$(found "$file" "$expr")
        [ "$have" = "$want" ] || echo "  $file says ${have:-nothing}"
    done)
    if [ -n "$problems" ]; then
        printf 'not %s everywhere:\n%s\nraise it first: scripts/release/version.sh %s\n' "$want" "$problems" "$want" >&2
        exit 1
    fi
    echo "every place says $want"
    ;;
*)
    new=${1#v}
    echo "$new" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$' || { echo "not a version: $1" >&2; exit 2; }
    V=$new perl -0pi -e 's/(\[workspace\.package\]\s*\nversion = ")[^"]+"/$1$ENV{V}"/' Cargo.toml
    V=$new perl -0pi -e 's/(\[package\]\s*\nname = "duowave-desktop"\s*\nversion = ")[^"]+"/$1$ENV{V}"/' desktop/src-tauri/Cargo.toml
    for file in desktop/src-tauri/tauri.conf.json package.json panel/package.json; do
        V=$new perl -0pi -e 's/("version":\s*")[^"]+"/$1$ENV{V}"/' "$file"
    done
    for file in Cargo.lock desktop/src-tauri/Cargo.lock; do
        V=$new perl -0pi -e 's/(name = "duowave(?:-desktop)?"\nversion = ")[^"]+"/$1$ENV{V}"/g' "$file"
    done
    V=$new perl -0pi -e 's/^(version: ")[^"]+"/$1$ENV{V}"/m' deploy/home-assistant/duowave/config.yaml
    V=$new perl -0pi -e 's/((?:aarch64|amd64): "[^":]+:)[^"]+"/$1$ENV{V}"/g' deploy/home-assistant/duowave/build.yaml
    "$0" --check "$new"
    ;;
esac
