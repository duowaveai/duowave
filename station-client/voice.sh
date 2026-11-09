#!/bin/sh
# Where the station's voice goes, as DuoWave keeps it: never in the firmware's own quasar.cfg. A copy of that file with
# the two addresses changed lives in /data/opt/duowave/voice and is bind-mounted over it — now, and at every boot by
# /etc/init.d/S95duowave, before Alice (S99quasar). So whatever happens to /data (a reset of the station wipes it, a
# package goes) and whatever happens to the firmware's file (an update, someone else's edit), the station is the usual
# Alice again: no copy, or a copy made from another file than the one there now, is not mounted.
#
#   voice.sh point <url> [<quic>] [--force]   the voice to this address; the firmware's own addresses: our copy goes.
#                                             Refused while our copy carries an address we did not write (someone
#                                             changed it through the mount) — --force replaces it all the same
#   voice.sh drop                             the firmware's own config back: our copy unmounted and gone
#   voice.sh boot                             at boot (S95duowave): our copy mounted, if it is still valid
#   voice.sh status                           where the voice goes now, and why
#
# busybox sh and its applets only: sed, grep, cmp, cp, mv, mount, umount, sync. Paths move for tests: DUOWAVE_CFG,
# DUOWAVE_DIR, DUOWAVE_INITD, DUOWAVE_MOUNTS.
set -u
CFG=${DUOWAVE_CFG:-/system/vendor/quasar/quasar.cfg}
DIR=${DUOWAVE_DIR:-/data/opt/duowave}
INITD=${DUOWAVE_INITD:-/etc/init.d}
MOUNTS=${DUOWAVE_MOUNTS:-/proc/mounts}
VOICE=$DIR/voice
# Our copy, mounted over CFG.
COPY=$VOICE/quasar.cfg
# The firmware's file as it was when the copy was made: at boot the copy is mounted only over this very file.
FIRMWARE=$VOICE/firmware.cfg
# The copy in force before the voice went to this station's own gateway: the safety net's way back.
PREVIOUS=$VOICE/previous.cfg
# The addresses we wrote last: an address in the copy that is not these was changed by someone else.
OURS=$VOICE/ours
# Why the last boot did not mount the copy: «<what> <unix time> <words>», for the gateway and the panel.
REFUSED=$VOICE/refused
SHIM=$INITD/S95duowave
SHIM_SOURCE=$DIR/S95duowave
LOCK=$DIR/.switching

say() { echo "$*"; }
fail() {
    echo "$*" >&2
    exit 1
}

