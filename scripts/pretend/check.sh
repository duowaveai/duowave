#!/bin/sh
# The whole home check on pretend stations, as it was run by hand on 23.11 (docs/plans/pretend-stations.md):
# the hub's panel password, the test Turbo joins Kitchen by the code Kitchen "says", Bedroom becomes Kitchen's ear by
# its own code, a phrase to Bedroom goes through the hub, and the Turbo keeps one record per station with its device.
#
#   scripts/pretend/run.sh --clean 3        # in one terminal: a clean set, and leave it running
#   scripts/pretend/check.sh                # in another: the check, PASS or FAIL per step
#
# It drives the gateways' HTTP API and types into `duowave pretend` through run.sh's input file. The test password is
# made once and kept in ~/.duowave-pretend/test-password (never in git); the panels take it too.
set -eu

HOME_DIR=${DUOWAVE_PRETEND_HOME:-"$HOME/.duowave-pretend"}
T=http://127.0.0.1:17780
HUB=10.77.0.2
EAR=10.77.0.3
J='content-type: application/json'
OUT="$HOME_DIR/pretend.out"
IN="$HOME_DIR/pretend.in"
failed=0

pass() { echo "PASS  $1"; }
fail() { echo "FAIL  $1"; failed=1; }
# Waits up to $1 seconds for the command after it to succeed.
wait_for() {
  limit=$1
  shift
  i=0
  until "$@" >/dev/null 2>&1; do
    i=$((i + 1))
    [ "$i" -ge "$limit" ] && return 1
    sleep 1
  done
}
json() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }

[ -f "$OUT" ] || { echo "No $OUT: start scripts/pretend/run.sh --clean 3 first." >&2; exit 2; }
curl -sf -m 3 "$T/health" >/dev/null || { echo "The test Turbo does not answer at $T." >&2; exit 2; }
if [ ! -f "$HOME_DIR/test-password" ]; then
  echo "pretend-$(openssl rand -hex 8)" >"$HOME_DIR/test-password"
  chmod 600 "$HOME_DIR/test-password"
fi
PW=$(cat "$HOME_DIR/test-password")
# The six digits a station said last: its 🔊 line, the name cut off (the words around the code are the gateway's own).
said() { grep "🔊 $1:" "$OUT" | tail -1 | sed 's/^.*🔊 [^:]*://' | tr -cd '0-9' | cut -c1-6; }
login() { curl -s -X POST "$T/api/v1/auth/login" -H "$J" -d "{\"password\":\"$PW\",\"name\":\"Test\"}" | json "d['message']['token']"; }

# 1. Search: three stations, each at its own address, with the station's SSH greeting.
found=$(curl -s -X POST "$T/api/v1/auth/setup" -H "$J" -d "{\"password\":\"$PW\",\"name\":\"Test\"}" >/dev/null; \
  curl -s -m 20 "$T/api/v1/stations/connect/discover" -H "authorization: Bearer $(login)" |
  json "sum(1 for s in d['message']['stations'] if s.get('ssh'))")
[ "$found" -ge 3 ] && pass "search finds $found stations with SSH" || fail "search finds $found stations"

# 2. The hub's panel password, as after its install: with the first-setup code from its log.
code=$(grep 'первая настройка' "$HOME_DIR/stations/Kitchen/gateway.log" | tail -1 | sed 's/.*код //')
curl -s -X POST "http://$HUB:8790/api/v1/auth/setup" -H "$J" -d "{\"password\":\"$PW\",\"name\":\"Test\",\"code\":\"$code\"}" |
  grep -q token && pass "Kitchen's panel password set" || fail "Kitchen's panel password"

# 3. The Turbo joins Kitchen's home by the code Kitchen says.
id=$(curl -s -X POST "$T/api/v1/placement/pair-code" -H "authorization: Bearer $(login)" -H "$J" -d "{\"address\":\"$HUB\"}" | json "d['message']['id']")
wait_for 10 grep -q "🔊 Kitchen" "$OUT" || fail "Kitchen said no code"
joined=$(curl -s -X POST "$T/api/v1/placement/pair-code/$id" -H "authorization: Bearer $(login)" -H "$J" \
  -d "{\"code\":\"$(said Kitchen)\"}" | json "d['message'].get('joined')")
[ "$joined" = True ] && pass "the Turbo joined Kitchen by its code" || fail "the Turbo joined Kitchen ($joined)"

# The Turbo's people are the home's now: sign in with the hub's password once the copy came.
wait_for 60 sh -c "curl -s -X POST '$T/api/v1/auth/login' -H '$J' -d '{\"password\":\"$PW\",\"name\":\"Test\"}' | grep -q token" ||
  fail "no sign-in on the Turbo after the home's copy"

# 4. Bedroom becomes Kitchen's ear by its own code.
id=$(curl -s -X POST "$T/api/v1/placement/pair-code" -H "authorization: Bearer $(login)" -H "$J" -d "{\"address\":\"$EAR\",\"kind\":\"ear\"}" | json "d['message']['id']")
wait_for 10 grep -q "🔊 Bedroom" "$OUT" || fail "Bedroom said no code"
ear=$(curl -s -X POST "$T/api/v1/placement/pair-code/$id" -H "authorization: Bearer $(login)" -H "$J" \
  -d "{\"code\":\"$(said Bedroom)\"}" | json "d['message'].get('ear')")
[ "$ear" = True ] && pass "Bedroom is Kitchen's ear" || fail "Bedroom as an ear ($ear)"

# 5. The ear's voice goes through the hub at once: no «Allow», no reboot.
wait_for 20 grep -q "from $EAR device" "$HOME_DIR/stations/Kitchen/gateway.log" &&
  pass "the hub took the ear's voice by itself" || fail "the hub did not take the ear's voice"
before=$(grep -c "💬 Bedroom" "$OUT" || true)
echo "Bedroom: what's the weather" >>"$IN"
wait_for 15 sh -c "[ \$(grep -c '💬 Bedroom' '$OUT') -gt $before ]" &&
  pass "a phrase to Bedroom was answered" || fail "no answer to the phrase"

# 6. A minute on: the Turbo keeps one record per station, each with its device (the panel tells the home by it).
sleep 60
records=$(curl -s "$T/health" | json "sorted((s['address'], bool(s.get('device'))) for s in d['message']['placement']['stations'])")
[ "$records" = "[('$HUB', True), ('$EAR', True)]" ] && pass "the Turbo's records: hub and ear, devices known" ||
  fail "the Turbo's records: $records"

# 7. The hub's home status: its voice through DuoWave, the Turbo on the line.
status=$(curl -s "$T/station/at:$HUB/api/v1/home/status" -H "authorization: Bearer $(login)" |
  json "' '.join(r['id']+'='+r['state'] for r in d['message']['rows'])")
case "$status" in
  *voice=ok*turbo=ok*) pass "home status: $status" ;;
  *) fail "home status: $status" ;;
esac

echo
[ "$failed" = 0 ] && echo "All passed. The panels: Turbo $T/panel (password in $HOME_DIR/test-password)." || echo "Something failed: see above and the logs in $HOME_DIR."
exit "$failed"
