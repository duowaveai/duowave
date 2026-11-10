#!/bin/sh
# The public tree from HEAD: git archive (no history, no untracked files), without what stays private
# (exclude.txt), with the public front in place — the README for strangers, CONTRIBUTING, the licence, NOTICE and
# TRADEMARKS at the root — then checked for anything private. It neither creates a repository nor pushes: publishing
# is the owner's step (docs/plans/open-source.md, section 2).
#
#   scripts/release/snapshot.sh <empty or new folder>
set -eu
ROOT=$(git rev-parse --show-toplevel)
OUT=${1:?usage: scripts/release/snapshot.sh <empty or new folder>}
if [ -e "$OUT" ] && [ -n "$(ls -A "$OUT" 2>/dev/null)" ]; then echo "$OUT is not empty" >&2; exit 1; fi
mkdir -p "$OUT"
[ -z "$(git -C "$ROOT" status --porcelain --untracked-files=no)" ] || echo "note: uncommitted changes are not in the snapshot (it is HEAD)" >&2

git -C "$ROOT" archive --format=tar HEAD | tar -x -C "$OUT"
grep -v '^[[:space:]]*\(#\|$\)' "$ROOT/scripts/release/exclude.txt" | sed 's/[[:space:]]*#.*//' | while read -r entry; do
    rm -rf "${OUT:?}/$entry"
done

# The public front. Lines between <!-- private:start --> and <!-- private:end --> are notes for this repository only.
front() {
    sed '/<!-- private:start -->/,/<!-- private:end -->/d' "$OUT/docs/open-source/$1" > "$OUT/$2"
    rm -f "$OUT/docs/open-source/$1"
}
front README.md README.md
front LICENSE LICENSE
front NOTICE NOTICE
front TRADEMARKS.md TRADEMARKS.md
front CONTRIBUTING.md CONTRIBUTING.md
[ -f "$OUT/docs/open-source/SECURITY.md" ] && front SECURITY.md SECURITY.md
[ -f "$OUT/docs/open-source/CODE_OF_CONDUCT.md" ] && front CODE_OF_CONDUCT.md CODE_OF_CONDUCT.md
rmdir "$OUT/docs/open-source" 2>/dev/null || true
# Links into what stayed private keep their words and lose the target; folders left empty go.
find "$OUT" -name '*.md' -type f -exec perl -pi -e 's/\[([^\]]+)\]\((?:\.\.\/|\.\/)*(?:docs\/)?(?:plans\/|reviews\/|SUMMARY\.md|GUIDE\.md|research\/original-alice\.md)[^)]*\)/$1/g' {} +
find "$OUT" -type d -empty -delete

node "$ROOT/scripts/release/check-private.mjs" --dir "$OUT"
# The public tree builds the gateway on its own: what the program carries from outside gateway/ (include_str!,
# rust-embed, the Dockerfile's COPY) has to be in it. 11.11.2026 the snapshot left out docs/design/glass/THEMES.md and
# CI could not build.
node "$OUT/scripts/check-docker-context.mjs"

# What waits for the owner's decisions (the name, the repository, the licensor, the address): listed, and the tree is
# not ready while any is left. DRAFT=1 builds it anyway, for a look.
left=$(grep -rlE '<(PROJECT NAME|PUBLIC REPOSITORY URL|REPOSITORY FOLDER|LICENSOR FULL NAME|PROJECT EMAIL|OWNER|REPOSITORY|RELEASE URL|ПОЧТА ПРОЕКТА|ФИО правообладателя|НАЗВАНИЕ|адрес публичного репозитория)>' "$OUT" || true)
if [ -n "$left" ]; then
    echo "placeholders still to fill:" >&2
    echo "$left" | sed "s#^$OUT/#  #" >&2
    [ "${DRAFT:-}" = 1 ] || { echo "not ready to publish (DRAFT=1 builds it anyway)" >&2; exit 2; }
fi
echo "the public tree: $OUT ($(du -sh "$OUT" | cut -f1))"
