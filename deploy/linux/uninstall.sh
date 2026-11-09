#!/bin/sh
# Removes the systemd services install.sh put in and the gateway's program in /opt/duowave;
# /var/lib/duowave (the database, journals, voices) stays — delete it yourself if you mean it. Point the station
# back to Yandex first: the panel's «Вернуть к Яндексу».
set -eu
for unit in duowave duowave-asr duowave-tts; do
    if [ -f "/etc/systemd/system/$unit.service" ]; then
        sudo systemctl disable --now "$unit.service" || true
        sudo rm -f "/etc/systemd/system/$unit.service"
        echo "removed $unit"
    fi
done
sudo systemctl daemon-reload
# The program, the one before an update and an unfinished one: nothing of the household's is there.
if [ -d /opt/duowave ]; then
    sudo rm -f /opt/duowave/duowave /opt/duowave/duowave.previous /opt/duowave/duowave.new
    sudo rmdir /opt/duowave 2>/dev/null || true
    echo "removed /opt/duowave"
fi
