#!/bin/sh
# Подписать ленту каталога пакетов ключом каталога (docs/plans/plugins.md, этап 8). Рядом с catalog.json ложится
# catalog.json.minisig: шлюз с ключом каталога (PACKAGES_PUBKEY или вшитым при сборке DUOWAVE_PACKAGES_PUBKEY) ставит
# из такой ленты и плагины с приложениями; без подписи — только пакеты без кода (персонажи, темы). Архивы подпись
# покрывает через свои SHA-256 в ленте.
#
# Ключ делается один раз: minisign -G -p packages.pub -s packages.key. Секретный — только в CI каталога, открытый —
# в сборку шлюза и в .env.
#
#   scripts/sign-catalog.sh public/catalog.json [packages.key]
set -eu
feed="${1:?путь к catalog.json}"
key="${2:-${PACKAGES_SECRET_KEY_FILE:-packages.key}}"
command -v minisign >/dev/null || { echo "нужен minisign (brew install minisign)" >&2; exit 1; }
minisign -S -s "$key" -m "$feed" -x "$feed.minisig" -t "duowave catalog $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo "$feed.minisig"
