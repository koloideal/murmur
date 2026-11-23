#!/usr/bin/env python3
"""Waybar island for murmurd.

Usage:
  murmur-island.py status       — one JSON line (poll mode)
  murmur-island.py status-live  — streams JSON lines forever (smooth pulse)
  murmur-island.py media        — player icon, hidden while murmur is offline
"""

import html
import json
import math
import os
import re
import socket
import subprocess
import sys
import time

SOCK = os.path.join(os.environ.get("XDG_RUNTIME_DIR", "/tmp"), "murmur.sock")

# ctl socket answers get cached this long — enough for tooltips, cheap
# for the 8Hz pulse loop.
STATE_TTL = 4.0


def ctl(req: dict) -> dict:
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(2)
    try:
        s.connect(SOCK)
        s.sendall(json.dumps(req).encode() + b"\n")
        return json.loads(s.makefile("rb").readline())
    finally:
        s.close()


_state = {"at": 0.0, "data": None}


def murmur_state() -> dict | None:
    """Daemon status + peer list, or None if murmurd is unreachable."""
    if time.time() - _state["at"] < STATE_TTL:
        return _state["data"]
    data = None
    try:
        res = ctl({"cmd": "status"})
        if res.get("ok"):
            d = res["data"]
            selfid = d.get("self", "")
            peers = []
            if d.get("online"):
                try:
                    res = ctl({"cmd": "devices"})
                    peers = [
                        x
                        for x in (res.get("data") or {}).get("devices") or []
                        if x.get("device_id") != selfid and not x.get("revoked")
                    ]
                except Exception:
                    pass
            data = {
                "online": d.get("online") is True,
                "revoked": d.get("revoked") is True,
                "lan": bool(d.get("lan_peers")),
                "lan_peers": d.get("lan_peers") or [],
                "lan_rtts": d.get("lan_rtt_ms") or {},
                "self": selfid,
                "name": d.get("name", "?"),
                "kind": d.get("family_kind", "serverful"),
                "server": d.get("server", "?"),
                "epoch": d.get("key_epoch", 0),
                "active": d.get("active_node"),
                "presence": {p["device"]: p for p in d.get("presence") or []},
                "peers": peers,
            }
    except Exception:
        data = None
    _state.update(at=time.time(), data=data)
    return data


def murmur_up(st: dict | None) -> bool:
    return bool(
        st
        and st["online"]
        and not st["revoked"]
        and any(x.get("online") for x in st["peers"])
    )


# --- bar theme ---------------------------------------------------------------
# colors.css is rewritten by bar-theme.sh on every wallpaper switch; it
# defines @color_bg/fg/accent for the island pills. We read it ourselves
# (mtime-cached) so every colored span follows the current theme — GTK
# vars don't resolve inside pango markup anyway.
THEME_CSS = os.path.expanduser("~/.config/hypr/colors.css")
_theme = {"mtime": -1.0, "bg": (40, 40, 40), "fg": (235, 219, 178),
          "accent": (214, 93, 14)}


def theme_colors() -> tuple[tuple, tuple, tuple]:
    """(bg, fg, accent) from waybar's colors.css; gruvbox-dark fallback."""
    try:
        mt = os.stat(THEME_CSS).st_mtime
        if mt != _theme["mtime"]:
            src = open(THEME_CSS, encoding="utf-8").read()

            def grab(name: str) -> tuple | None:
                m = re.search(
                    rf'@define-color\s+{name}\s+rgb\((\d+),\s*(\d+),\s*(\d+)\)',
                    src)
                if m:
                    return tuple(int(g) for g in m.groups())
                m = re.search(
                    rf'@define-color\s+{name}\s+#([0-9a-fA-F]{{6}})', src)
                return (tuple(bytes.fromhex(m.group(1))) if m else None)

            for name, key in (("color_bg", "bg"), ("color_fg", "fg"),
                              ("color_accent", "accent")):
                c = grab(name)
                if c:
                    _theme[key] = c
            _theme["mtime"] = mt
    except OSError:
        pass
    return _theme["bg"], _theme["fg"], _theme["accent"]


