#!/bin/sh
# The station's voice back to Yandex: the firmware's own quasar.cfg (our copy over it unmounted and gone), then a
# reboot. Only the voice: DuoWave stays on the station (the gateway on it and the call client, if any, keep running)
# and the panel connects it again. To take DuoWave off the station altogether: sh /data/opt/duowave/uninstall.sh
# Left here by the gateway; runs without it: sh /data/opt/duowave/restore.sh
set -eu
CFG=/system/vendor/quasar/quasar.cfg
if [ -f /data/opt/duowave/voice.sh ]; then
    sh /data/opt/duowave/voice.sh drop
else
    while grep -q " $CFG " /proc/mounts; do umount "$CFG"; done
    rm -f /data/opt/duowave/voice/quasar.cfg /data/opt/duowave/voice/firmware.cfg
fi
echo "back to the firmware's own voice server; DuoWave stays here (to take it off: sh /data/opt/duowave/uninstall.sh); rebooting"
reboot
