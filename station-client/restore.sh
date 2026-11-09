#!/bin/sh
# Back to how this station was before the voice gateway: the firmware's own quasar.cfg (our copy over it unmounted and
# gone), then a reboot. Left here by the gateway; runs without it: sh /data/opt/duowave/restore.sh
set -eu
CFG=/system/vendor/quasar/quasar.cfg
if [ -f /data/opt/duowave/voice.sh ]; then
    sh /data/opt/duowave/voice.sh drop
else
    while grep -q " $CFG " /proc/mounts; do umount "$CFG"; done
    rm -f /data/opt/duowave/voice/quasar.cfg /data/opt/duowave/voice/firmware.cfg
fi
echo "back to the firmware's own voice server; rebooting"
reboot
