#!/bin/bash
# omnivoice-server — окружение (python 3.11, свой torch 2.8: в один процесс два torch не грузятся,
# поэтому отдельный sidecar, а не движок в tts-server). Веса модели (~2 ГБ) скачаются с HuggingFace
# при первом старте сервера в ./hf-cache.
set -euo pipefail
cd "$(dirname "$0")"

if [[ ! -x env/bin/python ]]; then
  echo ">> [omnivoice-server] создаю venv (python 3.11)"
  uv venv --python 3.11 env
fi
echo ">> [omnivoice-server] ставлю зависимости (torch 2.8 + omnivoice)"
uv pip install -p env/bin/python -q -r requirements.txt
echo ">> [omnivoice-server] готово. Запуск: deploy/macos/run-omnivoice.sh  (или env/bin/python server.py)"
