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
