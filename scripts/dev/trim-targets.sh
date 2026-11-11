#!/bin/sh
# Keeps the Rust build caches of this repository and its worktrees in bounds without making builds start over. Cargo
# never trims a target folder by itself: what removed worktrees built, old versions of dependencies, builds with other
# flags and stale incremental state stay there for good — on 11.11.2026 they had filled a 926 GB disk.
#
# Daily, in every Cargo target folder of the main checkout and of each worktree (`target`, `desktop/src-tauri/target`):
#   - a folder nothing has read or written for TRIM_STALE_DAYS days (default 7) goes whole: a forgotten worktree's;
#   - so does a profile or platform folder inside (`release-speed`, `x86_64-pc-windows-gnu`), and any build unit of a
#     dependency no build has read that long: an old version, a build with other flags;
#   - our own crates' build units and incremental state go after TRIM_OWN_DAYS days unread (default 2): each worktree
#     builds its own, they are the bulk of what removed worktrees leave behind, and a minute builds them again.
# If all the folders together still take more than TRIM_LIMIT_GB (default 80), next go, in this order: profile and
# platform folders no build used within a day (least recently used first), incremental state, our own crates' units
# unread for 12 hours, and only last a folder some build used today — which then builds from scratch.
#
# "Read" is the access time: a build reads the fingerprint of every unit it needs, and macOS keeps access times. "Our
# own" is a unit whose sources are not in ~/.cargo or the toolchain. Only build caches go: never a worktree, its files
# or its branch. While cargo or rustc runs, it waits (an hour at most, then skips the day): no build loses files.
#
#   scripts/dev/trim-targets.sh [repository]     the repository this script is in by default
#   TRIM_DRY_RUN=1 scripts/dev/trim-targets.sh   only say what it would do
#
# Daily on a Mac (launchd, ~/Library/LaunchAgents/app.duowave.dev.trim-targets.plist): scripts/dev/install-trim.sh.
set -eu
REPO=${1:-$(cd "$(dirname "$0")/../.." && pwd)}
LIMIT_GB=${TRIM_LIMIT_GB:-80}
STALE_DAYS=${TRIM_STALE_DAYS:-7}
OWN_DAYS=${TRIM_OWN_DAYS:-2}
DRY=${TRIM_DRY_RUN:-}
GONE=removed
[ -z "$DRY" ] || GONE="would go"
NOW=$(date +%s)
WORK=$(mktemp -d -t trim-targets)
trap 'rm -rf "$WORK"' EXIT
: >"$WORK/gone"
: >"$WORK/tally"

say() { printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$*"; }
kb() { du -sk "$@" 2>/dev/null | awk '{ s += $1 } END { print s + 0 }'; }
gb() { awk -v kb="$1" 'BEGIN { printf "%.1f", kb / 1048576 }'; }
sum() { awk '{ s += $1 } END { print s + 0 }' "$1"; }
# Removes the paths after the size in KB ($1) they take; the tally keeps what went.
drop() {
    echo "$1" >>"$WORK/tally"
    shift
    [ -n "$DRY" ] || rm -rf "$@"
}
# A whole folder: also remembered, so a dry run does not count what is inside it twice.
drop_folder() {
    drop "$(kb "$1")" "$1"
    printf '%s\n' "$1" >>"$WORK/gone"
}
dropped() {
    while IFS= read -r folder; do
        case $1/ in "$folder"/*) return 0 ;; esac
    done <"$WORK/gone"
    return 1
}
total() { while IFS= read -r target; do kb "$target"; done <"$WORK/targets" | awk '{ s += $1 } END { print s + 0 }'; }
# Has a build read or written anything under $1 within $2 hours? Reads count on fingerprints only: this script reads
# dep-info files itself, and that must not make a folder look used.
used() {
    [ -n "$(find "$1" -type f \( -mmin -"$(($2 * 60))" -o \( -path '*/.fingerprint/*' -amin -"$(($2 * 60))" \) \) \
        -print -quit 2>/dev/null)" ]
}
# The same for incremental state, which only rustc reads.
used_any() { [ -n "$(find "$1" -type f \( -amin -"$(($2 * 60))" -o -mmin -"$(($2 * 60))" \) -print -quit 2>/dev/null)" ]; }
# When a build last read or wrote under $1, in seconds since 1970: builds read every fingerprint they need.
last_use() {
    find "$1" -type f -path '*/.fingerprint/*' -exec stat -f '%a %m' {} + 2>/dev/null |
        awk '{ t = $1 > $2 ? $1 : $2; if (t > m) m = t } END { print m + 0 }'
}
# The hashes of the build units in profile folder $1 nothing has read since $2 (our own crates') or $3 (dependencies),
# in seconds since 1970. A unit's dep-info names its sources; a dependency's are in ~/.cargo or the toolchain.
stale_units() {
    {
        find "$1/deps" -maxdepth 1 -name '*.d' -exec awk 'FNR == 1 {
            line = $0; sub(/^[^:]*: */, "", line); split(line, words, " "); source = words[1]
            if (source != "" && source !~ /\/\.cargo\/(registry|git)\// && source !~ /\/\.rustup\// && source !~ /^\/rustc\//) {
                n = split(FILENAME, parts, "/"); hash = parts[n]; sub(/\.d$/, "", hash); sub(/^.*-/, "", hash); print "own " hash
            }
            nextfile }' {} + 2>/dev/null
        find "$1/.fingerprint" -mindepth 2 -maxdepth 2 -type f -exec stat -f 'use %a %m %N' {} + 2>/dev/null
    } | awk -v own_since="$2" -v rest_since="$3" '
        $1 == "own" { own[$2] = 1; next }
        $1 == "use" {
            n = split($4, parts, "/"); hash = parts[n - 1]; sub(/^.*-/, "", hash)
            t = $2 > $3 ? $2 : $3
            if (t > last[hash]) last[hash] = t
        }
        END {
            for (hash in last)
                if (hash ~ /^[0-9a-f]+$/ && length(hash) == 16 && last[hash] < ((hash in own) ? own_since : rest_since)) print hash
        }'
}
# Removes those units of profile folder $1 (as stale_units with $2 and $3) and says so with the reason $4.
sweep() {
    profile=$1
    why=$4
    stale_units "$1" "$2" "$3" | {
        units=0
        freed=0
        while read -r hash; do
            size=$(kb "$profile"/deps/*-"$hash"* "$profile"/build/*-"$hash" "$profile"/.fingerprint/*-"$hash")
            drop "$size" "$profile"/deps/*-"$hash"* "$profile"/build/*-"$hash" "$profile"/.fingerprint/*-"$hash"
            units=$((units + 1))
            freed=$((freed + size))
        done
        [ "$units" -eq 0 ] || say "$profile: $units build unit(s) $why, $(gb "$freed") GB: $GONE"
    }
}

# A build running now would lose files under it: wait for it to end.
if [ -z "$DRY" ]; then
    waited=0
    while pgrep -x cargo >/dev/null || pgrep -x rustc >/dev/null; do
        [ "$waited" -lt 60 ] || { say "cargo busy for an hour: skipped today"; exit 0; }
        sleep 60
        waited=$((waited + 1))
    done
fi

# Cargo marks each target folder it makes with CACHEDIR.TAG.
git -C "$REPO" worktree list --porcelain | sed -n 's/^worktree //p' | while IFS= read -r tree; do
    for target in "$tree/target" "$tree/desktop/src-tauri/target"; do
        if [ -f "$target/CACHEDIR.TAG" ]; then printf '%s\n' "$target"; fi
    done
done >"$WORK/targets"
before=$(total)

while IFS= read -r target; do
    if ! used "$target" $((STALE_DAYS * 24)); then
        say "$target: $(gb "$(kb "$target")") GB, unused for $STALE_DAYS+ days: $GONE whole"
        drop_folder "$target"
        continue
    fi
    for folder in "$target"/*/; do
        folder=${folder%/}
        [ -d "$folder" ] && ! used "$folder" $((STALE_DAYS * 24)) || continue
        say "$folder: $(gb "$(kb "$folder")") GB, unused for $STALE_DAYS+ days: $GONE"
        drop_folder "$folder"
    done
    find "$target" -maxdepth 3 -type d -name .fingerprint | while IFS= read -r prints; do
        profile=${prints%/.fingerprint}
        ! dropped "$profile" || continue
        sweep "$profile" $((NOW - OWN_DAYS * 86400)) $((NOW - STALE_DAYS * 86400)) \
            "unread for $OWN_DAYS+ days (ours) or $STALE_DAYS+ (dependencies)"
        states=0
        freed=0
        for state in "$profile"/incremental/*/; do
            state=${state%/}
            [ -d "$state" ] && ! used_any "$state" $((OWN_DAYS * 24)) || continue
            size=$(kb "$state")
            drop "$size" "$state"
            states=$((states + 1))
            freed=$((freed + size))
        done
        [ "$states" -eq 0 ] ||
            say "$profile: incremental state of $states crate(s) unread for $OWN_DAYS+ days, $(gb "$freed") GB: $GONE"
    done
done <"$WORK/targets"

now=$((before - $(sum "$WORK/tally")))
[ -n "$DRY" ] || now=$(total)
limit=$((LIMIT_GB * 1048576))
if [ "$now" -gt "$limit" ]; then
    say "build caches: $(gb "$now") GB, over $LIMIT_GB GB"
    while IFS= read -r target; do
        for folder in "$target"/*/; do
            folder=${folder%/}
            [ -d "$folder" ] && ! dropped "$folder" || continue
            printf '%s %s\n' "$(last_use "$folder")" "$folder"
        done
    done <"$WORK/targets" | sort -n >"$WORK/folders"
    day_ago=$((NOW - 86400))
    for pass in idle incremental own busy; do
        while read -r when folder; do
            [ "$now" -gt "$limit" ] || break
            if [ "$pass" = idle ]; then [ "$when" -lt "$day_ago" ] || continue; else [ "$when" -ge "$day_ago" ] || continue; fi
            ! dropped "$folder" || continue
            : >"$WORK/tally.pass"
            case $pass in
                idle | busy)
                    size=$(kb "$folder")
                    say "$folder: $(gb "$size") GB, over the limit ($pass): $GONE"
                    drop_folder "$folder"
                    echo "$size" >"$WORK/tally.pass"
                    ;;
                incremental)
                    for state in "$folder"/incremental "$folder"/*/incremental; do
                        [ -d "$state" ] || continue
                        size=$(kb "$state")
                        say "$state: $(gb "$size") GB, over the limit: $GONE"
                        drop "$size" "$state"
                        echo "$size" >>"$WORK/tally.pass"
                    done
                    ;;
                own)
                    lines=$(wc -l <"$WORK/tally")
                    find "$folder" -maxdepth 2 -type d -name .fingerprint | while IFS= read -r prints; do
                        sweep "${prints%/.fingerprint}" $((NOW - 12 * 3600)) 0 "of ours unread for 12 hours, over the limit"
                    done
                    tail -n +$((lines + 1)) "$WORK/tally" >"$WORK/tally.pass"
                    ;;
            esac
            now=$((now - $(sum "$WORK/tally.pass")))
        done <"$WORK/folders"
    done
fi
after=$((before - $(sum "$WORK/tally")))
[ -n "$DRY" ] || after=$(total)
say "done: build caches $(gb "$before") GB -> $(gb "$after") GB${DRY:+ (dry run)}, free $(df -h "$REPO" | awk 'NR == 2 { print $4 }')"