def _hex(c: tuple) -> str:
    return "#{:02x}{:02x}{:02x}".format(*c)


def _mix(a: tuple, b: tuple, k: float) -> tuple:
    return tuple(round(a[i] + (b[i] - a[i]) * k) for i in range(3))


def status_text(up: bool, lan: bool = False, kind: str = "serverful") -> str:
    bg, fg, accent = theme_colors()
    if not up:
        dot = "#fb4934"  # static red — nothing to breathe for
    else:
        # Breathing dot — ~2.5s sine between dim fg and gruvbox green.
        green = (184, 187, 38)
        k = (math.sin(time.time() / 2.5 * math.tau) + 1) / 2
        dot = _hex(_mix(_mix(bg, fg, 0.55), green, k))
    word = "online" if up else "offline"
    dim = _hex(_mix(bg, fg, 0.5))
    # Serverful gets a route hint ("on lan/wan"); a serverless family
    # has no second route — the word alone says it.
    link = ""
    if up and kind == "serverful":
        link = f'<span foreground="{dim}"> · on {"lan" if lan else "wan"}</span>'
    # kind=None: daemon unreachable — there's no family to name.
    tail = f'<span foreground="{dim}"> · as {kind}</span>' if kind else ""
    return (f'<span foreground="{_hex(fg)}"><b>murmur</b> {word}</span>'
            f'{link}{tail}'
            f' <span foreground="{dot}">●</span>')


def status_tooltip(st: dict | None) -> str:
    if st is None:
        return "murmurd unreachable — systemctl --user status murmurd"
    tip = [
        f"{st['name']} · {st['self']}",
        "revoked — re-pair: murmurd pair --code …"
        if st["revoked"]
        else ("online" if st["online"] else "offline"),
        f"relay  {st['server']}  ·  epoch {st['epoch']}"
        + (
            "   ·   lan ⇄ "
            + ", ".join(
                f"{p} · {(st.get('lan_rtts') or {}).get(p) or 0} ms"
                for p in st["lan_peers"]
            )
            if st["lan_peers"]
            else ""
        ),
        f"active {st['active'] or 'none'}"
        + (" (me)" if st["active"] == st["self"] else ""),
        "",
    ]
    if not st["peers"]:
        tip.append("no peers online")
    for x in sorted(st["peers"], key=lambda p: p.get("name", "")):
        on = x.get("online") is True
        dev_id = x.get("device_id", "?")
        s = st["presence"].get(dev_id, {}).get("state", "")
        rtt = f"{x['rtt_ms']} ms" if on and x.get("rtt_ms") is not None else ""
        tail = " ".join(filter(None, [s, rtt])) or ("online" if on else "offline")
        tip.append(f"{'●' if on else '○'} {x.get('name', '?')}  {dev_id}  {tail}")
    return "\n".join(tip)


def status() -> dict:
    st = murmur_state()
    up = murmur_up(st)
    # Serverless: no relay exists — the family is "online" only while
    # a peer is actually linked; there's no wan to fall back to.
    if st and st["kind"] == "serverless" and not st["lan_peers"]:
        up = False
    return {
        "text": status_text(up, lan=bool(st and st["lan"]), kind=st["kind"] if st else None),
        "class": "on" if up else "off",
        "tooltip": status_tooltip(st),
    }


TITLE_LEN = 30

