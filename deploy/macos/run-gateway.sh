#!/bin/sh
# What launchd runs: the gateway (duowave serve) from the project folder — its settings from .env there, the paths
# in it beside this folder — with the Mac kept awake while it runs (caffeinate: no idle sleep, no system sleep on AC
# power). The program is the copy install.sh put in target/service. The log of the previous run is kept as
# duowave.log.1 (up to .5): it is rotated here, at every start.
set -eu
cd "$(dirname "$0")/../.."
mkdir -p logs
for n in 4 3 2 1; do [ -f "logs/duowave.log.$n" ] && mv "logs/duowave.log.$n" "logs/duowave.log.$((n + 1))"; done
[ -f logs/duowave.log ] && mv logs/duowave.log logs/duowave.log.1
if [ ! -x target/service/duowave ]; then
    echo "$(date '+%F %T') no target/service/duowave: run deploy/macos/install.sh" >> logs/duowave.log
    exit 1
fi
exec /usr/bin/caffeinate -is target/service/duowave serve >> logs/duowave.log 2>&1
