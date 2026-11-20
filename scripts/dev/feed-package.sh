#!/bin/sh
# «Лента» на пробном шлюзе (docs/plans/feed.md, этап 6): кладёт пакет из копии каталога в PACKAGES шлюза, как его
# положил бы каталог, и по желанию — серии эксперимента series-feed в папку приложения, без трат.
#
#   scripts/dev/feed-package.sh <DATA_DIR> <папка пакета «Лента» в каталоге, packages/p-8711b037a2db> [--deps <node_modules>] [--import <series-feed>]
#
# --deps: готовые зависимости (папка node_modules с Remotion) вместо npm ci шлюза — на машине разработчика, чтобы не
#   качать сотни мегабайт; без него шлюз сам поставит их по package-lock.json при первом включении.
# --import: 14 серий теста — ролики, реплики, развилки, оценки (node engine/cli.mjs import).
set -eu

[ $# -ge 2 ] || { echo "usage: $0 <DATA_DIR> <packages/p-8711b037a2db> [--deps <node_modules>] [--import <series-feed>]" >&2; exit 2; }
data=$1
source=$2
shift 2
deps=""
tests=""
while [ $# -gt 0 ]; do
    case "$1" in
        --deps) deps=$2; shift 2 ;;
        --import) tests=$2; shift 2 ;;
        *) echo "unknown: $1" >&2; exit 2 ;;
    esac
done
[ -f "$source/manifest.json" ] || { echo "no manifest.json in $source" >&2; exit 1; }
version=$(node -e "process.stdout.write(JSON.parse(require('fs').readFileSync('$source/manifest.json','utf8')).version)")
# id пакета — случайный (docs/plans/random-package-ids.md): берётся из манифеста, как его возьмёт шлюз.
id=$(node -e "process.stdout.write(JSON.parse(require('fs').readFileSync('$source/manifest.json','utf8')).id)")
dir=$data/packages/$id/$version
rm -rf "$dir"
mkdir -p "$dir"
# Всё, кроме зависимостей и служебного: так пакет лежит после каталога.
rsync -a --exclude node_modules --exclude '.*' --exclude remotion/dev "$source/" "$dir/"
if [ -n "$deps" ]; then
    cp -RP "$deps" "$dir/node_modules"
    # Метка шлюза (src/apps/node.rs): зависимости этого lock уже стоят — npm ci не нужен.
    if command -v sha256sum >/dev/null 2>&1; then
        sha=$(sha256sum "$dir/package-lock.json" | cut -d' ' -f1)
    else
        sha=$(shasum -a 256 "$dir/package-lock.json" | cut -d' ' -f1)
    fi
    printf %s "$sha" > "$dir/node_modules/.duowave-dependencies"
fi
echo "feed ($id) $version → $dir"
if [ -n "$tests" ]; then
    DUOWAVE_APP_FILES=$data/plugin-files/$id node "$dir/engine/cli.mjs" import "$tests"
fi
