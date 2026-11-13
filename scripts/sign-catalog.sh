#!/bin/sh
# Подписать ленту каталога пакетов ключом каталога (docs/plans/plugins.md, этап 8). Рядом с catalog.json ложится
# catalog.json.minisig: шлюз с ключами каталога (вшитые gateway/packages.pub или PACKAGES_PUBKEY) ставит из такой
# ленты и плагины с приложениями, если подпись сходится с любым из ключей; без подписи — только пакеты без кода
# (персонажи, темы). Архивы подпись покрывает через свои SHA-256 в ленте.
#
# Ключей два, выпускной и запасной: minisign -G -p <имя>.pub -s <имя>.key для каждого. Выпускной секретный — в
# секретах CI каталога, запасной — офлайн; открытые — подряд в gateway/packages.pub и в packages.pub каталога.
# Смена, потеря и утечка — docs/plans/key-rotation.md, «Ключ каталога пакетов».
#
#   scripts/sign-catalog.sh public/catalog.json [packages.key]
set -eu
feed="${1:?путь к catalog.json}"
key="${2:-${PACKAGES_SECRET_KEY_FILE:-packages.key}}"
command -v minisign >/dev/null || { echo "нужен minisign (brew install minisign)" >&2; exit 1; }
minisign -S -s "$key" -m "$feed" -x "$feed.minisig" -t "duowave catalog $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo "$feed.minisig"
