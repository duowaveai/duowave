#!/bin/sh
# What launchd (or a shell) runs for cloned-voice synthesis: omnivoice-server/ with its own venv —
# omnivoice needs its own torch, and a process can load only one, so it is a sidecar, not a tts-server engine.
set -eu
cd "$(dirname "$0")/../.."
mkdir -p logs
[ -f logs/omnivoice.log ] && mv logs/omnivoice.log logs/omnivoice.log.1
exec omnivoice-server/env/bin/python omnivoice-server/server.py --port 8092 >> logs/omnivoice.log 2>&1
