#!/bin/sh
# The station's one-line install for a release: station-client/station.sh with the release's version and the SHA-256 of
# its station build written in, so the script downloads that very build and checks it (plain HTTP on the station:
# its busybox has no TLS). release.yml runs it beside SHA256SUMS; the catalog's site takes the result with the build
# (duowave-packages, pages.yml), and get.duowave.app serves both over plain HTTP: /station.sh and the build at
# /station/<version>/ (scripts/release/cloudflare).
#
#   scripts/release/station-script.sh <duowave-station-aarch64.tar.gz> <version> <out>
#   scripts/release/station-script.sh release/duowave-station-aarch64.tar.gz 0.18.3 release/station.sh
set -eu
archive=${1:?the station build: duowave-station-aarch64.tar.gz}
version=${2:?the version: 0.18.3}
out=${3:?where to write the script}
here=$(cd "$(dirname "$0")/../.." && pwd)
version=${version#v}

case $version in
    '' | *[!0-9A-Za-z.+-]*) echo "not a version: $version" >&2; exit 1 ;;
esac
# The build says its own version (release.yml's Archive step): the script and the build are one release.
release=$(tar -xzOf "$archive" release.json)
case $release in
    *'"version":"'"$version"'"'*'"platform":"duowave-station-aarch64"'*) ;;
    *) echo "the build is not $version for the station: $release" >&2; exit 1 ;;
esac
if command -v sha256sum >/dev/null 2>&1; then
    sha=$(sha256sum "$archive" | cut -d' ' -f1)
else
    sha=$(shasum -a 256 "$archive" | cut -d' ' -f1)
fi
sed -e "s/@VERSION@/$version/" -e "s/@SHA256@/$sha/" "$here/station-client/station.sh" >"$out"
grep -q '@VERSION@\|@SHA256@' "$out" && { echo "placeholders left in $out" >&2; exit 1; }
chmod 755 "$out"
echo "$out: $version, $sha"
