#!/bin/sh
# The reference app `duowave-echo-app` laid out as a package in a gateway's packages folder — to try an app by hand:
# its tool, its fall, and its long task with the widget «Долгий счёт» (docs/plugins/AUTHORING.md, «Долгие задачи»).
#
#   cargo build -p duowave --bin duowave-echo-app
#   scripts/dev/echo-app-package.sh <DATA_DIR> [path to the built duowave-echo-app]
#
# <DATA_DIR> is the DATA_DIR of the gateway you try it with — a trial one, never the one you live with. Then: the
# panel → «Приложения» → «Эхо-приложение» → switch on. The build is for this machine only.
set -eu

[ $# -ge 1 ] || { echo "usage: $0 <DATA_DIR> [duowave-echo-app]" >&2; exit 2; }
here=$(cd "$(dirname "$0")/../.." && pwd)
data=$1
binary=${2:-$here/target/debug/duowave-echo-app}
[ -f "$binary" ] || { echo "no build at $binary: cargo build -p duowave --bin duowave-echo-app" >&2; exit 1; }

case "$(uname -s)-$(uname -m)" in
    Darwin-arm64) platform=darwin-arm64 ;;
    Darwin-x86_64) platform=darwin-x86_64 ;;
    Linux-aarch64) platform=linux-aarch64 ;;
    Linux-x86_64) platform=linux-x86_64 ;;
    *) echo "no name for this platform: $(uname -s)-$(uname -m)" >&2; exit 1 ;;
esac

dir=$data/packages/echo-app/1.0.0
mkdir -p "$dir/bin"
cp "$binary" "$dir/bin/echo-app"
cp "$here/docs/plugins/apps/echo-app/panel.js" "$dir/panel.js"
if command -v sha256sum >/dev/null 2>&1; then
    sha=$(sha256sum "$dir/bin/echo-app" | cut -d' ' -f1)
else
    sha=$(shasum -a 256 "$dir/bin/echo-app" | cut -d' ' -f1)
fi
cat > "$dir/manifest.json" <<JSON
{
  "schema": 1, "kind": "app", "runtime": "process", "id": "echo-app", "version": "1.0.0",
  "title": "Эхо-приложение", "description": "Эталон приложения: повторяет вход, падает по просьбе и долго считает.",
  "api": "^2", "runs": ["pc", "station", "stationServer"],
  "budget": { "memoryMb": 64, "tasks": 1 }, "permissions": [],
  "targets": { "$platform": { "file": "bin/echo-app", "sha256": "$sha" } },
  "health": { "path": "/health", "intervalS": 5 },
  "contributes": {
    "tools": [
      { "name": "echo_app", "description": "Повторяет вход", "handler": "echo" },
      { "name": "echo_crash", "description": "Падает", "handler": "crash" },
      { "name": "echo_count", "description": "Долго считает", "handler": "count" }
    ],
    "panel": { "slots": ["desk.widgets"], "calls": ["count"] }
  }
}
JSON
echo "echo-app 1.0.0 → $dir"
