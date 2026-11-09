#!/bin/sh
# Installs the gateway (duowave), its recognition and our own voice as system launchd services on this Mac: they
# start at boot (no login needed), restart when they exit, and keep the Mac awake while the gateway runs. Changes the
# system: it asks for sudo itself — run it as yourself, not with sudo.
#
#   deploy/macos/install.sh                          # gateway, ASR, and our voice when .env has TTS_OWN_URL
#   deploy/macos/install.sh gateway                  # the gateway only
#   DUOWAVE=path/to/duowave deploy/macos/install.sh  # a program built elsewhere (a release's duowave-darwin-*)
#
# The program is built here from this checkout — the panel (pnpm --filter panel build: it goes inside the program),
# then cargo build --release, with Silero VAD on Apple Silicon (DUOWAVE_FEATURES overrides it, "" for none) — unless
# DUOWAVE names one. It is installed as target/service/duowave: later builds in target/ reach the service only when
# this script runs again. The gateway runs in this folder with the settings of its .env (PORT, UNIPROXY_MODE, the
# keys). Stop a gateway started by hand first: the port is one. Undo: deploy/macos/uninstall.sh.
set -eu
DIR=$(cd "$(dirname "$0")/../.." && pwd)
PROGRAM=${DUOWAVE:-}
# A path given from elsewhere, before the script moves to the project folder.
case "$PROGRAM" in '' | /*) ;; *) PROGRAM="$(pwd)/$PROGRAM" ;; esac
cd "$DIR"
# The service runs as the owner of this folder, and the program it runs must stay theirs: the gateway replaces it
# when it updates itself. A build as root would leave a target/ the next build cannot write.
[ "$(id -u)" -ne 0 ] || { echo "run it as yourself, not with sudo: it asks for sudo when it needs it" >&2; exit 1; }
USER_NAME=$(id -un)
SERVICE_PROGRAM=target/service/duowave
[ -f .env ] || { echo "no .env: settings come from it" >&2; exit 1; }

if [ -z "$PROGRAM" ]; then
    command -v cargo >/dev/null || { echo "no cargo: install Rust (https://rustup.rs), or set DUOWAVE to a built duowave" >&2; exit 1; }
    if command -v pnpm >/dev/null; then
        pnpm --filter panel build
    elif [ ! -f panel/dist/index.html ]; then
        echo "note: no pnpm and no panel/dist: the program goes without its panel (/panel will say so)" >&2
    fi
    # Silero's ONNX Runtime comes prebuilt for Apple Silicon: an Intel Mac hears the end of speech by energy.
    if [ "$(uname -m)" = arm64 ]; then default_features=silero; else default_features=; fi
    features=${DUOWAVE_FEATURES-$default_features}
    cargo build -p duowave --release ${features:+--features "$features"}
    PROGRAM=target/release/duowave
fi
[ -f "$PROGRAM" ] || { echo "no program at $PROGRAM" >&2; exit 1; }
"$PROGRAM" version >/dev/null || { echo "$PROGRAM does not run on this Mac" >&2; exit 1; }

# A copy for the service, put in place by a rename: a running gateway keeps its file until it restarts.
mkdir -p target/service
cp "$PROGRAM" "$SERVICE_PROGRAM.new"
mv -f "$SERVICE_PROGRAM.new" "$SERVICE_PROGRAM"
echo "program: $DIR/$SERVICE_PROGRAM ($("$SERVICE_PROGRAM" version))"

# launchd's PATH is the system's: add the folders of node (npx) and uvx, for MCP servers run as programs.
TOOLS_PATH=
for tool in node uvx; do
    found=$(command -v "$tool" 2>/dev/null || true)
    case "$found" in /*) TOOLS_PATH="$TOOLS_PATH:$(dirname "$found")" ;; esac
done

install_one() {
    label=$1 script=$2
    plist="/Library/LaunchDaemons/$label.plist"
    tmp=$(mktemp)
    sed -e "s#__LABEL__#$label#g" -e "s#__USER__#$USER_NAME#g" -e "s#__DIR__#$DIR#g" -e "s#__SCRIPT__#$script#g" \
        -e "s#__PATH__#/usr/bin:/bin:/usr/sbin:/sbin$TOOLS_PATH#g" "$DIR/deploy/macos/service.plist.template" > "$tmp"
    sudo launchctl bootout system "$plist" 2>/dev/null || true
    sudo install -m 644 -o root -g wheel "$tmp" "$plist"
    rm -f "$tmp"
    sudo launchctl bootstrap system "$plist"
    echo "installed $label: $plist (log: $DIR/logs)"
}

install_one app.duowave.gateway run-gateway.sh
[ "${1:-all}" = gateway ] || install_one app.duowave.asr run-asr.sh
# Our voice (TTS_PROVIDER mixed or own): tts-server on 8091, the port TTS_OWN_URL names.
if [ "${1:-all}" != gateway ] && grep -q '^TTS_OWN_URL=' .env; then install_one app.duowave.tts run-tts.sh; fi
echo "Status: sudo launchctl print system/app.duowave.gateway | head; $SERVICE_PROGRAM health (in $DIR: 0 when /health answers); tail -f logs/duowave.log"
