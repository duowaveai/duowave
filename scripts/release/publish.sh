#!/bin/sh
# The public repository from this one: the public tree (snapshot.sh: HEAD without the private, checked for anything
# private) laid over a clone of the public repository and committed there as the project — «DuoWave
# <hello@duowave.ai>», time in UTC — so the working history, its authors and their time zone stay here. The first
# run makes the repository (git init); every later one adds a commit with what changed since. It never pushes: it
# prints the commands, and pushing is the owner's step, by the owner's own hand from their terminal — never an agent's
# (docs/plans/open-source.md, «Порядок: путь до публикации»; the public clone's pre-push asks the owner for «да»).
#
#   scripts/release/publish.sh <public clone folder>          commit the tree as it is at HEAD
#   scripts/release/publish.sh <public clone folder> --tag    and tag it v<version> (scripts/release/version.sh)
#   MESSAGE="Fix …" scripts/release/publish.sh <folder>       with this commit message instead of «DuoWave <version>»
#   MERGE="2 5" scripts/release/publish.sh <folder> [--tag]    and take in these pull requests of the public repository
#
# A pull request of the public repository is first ported here — its commits with their authors (`git am`), checked
# like any change — and comes out with the next publish. MERGE makes that publish a merge commit: its first parent is
# the public main, the others are the pull requests' heads, its tree is ours as always. The contributors' commits stay
# in the public history under their names, GitHub marks the pull requests merged once main is pushed, and nothing of
# the working history goes out. The heads are fetched into refs/remotes/origin/pull/<n>: already public, the public
# clone's pre-push leaves them be and checks only our commit and its tree (docs/open-source/CONTRIBUTING.md, «Как мы
# принимаем PR»).
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

# The pull requests to take in: fetched before anything changes, so a wrong number stops the publish untouched.
HEADS=
for pull in ${MERGE:-}; do
    case "$pull" in *[!0-9]*) echo "MERGE: «$pull» is not a pull request number" >&2; exit 1 ;; esac
    git -C "$PUBLIC" rev-parse -q --verify HEAD >/dev/null || { echo "MERGE: the public repository has no history yet" >&2; exit 1; }
    git -C "$PUBLIC" fetch -q origin "+refs/pull/$pull/head:refs/remotes/origin/pull/$pull" \
        || { echo "MERGE: could not fetch pull request #$pull" >&2; exit 1; }
    head=$(git -C "$PUBLIC" rev-parse "refs/remotes/origin/pull/$pull")
    if git -C "$PUBLIC" merge-base --is-ancestor "$head" HEAD; then
        echo "MERGE: #$pull is already in the public main, skipped"
        continue
    fi
    git -C "$PUBLIC" merge-base HEAD "$head" >/dev/null || { echo "MERGE: #$pull has no history in common with main" >&2; exit 1; }
    HEADS="$HEADS $pull:$head"
done

TREE=$(mktemp -d)/tree
trap 'rm -rf "$(dirname "$TREE")"' EXIT
DRAFT= sh "$ROOT/scripts/release/snapshot.sh" "$TREE"   # never a draft: placeholders stop it

# The tree replaces what the public repository had: files removed here go there too.
rsync -a --checksum --delete --exclude /.git "$TREE/" "$PUBLIC/"   # by content: a same-size file changed in the same second would be skipped

cd "$PUBLIC"
git add -A
if [ -n "$HEADS" ]; then
    # One merge commit: our tree, the public main and the pull requests as parents, their authors credited.
    message=${MESSAGE:-"DuoWave $VERSION"}
    parents="-p HEAD"
    taken=
    credits=
    for entry in $HEADS; do
        pull=${entry%%:*}
        head=${entry#*:}
        parents="$parents -p $head"
        taken="$taken
Takes in #$pull: $(git log -1 --format=%s "$head")"
        # A pull request ported with changes of ours: its files as they come out differ from its head.
        changed=$(git diff --name-only "$head" --cached -- $(git diff --name-only "$(git merge-base HEAD "$head")" "$head") || true)
        [ -z "$changed" ] || echo "note: #$pull comes out with changes of ours in: $(echo $changed)"
        credits="$credits
$(git log --format='Co-authored-by: %an <%ae>' "HEAD..$head")"
    done
    credits=$(printf '%s\n' "$credits" | grep -v '^$' | grep -v 'DuoWave <hello@duowave.ai>' | awk '!seen[$0]++')
    body="$message
$taken"
    [ -z "$credits" ] || body="$body

$credits"
    commit=$(printf '%s\n' "$body" | TZ=UTC GIT_AUTHOR_NAME=DuoWave GIT_AUTHOR_EMAIL=hello@duowave.ai \
        GIT_COMMITTER_NAME=DuoWave GIT_COMMITTER_EMAIL=hello@duowave.ai git commit-tree "$(git write-tree)" $parents)
    git update-ref -m "publish.sh: $message" HEAD "$commit"
    echo "committed: $(git log --oneline -1) (merges$(for entry in $HEADS; do printf ' #%s' "${entry%%:*}"; done))"
elif git diff --cached --quiet; then
    echo "nothing changed since the last publish"
else
    if git rev-parse -q --verify HEAD >/dev/null; then message=${MESSAGE:-"DuoWave $VERSION"}; else message="DuoWave $VERSION: the first public version"; fi
    TZ=UTC GIT_AUTHOR_NAME=DuoWave GIT_AUTHOR_EMAIL=hello@duowave.ai GIT_COMMITTER_NAME=DuoWave \
        GIT_COMMITTER_EMAIL=hello@duowave.ai git commit -q -m "$message"
    echo "committed: $(git log --oneline -1)"
fi

if [ "$TAG" = --tag ]; then
    git rev-parse -q --verify "refs/tags/v$VERSION" >/dev/null && { echo "v$VERSION is already tagged: raise the version" >&2; exit 1; }
    TZ=UTC GIT_COMMITTER_NAME=DuoWave GIT_COMMITTER_EMAIL=hello@duowave.ai git tag -a "v$VERSION" -m "DuoWave $VERSION"
    echo "tagged: v$VERSION"
fi

echo
echo "to publish (the owner's step):"
git remote get-url origin >/dev/null 2>&1 || echo "  git -C $PUBLIC remote add origin <the public repository's URL>"
echo "  git -C $PUBLIC push origin main"
[ "$TAG" = --tag ] && echo "  git -C $PUBLIC push origin v$VERSION   # starts the release: a draft on GitHub"
exit 0
