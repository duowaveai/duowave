#!/bin/sh
# The voice gateway on the station itself: /data/opt/duowave/gateway.sh, started at boot by /etc/init.d/S95duowave
# before Alice (S99quasar). When the station's voice goes to it, the start waits for its /health up to 15 s, so Alice
# finds it up when she dials it. The program is in /data/opt/duowave/bin (installed by `duowave station install` —
# from the computer's panel or by hand) or in a YAM package (its path is in gateway-bin); its data — the database, the
# journal, its .env — in /data/opt/duowave/gateway (DATA_DIR). To remove: `duowave station remove`.
#
# The safety net (on unless /data/opt/duowave/safety-off): a gateway that ends within 30 s of its start three times in
# two minutes, or stops answering /health (restarted after two minutes without an answer, three times in half an
# hour), does not come up again. Then, first the version: right after an update the one before comes back. Then the
# address: while the station's voice goes to this gateway, it stays off (`disabled`) and the station reboots — at boot
# voice.sh puts the voice where it went before this gateway, or leaves the firmware's own: a minute later it is the
# usual Alice; `duowave station retry` turns the gateway on again. A gateway that runs and answers — without internet,
# with its computer asleep — is never touched.
#
# Its pid files say «<pid> <boot id>»: a pid from before a power cut, now another process's, is not ours.
DIR=${DUOWAVE_DIR:-/data/opt/duowave}
CFG=${DUOWAVE_CFG:-/system/vendor/quasar/quasar.cfg}
PROC=${DUOWAVE_PROC:-/proc}
DATA=$DIR/gateway
BIN=$(cat "$DIR/gateway-bin" 2>/dev/null)
LOG=$DATA/gateway.log
LOOP=$DIR/gateway-loop.pid
CHILD=$DIR/gateway.pid
WATCH=$DIR/gateway-watch.pid
STARTED=$DIR/gateway.started
CRASHES=$DIR/crashes
HANGS=$DIR/hangs
PAUSE=${DUOWAVE_RESPAWN_PAUSE:-5}
QUICK=${DUOWAVE_QUICK_SECS:-30}
WINDOW=${DUOWAVE_WINDOW_SECS:-120}
# /health: asked every PROBE s from GRACE s after a start; MISSES unanswered in a row restart the gateway, and the
# third such restart within HANG_WINDOW s is the safety net's, like quick ends.
PROBE=${DUOWAVE_PROBE_SECS:-30}
GRACE=${DUOWAVE_GRACE_SECS:-60}
MISSES=${DUOWAVE_MISSES:-4}
HANG_WINDOW=${DUOWAVE_HANG_WINDOW_SECS:-1800}
START_WAIT=${DUOWAVE_START_WAIT_SECS:-15}
# A stop waits STOP_WAIT s for the gateway to end on TERM (it ends within 12 s by itself), then kills it: the whole stop
# stays under the 20 s the computer gives a command over SSH.
STOP_WAIT=${DUOWAVE_STOP_WAIT_SECS:-14}
BOOT=$(cat "$PROC/sys/kernel/random/boot_id" 2>/dev/null)

port() {
    value=$(sed -n 's/^PORT=\([0-9]*\).*/\1/p' "$DATA/.env" 2>/dev/null | tail -n 1)
    echo "${value:-8790}"
}

healthy() {
    if [ -n "${DUOWAVE_HEALTH_CMD:-}" ]; then
        sh -c "$DUOWAVE_HEALTH_CMD" >/dev/null 2>&1
    else
        wget -q -T 5 -O /dev/null "http://127.0.0.1:$(port)/health" 2>/dev/null
    fi
}

voice_here() { grep -q '"uniProxyUrl" *: *"ws://127\.0\.0\.1:' "$CFG" 2>/dev/null; }

remember() { printf '%s %s\n' "$2" "$BOOT" >"$1"; }

# The process of a pid file: of this boot, and still the one we started — its command line has `marker` in it.
alive() {
    { read -r pid boot <"$1"; } 2>/dev/null || return 1
    case "$pid" in '' | *[!0-9]*) return 1 ;; esac
    [ -z "$BOOT" ] || [ "${boot:-}" = "$BOOT" ] || return 1
    tr '\0' ' ' <"$PROC/$pid/cmdline" 2>/dev/null | grep -q -- "$2"
}

pid_of() { cut -d' ' -f1 "$1" 2>/dev/null; }

