#!/bin/sh
# The daily trim of the build caches on this Mac (scripts/dev/trim-targets.sh): a copy of the script beside the user's
# data and a launchd job that runs it for this repository every day at 04:30 (or at the next wake). Run it again after
# the script changes; `--remove` takes the job away. Its log: ~/Library/Logs/duowave-trim-targets.log.
#
#   scripts/dev/install-trim.sh            install or update
#   scripts/dev/install-trim.sh --remove   remove the job
set -eu
REPO=$(git -C "$(dirname "$0")" rev-parse --path-format=absolute --git-common-dir | xargs dirname)
LABEL=app.duowave.dev.trim-targets
DIR="$HOME/Library/Application Support/duowave-dev"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
LOG="$HOME/Library/Logs/duowave-trim-targets.log"
DOMAIN="gui/$(id -u)"
launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
if [ "${1:-}" = "--remove" ]; then
    [ -f "$PLIST" ] && mv "$PLIST" "$PLIST.removed"
    echo "removed: $LABEL"
    exit 0
fi
CARGO=$(command -v cargo || echo "$HOME/.cargo/bin/cargo")
mkdir -p "$DIR" "$HOME/Library/LaunchAgents"
cp "$(dirname "$0")/trim-targets.sh" "$DIR/trim-targets.sh"
cat > "$PLIST" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>$LABEL</string>
    <key>ProgramArguments</key>
    <array><string>/bin/sh</string><string>$DIR/trim-targets.sh</string><string>$REPO</string></array>
    <key>EnvironmentVariables</key>
    <dict>
        <key>CARGO</key><string>$CARGO</string>
        <key>PATH</key><string>$(dirname "$CARGO"):/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin</string>
    </dict>
    <key>StartCalendarInterval</key><dict><key>Hour</key><integer>4</integer><key>Minute</key><integer>30</integer></dict>
    <key>LowPriorityIO</key><true/>
    <key>Nice</key><integer>10</integer>
    <key>StandardOutPath</key><string>$LOG</string>
    <key>StandardErrorPath</key><string>$LOG</string>
</dict>
</plist>
PLIST
plutil -lint "$PLIST" >/dev/null
launchctl bootstrap "$DOMAIN" "$PLIST"
echo "installed: $LABEL — daily at 04:30 for $REPO, log $LOG"
