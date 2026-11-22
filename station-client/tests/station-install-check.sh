#!/bin/sh
# The station's one-line install (station-client/station.sh) in a sandbox, under busybox: a pretend station (its
# quasar.cfg, /data, /tmp and meminfo in a folder), a pretend mirror, and stand-ins for wget, uname, id and df. The
# build is a real .tar.gz of a pretend `duowave` and its release.json, as release.yml packs them; the pretend program
# notes what it was asked. Nothing touches the machine it runs on beyond a temporary folder: safe anywhere, in CI
# under busybox (`docker run … busybox:1.36`, .github/workflows/ci.yml).
#
#   sh station-client/tests/station-install-check.sh <repository root>
#
# Prints «ok <what>» or «FAIL <what>: <why>» a line each; the exit code is the number of failures.
set -u
ROOT=$(cd "${1:?the repository root}" && pwd)
SCRIPT=$ROOT/station-client/station.sh
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
VERSION=9.9.9
RUN=
failures=0
ok() { echo "ok $1"; }
fail() {
    echo "FAIL $1: $2"
    failures=$((failures + 1))
}

# The build: a pretend program and its release.json, packed as release.yml's Archive step packs the real one.
mkdir -p "$WORK/build" "$WORK/server/$VERSION" "$WORK/stub"
cat >"$WORK/build/duowave" <<'EOF'
#!/bin/sh
echo "program $*" >>"$FAKE_LOG"
case "$1 ${2:-}" in
    "version "*) echo "duowave 9.9.9" ;;
    "station install")
        [ -z "${FAKE_INSTALL_FAILS:-}" ] || exit 1
        printf '#!/bin/sh\necho "gateway.sh $*" >>"%s"\n' "$FAKE_LOG" >"$FAKE_DIR/gateway.sh"
        : >"$FAKE_DIR/restore.sh"
        echo "gateway installed"
        ;;
    "panel setup")
        read -r name || true
        read -r password || true
        echo "panel: $name ${#password}" >>"$FAKE_LOG"
        [ ${#password} -ge 8 ] || exit 2
        ;;
    "station point") echo "rebooting" ;;
esac
EOF
chmod 755 "$WORK/build/duowave"
printf '{"version":"%s","platform":"duowave-station-aarch64"}\n' "$VERSION" >"$WORK/build/release.json"
tar -czf "$WORK/server/$VERSION/duowave-station-aarch64.tar.gz" -C "$WORK/build" duowave release.json
SHA=$(sha256sum "$WORK/server/$VERSION/duowave-station-aarch64.tar.gz" | cut -d' ' -f1)
# Another release's folder holding this build: its release.json names 9.9.9, not 9.9.8.
mkdir -p "$WORK/server/9.9.8"
cp "$WORK/server/$VERSION/duowave-station-aarch64.tar.gz" "$WORK/server/9.9.8/"

# The station's own tools, as far as the script asks them.
cat >"$WORK/stub/wget" <<'EOF'
#!/bin/sh
out=
url=
while [ $# -gt 0 ]; do
    case $1 in
        -O) out=$2; shift ;;
        -q) ;;
        *) url=$1 ;;
    esac
    shift
done
echo "wget $url" >>"$FAKE_LOG"
path=${url#http://*/}
[ -f "$FAKE_SERVER/$path" ] || exit 1
cp "$FAKE_SERVER/$path" "$out"
EOF
cat >"$WORK/stub/uname" <<'EOF'
#!/bin/sh
echo "${FAKE_MACHINE:-aarch64}"
EOF
cat >"$WORK/stub/id" <<'EOF'
#!/bin/sh
echo "${FAKE_UID:-0}"
EOF
cat >"$WORK/stub/df" <<'EOF'
#!/bin/sh
for path; do :; done
case $path in
    */ram) free=${FAKE_TMP_KB:-100000} ;;
    *) free=${FAKE_DATA_KB:-60000} ;;
esac
echo "Filesystem 1K-blocks Used Available Use% Mounted on"
echo "/dev/fake 90000 1000 $free 1% $path"
EOF
chmod 755 "$WORK/stub/"*

