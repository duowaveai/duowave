#!/usr/bin/env bash
# Where the station sends its voice: straight to Yandex, as shipped, or to our gateway.
#
#   scripts/station-voice.sh status                 # where it points now and how (read only)
#   scripts/station-voice.sh yandex                 # back to the firmware's own config, then reboot
#   scripts/station-voice.sh gateway HOST[:PORT]    # to our gateway (port 80 by default), then reboot
#   scripts/station-voice.sh legacy-yandex          # once, for a station connected before 08.11.2026: Yandex's
#                                                   # addresses written back into the firmware's own quasar.cfg
#
# Over ssh (STATION_SSH: an ssh alias or root@<station>, `station` by default; STATION_SSH_PORT when the port is not in
# ~/.ssh/config — the wiki's dropbear listens on 2222) it does what the panel's «Подключить колонку» does: puts
# station-client/voice.sh, the boot script's source and restore.sh into /data/opt/duowave, then voice.sh points the
# voice through our copy of quasar.cfg mounted over the firmware's own — that file is never changed (a reset of the
# station or an update of its firmware leaves the usual Alice) — and only then the station reboots.
#
# Before 08.11.2026 the gateway rewrote the firmware's own file. `legacy-yandex` puts Yandex's addresses back into it
# (a dated copy first, the result checked, the copy back otherwise), the one time our tools still write that file.
#
# This changes the station: run it only with its owner's consent. YES=1 skips the question.
set -euo pipefail

HERE=$(cd "$(dirname "$0")/.." && pwd)
STATION=${STATION_SSH:-station}
CFG=/system/vendor/quasar/quasar.cfg
DIR=/data/opt/duowave
YANDEX_URL='wss://uniproxy.alice.yandex.net/uni.ws'
YANDEX_QUIC='uniproxy-quic.alice.yandex.net:8443/uni.ws'
SSH=(ssh -o BatchMode=yes -o ConnectTimeout=8 ${STATION_SSH_PORT:+-p "$STATION_SSH_PORT"})

usage() {
    echo "usage: $0 status | yandex | gateway HOST[:PORT] | legacy-yandex" >&2
    exit 2
}

on_station() { "${SSH[@]}" "$STATION" "$@"; }

addresses() {
    on_station "if [ -f $DIR/voice.sh ]; then sh $DIR/voice.sh status; else grep -o '\"uniProxy[A-Za-z]*\" *: *\"[^\"]*\"' $CFG; echo 'copy: none (no voice.sh yet)'; fi"
}

ask() {
    echo "$1"
    if [ "${YES:-}" != 1 ]; then
        read -r -p "Proceed? [y/N] " answer
        [ "$answer" = y ] || {
            echo "Nothing changed."
            exit 1
        }
    fi
}

# voice.sh, the boot script's source and restore.sh, from this repository (station-client/restore.sh is the gateway's
# own restore_script(), checked by its tests): the same files the panel writes, whatever gateway runs now.
deliver() {
    for name in voice.sh S95duowave restore.sh; do
        on_station "mkdir -p $DIR && cat > $DIR/$name.new && chmod 755 $DIR/$name.new && mv -f $DIR/$name.new $DIR/$name" \
            <"$HERE/station-client/$name"
    done
}

reboot_and_wait() {
    echo "Rebooting the station..."
    on_station reboot || true
    # First it goes away (SSH answers for a moment after `reboot`), then it comes back.
    for _ in $(seq 1 15); do
        sleep 2
        on_station true 2>/dev/null || break
    done
    for _ in $(seq 1 36); do
        sleep 5
        if on_station true 2>/dev/null; then
            echo "Station is back:"
            addresses
            exit 0
        fi
    done
    echo "The station has not answered over ssh for 3 minutes; check it by voice." >&2
    exit 1
}

case "${1:-}" in
    status)
        addresses
        exit 0
        ;;
    yandex)
        echo "Station now:"
        addresses
        ask "Will take our copy of quasar.cfg away (the firmware's own config back), then reboot the station."
        deliver
        on_station "sh $DIR/voice.sh drop"
        reboot_and_wait
        ;;
    gateway)
        target=${2:-}
        [ -n "$target" ] || usage
        # Without a port ws:// means 80, written the way the station has it now. QUIC empty: no way to Yandex past it.
        url="ws://${target%:80}/uni.ws"
        echo "Station now:"
        addresses
        ask "Will point the voice at $url through our copy of quasar.cfg (the firmware's own stays), then reboot."
        deliver
        on_station "sh $DIR/voice.sh point '$url' ''"
        reboot_and_wait
        ;;
    legacy-yandex)
        echo "Station now:"
        addresses
        ask "Will write Yandex's addresses back into the firmware's own $CFG (a dated copy first), then reboot."
        # Our copy, if any, goes first: the firmware's own file must be what the station reads.
        on_station "[ -f $DIR/voice.sh ] && sh $DIR/voice.sh drop; grep -q ' $CFG ' /proc/mounts && { echo 'something is mounted over $CFG: not touching it' >&2; exit 1; }; exit 0"
        backup=$(on_station sh -s -- "$YANDEX_URL" "$YANDEX_QUIC" <<'REMOTE'
set -e
CFG=/system/vendor/quasar/quasar.cfg
BACKUP=/data/quasar.cfg.$(date +%Y%m%d-%H%M%S)
cp "$CFG" "$BACKUP"
sed -e "s#\"uniProxyUrl\" *: *\"[^\"]*\"#\"uniProxyUrl\" : \"$1\"#" \
    -e "s#\"uniProxyQuicUrl\" *: *\"[^\"]*\"#\"uniProxyQuicUrl\" : \"$2\"#" "$CFG" > /tmp/quasar.cfg.new
# Explicit: under set -e a failed test inside an && list would not stop the script.
if [ "$(wc -l < /tmp/quasar.cfg.new)" != "$(wc -l < "$CFG")" ] || ! grep -q "\"uniProxyUrl\" : \"$1\"" /tmp/quasar.cfg.new; then
    rm -f /tmp/quasar.cfg.new
    echo "the new file did not check out; nothing changed (copy: $BACKUP)" >&2
    exit 1
fi
mount -o remount,rw /
cat /tmp/quasar.cfg.new > "$CFG"
rm -f /tmp/quasar.cfg.new
sync
mount -o remount,ro / 2>/dev/null || true
echo "$BACKUP"
REMOTE
        )
        echo "Copy of the previous config on the station: $backup"
        if ! on_station "cat $CFG" | python3 -c '
import json, sys
aliced = json.load(sys.stdin)["aliced"]
sys.exit(0 if (aliced.get("uniProxyUrl"), aliced.get("uniProxyQuicUrl")) == (sys.argv[1], sys.argv[2]) else 1)' "$YANDEX_URL" "$YANDEX_QUIC"; then
            echo "The new config did not check out: restoring $backup, no reboot." >&2
            on_station "mount -o remount,rw / && cat $backup > $CFG && sync; mount -o remount,ro / 2>/dev/null || true"
            exit 1
        fi
        reboot_and_wait
        ;;
    *) usage ;;
esac
