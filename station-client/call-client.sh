#!/bin/sh
# Our call client for the station (Yandex Station Lite 2): the other party's audio from the gateway into the
# station's ALSA as a live Ogg/Opus stream, with the GStreamer the firmware already has (`maind --gst-launch`:
# souphttpsrc, decodebin, alsasink), and the station's processed microphone to the gateway: a socat bridge from the
# station's own audio service (127.0.0.1:9850) to the gateway's CALLS_STATION_MIC_PORT; the gateway subscribes
# for a call's length and takes vqe_gain (see README). Without installing anything: run it over SSH from the
# gateway's machine,
#
#   (cat station-client/call-client.sh; cat) | ssh -p 2222 root@192.0.2.23 'GATEWAY=http://192.0.2.10 sh -s'
#
# and stop it with Ctrl+C: the client runs while its stdin is open. No file stays on the station. Installed, it
# starts with the station: S99zduowave runs it with DAEMON=1, until it is told to stop (TERM).
#
# The speaker is the station's own, shared through dmix: the pipeline runs only while a call is on. The gateway
# answers the poll `call` once the station's call is answered and ends the stream when the call ends, so the
# pipeline opens the speaker for the call alone and closes it by itself at the end of its stream. It is never
# killed: live 02.11 a pipeline holding the speaker between calls, then killed, left the station silent for
# everyone (Alice too) until a reboot. MIC_PORT= (empty) leaves the microphone out.
# ⚠️ Running anything on the station is the owner's call: see README.

set -eu
# The sound guard (guard_sound below) can be tried on its own: `sh call-client.sh guard-sound`.
[ "${1:-}" = guard-sound ] || GATEWAY=${GATEWAY:?set GATEWAY=http://<the gateway>}
GATEWAY=${GATEWAY:-}
DEVICE=${DEVICE:-default}
MIC_PORT=${MIC_PORT-9851}
QUASAR=${QUASAR:-/system/vendor/quasar}
# http://192.0.2.10[:80] → 192.0.2.10
GATEWAY_HOST=${GATEWAY#*://}
GATEWAY_HOST=${GATEWAY_HOST%%[:/]*}

# The firmware's GStreamer, as the wiki runs it: from Quasar's folder, with its libraries, one pipeline string.
# Its log goes nowhere: once the SSH session is gone, writing it would end the pipeline mid-call (SIGPIPE).
gst() {
    cd "$QUASAR" && LD_LIBRARY_PATH="$QUASAR:libs" exec ./maind --gst-launch "$1" >/dev/null 2>&1
}

# The speaker stuck after a call (live 02.11: the output was prepared and let go again and again, never started, for
# everyone — Alice too — until a reboot): after a pipeline that failed, the speaker's state is watched for five minutes.
# Prepared and not playing for 90 s on end — Alice's media service is restarted; still so at 210 s — the station
# reboots, at most once an hour. Anything else (closed, playing) is a speaker that is well: the watch ends.
# SOUND_GUARD=0 turns it off.
PCM_STATUS=${PCM_STATUS:-/proc/asound/card0/pcm0p/sub0/status}
GUARD_MARK=${GUARD_MARK:-/data/opt/duowave/sound-guard.reboot}
guard_sound() {
    [ "${SOUND_GUARD:-1}" = 1 ] && [ -r "$PCM_STATUS" ] || return 0
    stuck=0
    while [ "$stuck" -lt 30 ]; do
        sleep "${GUARD_STEP:-10}"
        grep -q '^state: PREPARED' "$PCM_STATUS" 2>/dev/null || return 0
        stuck=$((stuck + 1))
        if [ "$stuck" -eq 9 ]; then
            echo "$(date '+%F %T') sound guard: the speaker prepared and silent for 90 s after a failed call; Alice's media service restarts"
            for pid in $(pidof maind 2>/dev/null); do
                grep -q -- '--mediad' "/proc/$pid/cmdline" 2>/dev/null && kill "$pid" 2>/dev/null
            done
        fi
        if [ "$stuck" -ge 21 ]; then
            now=$(date +%s)
            last=$(cat "$GUARD_MARK" 2>/dev/null || echo 0)
            case "$last" in '' | *[!0-9]*) last=0 ;; esac
            if [ $((now - last)) -gt 3600 ]; then
                echo "$now" > "$GUARD_MARK"
                echo "$(date '+%F %T') sound guard: the speaker still silent; the station reboots"
                sync
                reboot
            else
                echo "$(date '+%F %T') sound guard: the speaker still silent, but the station rebooted less than an hour ago"
            fi
            return 0
        fi
    done
}
if [ "${1:-}" = guard-sound ]; then
    guard_sound
    exit 0
fi

# The other party, a call at a time. Ogg/Opus, not raw PCM: this firmware has no raw audio parser (no
# rawaudioparse or audioparse, live 02.11), and decodebin plays Ogg/Opus as Alice's own sounds. Between calls
# only the poll runs; after the gateway restarts the poll fails for a while and comes back by itself.
listen_calls() {
    while :; do
        state=$(wget -q -T 70 -O - "$GATEWAY/calls/next/$DEVICE" 2>/dev/null) || state=unreachable
        case $state in
            call)
                echo "$(date '+%F %T') call: playing"
                code=0
                (gst "souphttpsrc location=\"$GATEWAY/calls/listen/$DEVICE?format=ogg\" is-live=true timeout=0 \
                    ! decodebin ! audioconvert ! audioresample ! alsasink sync=false") || code=$?
                echo "$(date '+%F %T') call: over (pipeline $code)"
                [ "$code" = 0 ] || guard_sound
                ;;
            idle) ;;
            *) sleep 5 ;;
        esac
    done
}
listen_calls &
LISTEN=$!

# The station's microphone: its audio service bridged to the gateway as it is; the gateway speaks its protocol.
# A bridge lasts while both ends do, so it is made again, as the poll asks again: after the gateway restarts it
# comes back by itself. The gateway's end first: while the gateway is away the audio service is not touched.
talk() {
    bridge=
    trap 'if [ -n "$bridge" ]; then kill "$bridge" 2>/dev/null || true; fi; exit 0' TERM
    while :; do
        socat "TCP:$GATEWAY_HOST:$MIC_PORT" TCP:127.0.0.1:9850 2>/dev/null &
        bridge=$!
        wait "$bridge" || true
        bridge=
        sleep 5
    done
}
TALK=
if [ -n "$MIC_PORT" ]; then
    talk &
    TALK=$!
fi

stop() {
    # The poll loop goes; a pipeline playing a call does not: the gateway ends its stream (and the station leaves
    # our calls), and the pipeline closes the speaker by itself.
    kill "$LISTEN" 2>/dev/null || true
    wget -q -O /dev/null --post-data= "$GATEWAY/calls/release/$DEVICE" 2>/dev/null || true
    if [ -n "$TALK" ]; then kill "$TALK" 2>/dev/null || true; fi
}

trap 'stop; exit 0' INT TERM HUP
if [ -n "${DAEMON:-}" ]; then
    # Started with the station: until a TERM (the poll loop never ends by itself).
    wait "$LISTEN" || true; stop
else
    # Until our stdin closes: the SSH session ended (or Ctrl+D in a terminal). One line: under `sh -s`, cat reads
    # whatever of the script comes after it.
    cat >/dev/null || true; stop
fi