# A fresh station for a case: C is its folder.
fresh() {
    C=$WORK/case-$1
    mkdir -p "$C/system/vendor/quasar" "$C/data/opt" "$C/ram"
    cat >"$C/system/vendor/quasar/quasar.cfg" <<'EOF'
{
    "aliced" : { "uniProxyUrl" : "wss://uniproxy.alice.yandex.net/uni.ws" },
    "common" : { "deviceType" : "plum", "softwareVersion" : "1.0" }
}
EOF
    echo "MemAvailable:     150000 kB" >"$C/meminfo"
    : >"$C/log"
    : >"$C/answers"
    unset FAKE_MACHINE FAKE_UID FAKE_DATA_KB FAKE_TMP_KB FAKE_INSTALL_FAILS
    SUM=$SHA
    WANT=$VERSION
    TTY=$C/answers
}
# The answers typed at the terminal, a line each.
answers() { printf '%s\n' "$@" >"$C/answers"; }
# station.sh on the pretend station, with these arguments; its exit code, its words in $C/out.
station() {
    env PATH="$WORK/stub:$PATH" FAKE_LOG="$C/log" FAKE_SERVER="$WORK/server" FAKE_DIR="$C/data/opt/duowave" \
        DUOWAVE_BASE=http://mirror.test DUOWAVE_VERSION="$WANT" DUOWAVE_SHA256="$SUM" \
        DUOWAVE_CFG="$C/system/vendor/quasar/quasar.cfg" DUOWAVE_DIR="$C/data/opt/duowave" DUOWAVE_TMP="$C/ram" \
        DUOWAVE_MEMINFO="$C/meminfo" DUOWAVE_TTY="$TTY" LANG= LC_ALL= LC_MESSAGES= \
        sh "${RUN:-$SCRIPT}" "$@" >"$C/out" 2>&1
}
logged() { grep -q -- "$1" "$C/log"; }
said() { grep -q -- "$1" "$C/out"; }
installed() { cmp -s "$WORK/build/duowave" "$C/data/opt/duowave/bin/duowave"; }
nothing_left() { [ -z "$(ls -A "$C/ram")" ]; }
# Refused before anything changed: no download, no program, nothing in /tmp.
untouched() { ! logged wget && [ ! -e "$C/data/opt/duowave/bin/duowave" ] && nothing_left; }
why() { echo "exit $1; $(tr '\n' '|' <"$C/out"); log: $(tr '\n' '|' <"$C/log")"; }

# 1. --yes: downloaded from the mirror's version folder, checked, installed, the voice pointed with a reboot.
fresh yes
station --yes
code=$?
if [ $code = 0 ] && logged "wget http://mirror.test/$VERSION/duowave-station-aarch64.tar.gz" && installed &&
    logged "program station install" && logged "program station point local --reboot" && ! logged "panel setup" &&
    nothing_left && said "bin/duowave station code"; then
    ok "--yes: the build downloaded, checked, installed, the voice pointed, /tmp left clean, the code's command named"
else
    fail "--yes" "$(why $code)"
fi

# 2. Asked at the terminal: «да» to the install and the password, «нет» to the voice.
fresh asked
answers да да Мама password-1 нет
station
code=$?
if [ $code = 0 ] && installed && logged "panel: Мама 10" && ! logged "station point" && said "station point local --reboot" &&
    said "Ставить DuoWave на колонку?" && ! said "station code"; then
    ok "asked: installed, the panel's password set, the voice left as it was with the command for later"
else
    fail "asked" "$(why $code)"
fi

# 3. «нет» at the plan: nothing downloaded, nothing changed.
fresh no
answers нет
station
code=$?
if [ $code = 0 ] && untouched && said "Ничего не менял"; then ok "«нет»: nothing downloaded or changed"; else fail "«нет»" "$(why $code)"; fi

# 4. Another processor.
fresh armv7
export FAKE_MACHINE=armv7l
station --yes
code=$?
if [ $code != 0 ] && untouched && said aarch64; then ok "armv7l: refused before anything"; else fail "armv7l" "$(why $code)"; fi

# 5. Not root.
fresh user
export FAKE_UID=1000
station --yes
code=$?
if [ $code != 0 ] && untouched && said root; then ok "not root: refused"; else fail "not root" "$(why $code)"; fi