url_of() { sed -n 's#.*"uniProxyUrl" *: *"\([^"]*\)".*#\1#p' "$1" 2>/dev/null | head -n 1; }
quic_of() { sed -n 's#.*"uniProxyQuicUrl" *: *"\([^"]*\)".*#\1#p' "$1" 2>/dev/null | head -n 1; }
local_url() { case "$1" in ws://127.0.0.1:*) return 0 ;; *) return 1 ;; esac; }

# Something is mounted over the config (ours, by the files beside it).
mounted() { grep -q " $CFG " "$MOUNTS" 2>/dev/null; }

unmount() {
    while mounted; do
        umount "$CFG" || return 1
    done
}

# Our copy over the config, read-only like the firmware's own file: a tool that writes quasar.cfg in place gets the
# same refusal as on the firmware's partition, not our copy changed under us.
bind() {
    mount -o bind "$1" "$CFG" || return 1
    mount -o remount,ro,bind "$CFG" 2>/dev/null || true
}

# One change at a time; a lock older than ten minutes is a change that died midway.
lock() {
    mkdir -p "$DIR" || fail "no $DIR"
    if ! (set -C && echo $$ >"$LOCK") 2>/dev/null; then
        if [ -n "$(find "$LOCK" -mmin +10 2>/dev/null)" ]; then
            rm -f "$LOCK"
            (set -C && echo $$ >"$LOCK") 2>/dev/null || fail "quasar.cfg is being changed by someone else: wait a minute"
        else
            fail "quasar.cfg is being changed by someone else: wait a minute"
        fi
    fi
    trap 'rm -f "$LOCK"' EXIT
}

# The firmware's own file: the config itself, or, while our copy is mounted over it, the copy of it we kept.
firmware_file() {
    if mounted; then
        [ -f "$FIRMWARE" ] || return 1
        echo "$FIRMWARE"
    else
        echo "$CFG"
    fi
}

# The boot script on the firmware's partition: written once (and when it changes), the root put back read-only.
install_shim() {
    [ -f "$SHIM_SOURCE" ] || {
        echo "no $SHIM_SOURCE" >&2
        return 1
    }
    cmp -s "$SHIM_SOURCE" "$SHIM" 2>/dev/null && return 0
    mount -o remount,rw / || return 1
    written=0
    if cat "$SHIM_SOURCE" >"$SHIM.new" && chmod 755 "$SHIM.new" && mv -f "$SHIM.new" "$SHIM"; then written=1; fi
    rm -f "$SHIM.new"
    sync
    mount -o remount,ro / 2>/dev/null || true
    [ "$written" = 1 ]
}

drop_copy() {
    unmount || fail "$CFG stays mounted: umount failed; nothing else changed"
    rm -f "$COPY" "$FIRMWARE" "$PREVIOUS" "$OURS" "$VOICE/.quasar.cfg.old" "$VOICE/.quasar.cfg.new"
}

point() {
    url=$1
    quic=$2
    force=$3
    case "$url" in '' | *[!A-Za-z0-9._~:/?\&=%@-]*) fail "not an address to write: '$url'" ;; esac
    case "$quic" in *[!A-Za-z0-9._~:/?\&=%@-]*) fail "not an address to write: '$quic'" ;; esac
    lock
    base=$(firmware_file) || fail "something is mounted over $CFG, but not our copy (no $FIRMWARE); nothing changed"
    grep -q '"uniProxyUrl"' "$base" 2>/dev/null || fail "no uniProxyUrl in $base: not a station this script knows; nothing changed"
    if [ -z "$force" ]; then
        foreign=$(changed_copy) && fail "the address in our copy is $foreign, not the one we wrote: someone else changed it; nothing changed (--force replaces it)"
    fi
    mkdir -p "$VOICE" || fail "no $VOICE"
    new=$VOICE/.quasar.cfg.new
    sed -e "s#\(\"uniProxyUrl\" *: *\"\)[^\"]*\"#\1$url\"#" -e "s#\(\"uniProxyQuicUrl\" *: *\"\)[^\"]*\"#\1$quic\"#" "$base" >"$new"
    if ! grep -q "\"uniProxyUrl\" *: *\"$url\"" "$new" || [ "$(wc -l <"$new")" != "$(wc -l <"$base")" ]; then
        rm -f "$new"
        fail "the new file did not check out; nothing changed"
    fi
    if cmp -s "$new" "$base"; then
        # The firmware's own addresses: no copy is needed.
        rm -f "$new"
        drop_copy
        rm -f "$REFUSED"
        say "voice: $url (the firmware's own config)"
        return 0
    fi
    # The safety net's way back: where the voice went before this station's own gateway — the copy in force then, or
    # none (the firmware's own) when there was none.
    if local_url "$url"; then
        # What is in force now — our copy, or the way back itself — read through the mount.
        if mounted && [ -f "$PREVIOUS" ] && cmp -s "$CFG" "$PREVIOUS"; then
            :
        elif mounted; then
            local_url "$(url_of "$CFG")" || cp "$CFG" "$PREVIOUS" || fail "no room for $PREVIOUS; nothing changed"
        else
            rm -f "$PREVIOUS"
        fi
    else
        rm -f "$PREVIOUS"
    fi
    if ! mounted; then
        cp "$CFG" "$VOICE/.firmware.cfg.new" && mv -f "$VOICE/.firmware.cfg.new" "$FIRMWARE" || {
            rm -f "$new" "$VOICE/.firmware.cfg.new"
            fail "no room for $FIRMWARE; nothing changed"
        }
    fi
    install_shim || {
        rm -f "$new"
        fail "the boot script $SHIM is not written; nothing changed"
    }
    old=$VOICE/.quasar.cfg.old
    was_mounted=0
    mounted && was_mounted=1
    rm -f "$old"
    [ -f "$COPY" ] && cp "$COPY" "$old"
    mv -f "$new" "$COPY" || fail "$COPY not written; nothing changed"
    if ! unmount || ! bind "$COPY"; then
        # Back as it was: the copy before (mounted again, when it was), or none.
        if [ -f "$old" ]; then
            mv -f "$old" "$COPY"
            [ "$was_mounted" = 1 ] && ! mounted && bind "$COPY"
        else
            rm -f "$COPY"
        fi
        fail "the copy could not be mounted over $CFG; it is as it was"
    fi
    rm -f "$old"
    if ! cmp -s "$CFG" "$COPY"; then
        unmount
        rm -f "$COPY"
        fail "mounted, but $CFG does not read as the copy: undone"
    fi
    echo "$url $quic" >"$OURS"
    rm -f "$REFUSED"
    sync
    say "copy: $COPY"
    say "voice: $url"
}

