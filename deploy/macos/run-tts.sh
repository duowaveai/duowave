#!/bin/sh
# What launchd runs for our own voice: tts-server/ with every model of tts-server/models, in the recognition's
# environment (torch, onnxruntime); piper-tts comes from tts-server/.venv-piper.
set -eu
cd "$(dirname "$0")/../.."
mkdir -p logs
[ -f logs/tts.log ] && mv logs/tts.log logs/tts.log.1
exec asr-server/.venv/bin/python tts-server/server.py --port 8091 >> logs/tts.log 2>&1
