#!/bin/sh
# The station's scripts on a real kernel and a real busybox: voice.sh's bind mount over a file on a read-only
# partition, the boot script, gateway.sh's pid files against a real /proc. For a throwaway machine only — the station
# emulator (KUYASE, hostname qemu-*, its writes gone with it) or a privileged container — never a real station: it
# mounts over /system/vendor/quasar/quasar.cfg and changes the firmware's file to play an update.
#
#   STATION_CHECK_SANDBOX=1 sh station-client/tests/station-check.sh <station-client folder>
#
# A container has no station: a read-only tmpfs stands for /system/vendor/quasar, with a made-up quasar.cfg in it.
# In the emulator the firmware's own file is used (and the emulator's snapshot throws every change away). Prints
# «ok <what>» or «FAIL <what>: <why>» a line each; the exit code is the number of failures.
set -u
SCRIPTS=${1:?the station-client folder}
[ "${STATION_CHECK_SANDBOX:-}" = 1 ] || {
    echo "not without STATION_CHECK_SANDBOX=1: this mounts over quasar.cfg" >&2
    exit 100
}
case "$(hostname 2>/dev/null)" in
    qemu-*) WHERE=emulator ;;
    *)
        if [ -f /.dockerenv ] || grep -qa docker /proc/1/cgroup 2>/dev/null; then WHERE=container; else
            echo "this is neither the station emulator nor a container: refusing to run" >&2
            exit 100
        fi
        ;;
esac

CFG=/system/vendor/quasar/quasar.cfg
DIR=/data/opt/duowave-check
export DUOWAVE_CFG=$CFG DUOWAVE_DIR=$DIR DUOWAVE_INITD=/tmp/duowave-check-initd DUOWAVE_MOUNTS=/proc/mounts
failures=0
ok() { echo "ok $1"; }
fail() {
    echo "FAIL $1: $2"
    failures=$((failures + 1))
}
url() { sed -n 's#.*"uniProxyUrl" *: *"\([^"]*\)".*#\1#p' "$CFG" | head -n 1; }
mounted() { grep -q " $CFG " /proc/mounts; }
unmount_all() { while mounted; do umount "$CFG" || return 1; done; }
voice() { sh "$DIR/voice.sh" "$@"; }

rm -rf "$DIR" "$DUOWAVE_INITD"
mkdir -p "$DIR" "$DUOWAVE_INITD"
cp "$SCRIPTS/voice.sh" "$SCRIPTS/S95duowave" "$SCRIPTS/gateway.sh" "$DIR/"

if [ "$WHERE" = container ]; then
    mkdir -p /system/vendor/quasar
    mount -t tmpfs tmpfs /system/vendor/quasar
    cat >"$CFG" <<'EOF'
{
    "aliced" : {
        "apiKey" : "not-a-real-key",
        "uniProxyQuicUrl" : "uniproxy-quic.alice.yandex.net:8443/uni.ws",
        "uniProxyUrl" : "wss://uniproxy.alice.yandex.net/uni.ws"
    },
    "common" : { "deviceType" : "check", "softwareVersion" : "1.0" }
}
EOF
    mount -o remount,ro /system/vendor/quasar
    # The «firmware's partition» for the boot script and its remounts: a remount of / is not ours to do here.
    remount_partition() { mount -o "remount,$1" /system/vendor/quasar; }
else
    remount_partition() { mount -o "remount,$1" /; }
fi
[ -f "$CFG" ] || {
    echo "no $CFG here" >&2
    exit 100
}
unmount_all
FIRMWARE=$(cat "$CFG")
FIRMWARE_URL=$(url)
echo "where: $WHERE, $(uname -r), $(busybox 2>&1 | head -n 1)"

# 1. Pointed: our copy over the file, read-only, the firmware's own under it.
if voice point ws://192.0.2.10/uni.ws '' >/tmp/duowave-check.out 2>&1 && mounted && [ "$(url)" = ws://192.0.2.10/uni.ws ]; then
    ok "point: the copy is mounted and read as the config"
else
    fail "point" "$(cat /tmp/duowave-check.out)"
fi
if (echo x >>"$CFG") 2>/dev/null; then fail "read-only" "the mounted copy took a write"; else ok "read-only: the copy refuses writes, as the firmware's file does"; fi
[ -f "$DUOWAVE_INITD/S95duowave" ] && ok "the boot script is written" || fail "boot script" "not in $DUOWAVE_INITD"