# 6. Little room in /data, in /tmp, little memory.
fresh data
export FAKE_DATA_KB=10000
station --yes
code=$?
if [ $code != 0 ] && untouched && said "/data"; then ok "10 MB free in /data: refused"; else fail "/data" "$(why $code)"; fi
fresh tmp
export FAKE_TMP_KB=5000
station --yes
code=$?
if [ $code != 0 ] && untouched && said "$C/ram"; then ok "5 MB free in /tmp: refused"; else fail "/tmp" "$(why $code)"; fi
fresh memory
echo "MemAvailable:     10000 kB" >"$C/meminfo"
station --yes
code=$?
if [ $code != 0 ] && untouched && said "ОЗУ"; then ok "10 MB of memory: refused"; else fail "memory" "$(why $code)"; fi

# 7. Not a station.
fresh nostation
rm "$C/system/vendor/quasar/quasar.cfg"
station --yes
code=$?
if [ $code != 0 ] && untouched && said "не Яндекс Станция"; then ok "no quasar.cfg: refused"; else fail "no quasar.cfg" "$(why $code)"; fi

# 8. No terminal and no --yes: nothing asked, nothing done.
fresh notty
TTY=$C/no-such-tty
station
code=$?
if [ $code != 0 ] && untouched && said "sh -s -- --yes"; then ok "no terminal: refused, names --yes"; else fail "no terminal" "$(why $code)"; fi

# 9. A build that is not the one the script names: its checksum, then its release.json.
fresh badsum
SUM=0000000000000000000000000000000000000000000000000000000000000000
station --yes
code=$?
if [ $code != 0 ] && [ ! -e "$C/data/opt/duowave/bin/duowave" ] && nothing_left && said "Контрольная сумма"; then
    ok "another checksum: refused, nothing installed, /tmp clean"
else
    fail "checksum" "$(why $code)"
fi
fresh otherversion
WANT=9.9.8
station --yes
code=$?
if [ $code != 0 ] && [ ! -e "$C/data/opt/duowave/bin/duowave" ] && nothing_left && said "не та версия"; then
    ok "the build's release.json names another version: refused"
else
    fail "release.json" "$(why $code)"
fi

# 10. The download fails.
fresh nodownload
WANT=1.0.0
station --yes
code=$?
if [ $code != 0 ] && [ ! -e "$C/data/opt/duowave/bin/duowave" ] && nothing_left && said "Не скачалось"; then
    ok "no download: said so, nothing installed"
else
    fail "no download" "$(why $code)"
fi

# 11. Over an earlier install: its gateway stopped first, the program replaced, the data kept.
fresh over
mkdir -p "$C/data/opt/duowave/bin" "$C/data/opt/duowave/gateway"
printf '#!/bin/sh\necho "duowave 0.1.0"\n' >"$C/data/opt/duowave/bin/duowave"
chmod 755 "$C/data/opt/duowave/bin/duowave"
printf '#!/bin/sh\necho "gateway.sh $*" >>"%s"\n' "$C/log" >"$C/data/opt/duowave/gateway.sh"
echo kept >"$C/data/opt/duowave/gateway/duowave.sqlite"
station --yes
code=$?
if [ $code = 0 ] && logged "gateway.sh stop" && installed && [ "$(cat "$C/data/opt/duowave/gateway/duowave.sqlite")" = kept ] &&
    said "duowave 0.1.0"; then
    ok "over an earlier install: stopped, replaced, the data kept"
else
    fail "over an earlier install" "$(why $code)"
fi

# 12. A YAM package's gateway: left to YAM.
fresh yam
mkdir -p "$C/data/opt/duowave/bin"
echo old >"$C/data/opt/duowave/bin/duowave"
echo yam >"$C/data/opt/duowave/managed-by"
station --yes
code=$?
if [ $code != 0 ] && ! logged wget && [ "$(cat "$C/data/opt/duowave/bin/duowave")" = old ] && said "yam install"; then
    ok "installed by YAM: refused, left to YAM"
else
    fail "YAM" "$(why $code)"
fi

# 13. The gateway does not start: said, with where to look.
fresh nostart
export FAKE_INSTALL_FAILS=1
station --yes
code=$?
if [ $code != 0 ] && said "station log" && ! logged "station point"; then ok "no start: said, the voice not pointed"; else fail "no start" "$(why $code)"; fi

