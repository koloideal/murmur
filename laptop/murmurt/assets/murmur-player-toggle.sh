#!/bin/sh
# Toggle the murmur MPRIS popup via the GApplication Activate method —
# instant when the daemon is warm, dbus-activation spawns it otherwise.
# The fallback also goes through --daemon: a bare instance exits with
# its window and would leave the bus name unowned again.
gdbus call --session --dest murmur.player \
    --object-path /murmur/player \
    --method org.gtk.Application.Activate "@a{sv} {}" >/dev/null 2>&1 ||
    exec "$HOME/.local/bin/murmur-player.py" --daemon