# Every gateway of ours that runs, by its command line, not only the one its pid file names: `<program> serve` of the
# program gateway-bin names, of any program in this folder, of an update tried from memory (/tmp/duowave-updates). A
# pid file can be gone or stale; a gateway left running holds the port, and the next start falls on it.
serving() {
    child=
    if alive "$CHILD" serve; then
        child=$(pid_of "$CHILD")
        kill -0 "$child" 2>/dev/null && echo "$child"
    fi
    for entry in "$PROC"/[0-9]*; do
        pid=${entry##*/}
        [ "$pid" = "$child" ] && continue
        # shellcheck disable=SC2046 # its words, split on purpose
        set -- $(tr '\0' ' ' <"$entry/cmdline" 2>/dev/null)
        [ "${2:-}" = serve ] || continue
        case "${1:-}" in
            "$BIN" | "$DIR"/* | /tmp/duowave-updates/*) kill -0 "$pid" 2>/dev/null && echo "$pid" ;;
        esac
    done
}

# The programs the gateway started from its data (the apps' builds, its Node): they end with it. Left behind, the wake
# word's program stayed after a gateway that hung on 20.11.2026.
apps_left() {
    for entry in "$PROC"/[0-9]*; do
        pid=${entry##*/}
        # shellcheck disable=SC2046 # its words, split on purpose
        set -- $(tr '\0' ' ' <"$entry/cmdline" 2>/dev/null)
        case "${1:-}" in "$DATA"/*) kill -0 "$pid" 2>/dev/null && echo "$pid" ;; esac
    done
}

note() { [ -d "$DATA" ] && echo "$1" >>"$LOG"; }

# Times kept in `file`, the newest `now` added; prints how many fall within `window` s.
count() {
    file=$1
    now=$2
    window=$3
    recent=1
    kept=""
    if [ -f "$file" ]; then
        while read -r at; do
            case "$at" in '' | *[!0-9]*) continue ;; esac
            if [ $((now - at)) -le "$window" ]; then
                recent=$((recent + 1))
                kept="$kept$at
"
            fi
        done <"$file"
    fi
    printf '%s%s\n' "$kept" "$now" >"$file"
    echo "$recent"
}

# The safety net, once the gateway does not come up: the version before an update first, then the station's voice.
give_back() {
    now=$1
    [ -f "$DIR/safety-off" ] && return 0
    rm -f "$CRASHES" "$HANGS"
    # First the version: just updated, and the one before is beside it.
    if [ -f "$DIR/updated" ] && [ -x "$BIN.previous" ]; then
        mv -f "$BIN" "$BIN.failed" && mv -f "$BIN.previous" "$BIN"
        rm -f "$DIR/updated"
        echo "previous-version $now" >"$DIR/safety-fired"
        echo "safety net: the version before the update is back" >>"$LOG"
        return 0
    fi
    # Then the address: only while the voice goes to this gateway. The reboot cannot repeat: off, the gateway is not
    # started, and at boot voice.sh does not mount a voice that goes to a gateway that is off.
    if voice_here; then
        echo "previous-address $now" >"$DIR/safety-fired"
        : >"$DIR/disabled"
        echo "safety net: the gateway does not come up; the station reboots to where its voice went before it" >>"$LOG"
        sync
        reboot
    fi
}

# A start that ended within QUICK s: its time is kept; the third within WINDOW s is the safety net's.
quick() {
    [ "$(count "$CRASHES" "$1" "$WINDOW")" -ge 3 ] || return 0
    give_back "$1"
}

# Started again if it stops; an update replaces the program and starts it in the same process (exec).
run() {
    mkdir -p "$DATA"
    while [ -x "$BIN" ] && [ ! -f "$DIR/disabled" ]; do
        started=$(date +%s)
        echo "$started" >"$STARTED"
        DATA_DIR=$DATA "$BIN" serve </dev/null >>"$LOG" 2>&1 &
        remember "$CHILD" $!
        wait $!
        ended=$(date +%s)
        if [ $((ended - started)) -lt "$QUICK" ]; then quick "$ended"; else rm -f "$CRASHES"; fi
        [ -f "$DIR/disabled" ] && break
        sleep "$PAUSE"
    done
}

# A gateway that runs but has stopped answering: restarted (the loop starts it again); the third time within
# HANG_WINDOW the safety net takes over, as for quick ends.
keep_watch() {
    mkdir -p "$DATA"
    misses=0
    while [ ! -f "$DIR/disabled" ]; do
        sleep "$PROBE"
        if ! alive "$CHILD" serve; then
            misses=0
            continue
        fi
        started=$(cat "$STARTED" 2>/dev/null || echo 0)
        case "$started" in '' | *[!0-9]*) started=0 ;; esac
        [ $(($(date +%s) - started)) -ge "$GRACE" ] || continue
        if healthy; then
            misses=0
            continue
        fi
        misses=$((misses + 1))
        [ "$misses" -ge "$MISSES" ] || continue
        misses=0
        now=$(date +%s)
        echo "safety net: no answer on /health for $((PROBE * MISSES)) s; the gateway is restarted" >>"$LOG"
        hung=$(count "$HANGS" "$now" "$HANG_WINDOW")
        pid=$(pid_of "$CHILD")
        kill "$pid" 2>/dev/null
        sleep "${DUOWAVE_KILL_WAIT_SECS:-10}"
        alive "$CHILD" serve && kill -9 "$pid" 2>/dev/null
        [ "$hung" -ge 3 ] && give_back "$now"
    done
}

# The loop and the watch as programs of their own, away from whoever started them: a session of their own (setsid, where
# busybox has it) and none of the caller's input or output. As shell functions under a redirection they kept busybox
# sh's saved copy of the caller's output: an install over SSH (Command::output) waited for it forever, and the service
# stayed in that SSH session (11.11.2026).
detached() {
    if command -v setsid >/dev/null 2>&1; then
        setsid sh "$0" "$1" </dev/null >/dev/null 2>&1 &
    else
        sh "$0" "$1" </dev/null >/dev/null 2>&1 &
    fi
}

start() {
    # /data wiped (a factory reset), the package removed, or the safety net gave the station back: nothing to start.
    [ -n "$BIN" ] && [ -x "$BIN" ] || return 0
    [ -f "$DIR/disabled" ] && return 0
    alive "$LOOP" gateway.sh && return 0
    mkdir -p "$DATA"
    # Kept under 256 KB: the station's /data is small.
    [ -f "$LOG" ] && [ "$(wc -c <"$LOG")" -gt 262144 ] && mv "$LOG" "$LOG.old"
    detached run
    remember "$LOOP" $!
    detached watch
    remember "$WATCH" $!
    # Before Alice: when her voice goes here, the gateway is up by the time she dials it.
    if voice_here; then
        waited=0
        while [ "$waited" -lt "$START_WAIT" ]; do
            healthy && return 0
            sleep 1
            waited=$((waited + 1))
        done
        echo "the gateway did not answer /health within $START_WAIT s of its start; Alice goes on" >>"$LOG"
    fi
    return 0
}

# TERM: the gateway closes its sessions and the database and exits; the loop and the watch go first, so neither starts
# it again. The stop is over only once the gateway is: it is waited for, killed after STOP_WAIT s, and so are its apps'
# programs. On 20.11.2026 a stop sent TERM and went, the gateway hung on its way out, and the next start would have
# fallen on its port.
stop() {
    alive "$WATCH" gateway.sh && kill "$(pid_of "$WATCH")"
    alive "$LOOP" gateway.sh && kill "$(pid_of "$LOOP")"
    rm -f "$WATCH" "$LOOP"
    pids=$(serving)
    # shellcheck disable=SC2086 # one pid a word
    [ -n "$pids" ] && kill $pids 2>/dev/null
    waited=0
    while [ -n "$pids" ] && [ "$waited" -lt "$STOP_WAIT" ]; do
        sleep 1
        waited=$((waited + 1))
        pids=$(serving)
    done
    if [ -n "$pids" ]; then
        note "the gateway did not end within $STOP_WAIT s of TERM: killed"
        # shellcheck disable=SC2086
        kill -9 $pids 2>/dev/null
    fi
    pids=$(apps_left)
    if [ -n "$pids" ]; then
        # shellcheck disable=SC2086
        kill $pids 2>/dev/null
        sleep 1
        pids=$(apps_left)
        # shellcheck disable=SC2086
        [ -n "$pids" ] && kill -9 $pids 2>/dev/null
    fi
    rm -f "$CHILD"
}

case "${1:-}" in
    start) start ;;
    stop) stop ;;
    restart)
        stop
        sleep 1
        start
        ;;
    # Each says its own pid too: had setsid to fork, the one start() noted would be gone.
    run)
        remember "$LOOP" $$
        run
        ;;
    watch)
        remember "$WATCH" $$
        keep_watch
        ;;
    status)
        if alive "$CHILD" serve; then
            echo "gateway: running (pid $(pid_of "$CHILD"))"
        elif [ -f "$DIR/disabled" ]; then
            echo "gateway: off — the safety net gave the station back (duowave station retry)"
        else
            echo "gateway: not running"
        fi
        ;;
esac
exit 0
