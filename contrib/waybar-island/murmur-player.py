#!/usr/bin/env python3
"""Floating MPRIS popup for the phone player.

Mirrors the swaync mpris widget: blurred cover-art backdrop, artwork,
title/artist, shuffle/prev/play/next/repeat and a seek bar, floating
under the waybar murmur island.

Runs as a warm GApplication daemon (`--daemon` via exec-once): the D-Bus
Activate method toggles the popup, so a waybar click is one gdbus call.
Closing destroys the window — rebuilding inside a warm process costs
~ms. The window is positioned once on open; after that the user owns it.
"""

from __future__ import annotations

import json
import os
import socket
import subprocess
import sys
from collections.abc import Callable

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Gdk", "4.0")
gi.require_version("GdkPixbuf", "2.0")
from gi.repository import Gdk, GdkPixbuf, Gio, GLib, Gtk

# --- wire constants ---------------------------------------------------------

APP_ID = "murmur.player"
BUS_NAME = "org.mpris.MediaPlayer2.murmur"
OBJECT_PATH = "/org/mpris/MediaPlayer2"
PLAYER_IFACE = "org.mpris.MediaPlayer2.Player"
PROPS_IFACE = "org.freedesktop.DBus.Properties"

CTL_SOCK = os.path.join(
    os.environ.get("XDG_RUNTIME_DIR", "/tmp"), "murmur.sock")

WINDOW_WIDTH = 460
POPUP_Y = 46
FALLBACK_X = 200
GEO_FILE = "/tmp/murmur-island-cx"  # island center-x, see murmur-island.py
PLACE_INTERVAL_MS = 80
PLACE_TRIES = 30
SEEK_STEP_US = 5_000_000  # arrow keys jump ±5s

CSS = b"""
window { border-radius: 16px; }
picture.bg { border-radius: 16px; }
box.scrim { border-radius: 16px;
  background: linear-gradient(rgba(20,20,20,0.55), rgba(20,20,20,0.78)); }
box.player { padding: 24px; }
picture.art { border-radius: 10px; }
label.title { font-family: "Unbounded"; font-weight: 600; }
label.artist { font-family: "Onest"; color: #d5c4a1; }
button.transport { min-width: 38px; min-height: 38px;
                   background: rgba(60,56,54,0.85); border-radius: 19px;
                   color: #ebdbb2; }
button.transport:hover { background: rgba(80,73,69,0.95); }
button.transport.dim { color: #7c6f64; }
scale trough { min-height: 10px; background: rgba(40,40,40,0.95);
               border: 1px solid #a89984; border-radius: 5px; }
scale highlight { background: #d65d0e; border-radius: 4px; }
scale fill { background: transparent; }
scale slider { min-width: 0; min-height: 0; margin: 0; padding: 0;
               background: transparent; border: none; box-shadow: none;
               color: transparent; opacity: 0; }
label.pos { font-family: "Onest"; color: #d5c4a1; font-size: 11px; }
"""


# --- murmurd control socket -------------------------------------------------

class Ctl:
    """murmurd's local control interface — JSON lines over a Unix socket."""

    def __init__(self, path: str = CTL_SOCK) -> None:
        self._path = path

    def ask(self, request: dict) -> dict:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
            sock.settimeout(1)
            sock.connect(self._path)
            sock.sendall(json.dumps(request).encode() + b"\n")
            return json.loads(sock.makefile("rb").readline())

    def murmur_up(self) -> bool:
        """Daemon reachable, online, not revoked, and a peer is online."""
        try:
            status = self.ask({"cmd": "status"}).get("data") or {}
            if not (status.get("online") and not status.get("revoked")):
                return False
            devices = self.ask({"cmd": "devices"})
            peers = [
                d for d in (devices.get("data") or {}).get("devices") or []
                if d.get("device_id") != status.get("self")
                and not d.get("revoked")
            ]
            return any(d.get("online") for d in peers)
        except (OSError, ValueError):
            return False


# --- hyprland ---------------------------------------------------------------

