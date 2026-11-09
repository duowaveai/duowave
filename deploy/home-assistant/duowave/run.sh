#!/bin/sh
# Home Assistant starts the gateway with the add-on's options (/data/options.json); all it keeps is under /data.
# `run.sh health` is the add-on's health check: the same port, so it asks the gateway that runs.
set -eu
# {"port": 7780}: the number after "port"; 7780 when it is missing or not a port.
PORT=$(sed -n 's/.*"port"[[:space:]]*:[[:space:]]*\([0-9][0-9]*\).*/\1/p' /data/options.json 2>/dev/null | head -n 1)
case "$PORT" in '' | *[!0-9]*) PORT=7780 ;; esac
{ [ "$PORT" -ge 1 ] && [ "$PORT" -le 65535 ]; } 2>/dev/null || PORT=7780
# An add-on's container is a Docker one too: say who installs the updates — Home Assistant.
export PORT DATA_DIR=/data DUOWAVE_MANAGED=home-assistant
cd /data
if [ "${1:-}" = health ]; then exec duowave health; fi
exec duowave serve
