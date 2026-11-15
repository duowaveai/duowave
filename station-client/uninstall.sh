#!/bin/sh
# Takes DuoWave off this station, as if it had never been here: the voice back to the firmware's own quasar.cfg, the
# gateway on the station and the call client stopped, their scripts out of /etc/init.d, /data/opt/duowave gone, then a
# reboot. --keep-data keeps the station gateway's data (/data/opt/duowave/gateway: its database, journal, .env).
# Only the voice back to Yandex, DuoWave kept: sh /data/opt/duowave/restore.sh
# Left here by the gateway; runs without it: sh /data/opt/duowave/uninstall.sh [--keep-data]
# (--no-reboot: the panel's steps, which check what is left and then reboot the station themselves.)
set -u
DIR=/data/opt/duowave
CFG=/system/vendor/quasar/quasar.cfg
KEEP=
REBOOT=1
for arg in "$@"; do
    case "$arg" in
        --keep-data) KEEP=1 ;;
        --no-reboot) REBOOT= ;;
        *) echo "usage: sh /data/opt/duowave/uninstall.sh [--keep-data]" >&2; exit 2 ;;
    esac
done

# The voice first: nothing else changes unless it is back with the firmware's own config.
if [ -f "$DIR/voice.sh" ]; then
    sh "$DIR/voice.sh" drop || { echo "the voice is not back with Yandex; nothing else changed" >&2; exit 1; }
else
    while grep -q " $CFG " /proc/mounts; do
        umount "$CFG" || { echo "$CFG stays mounted; nothing else changed" >&2; exit 1; }
    done
fi

# What runs: the gateway on the station, the call client, the gateway's service of old builds.
[ -f "$DIR/gateway.sh" ] && sh "$DIR/gateway.sh" stop
for service in /etc/init.d/S99zduowave /etc/init.d/S99zgateway; do
    [ -f "$service" ] && sh "$service" stop
done

# Ours on the firmware's partition, read-only but for the moment. A boot script left behind finds nothing to start.
ours=
for file in /etc/init.d/S95duowave /etc/init.d/S99zduowave /etc/init.d/S99zgateway; do
    [ -e "$file" ] && ours="$ours $file"
done
if [ -n "$ours" ]; then
    mount -o remount,rw /
    rm -f $ours
    sync
    mount -o remount,ro / 2>/dev/null || true
    for file in $ours; do
        [ -e "$file" ] && echo "$file stays (the partition did not open): with $DIR gone it starts nothing" >&2
    done
fi

# A YAM package's gateway: the package stays until `yam remove`, and its updates would turn the gateway on again.
yam=
[ "$(cat "$DIR/managed-by" 2>/dev/null)" = yam ] && yam=1
case "$(cat "$DIR/gateway-bin" 2>/dev/null)" in */duowave.gateway/*) yam=1 ;; esac
[ -f "$DIR/yam-off" ] && yam=1

if [ -n "$KEEP" ] && [ -d "$DIR/gateway" ]; then
    for file in "$DIR"/* "$DIR"/.[!.]*; do
        [ -e "$file" ] || continue
        [ "$file" = "$DIR/gateway" ] && continue
        rm -rf "$file"
    done
    echo "the gateway's data stays in $DIR/gateway"
else
    rm -rf "$DIR"
fi
if [ -n "$yam" ]; then
    mkdir -p "$DIR" && : > "$DIR/yam-off"
    echo "the YAM package duowave/gateway stays, its gateway off; to remove it: yam remove duowave/gateway"
fi
if [ -n "$REBOOT" ]; then
    echo "DuoWave is off this station; rebooting"
    reboot
else
    echo "DuoWave is off this station; the reboot is next"
fi
