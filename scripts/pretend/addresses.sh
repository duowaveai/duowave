#!/bin/sh
# Addresses for pretend stations on this Mac (docs/plans/pretend-stations.md): 10.77.0.1 for the test Turbo and
# 10.77.0.2… for the stations, on the loopback lo0. Only this Mac sees them; a reboot takes them away.
#
#   sudo scripts/pretend/addresses.sh 3      # the Turbo and three stations: 10.77.0.1–10.77.0.4
#   sudo scripts/pretend/addresses.sh off    # all of them away
set -eu

NET=10.77.0
count=${1:-}

if [ "$(uname)" != "Darwin" ]; then
  echo "macOS only." >&2
  exit 2
fi
if [ "$(id -u)" != "0" ]; then
  echo "Needs the Mac administrator's password: sudo $0 ${count:-3}" >&2
  exit 2
fi

ours() {
  ifconfig lo0 | awk -v net="$NET." '$1 == "inet" && index($2, net) == 1 { print $2 }'
}

if [ "$count" = "off" ]; then
  for address in $(ours); do
    ifconfig lo0 -alias "$address"
    echo "removed $address"
  done
  exit 0
fi

case "$count" in
  ''|*[!0-9]*) echo "How many stations: a number from 1 to 10, or off." >&2; exit 2 ;;
esac
if [ "$count" -lt 1 ] || [ "$count" -gt 10 ]; then
  echo "How many stations: from 1 to 10." >&2
  exit 2
fi

# A network of the same numbers elsewhere (a VPN, the office): the pretend addresses would hide it — refuse.
destination=$(route -n get "$NET.200" 2>/dev/null | awk '$1 == "destination:" { print $2 }')
interface=$(route -n get "$NET.200" 2>/dev/null | awk '$1 == "interface:" { print $2 }')
if [ -n "$destination" ] && [ "$destination" != "default" ] && [ "$interface" != "lo0" ]; then
  echo "The network $NET.0/24 is already on this Mac ($interface, a VPN?) — pretend stations would hide it. Not adding." >&2
  exit 1
fi

last=$((count + 1))
i=1
while [ "$i" -le "$last" ]; do
  address="$NET.$i"
  if ours | grep -qx "$address"; then
    echo "already there $address"
  else
    ifconfig lo0 alias "$address" netmask 255.255.255.255 up
    echo "added $address"
  fi
  i=$((i + 1))
done
echo "Done: $NET.1 — the test Turbo, $NET.2–$NET.$last — the stations. Now: scripts/pretend/run.sh $count"
