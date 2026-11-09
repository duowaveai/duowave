#!/bin/sh
# Installs the gateway (duowave) as a systemd service on Linux: it starts at boot, restarts when it exits, and listens
# on port 7780 (PORT in its .env). Changes the system: sudo. Undo: deploy/linux/uninstall.sh (the data stays).
#
#   deploy/linux/install.sh                          # builds the gateway from this checkout and installs it
#   DUOWAVE=path/to/duowave deploy/linux/install.sh  # a program built elsewhere: a release's duowave-linux-*
#
# The build: the panel (pnpm --filter panel build: it goes inside the program), then cargo build --release, with
# Silero VAD on x86_64 and aarch64 (DUOWAVE_FEATURES overrides it, "" for none). The program goes to
# /opt/duowave/duowave, owned by the service's user: the gateway replaces it there when it updates itself (the
# one before stays as duowave.previous). Data and settings: /var/lib/duowave (DATA_DIR) — the database,
# journals, voices, and a .env for what the panel does not set. The service runs as the user who installs it, in
# this checkout: its .env is read after DATA_DIR's, the Silero model is its assets/silero_vad.onnx.
set -eu
DIR=$(cd "$(dirname "$0")/../.." && pwd)
PROGRAM=${DUOWAVE:-}
# A path given from elsewhere, before the script moves to the project folder.
case "$PROGRAM" in '' | /*) ;; *) PROGRAM="$(pwd)/$PROGRAM" ;; esac
cd "$DIR"
USER_NAME=${SUDO_USER:-$(id -un)}
DATA=/var/lib/duowave
OPT=/opt/duowave
command -v systemctl >/dev/null || { echo "no systemd here: run duowave serve yourself, or use Docker" >&2; exit 1; }

if [ -z "$PROGRAM" ]; then
    # Built as the owner of the checkout: a build under sudo would leave a target/ the next build cannot write.
    [ -z "${SUDO_USER:-}" ] || { echo "run it without sudo to build (it asks for sudo itself), or set DUOWAVE" >&2; exit 1; }
    command -v cargo >/dev/null || { echo "no cargo: install Rust (https://rustup.rs), or set DUOWAVE to a built duowave" >&2; exit 1; }
    if command -v pnpm >/dev/null; then
        pnpm --filter panel build
    elif [ ! -f panel/dist/index.html ]; then
        echo "note: no pnpm and no panel/dist: the program goes without its panel (/panel will say so)" >&2
    fi
    # Silero's ONNX Runtime comes prebuilt for these two; elsewhere the end of speech is heard by energy.
    case "$(uname -m)" in x86_64 | aarch64) default_features=silero ;; *) default_features= ;; esac
    features=${DUOWAVE_FEATURES-$default_features}
    cargo build -p duowave --release ${features:+--features "$features"}
    PROGRAM=target/release/duowave
fi
[ -f "$PROGRAM" ] || { echo "no program at $PROGRAM" >&2; exit 1; }
"$PROGRAM" version >/dev/null || { echo "$PROGRAM does not run here" >&2; exit 1; }

# The program, put in place by a rename: a running gateway keeps its file until it restarts.
sudo install -d -o "$USER_NAME" -m 755 "$OPT"
sudo install -o "$USER_NAME" -m 755 "$PROGRAM" "$OPT/duowave.new"
sudo mv -f "$OPT/duowave.new" "$OPT/duowave"
echo "program: $OPT/duowave ($("$OPT/duowave" version))"

# The data folder before the first start, with the station's port: the gateway reads $DATA/.env itself.
sudo install -d -o "$USER_NAME" -m 750 "$DATA"
if [ ! -f "$DATA/.env" ]; then
    printf '# The panel and the station: this port of this machine (any free one; re-point the station after a change).\nPORT=7780\n' \
        | sudo -u "$USER_NAME" tee "$DATA/.env" >/dev/null
    sudo chmod 600 "$DATA/.env"
fi

tmp=$(mktemp)
sed -e "s#__USER__#$USER_NAME#g" -e "s#__DIR__#$DIR#g" "$DIR/deploy/linux/duowave.service.template" > "$tmp"
sudo install -m 644 "$tmp" /etc/systemd/system/duowave.service
rm -f "$tmp"
sudo systemctl daemon-reload
sudo systemctl enable duowave.service
# A restart, not a start: a gateway already running takes the new program now.
sudo systemctl restart duowave.service
echo "installed duowave: systemctl status duowave; journalctl -u duowave -f"
echo "check: DATA_DIR=$DATA $OPT/duowave health (in $DIR: 0 when /health answers)"
echo "the panel: http://$(hostname -I 2>/dev/null | cut -d' ' -f1 || echo '<this machine>')/panel"
