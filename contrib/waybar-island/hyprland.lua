-- The popup floats above the bar; no compositor border or shadow —
-- the window draws its own rounded styling, and a compositor border
-- would flash for a frame while the window is moved into place.
hl.window_rule({
    name  = "murmur-player-popup",
    match = { class = "^(murmur%.player)$" },
    float = true,
    border_size = 0,
    no_shadow = true,
})


-- Hotkeys while hovering the island's track label: murmur-island-hover.py
-- enters this submap when the cursor sits over the label and pops back
-- out when it leaves. Unbound keys still reach the focused window.
hl.define_submap("murmur", function()
    local pc = function(args) return hl.dsp.exec_cmd("playerctl -p murmur " .. args) end
    hl.bind("space", pc("play-pause"))
    hl.bind("right", pc("position 5+"), { repeating = true })
    hl.bind("left",  pc("position 5-"), { repeating = true })
    hl.bind("n",     pc("next"))
    hl.bind("p",     pc("previous"))
    hl.bind("s",     pc("shuffle toggle"))
    hl.bind("r", hl.dsp.exec_cmd(
        "sh -c 'c=$(playerctl -p murmur loop); case $c in None) n=Playlist;; Playlist) n=Track;; *) n=None;; esac; playerctl -p murmur loop $n'"))
    hl.bind("escape", hl.dsp.submap("reset"))
end)

-- Autostart (inside hl.on("hyprland.start", ...) or however you autostart):
--   hl.exec_cmd(home .. "/.local/bin/murmur-island-hover.py")
