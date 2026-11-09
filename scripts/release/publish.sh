#!/bin/sh
# The public repository from this one: the public tree (snapshot.sh: HEAD without the private, checked for anything
# private) laid over a clone of the public repository and committed there as the project — «DuoWave
# <hello@duowave.app>», time in UTC — so the working history, its authors and their time zone stay here. The first
# run makes the repository (git init); every later one adds a commit with what changed since. It never pushes: it
# prints the commands, and pushing is the owner's step (docs/plans/open-source.md, «Порядок: путь до публикации»).
#
#   scripts/release/publish.sh <public clone folder>          commit the tree as it is at HEAD
#   scripts/release/publish.sh <public clone folder> --tag    and tag it v<version> (scripts/release/version.sh)
#   MESSAGE="Fix …" scripts/release/publish.sh <folder>       with this commit message instead of «DuoWave <version>»
set -eu
ROOT=$(git rev-parse --show-toplevel)
PUBLIC=${1:?usage: scripts/release/publish.sh <public clone folder> [--tag]}
TAG=${2:-}
VERSION=$("$ROOT/scripts/release/version.sh")

if [ -d "$PUBLIC/.git" ]; then
    [ -z "$(git -C "$PUBLIC" status --porcelain)" ] || { echo "$PUBLIC has changes of its own: commit or drop them" >&2; exit 1; }
elif [ -e "$PUBLIC" ] && [ -n "$(ls -A "$PUBLIC" 2>/dev/null)" ]; then
    echo "$PUBLIC is neither a git repository nor empty" >&2
    exit 1
else
    mkdir -p "$PUBLIC"
    git -C "$PUBLIC" init -q -b main
fi

TREE=$(mktemp -d)/tree
trap 'rm -rf "$(dirname "$TREE")"' EXIT
DRAFT= sh "$ROOT/scripts/release/snapshot.sh" "$TREE"   # never a draft: placeholders stop it

# The tree replaces what the public repository had: files removed here go there too.
rsync -a --delete --exclude /.git "$TREE/" "$PUBLIC/"

cd "$PUBLIC"
git add -A
if git diff --cached --quiet; then
    echo "nothing changed since the last publish"
else
    if git rev-parse -q --verify HEAD >/dev/null; then message=${MESSAGE:-"DuoWave $VERSION"}; else message="DuoWave $VERSION: the first public version"; fi
    TZ=UTC GIT_AUTHOR_NAME=DuoWave GIT_AUTHOR_EMAIL=hello@duowave.app GIT_COMMITTER_NAME=DuoWave \
        GIT_COMMITTER_EMAIL=hello@duowave.app git commit -q -m "$message"
    echo "committed: $(git log --oneline -1)"
fi

if [ "$TAG" = --tag ]; then
    git rev-parse -q --verify "refs/tags/v$VERSION" >/dev/null && { echo "v$VERSION is already tagged: raise the version" >&2; exit 1; }
    TZ=UTC GIT_COMMITTER_NAME=DuoWave GIT_COMMITTER_EMAIL=hello@duowave.app git tag -a "v$VERSION" -m "DuoWave $VERSION"
    echo "tagged: v$VERSION"
fi

echo
echo "to publish (the owner's step):"
git remote get-url origin >/dev/null 2>&1 || echo "  git -C $PUBLIC remote add origin <the public repository's URL>"
echo "  git -C $PUBLIC push origin main"
[ "$TAG" = --tag ] && echo "  git -C $PUBLIC push origin v$VERSION   # starts the release: a draft on GitHub"
exit 0