class Hyprland:
    """Minimal hyprctl wrapper — position queries and lua-dispatch moves.

    Static move/size rule effects are flaky for this window on
    hyprland 0.57, so we drive `hl.dsp.window.move` ourselves.
    """

    @staticmethod
    def client_at(class_name: str) -> list[int] | None:
        try:
            out = subprocess.run(
                ["hyprctl", "clients", "-j"],
                capture_output=True, text=True, timeout=1).stdout
            return next(
                (c["at"] for c in json.loads(out)
                 if c.get("class") == class_name), None)
        except Exception:
            return None

    @staticmethod
    def move(class_name: str, x: int, y: int) -> None:
        expr = (
            "hl.dispatch(hl.dsp.window.move("
            f"{{window='class:{class_name}', x={x}, y={y}}}))"
        )
        subprocess.Popen(
            ["hyprctl", "eval", expr],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def island_anchor() -> tuple[int, int]:
    """(center-x, y) published by the island's media stream —
    y = pill bottom + the island's own horizontal gap, so the popup
    hangs exactly one pill-gap under the bar. Defaults when missing.
    """
    try:
        parts = open(GEO_FILE).read().split()
        cx = int(parts[0])
        y = int(parts[1]) if len(parts) > 1 else POPUP_Y
        return cx, y
    except (OSError, ValueError):
        return 0, POPUP_Y


def island_centered_x(width: int = WINDOW_WIDTH) -> int:
    """Window x that centers it under the waybar murmur island.

    The island's media stream publishes its center-x every frame;
    fall back to a fixed spot when the file is missing or stale.
    """
    cx, _ = island_anchor()
    return max(8, cx - width // 2) if cx else FALLBACK_X


def popup_y() -> int:
    _, y = island_anchor()
    return y


# --- MPRIS ------------------------------------------------------------------

class Mpris:
    """org.mpris.MediaPlayer2.Player proxy for the murmur MPRIS object."""

    def __init__(self) -> None:
        self._bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)

    def invoke(self, method: str) -> None:
        try:
            self._bus.call_sync(BUS_NAME, OBJECT_PATH, PLAYER_IFACE,
                                method, None, None,
                                Gio.DBusCallFlags.NONE, -1, None)
        except GLib.Error:
            pass

    def set_property(self, name: str, value: object) -> None:
        variant = (
            value if isinstance(value, GLib.Variant)
            else GLib.Variant("b", value) if isinstance(value, bool)
            else GLib.Variant("s", value))
        try:
            self._bus.call_sync(
                BUS_NAME, OBJECT_PATH, PROPS_IFACE, "Set",
                GLib.Variant("(ssv)", (PLAYER_IFACE, name, variant)),
                None, Gio.DBusCallFlags.NONE, -1, None)
        except GLib.Error:
            pass

    def set_position(self, track_id: str, pos_us: int) -> None:
        try:
            self._bus.call_sync(
                BUS_NAME, OBJECT_PATH, PLAYER_IFACE, "SetPosition",
                GLib.Variant("(ox)", (track_id, pos_us)),
                None, Gio.DBusCallFlags.NONE, -1, None)
        except GLib.Error:
            pass

    def properties(self) -> dict | None:
        try:
            reply = self._bus.call_sync(
                BUS_NAME, OBJECT_PATH, PROPS_IFACE, "GetAll",
                GLib.Variant("(s)", (PLAYER_IFACE,)),
                GLib.VariantType("(a{sv})"),
                Gio.DBusCallFlags.NONE, -1, None)
            return reply.unpack()[0]
        except GLib.Error:
            return None


# --- small helpers ----------------------------------------------------------

def us_to_clock(t_us: int) -> str:
    """Microseconds → m:ss for the seek labels."""
    s = max(0, int(t_us) // 1_000_000)
    return f"{s // 60}:{s % 60:02d}"


def blurred_cover(path: str, w: int, h: int) -> Gdk.Texture | None:
    """Cheap blur: downscale hard then upscale — reads as a blurred cover."""
    try:
        px = GdkPixbuf.Pixbuf.new_from_file(path)
        px = px.scale_simple(24, 24, GdkPixbuf.InterpType.BILINEAR)
        px = px.scale_simple(w, h, GdkPixbuf.InterpType.BILINEAR)
        return Gdk.Texture.new_for_pixbuf(px)
    except Exception:
        return None


# --- the window -------------------------------------------------------------

class PlayerWindow(Gtk.ApplicationWindow):
    """The popup itself: builds the layout once, streams MPRIS state in."""

    def __init__(self, app: Gtk.Application, mpris: Mpris, ctl: Ctl) -> None:
        super().__init__(application=app, title="murmur",
                         decorated=False, resizable=False,
                         default_width=WINDOW_WIDTH)
        self._mpris = mpris
        self._ctl = ctl
        self._track_id: str | None = None
        self._duration_us = 0
        self._position_us = 0
        self._art_path: str | None = None
        self._place_tries = 0

        keys = Gtk.ShortcutController()
        for keyval, action in self._key_bindings():
            # `action` must bind by value — otherwise every shortcut
            # would call whatever the loop left in `action` last.
            keys.add_shortcut(Gtk.Shortcut.new(
                Gtk.KeyvalTrigger.new(
                    keyval, Gdk.ModifierType.NO_MODIFIER_MASK),
                Gtk.CallbackAction.new(
                    lambda *a, act=action: act() or True)))
        self.add_controller(keys)

        self.set_child(self._build())

        self._sources = [
            GLib.timeout_add(PLACE_INTERVAL_MS, self._place),
            GLib.timeout_add(1000, self._tick_position),
            GLib.timeout_add(2500, self._close_when_offline),
        ]
        self.connect(
            "destroy",
            lambda *_: [GLib.source_remove(s) for s in self._sources])
        GLib.idle_add(self._refresh)

    # -- keyboard --------------------------------------------------------------

    def _key_bindings(self) -> list[tuple[int, Callable[[], object]]]:
        """Media-player-standard keys, active while the popup has focus."""
        bindings: list[tuple[int, Callable[[], object]]] = [
            (Gdk.KEY_Escape, self.close),
            (Gdk.KEY_space, lambda: self._mpris.invoke("PlayPause")),
            (Gdk.KEY_Left, lambda: self._seek_by(-SEEK_STEP_US)),
            (Gdk.KEY_Right, lambda: self._seek_by(SEEK_STEP_US)),
            (Gdk.KEY_n, lambda: self._mpris.invoke("Next")),
            (Gdk.KEY_p, lambda: self._mpris.invoke("Previous")),
            (Gdk.KEY_s, lambda: self._toggle_shuffle()),
            (Gdk.KEY_r, lambda: self._cycle_loop()),
        ]
        # Dedicated media keys — names exist in GDK4 but guard anyway.
        for name, method in (("AudioPlay", "PlayPause"),
                             ("AudioNext", "Next"),
                             ("AudioPrev", "Previous")):
            keyval = getattr(Gdk, f"KEY_{name}", None)
            if keyval is not None:
                bindings.append(
                    (keyval, lambda m=method: self._mpris.invoke(m)))
        return bindings

    def _seek_by(self, delta_us: int) -> None:
        if self._track_id and self._duration_us:
            pos = min(max(self._position_us + delta_us, 0),
                      self._duration_us)
            self._mpris.set_position(self._track_id, pos)
            self._set_position(pos)

    def _toggle_shuffle(self) -> None:
        props = self._mpris.properties() or {}
        self._mpris.set_property("Shuffle", not props.get("Shuffle", False))

    def _cycle_loop(self) -> None:
        props = self._mpris.properties() or {}
        nxt = {"None": "Playlist", "Playlist": "Track",
               "Track": "None"}
        self._mpris.set_property(
            "LoopStatus", nxt.get(props.get("LoopStatus", "None"), "Playlist"))

    # -- layout --------------------------------------------------------------

    def _build(self) -> Gtk.Widget:
        self._bg = Gtk.Picture(content_fit=Gtk.ContentFit.COVER,
                               can_shrink=True, hexpand=True, vexpand=True)
        self._bg.add_css_class("bg")
        self._bg.set_opacity(0.85)
        scrim = Gtk.Box(hexpand=True, vexpand=True)
        scrim.add_css_class("scrim")

        self._art = Gtk.Picture(can_shrink=True)
        self._art.set_size_request(96, 96)
        self._art.add_css_class("art")

        self._title = Gtk.Label(xalign=0, ellipsize=3, max_width_chars=28)
        self._title.add_css_class("title")
        self._artist = Gtk.Label(xalign=0, ellipsize=3, max_width_chars=32)
        self._artist.add_css_class("artist")

        self._b_play = self._transport("media-playback-start-symbolic")
        self._b_play.connect(
            "clicked", lambda *_: self._mpris.invoke("PlayPause"))
        self._b_shuffle = self._transport(
            "media-playlist-shuffle-symbolic", dim=True)
        self._b_loop = self._transport(
            "media-playlist-repeat-symbolic", dim=True)
        buttons = [
            self._b_shuffle,
            self._transport("media-skip-backward-symbolic"),
            self._b_play,
            self._transport("media-skip-forward-symbolic"),
            self._b_loop,
        ]
        self._b_shuffle.connect(
            "clicked", lambda *_: self._toggle_shuffle())
        buttons[1].connect(
            "clicked", lambda *_: self._mpris.invoke("Previous"))
        buttons[3].connect(
            "clicked", lambda *_: self._mpris.invoke("Next"))
        self._b_loop.connect(
            "clicked", lambda *_: self._cycle_loop())
        controls = Gtk.Box(spacing=12, halign=Gtk.Align.CENTER)
        for b in buttons:
            controls.append(b)

        self._pos = Gtk.Label(xalign=0)
        self._pos.add_css_class("pos")
        self._scale = Gtk.Scale.new_with_range(
            Gtk.Orientation.HORIZONTAL, 0, 1, 1)
        self._scale.set_hexpand(True)
        self._scale.set_draw_value(False)
        # arrows belong to ±5s seeking, not to the scale's own stepping
        self._scale.set_focusable(False)
        self._scale.connect("change-value", self._on_seek)
        self._dur = Gtk.Label(xalign=1)
        self._dur.add_css_class("pos")
        pos_row = Gtk.Box(spacing=6)
        for w in (self._pos, self._scale, self._dur):
            pos_row.append(w)

        right = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6,
                        hexpand=True, valign=Gtk.Align.CENTER)
        for w in (self._title, self._artist, controls, pos_row):
            right.append(w)

        content = Gtk.Box(spacing=14)
        content.add_css_class("player")
        content.append(self._art)
        content.append(right)

        overlay = Gtk.Overlay()
        overlay.set_child(self._bg)
        overlay.add_overlay(scrim)
        overlay.add_overlay(content)
        return overlay

    @staticmethod
    def _transport(icon: str, dim: bool = False) -> Gtk.Button:
        b = Gtk.Button(icon_name=icon)
        b.add_css_class("transport")
        if dim:
            b.add_css_class("dim")
        b.set_focusable(False)
        return b

    # -- timers ---------------------------------------------------------------

    def _place(self) -> bool:
        """Center under the island once, then stop — the user may drag.

        Retries until hyprland registers the client (the surface exists
        in GTK slightly before the compositor lists it).
        """
        if self.get_surface() is None:
            return True
        x = island_centered_x()
        y = popup_y()
        if Hyprland.client_at(APP_ID) == [x, y]:
            return False
        if self._place_tries >= PLACE_TRIES:
            return False
        self._place_tries += 1
        Hyprland.move(APP_ID, x, y)
        return True

    def _close_when_offline(self) -> bool:
        if not self._ctl.murmur_up():
            self.close()
            return False
        return True

    def _tick_position(self) -> bool:
        props = self._mpris.properties()
        if props is not None:
            pos = props.get("Position") or 0
            if props.get("PlaybackStatus") == "Playing":
                pos += 1_000_000  # MPRIS doesn't advance Position by itself
            self._set_position(pos)
        return True

    # -- MPRIS → widgets -------------------------------------------------------

    def _refresh(self) -> bool:
        props = self._mpris.properties()
        if props is None:
            self._title.set_text("phone player offline")
            self._artist.set_text("")
            self._scale.set_sensitive(False)
            return True

        meta = props.get("Metadata") or {}
        status = props.get("PlaybackStatus", "Stopped")
        self._track_id = meta.get("mpris:trackid")
        self._duration_us = meta.get("mpris:length") or 0
        self._title.set_text(meta.get("xesam:title") or "—")
        artists = meta.get("xesam:artist") or []
        self._artist.set_text(artists[0] if artists else "")
        self._b_play.set_icon_name(
            "media-playback-pause-symbolic" if status == "Playing"
            else "media-playback-start-symbolic")
        loop = props.get("LoopStatus", "None")
        if props.get("Shuffle"):
            self._b_shuffle.remove_css_class("dim")
        else:
            self._b_shuffle.add_css_class("dim")
        self._b_loop.set_icon_name(
            "media-playlist-repeat-song-symbolic" if loop == "Track"
            else "media-playlist-repeat-symbolic")
        if loop != "None":
            self._b_loop.remove_css_class("dim")
        else:
            self._b_loop.add_css_class("dim")

        art_url = meta.get("mpris:artUrl") or ""
        art_path = art_url[7:] if art_url.startswith("file://") else None
        self._art.set_filename(art_path)
        if art_path != self._art_path:
            self._art_path = art_path
            cover = blurred_cover(art_path, 640, 240) if art_path else None
            self._bg.set_paintable(cover)

        self._set_position(props.get("Position") or 0)
        return True

    def _set_position(self, pos_us: int) -> None:
        self._position_us = pos_us
        self._pos.set_text(us_to_clock(pos_us))
        self._dur.set_text(us_to_clock(self._duration_us))
        if self._duration_us:
            self._scale.set_range(0, self._duration_us)
            self._scale.set_value(pos_us)
        else:
            self._scale.set_value(0)

    def _on_seek(self, _scale: Gtk.Scale, _type, value: float) -> bool:
        if self._track_id and self._duration_us:
            self._mpris.set_position(self._track_id, int(value))
        return False


# --- application wiring -----------------------------------------------------

class PopupController:
    """Owns the single popup window; Activate toggles it.

    Toggle-off destroys the window: hide→show doesn't reliably re-map a
    wayland surface, and a rebuild inside the warm process costs ~ms.
    """

    def __init__(self, app: Gtk.Application, ctl: Ctl, mpris: Mpris,
                 daemon_mode: bool) -> None:
        self._app = app
        self._ctl = ctl
        self._mpris = mpris
        self._win: PlayerWindow | None = None
        self._css_loaded = False
        # exec-once starts the app just to keep it warm: its startup
        # activate is noise, so the first one is swallowed.
        self._swallow_next = daemon_mode

    def toggle(self, *_args: object) -> None:
        if self._swallow_next:
            self._swallow_next = False
            return
        win = self._win
        if win is not None and win.get_visible():
            win.close()
            return
        self._win = None  # no window, or a dead reference — rebuild below
        self._spawn()

    def _spawn(self) -> None:
        if not self._css_loaded:
            provider = Gtk.CssProvider()
            provider.load_from_data(CSS)
            Gtk.StyleContext.add_provider_for_display(
                Gdk.Display.get_default(), provider,
                Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION)
            self._css_loaded = True
        win = PlayerWindow(self._app, self._mpris, self._ctl)
        win.connect("destroy", lambda *_: self._forget(win))
        self._win = win
        win.present()

    def _forget(self, win: PlayerWindow) -> None:
        if self._win is win:
            self._win = None


def main() -> int:
    app = Gtk.Application(application_id=APP_ID)
    daemon_mode = "--daemon" in sys.argv
    if daemon_mode:
        # No windows yet — gtk would quit right after activate without a hold.
        app.hold()
    popup = PopupController(app, Ctl(), Mpris(), daemon_mode)
    app.connect("activate", popup.toggle)
    return app.run([a for a in sys.argv if a != "--daemon"])


if __name__ == "__main__":
    sys.exit(main())