# 14. The panel's password: a short one, then a good one.
fresh password
answers да да "" short "" password-2 нет
station
code=$?
if [ $code = 0 ] && logged "panel:  5" && logged "panel:  10" && said "Ещё раз"; then
    ok "the panel's password asked again after a short one"
else
    fail "the panel's password" "$(why $code)"
fi

# 15. In English, and Yandex's updates still on.
fresh english
printf '{ "x": ["--updatesd"] }\n' >"$C/system/vendor/quasar/launch.json"
answers no
station --lang en
code=$?
if [ $code = 0 ] && said "Install DuoWave on the station?" && said "Yandex's updates are not turned off" && untouched; then
    ok "--lang en, and the warning about Yandex's updates"
else
    fail "English" "$(why $code)"
fi

# 16. Straight from git: no version, no checksum — refused.
fresh unrendered
WANT=
SUM=
station --yes
code=$?
if [ $code != 0 ] && untouched; then ok "no release written in: refused"; else fail "unrendered" "$(why $code)"; fi

# 17. As it comes: through a pipe, the script on stdin (wget -qO- … | sh -s -- --yes).
fresh pipe
code=0
env PATH="$WORK/stub:$PATH" FAKE_LOG="$C/log" FAKE_SERVER="$WORK/server" FAKE_DIR="$C/data/opt/duowave" \
    DUOWAVE_BASE=http://mirror.test DUOWAVE_VERSION="$VERSION" DUOWAVE_SHA256="$SHA" \
    DUOWAVE_CFG="$C/system/vendor/quasar/quasar.cfg" DUOWAVE_DIR="$C/data/opt/duowave" DUOWAVE_TMP="$C/ram" \
    DUOWAVE_MEMINFO="$C/meminfo" sh -s -- --yes <"$SCRIPT" >"$C/out" 2>&1 || code=$?
if [ $code = 0 ] && installed && logged "station point local --reboot"; then ok "through a pipe"; else fail "pipe" "$(why $code)"; fi

# 18. A download cut short runs nothing.
fresh cut
head -c 4000 "$SCRIPT" >"$WORK/cut.sh"
RUN=$WORK/cut.sh
station --yes
code=$?
RUN=
if [ ! -s "$C/log" ] && [ ! -e "$C/data/opt/duowave/bin/duowave" ]; then ok "a script cut short runs nothing"; else fail "cut short" "$(why $code)"; fi

# 19. The release's script: version and checksum written in (scripts/release/station-script.sh), and it installs.
fresh rendered
if sh "$ROOT/scripts/release/station-script.sh" "$WORK/server/$VERSION/duowave-station-aarch64.tar.gz" "v$VERSION" \
    "$WORK/rendered.sh" >/dev/null 2>"$C/render" && grep -q "@VERSION@\|@SHA256@" "$WORK/rendered.sh"; then
    fail "rendered" "placeholders left"
elif [ ! -f "$WORK/rendered.sh" ]; then
    fail "rendered" "$(cat "$C/render")"
else
    WANT=
    SUM=
    RUN=$WORK/rendered.sh
    station --yes
    code=$?
    RUN=
    if [ $code = 0 ] && installed && logged "wget http://mirror.test/$VERSION/"; then
        ok "the release's script names its version and checksum and installs"
    else
        fail "rendered" "$(why $code)"
    fi
fi
if sh "$ROOT/scripts/release/station-script.sh" "$WORK/server/$VERSION/duowave-station-aarch64.tar.gz" 9.9.8 \
    "$WORK/wrong.sh" >/dev/null 2>&1; then
    fail "render" "wrote a 9.9.8 script for a 9.9.9 build"
else
    ok "no release script for a build of another version"
fi

# 20. The address the station fetches from by default: plain HTTP (no TLS in its busybox), the Worker's host.
# shellcheck disable=SC2016 # the script's own text, looked for as it is written
if grep -q 'BASE=${DUOWAVE_BASE:-http://get.duowave.app/station}' "$SCRIPT" &&
    ! grep -q 'https://get.duowave.app\|packages.duowave.app/station' "$SCRIPT"; then
    ok "the default address is http://get.duowave.app/station"
else
    fail "default address" "$(grep -n 'duowave.app' "$SCRIPT")"
fi

echo "$failures failed"
exit "$failures"
