# waybar-island

A reference waybar "island" for murmurd: a status pill plus a media
widget that opens a floating MPRIS popup — like the swaync player, but
under the bar.

```
murmur online ●  <track title> ∿
```

- status: `murmur online`/`offline` — daemon reachable + a family peer
  online; the dot breathes while up, solid red when down. Click →
  `murmurctl status` in a terminal.
- media: first 30 chars of the track title + a `∿` wave (the murmur
  logo) that shimmers while playing. Hidden when murmur is offline or
  the phone player is stopped/closed. Click → toggles the popup,
  right-click → play/pause.
- popup: artwork + blurred-cover background, transport buttons, seek
  bar — a plain `org.mpris.MediaPlayer2.murmur` client, so it stays in
  sync with swaync/playerctl.

## Files

| file | goes to |
|---|---|
| `murmur-island.py` | `~/.local/bin/` — waybar custom-module streams (`status-live`, `media-live`) |
| `murmur-player.py` | `~/.local/bin/` — GTK4 popup, GApplication daemon |
| `murmur-player-toggle.sh` | `~/.local/bin/` — one `gdbus` Activate call |
| `murmur.player.service` | `~/.local/share/dbus-1/services/` — D-Bus activation |
| `waybar.modules.jsonc` | merge into waybar `config.jsonc` |
| `waybar.style.css` | merge into waybar `style.css` |
| `hyprland.conf` | `exec-once` line — keeps the daemon warm |
| `hyprland.lua` | window rule — float, no border/shadow |

## Dependencies

`python3`, `gtk4` + `pygobject`, `playerctl`, `hyprctl`, a
`JetBrainsMono Nerd Font` for the `∿` glyph. For popup centering the
island prefers `grim` + `python-pillow` (it measures the real pill on
screen); without them it falls back to Pango text-metrics estimates.

The styling uses **Unbounded** (island text + player title) and
**Onest** (player body) — both variable OFL fonts from Google Fonts;
drop `Onest[wght].ttf` / `Unbounded[wght].ttf` into
`~/.local/share/fonts` and `fc-cache -f`.

## Notes

- `status-live`/`media-live` are streaming modules: the scripts keep
  running and waybar updates on every emitted line — that is what makes
  the dot/wave animation smooth.
- The media stream publishes the island's real on-screen center-x to
  `/tmp/murmur-island-cx` (measured from a screenshot, so it matches
  the rendered pill exactly); the player reads it and centers the
  popup underneath once per open. Set `MURMUR_ISLAND_LEFT` (px from
  screen edge to the island's left edge) to point the measurement at
  the right spot — the default fits the author's bar.
- Keys while the popup is focused: `Esc` close, `space` play/pause,
  `←`/`→` seek ±5s, `n`/`p` next/prev, `s` shuffle, `r` repeat —
  plus the standard media keys.
- `murmur-player.py --daemon` holds a headless GApplication
  (`app.hold()`); each Activate rebuilds or destroys the window.
- Popup closes itself when murmur goes offline and refuses to fight
  the user: it positions once, afterwards you can drag it anywhere.
- Text/wave colors come from `~/.config/hypr/colors.css`
  (`@color_bg/fg/accent`, re-read on change) — the island follows
  wallpaper-driven theme switches automatically.