# The island's on-screen center is written here so the player popup
# can center itself under the group. Refreshed every GEO_INTERVAL.
GEO_FILE = "/tmp/murmur-island-cx"
GEO_INTERVAL = 1.5
# A strip of the bar is screenshotted and the pill located by its flat
# background — pango metrics don't match waybar's real render, and the
# error grows with title length.
BAR_CROP_W = 1600
BAR_H = 44
PILL_ROWS = range(8, 38)
# Pixels from the left screen edge to the murmur island's left edge —
# first group width + margin. Measure it on your bar and either edit
# the default or export MURMUR_ISLAND_LEFT.
ISLAND_LEFT = int(os.environ.get("MURMUR_ISLAND_LEFT", "282"))
PAD_L, GAP, PAD_R = 18, 14, 18

_pango = None


def _measure(markup: str) -> int:
    """Pixel width of the emitted markup in the bar font."""
    global _pango
    if _pango is None:
        import gi
        gi.require_version("Pango", "1.0")
        gi.require_version("PangoCairo", "1.0")
        import cairo
        from gi.repository import Pango, PangoCairo
        surf = cairo.ImageSurface(cairo.FORMAT_ARGB32, 1, 1)
        ctx = PangoCairo.create_context(cairo.Context(surf))
        lay = Pango.Layout(ctx)
        fd = Pango.FontDescription("JetBrainsMono Nerd Font")
        # CSS px are ~96dpi; pango wants points — 14px == 10.5pt.
        fd.set_size(int(14 * 72 / 96 * Pango.SCALE))
        lay.set_font_description(fd)
        _pango = lay
    _pango.set_markup(markup, -1)
    return _pango.get_pixel_size()[0]


