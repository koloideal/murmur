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
        raw = []
        iter_labels(frame, raw)
        # Hidden modules linger in the tree as empty labels teleported
        # offscreen (x ≈ INT_MIN) — drop them before any geometry math.
        # The island's modules render edge-to-edge, so after sorting by x
        # the contiguous run starting at the status label is the island;
        # the media label is the one whose text ends with the wave glyph.
        entries = []
        for label in raw:
            try:
                ext = label.get_extents(Atspi.CoordType.SCREEN)
                if ext.x < -100000:
                    continue
                entries.append((ext.x, ext.x + ext.width, ext.y,
                                ext.y + ext.height, label.get_name() or ""))
            except Exception:
                pass
        entries.sort()
        status_idx = next(
            (i for i, e in enumerate(entries) if e[4].startswith("murmur ")),
            None,
        )
        if status_idx is None:
            return None
        run_end = entries[status_idx][1]
        for e in entries[status_idx + 1:]:
            if e[0] - run_end > 4:
                break  # gap → outside the island pill
            run_end = max(run_end, e[1])
            if e[4].rstrip().endswith("∿"):
                return (e[0], e[2], e[1], e[3])
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
