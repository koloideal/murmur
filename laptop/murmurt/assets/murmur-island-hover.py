#!/usr/bin/env python3
"""Media hotkeys when hovering the murmur-media waybar island.

Waybar modules can't receive keyboard input, so this watches the mouse:
when the cursor sits over the murmur-media label (found in waybar's
AT-SPI tree), hyprland is put into the "murmur" submap — defined in
hyprland.lua with playerctl binds — and popped back out when it leaves.
"""
import json
import os
import re
import subprocess
import sys
import time

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi


def waybar_accessible():
    desktop = Atspi.get_desktop(0)
    for i in range(desktop.get_child_count()):
        try:
            app = desktop.get_child_at_index(i)
            if app and "waybar" in (app.get_name() or "").lower():
                return app
        except Exception:
            pass
    return None


def iter_labels(node, out):
    try:
        if node.get_role_name() == "label":
            out.append(node)
            return
        for i in range(node.get_child_count()):
            child = node.get_child_at_index(i)
            if child:
                iter_labels(child, out)
    except Exception:
        pass


def media_rect():
    """Extents of the label following the murmur status label, screen coords.

    The waybar frame spans the top of the screen, so the compositor-space
    component extents double as screen coordinates.
    """
    app = waybar_accessible()
    if not app:
        return None
    try:
        frame = None
        for i in range(app.get_child_count()):
            child = app.get_child_at_index(i)
            if child and child.get_role_name() == "frame":
                frame = child
                break
        if not frame:
            return None
        labels = []
        iter_labels(frame, labels)
        for idx, label in enumerate(labels):
            name = label.get_name() or ""
            if name.startswith("murmur ") and idx + 1 < len(labels):
                media = labels[idx + 1]
                if not (media.get_name() or "").strip():
                    return None
                ext = media.get_extents(Atspi.CoordType.SCREEN)
                return (ext.x, ext.y, ext.x + ext.width, ext.y + ext.height)
    except Exception:
        return None
    return None


def cursor_pos():
    out = subprocess.run(
        ["hyprctl", "cursorpos"], capture_output=True, text=True
    ).stdout.strip()
    m = re.match(r"(-?\d+),\s*(-?\d+)", out)
    if not m:
        return None
    return int(m.group(1)), int(m.group(2))


def murmur_player_exists():
    r = subprocess.run(
        ["playerctl", "-l"], capture_output=True, text=True
    )
    return "murmur" in r.stdout


def current_submap():
    out = subprocess.run(
        ["hyprctl", "submap"], capture_output=True, text=True
    ).stdout.strip()
    return out or "default"


def set_submap(name):
    subprocess.run(
        ["hyprctl", "dispatch", f'hl.dsp.submap("{name}")'],
        capture_output=True,
    )


def main():
    if subprocess.run(
        ["pgrep", "-f", "murmur-island-hover.py"], capture_output=True, text=True
    ).stdout.count("\n") > 2:  # self + possibly one other line pattern
        pass  # flock below is the real guard

    lock = open(os.path.expanduser("~/.cache/murmur-island-hover.lock"), "w")
    import fcntl
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        print("already running", file=sys.stderr)
        sys.exit(1)

    rect = None
    rect_at = 0.0
    in_submap = False

    while True:
        try:
            now = time.monotonic()
            if now - rect_at > 1.0:  # the track label width moves around
                rect = media_rect()
                rect_at = now

            pos = cursor_pos()
            inside = (
                rect is not None
                and pos is not None
                and rect[0] <= pos[0] < rect[2]
                and rect[1] <= pos[1] < rect[3]
            )

            if inside and not in_submap:
                if murmur_player_exists() and current_submap() == "default":
                    set_submap("murmur")
                    in_submap = True
            elif not inside and in_submap:
                if current_submap() == "murmur":
                    set_submap("reset")
                in_submap = False

            time.sleep(0.1)
        except Exception as e:
            print(f"murmur-island-hover: {e}", file=sys.stderr)
            time.sleep(1.0)


if __name__ == "__main__":
    main()
