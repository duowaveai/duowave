#!/bin/sh
# DuoWave onto a Yandex Station from this computer, in one line (macOS, Linux):
#   curl -fsSL https://github.com/duowaveai/duowave/releases/latest/download/install.sh | sh
# It downloads DuoWave for this computer (the gateway alone: the station's signed build is inside it), checks the file
# against the release's SHA256SUMS and runs `duowave station setup` (gateway/src/placement/setup.rs): the station is
# found in the home network, looked at over SSH, nothing changes before your «да», then DuoWave goes onto the
# station. Nothing stays on this computer: the program lives in a temporary folder, removed at the end.
# The station needs root and SSH first: https://hackalice.me/
# Arguments go to the setup: `| sh -s -- 192.168.1.20` names the station's address.
set -eu

REPO="${DUOWAVE_REPO:-duowaveai/duowave}"
VERSION="${DUOWAVE_VERSION:-latest}"

case "${LC_ALL:-${LC_MESSAGES:-${LANG:-}}}" in
ru*) ru=1 ;;
*) ru= ;;
esac
say() { if [ -n "$ru" ]; then echo "$1"; else echo "$2"; fi; }
die() {
    say "$1" "$2" >&2
    exit 1
}

case "$(uname -s)" in
Darwin) os=darwin ;;
Linux) os=linux ;;
*) die "Эта система не поддерживается: $(uname -s). На Windows: irm https://github.com/$REPO/releases/latest/download/install.ps1 | iex" \
    "This system is not supported: $(uname -s). On Windows: irm https://github.com/$REPO/releases/latest/download/install.ps1 | iex" ;;
esac
case "$(uname -m)" in
x86_64 | amd64) arch=x86_64 ;;
arm64 | aarch64) arch=aarch64 ;;
*) die "Этот процессор не поддерживается: $(uname -m)" "This processor is not supported: $(uname -m)" ;;
esac

# DUOWAVE_BASE: another place with the same files (a mirror; a folder served for a check).
if [ -n "${DUOWAVE_BASE:-}" ]; then
    base="$DUOWAVE_BASE"
elif [ "$VERSION" = latest ]; then
    base="https://github.com/$REPO/releases/latest/download"
else
    base="https://github.com/$REPO/releases/download/$VERSION"
fi
name="duowave-$os-$arch.tar.gz"

dir="$(mktemp -d)"
trap 'rm -rf "$dir"' EXIT INT TERM

fetch() {
    if command -v curl >/dev/null 2>&1; then
        if [ -n "${3:-}" ]; then curl -fL --retry 2 --progress-bar -o "$2" "$1"; else curl -fsSL --retry 2 -o "$2" "$1"; fi
    elif command -v wget >/dev/null 2>&1; then
        if [ -n "${3:-}" ]; then wget -q --show-progress -O "$2" "$1"; else wget -q -O "$2" "$1"; fi
    else
        die "Нужен curl или wget." "curl or wget is needed."
    fi
}

say "Скачиваю DuoWave для этого компьютера ($os/$arch)…" "Downloading DuoWave for this computer ($os/$arch)…"
fetch "$base/$name" "$dir/$name" progress || die "Не скачалось: $base/$name" "Download failed: $base/$name"
fetch "$base/SHA256SUMS" "$dir/SHA256SUMS" || die "В выпуске нет SHA256SUMS — проверить файл нечем, не запускаю." \
    "The release has no SHA256SUMS — nothing to check the file against, not running it."

want="$(awk -v n="$name" '$2 == n || $2 == "*" n { print $1; exit }' "$dir/SHA256SUMS")"
if command -v sha256sum >/dev/null 2>&1; then
    got="$(sha256sum "$dir/$name" | cut -d' ' -f1)"
else
    got="$(shasum -a 256 "$dir/$name" | cut -d' ' -f1)"
fi
[ -n "$want" ] || die "В SHA256SUMS нет $name — не запускаю." "$name is not in SHA256SUMS — not running it."
[ "$want" = "$got" ] || die "Контрольная сумма не совпала: файл повреждён или подменён. Попробуйте ещё раз." \
    "The checksum does not match: the file is damaged or replaced. Try again."

tar -xzf "$dir/$name" -C "$dir" duowave
chmod +x "$dir/duowave"

# The setup asks questions: when this script comes through a pipe (curl … | sh), its stdin is the script itself,
# so the answers come from the terminal.
if [ -r /dev/tty ] && (: </dev/tty) 2>/dev/null; then
    "$dir/duowave" station setup "$@" </dev/tty
else
    "$dir/duowave" station setup "$@"
fi