# Someone else's hand in our copy: the copy's file with an address we did not write, or something mounted over the
# config that is neither our copy nor the way back the boot mounts when this station's gateway is off (previous.cfg —
# ours too: live 08.11, «Попробовать снова» after the safety net was refused as someone else's change). Prints the
# address it found, and is true.
changed_copy() {
    [ -f "$OURS" ] || return 1
    ours=$(cat "$OURS")
    if [ -f "$COPY" ] && [ "$(url_of "$COPY") $(quic_of "$COPY")" != "$ours" ]; then
        url_of "$COPY"
        return 0
    fi
    if mounted && [ -f "$FIRMWARE" ] && ! cmp -s "$CFG" "$COPY" && ! { [ -f "$PREVIOUS" ] && cmp -s "$CFG" "$PREVIOUS"; }; then
        url_of "$CFG"
        return 0
    fi
    return 1
}

refuse() {
    mkdir -p "$VOICE" 2>/dev/null
    printf '%s %s %s\n' "$1" "$(date +%s)" "$2" >"$REFUSED" 2>/dev/null
    say "duowave: $2"
}

# At boot: never fails the boot; anything doubtful leaves the firmware's own config.
boot() {
    if [ ! -f "$COPY" ]; then
        rm -f "$REFUSED" 2>/dev/null
        return 0
    fi
    mounted && return 0
    if [ ! -f "$FIRMWARE" ] || ! cmp -s "$CFG" "$FIRMWARE"; then
        refuse firmware-changed "the station's quasar.cfg is not the file our copy was made from (an update of the firmware, or someone else's edit): it keeps its own; connect it again"
        return 0
    fi
    if foreign=$(changed_copy); then
        refuse copy-changed "our copy carries $foreign, not the address we wrote: the station keeps its own config"
        return 0
    fi
    pick=$COPY
    if local_url "$(url_of "$COPY")"; then
        program=$(cat "$DIR/gateway-bin" 2>/dev/null)
        if [ -f "$DIR/disabled" ] || [ -z "$program" ] || [ ! -x "$program" ]; then
            if [ -f "$PREVIOUS" ] && ! local_url "$(url_of "$PREVIOUS")"; then
                pick=$PREVIOUS
            else
                refuse gateway-off "the station's own gateway is off or gone: the station keeps its own config"
                return 0
            fi
        fi
    fi
    grep -q '"uniProxyUrl"' "$pick" 2>/dev/null || {
        refuse broken "our copy has no uniProxyUrl: the station keeps its own config"
        return 0
    }
    if bind "$pick"; then
        if [ "$pick" = "$PREVIOUS" ]; then
            refuse previous "the station's own gateway is off: the voice goes where it went before it"
        else
            rm -f "$REFUSED"
        fi
    else
        refuse mount-failed "our copy could not be mounted: the station keeps its own config"
    fi
    return 0
}

status() {
    say "voice: $(url_of "$CFG")"
    if mounted; then
        if [ -f "$FIRMWARE" ]; then say "copy: mounted"; else say "copy: someone else's mount"; fi
    elif [ -f "$COPY" ]; then
        say "copy: not mounted"
    else
        say "copy: none"
    fi
    [ -f "$FIRMWARE" ] && say "firmware: $(url_of "$FIRMWARE")"
    [ -f "$REFUSED" ] && say "refused: $(cat "$REFUSED")"
    if [ -f "$SHIM" ]; then say "boot: $SHIM"; else say "boot: none"; fi
    return 0
}

case "${1:-}" in
    point)
        [ -n "${2:-}" ] || fail "usage: voice.sh point <url> [<quic>] [--force]"
        quic=${3:-}
        force=${4:-}
        [ "$quic" = --force ] && quic= && force=--force
        point "$2" "$quic" "$force"
        ;;
    drop)
        lock
        drop_copy
        rm -f "$REFUSED"
        say "voice: $(url_of "$CFG") (the firmware's own config)"
        ;;
    boot) boot ;;
    status) status ;;
    *)
        sed -n '9,15p' "$0" | sed 's/^# \{0,1\}//'
        exit 2
        ;;
esac
exit 0
