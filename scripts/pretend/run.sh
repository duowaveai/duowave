#!/bin/sh
# Pretend stations and the test Turbo beside them, on this Mac (docs/plans/pretend-stations.md, scripts/pretend/README.md).
#
#   scripts/pretend/run.sh [--build] [--clean] [--say] [N]     # N stations, 3 by default, at most 10
#
# Each one — a gateway of this branch with its own folder in ~/.duowave-pretend (state, log) and its own address
# 10.77.0.x (scripts/pretend/addresses.sh first). The terminal is `duowave pretend`: what the stations say, their
# answers, and the phrases typed to them. Ctrl+C stops them all. The Mac's live DuoWave and its home are not touched.
set -eu

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
HOME_DIR=${DUOWAVE_PRETEND_HOME:-"$HOME/.duowave-pretend"}
NET=10.77.0
TURBO_PORT=17780
YANDEX_PORT=17700
NAMES="Kitchen Bedroom Nursery Living Study Hallway Bathroom Balcony Workshop Attic"

build=0 clean=0 say=0 count=3
for arg in "$@"; do
  case "$arg" in
    --build) build=1 ;;
    --clean) clean=1 ;;
    --say) say=1 ;;
    ''|*[!0-9]*) echo "Unknown: $arg. Takes: --build --clean --say and the number of stations." >&2; exit 2 ;;
    *) count=$arg ;;
  esac
done
if [ "$count" -lt 1 ] || [ "$count" -gt 10 ]; then
  echo "Stations: from 1 to 10." >&2
  exit 2
fi

# The addresses: the Turbo's and each station's.
last=$((count + 1))
i=1
while [ "$i" -le "$last" ]; do
  if ! ifconfig lo0 | grep -q "inet $NET.$i "; then
    echo "No address $NET.$i. First, once after each Mac reboot:" >&2
    echo "  sudo $ROOT/scripts/pretend/addresses.sh $count" >&2
    exit 1
  fi
  i=$((i + 1))
done

# The program: this branch's build, copied aside — the build folder may be shared with other checkouts.
BIN="$HOME_DIR/bin/duowave"
if [ "$build" = 1 ] || [ ! -x "$BIN" ]; then
  echo "Building this branch's gateway…"
  (cd "$ROOT" && cargo build -p duowave --bin duowave)
  target=${CARGO_TARGET_DIR:-"$ROOT/target"}
  mkdir -p "$HOME_DIR/bin"
  cp "$target/debug/duowave" "$BIN"
fi
if [ ! -f "$ROOT/panel/dist/index.html" ]; then
  echo "The panel is not built (panel/dist): pnpm install && pnpm --filter panel build — without it the panel pages are empty." >&2
fi

if [ "$clean" = 1 ]; then
  rm -rf "$HOME_DIR/turbo" "$HOME_DIR/stations"
  echo "Clean sheet: $HOME_DIR/turbo and $HOME_DIR/stations erased."
fi

stations=""
i=2
while [ "$i" -le "$last" ]; do
  stations="${stations:+$stations,}$NET.$i"
  i=$((i + 1))
done

pids=""
stop() {
  trap - INT TERM EXIT
  [ -n "$pids" ] && kill $pids 2>/dev/null || true
  wait 2>/dev/null || true
  echo "Pretend stations stopped."
}
trap stop INT TERM EXIT

# One gateway: its folder, its log, and the variables given.
start() {
  dir=$1
  shift
  mkdir -p "$dir/data" "$dir/station-root"
  # No station microphone bridge (port 9851 on every address): the live app may hold it, and the others would clash.
  (cd "$dir" && exec env "$@" DATA_DIR="$dir/data" CALLS_STATION_MIC_PORT= "$BIN" serve >>"$dir/gateway.log" 2>&1) &
  pids="$pids $!"
}

start "$HOME_DIR/turbo" PORT=$TURBO_PORT DUOWAVE_PRETEND=turbo DUOWAVE_PRETEND_ADDRESS=$NET.1 \
  DUOWAVE_PRETEND_STATIONS="$stations" DUOWAVE_PRETEND_NAME="Test Turbo"

args=""
i=2
for name in $NAMES; do
  [ "$i" -le "$last" ] || break
  dir="$HOME_DIR/stations/$name"
  # Its «firmware»: the station's quasar.cfg with the voice pointed at its own gateway, as after an install.
  cfg="$dir/station-root/system/vendor/quasar/quasar.cfg"
  if [ ! -f "$cfg" ]; then
    mkdir -p "$(dirname "$cfg")"
    printf '%s\n' '{"aliced":{"uniProxyUrl":"ws://127.0.0.1:8790/uni.ws","uniProxyQuicUrl":""},"common":{"deviceType":"pretend"}}' >"$cfg"
  fi
  start "$dir" PORT=8790 DUOWAVE_ROLE=station DUOWAVE_PRETEND=station DUOWAVE_PRETEND_ADDRESS=$NET.$i \
    DUOWAVE_PRETEND_NAME="$name" DUOWAVE_PRETEND_STATIONS="$stations" DUOWAVE_PRETEND_SAY=$say \
    DUOWAVE_STATION_ROOT="$dir/station-root" YANDEX_UNIPROXY_URL="ws://127.0.0.1:$YANDEX_PORT/uni.ws"
  args="$args --station $name=$NET.$i:8790 --log $name=$dir/gateway.log"
  echo "Station $name: panel http://$NET.$i:8790/panel, log $dir/gateway.log"
  i=$((i + 1))
done
echo "Test Turbo: panel http://127.0.0.1:$TURBO_PORT/panel, log $HOME_DIR/turbo/gateway.log"
echo "The Turbo's panel opens from this Mac without a code. A station's panel asks for the first-setup code: grep 'первая настройка' <the station's log>"

sayflag=""
[ "$say" = 1 ] && sayflag="--say"
# The terminal's lines and the ones scripts/pretend/check.sh adds to pretend.in both reach `duowave pretend`; what it
# shows is kept in pretend.out for the check too.
IN="$HOME_DIR/pretend.in"
OUT="$HOME_DIR/pretend.out"
: >"$IN"
: >"$OUT"
# shellcheck disable=SC2086
{ tail -n 0 -f "$IN" & cat; wait; } | "$BIN" pretend --yandex-port $YANDEX_PORT $sayflag $args | tee "$OUT"