# 2. A reboot: the boot script mounts it again.
unmount_all
[ "$(url)" = "$FIRMWARE_URL" ] && ok "under the copy: the firmware's own file, untouched" || fail "untouched" "$(url)"
sh "$DUOWAVE_INITD/S95duowave" start >/dev/null 2>&1
mounted && [ "$(url)" = ws://192.0.2.10/uni.ws ] && ok "boot: mounted again" || fail "boot" "url $(url)"

# 3. Someone else's change in our copy: refused unless forced.
sed -i 's#ws://192.0.2.10/uni.ws#ws://198.51.100.7/uni.ws#' "$DIR/voice/quasar.cfg"
if voice point ws://192.0.2.11/uni.ws '' >/tmp/duowave-check.out 2>&1; then fail "foreign" "replaced without --force"; else ok "a copy someone else changed is not replaced"; fi
unmount_all
voice boot >/dev/null 2>&1
! mounted && grep -q '^copy-changed ' "$DIR/voice/refused" && ok "boot does not mount a copy someone else changed" || fail "boot copy-changed" "url $(url)"
voice point ws://192.0.2.11/uni.ws '' --force >/dev/null 2>&1 && [ "$(url)" = ws://192.0.2.11/uni.ws ] && ok "--force replaces it" || fail "--force" "url $(url)"

# 4. This station's own gateway, then off: at boot the voice goes where it went before it.
printf '#!/bin/sh\nexec sleep 300\n' >"$DIR/program" && chmod 755 "$DIR/program"
echo "$DIR/program" >"$DIR/gateway-bin"
voice point ws://127.0.0.1:8790/uni.ws '' >/dev/null 2>&1
[ -f "$DIR/voice/previous.cfg" ] && ok "pointed at its own gateway: the way back kept" || fail "previous" "no previous.cfg"
: >"$DIR/disabled"
unmount_all
voice boot >/dev/null 2>&1
[ "$(url)" = ws://192.0.2.11/uni.ws ] && ok "its gateway off: boot mounts the way back" || fail "gateway off" "url $(url)"
rm -f "$DIR/disabled" "$DIR/gateway-bin"
unmount_all
voice boot >/dev/null 2>&1
[ "$(url)" = ws://192.0.2.11/uni.ws ] && ok "its program gone: the way back too" || fail "program gone" "url $(url)"
# The way back mounted is ours (live 08.11): pointing here again is no one else's change, and keeps the way back.
voice point ws://127.0.0.1:8790/uni.ws '' >/dev/null 2>&1 && [ "$(url)" = ws://127.0.0.1:8790/uni.ws ] &&
    [ "$(sed -n 's#.*"uniProxyUrl" *: *"\([^"]*\)".*#\1#p' "$DIR/voice/previous.cfg")" = ws://192.0.2.11/uni.ws ] &&
    ok "from the way back: its own gateway again, the way back kept" || fail "again from the way back" "url $(url)"
unmount_all
voice boot >/dev/null 2>&1
# And away from it: the copy itself rewritten, so the next boot keeps it.
voice point ws://192.0.2.11/uni.ws '' >/dev/null 2>&1 && unmount_all && voice boot >/dev/null 2>&1 &&
    [ "$(url)" = ws://192.0.2.11/uni.ws ] && [ ! -f "$DIR/voice/previous.cfg" ] &&
    ok "from the way back: away, and the next boot keeps it" || fail "away from the way back" "url $(url)"

# 5. An update rewrote the firmware's file: the copy is not mounted, and why is said.
unmount_all
remount_partition rw
sed -i 's#"apiKey"#"newKey" : 1,\n        "apiKey"#' "$CFG"
remount_partition ro
voice boot >/dev/null 2>&1
if ! mounted && [ "$(url)" = "$FIRMWARE_URL" ] && grep -q '^firmware-changed ' "$DIR/voice/refused"; then
    ok "the firmware's file changed: the usual Alice, and why"
else
    fail "firmware changed" "mounted=$(mounted && echo yes || echo no) url=$(url)"
fi
remount_partition rw
printf '%s\n' "$FIRMWARE" >"$CFG"
remount_partition ro

# 6. The way back: the firmware's own addresses take the copy away.
voice point ws://192.0.2.12/uni.ws '' >/dev/null 2>&1
voice point wss://uniproxy.alice.yandex.net/uni.ws uniproxy-quic.alice.yandex.net:8443/uni.ws --force >/dev/null 2>&1
! mounted && [ ! -f "$DIR/voice/quasar.cfg" ] && [ "$(cat "$CFG")" = "$FIRMWARE" ] && ok "back to Yandex: no copy, the firmware's own file" || fail "back" "url $(url)"

# 7. A reset wipes /data: the boot script finds nothing and does nothing.
voice point ws://192.0.2.13/uni.ws '' >/dev/null 2>&1
unmount_all
SHIM=$DUOWAVE_INITD/S95duowave
mv "$DIR" "$DIR.away"
sh "$SHIM" start >/dev/null 2>&1
! mounted && [ "$(cat "$CFG")" = "$FIRMWARE" ] && ok "/data wiped: the usual Alice" || fail "wiped" "url $(url)"
mv "$DIR.away" "$DIR"

# 8. gateway.sh's pid files against a real /proc: this boot's and the gateway's, or not its.
sleep 300 &
standing=$!
boot=$(cat /proc/sys/kernel/random/boot_id 2>/dev/null)
echo "$DIR/program" >"$DIR/gateway-bin"
service() { DUOWAVE_PROC=/proc sh "$DIR/gateway.sh" "$@"; }
printf '%s %s\n' "$standing" "$boot" >"$DIR/gateway.pid"
# `sleep 300` is not «… serve»: not the gateway, whatever its number.
[ "$(service status)" = "gateway: not running" ] && ok "a pid of another program is not the gateway" || fail "pid" "$(service status)"
kill "$standing" 2>/dev/null
# A program run as «… serve», as the service runs the gateway.
printf '#!/bin/sh\nsleep 300\n' >"$DIR/program"
sh "$DIR/program" serve &
named=$!
sleep 1
printf '%s %s\n' "$named" "$boot" >"$DIR/gateway.pid"
[ "$(service status)" = "gateway: running (pid $named)" ] && ok "this boot's gateway is running" || fail "running" "$(service status)"
printf '%s %s\n' "$named" "another-boot" >"$DIR/gateway.pid"
[ "$(service status)" = "gateway: not running" ] && ok "a pid from before a power cut is not the gateway" || fail "boot id" "$(service status)"
kill "$named" 2>/dev/null

# 9. A start from a pipe, as an install over SSH runs it (Command::output): the pipe comes back at once, and the loop
# and the watch run on in a session of their own (live 11.11: they kept the pipe, and the install waited for good).
printf '#!/bin/sh\nexec sleep 300\n' >"$DIR/program"
chmod +x "$DIR/program"
rm -f "$DIR/gateway.pid" "$DIR/gateway-loop.pid" "$DIR/gateway-watch.pid" "$DIR/disabled"
pipe=/tmp/duowave-check-pipe
rm -f "$pipe"
mkfifo "$pipe"
DUOWAVE_START_WAIT_SECS=0 DUOWAVE_PROC=/proc sh "$DIR/gateway.sh" start >"$pipe" 2>&1 &
cat "$pipe" >/dev/null &
reader=$!
waited=0
while kill -0 "$reader" 2>/dev/null && [ "$waited" -lt 10 ]; do
    sleep 1
    waited=$((waited + 1))
done
if kill -0 "$reader" 2>/dev/null; then
    fail "a start from a pipe" "the pipe stayed open"
    kill "$reader" 2>/dev/null
else
    ok "a start from a pipe gives the pipe back"
fi
rm -f "$pipe"
sleep 1
loop=$(cut -d' ' -f1 "$DIR/gateway-loop.pid" 2>/dev/null)
if command -v setsid >/dev/null 2>&1; then
    session=$(cut -d' ' -f6 "/proc/$loop/stat" 2>/dev/null)
    [ -n "$loop" ] && [ "$session" = "$loop" ] && ok "the loop runs in a session of its own" || fail "session" "loop ${loop:-?}, session ${session:-?}"
fi
case "$(service status)" in
    "gateway: running"*) ok "the service runs on after the caller" ;;
    *) fail "service" "$(service status)" ;;
esac
service stop

unmount_all
rm -rf "$DIR" "$DUOWAVE_INITD"
[ "$WHERE" = container ] && umount /system/vendor/quasar
echo "failures: $failures"
exit "$failures"
