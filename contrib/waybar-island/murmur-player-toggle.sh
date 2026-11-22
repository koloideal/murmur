#!/bin/sh
# Toggle the murmur MPRIS popup via the GApplication Activate method —
# instant when the daemon is warm, dbus-activation spawns it otherwise.
gdbus call --session --dest murmur.player \
    --object-path /murmur/player \
    --method org.gtk.Application.Activate "@a{sv} {}" >/dev/null 2>&1 ||
    exec "$HOME/.local/bin/murmur-player.py"
