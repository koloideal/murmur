# murmur

A personal device-continuity layer between an Android phone and Wayland
laptops: notification sync with bidirectional dismissal, the phone as a
native MPRIS player, one shared clipboard — end-to-end encrypted over a
zero-knowledge relay you host yourself, or over a pure LAN family with
no server at all.

![Rust](https://img.shields.io/badge/rust-1.85%2B-orange)
![Kotlin](https://img.shields.io/badge/kotlin-2.1-blue)
[![License](https://img.shields.io/badge/license-MIT-green)](LICENSE)

📖 **Design doc (RU):** [DESIGN.md](DESIGN.md) · 🌍 **README:** [RU](README.ru.md)

---

## Architecture

```
                    serverful topology
 phone ──WSS──▶ murmurs (relay) ◀──WSS── murmurd × N laptops
                 │
                 └─ sees: device ids, frame sizes, timing
                 └─ never sees: plaintext, family key, content of any kind

                    serverless topology
 phone ◀──UDP beacons / direct TCP──▶ laptops      (nothing leaves the subnet)
```

Both topologies carry the **same sealed envelope format**. In a serverful
family the LAN additionally acts as a fast path: a direct TCP copy of each
envelope races the relay copy and typically wins by an order of magnitude
in latency; the loser is dropped by sequence-number dedup, so the redundant
transports are purely additive — a dead LAN link can never eat a message.

## Threat model

The relay is assumed hostile.

- **Family key** `K` (32 bytes) is minted locally by the first device and
  distributed only inside the pairing handshake. The relay never sees it.
- Every frame payload is `XChaCha20-Poly1305(K, random 192-bit nonce)`.
  What reaches the relay is ciphertext, a device id, a size, a timestamp.
- **Pairing** uses ephemeral X25519; the invitation is verified by a
  symmetric 6-digit SAS derived from both public keys and the session
  transcript — a relay performing a MITM key substitution produces
  different codes on the two screens.
- **Auth tokens** are per-device 32-byte secrets; the relay stores only
  `SHA-256(token)`. Revocation flips one bit in the registry, kills the
  live connection immediately, and the protocol carries `KeyRotate` for
  family-key rotation.
- The relay is **live-only by design**: no queues, no history, no stored
  content. There is nothing on the server to exfiltrate.

## Correctness under adverse networks

The interesting part of this project is not the features but the machinery
that keeps them consistent across flapping Wi-Fi, DHCP renumbering,
phone hotspots and devices roaming between subnets:

- **Coordinator-free active-node election.** Each laptop derives its
  presence state (`active`/`idle`/`locked` from `ext-idle-notify-v1` +
  logind `LockedHint`) and gossips it. Every node computes the same
  deterministic function over the same presence map — the active node is
  the one in state `active` with the freshest transition timestamp, ties
  broken by device id. No leader election protocol, no coordinator, and
  clipboard echo storms are structurally impossible.
- **Clipboard anti-echo.** Each node tracks the hash of the last clip it
  sent or applied; applying a remote clip arms `expected_hash` so the
  local watcher doesn't rebroadcast it. Password-manager clips are
  suppressed via the `x-kde-passwordManagerHint` MIME marker before they
  ever leave the machine.
- **Sequence dedup.** Per-sender monotone sequence numbers (seeded from
  wall clock at startup, so a restarted daemon never replays) — relays,
  reconnects and redundant transports may duplicate, never reorder or
  resurrect.
- **Self-authenticating discovery.** LAN beacons are themselves sealed
  envelopes: only a family member can produce a frame that opens, so the
  beacon needs no handshake and a rogue broadcaster gets nothing but a
  dropped packet. Laptops dial phone beacons and vice versa — at most one
  link per pair, no connection storms.
- **Supervision everywhere.** On Android every long-lived actor loop
  (accept, dial, beacon, relay) runs under a supervisor that restarts it
  after failure; laptops redial remembered peer addresses and gossip
  `DialHint`s so links re-establish across DHCP changes. Reconnects use
  exponential backoff with jitter; Android additionally retries
  immediately on `ConnectivityManager` network-change callbacks.

## Desktop integration — native, not emulated

The laptop side is written against real Wayland/freedesktop primitives,
so the phone stops being a special case anywhere in the stack:

- **MPRIS2** (`org.mpris.MediaPlayer2.murmur` over zbus): the phone is a
  regular system media player — media keys, `playerctl`, waybar/swaync
  widgets work unmodified. Position is extrapolated locally between
  updates with `Seeked` signals on correction; album art is served as
  content-hashed `file://` URLs.
- **Clipboard** via `ext-data-control-v1` — real selection events, not
  polling; `text/plain`, `text/uri-list`, `image/png`.
- **Notifications** through `org.freedesktop.Notifications`: dismissal
  is mapped bidirectionally (closing a notification anywhere — including
  on the phone — closes it everywhere, with anti-loop guards).
- **Waybar island** (hyprland): sync status, clipboard direction and a
  media widget with a floating player popup. The popup positions itself
  by reading exact widget geometry from waybar's **AT-SPI accessibility
  tree** — no screenshot scraping, no hardcoded offsets. Hovering the
  track title puts the compositor in a key submap, so media hotkeys work
  without opening anything.

## `murmurt` — relocatable setup as a binary

The island ships inside `murmurt`, a single self-contained Rust binary
(assets embedded at compile time — scripts, OFL fonts, config snippets):

```
murmurt install     # writes scripts/fonts/dbus service, marker-merges
                    # waybar jsonc/css + hyprland configs (idempotent,
                    # backup-once, never clobbers a manual setup)
murmurt doctor      # dependency audit with per-distro install hints
murmurt geometry    # AT-SPI island geometry probe used by the popup
murmurt uninstall   # strips marker blocks, removes files, done
```

## Engineering layout

| Package | Stack | Role |
|---|---|---|
| `server/` | Rust | `murmurs` — dumb authenticated relay. Live-only, zero stored content. Static musl binary. |
| `laptop/` | Rust workspace | `murmurd` daemon + `murmurctl` CLI + `murmurt` installer. |
| `mobile/` | Kotlin | Android foreground agent: notification/media listeners, share & process-text targets. |
| `shared/` | Rust | `murmur-proto` — envelope, crypto, message types shared by relay and laptops. |

`murmurd` is strictly feature-per-module: each feature is one file with a
`feature()` constructor (an incoming-message filter plus a `run(ctx)`),
one line in the registry. Adding a capability to the system is: one new
`Message` variant in `murmur-proto`, one new file, one line. Admin control
of every daemon is newline-JSON over a unix socket; config files are
persistence, not an interface — the daemon is the single writer.

## 🚀 Quick Start

Everything installs from release binaries — no toolchains needed.

**1. Relay** (any Linux box you own, root):

```console
$ curl -fsSL https://raw.githubusercontent.com/koloideal/murmur/main/install-server.sh | sudo sh
==> bootstrap code: T45K-TP6D      # one-shot, armed while the family is empty
```

Point a TLS reverse proxy (caddy/nginx) at `127.0.0.1:8770` — laptops will
use the `wss://` URL.

**2. First laptop** (your user, no root):

```console
$ curl -fsSL https://raw.githubusercontent.com/koloideal/murmur/main/install-client.sh | sh
$ murmurd pair --server wss://murmur.example.org --code T45K-TP6D
bootstrap pairing — verification code: 452690       # verify: murmurs bootstrap-code
$ systemctl --user start murmurd && murmurd doctor
```

The first device to use the bootstrap code becomes the founder and mints
the family key locally.

**3. Phone:** grab `murmur-android.apk` from
[Releases](https://github.com/koloideal/murmur/releases)
(Obtainium-friendly). Run `murmurctl invite` on a laptop, scan the QR —
the invite carries the server URL itself.

**4. More laptops:** `murmurctl invite` on any member, then
`murmurd pair --code …` on the new one.

**Serverless instead?** Skip the relay entirely — on the first laptop:
`murmurd init --kind serverless --name my-laptop`, then `murmurctl invite`
and scan with the phone. LAN-only, nothing ever leaves your network.

Prefer building yourself? `cargo build --release` in `server/` or
`laptop/`, `./gradlew assembleDebug` in `mobile/`.

## 📌 Design

The full protocol — pairing transcript, envelope format, presence
election, clipboard rules, deliberate v1 simplifications — is specified
in [DESIGN.md](DESIGN.md) (RU).

## 📄 Why not KDE Connect / Apple Continuity / Tailscale

KDE Connect needs a shared LAN. Continuity needs Apple hardware end to
end. Tailscale needs their coordinator. murmur needs none of that — a
server you already own, which cannot read a single byte of what passes
through it; or, for a LAN family, not even that.

---

MIT 2025-2026 | made by [kolo](https://t.me/kolo_id)
