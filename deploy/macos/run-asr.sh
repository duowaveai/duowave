#!/bin/sh
# What launchd runs for recognition: GigaAM from asr-server/ with its own environment.
set -eu
cd "$(dirname "$0")/../.."
mkdir -p logs
[ -f logs/asr.log ] && mv logs/asr.log logs/asr.log.1
# The model the panel chose is in asr-server/chosen-model: the server reads it itself and falls back to its default
# when it does not load (as GIGAAM_MODEL it would count as an explicit choice, and a model that fails would stop
# the server at every start under KeepAlive). GIGAAM_MODEL in the environment still wins.
exec asr-server/.venv/bin/python asr-server/server.py --device auto >> logs/asr.log 2>&1
