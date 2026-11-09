#!/bin/sh
# Stops and removes the launchd services install.sh put in. The project folder stays as it is: .env, data/, logs/
# and the service's program in target/service.
set -eu
for label in app.duowave.gateway app.duowave.asr app.duowave.tts; do
    plist="/Library/LaunchDaemons/$label.plist"
    [ -f "$plist" ] || continue
    sudo launchctl bootout system "$plist" 2>/dev/null || true
    sudo rm -f "$plist"
    echo "removed $label"
done