def _geo_pill_rect() -> tuple[int, int, int, int] | None:
    """(center-x, bottom, gap) of the island pill, measured on screen.

    The pill's flat background color is sampled just inside its left
    edge; a mid-height scan then finds both edges — a column counts as
    inside the pill while background shows between the glyphs. `bottom`
    is the pill's lower edge; `gap` is the horizontal distance to the
    previous pill — the popup uses it as its own vertical offset.
    """
    try:
        png = subprocess.run(
            ["grim", "-g", f"0,0 {BAR_CROP_W}x{BAR_H}", "-"],
            capture_output=True, timeout=2).stdout
        if not png:
            return None
        import io
        from collections import Counter
        from PIL import Image
        px = Image.open(io.BytesIO(png)).convert("RGB").load()
        bg = Counter(
            px[x, y] for x in range(ISLAND_LEFT + 4, ISLAND_LEFT + 30)
            for y in range(7, 12)).most_common(1)[0][0]

        def inside(x: int) -> bool:
            # Most of the column is background → we're inside the pill;
            # a glyph column shows background on few rows at most.
            return sum(
                all(abs(px[x, y][i] - bg[i]) <= 10 for i in range(3))
                for y in PILL_ROWS) >= 18

        def edge(x: int, step: int) -> int:
            # Walk while inside the pill, tolerating glyph columns
            # (≤3 consecutive non-background columns).
            miss = 0
            while 0 <= x < BAR_CROP_W:
                miss = 0 if inside(x) else miss + 1
                if miss > 3:
                    return x - step * miss
                x += step
            return x

        left = edge(ISLAND_LEFT + 6, -1)
        right = edge(ISLAND_LEFT + 6, 1)
        if not 40 < right - left < 700:
            return None

        def has_bg(x: int) -> bool:
            # Any pill-colored pixel in the column — looser than
            # `inside`, catches rounded-corner slivers at the edges.
            return any(
                all(abs(px[x, y][i] - bg[i]) <= 10 for i in range(3))
                for y in PILL_ROWS)

        # Pill bottom from the right padding column — guaranteed
        # background, no glyphs.
        bx = right - 3
        bg_rows = [y for y in PILL_ROWS
                   if all(abs(px[bx, y][i] - bg[i]) <= 10 for i in range(3))]
        if not bg_rows:
            return None
        bottom = max(bg_rows)

        # Horizontal gap to the previous pill — empty columns between
        # our left edge and the neighbor's right edge.
        gap = 8  # fallback ≈ waybar module spacing
        x = left - 1
        run = 0
        while x > ISLAND_LEFT - 200 and not has_bg(x):
            run += 1
            x -= 1
        if 0 < run < 200:
            gap = run
        return ((left + right) // 2, bottom, gap)
    except Exception:
        return None


def _write_geo(media_markup: str, up: bool) -> None:
    rect = _geo_pill_rect()
    cx, pop_y = 0, 46
    if rect is not None:
        cx, bottom, gap = rect
        pop_y = bottom + gap
    else:
        # No grim/PIL — estimate from pango text metrics instead.
        try:
            s_px = _measure(status_text(up))
            m_px = _measure(media_markup) if media_markup else 0
            inner = PAD_L + s_px + (GAP + m_px if media_markup else 0) + PAD_R
            cx = ISLAND_LEFT + int(inner / 2)
        except Exception:
            return
    try:
        with open(GEO_FILE, "w") as f:
            f.write(f"{cx} {pop_y}")
    except OSError:
        pass


def media_fetch() -> dict | None:
    try:
        out = subprocess.run(
            [
                "playerctl", "-p", "murmur", "metadata", "--format",
                "{{status}}|{{title}}|{{artist}}",
            ],
            capture_output=True,
            text=True,
            timeout=2,
        )
        if out.returncode != 0 or not out.stdout.strip():
            return None
    except Exception:
        return None
    st, _, rest = out.stdout.strip().partition("|")
    title, _, artist = rest.partition("|")
    return {"status": st, "title": title, "artist": artist}


def media_frame(md: dict | None, now: float) -> dict:
    # Player closed/stopped on the phone — hide the widget entirely.
    if md is None or md["status"] == "Stopped":
        return {"text": "", "class": "hidden", "tooltip": ""}
    title = md["title"] or ""
    shown = title[:TITLE_LEN] + ("…" if len(title) > TITLE_LEN else "")
    tip = f"{title} — {md['artist']}".strip(" —") or "phone player"
    tip += f"\n{md['status'].lower()}"
    # ∿ — the sine-wave logo. Breathes dim→accent while playing, dim when
    # paused; both ends follow the current bar theme.
    bg, fg, accent = theme_colors()
    k = (math.sin(now * 3.0) + 1) / 2 if md["status"] == "Playing" else 0.0
    col = _hex(_mix(_mix(bg, fg, 0.55), accent, k))
    wave = f'<span foreground="{col}">∿</span>'
    text = (f'<span foreground="{_hex(fg)}">{html.escape(shown)}</span> {wave}'
            if shown else wave)
    cls = "playing" if md["status"] == "Playing" else "paused"
    return {"text": text, "class": cls, "tooltip": tip}


def media_live():
    md = {"at": 0.0, "v": None}
    geo = {"at": 0.0}
    while True:
        now = time.time()
        st = murmur_state()
        if not murmur_up(st):
            out = {"text": "", "class": "hidden", "tooltip": ""}
            print(json.dumps(out, ensure_ascii=False), flush=True)
            if now - geo["at"] >= GEO_INTERVAL:
                geo["at"] = now
                _write_geo("", False)
            time.sleep(0.5)
            continue
        if now - md["at"] >= 1.0:
            md.update(v=media_fetch(), at=now)
        frame = media_frame(md["v"], now)
        if now - geo["at"] >= GEO_INTERVAL:
            geo["at"] = now
            _write_geo(frame["text"], True)
        print(json.dumps(frame, ensure_ascii=False), flush=True)
        time.sleep(0.15)


if __name__ == "__main__":
    what = sys.argv[1] if len(sys.argv) > 1 else "status"
    if what == "status-live":
        while True:
            print(json.dumps(status(), ensure_ascii=False), flush=True)
            time.sleep(0.12)
    elif what == "media-live":
        media_live()
    else:
        out = {"status": status}[what]()
        print(json.dumps(out, ensure_ascii=False))
