# murmur

**murmur** is a lightweight personal continuity layer between an Android phone and Wayland laptops — phone notifications on your desktops, your phone as a regular system media player, and one shared clipboard. No LAN, no VPN, no third-party coordinators: just a dumb relay on a server you own, with everything end-to-end encrypted.

murmur is the **"quietest"**, **"most private"**, and **"most modular"** way to make your devices feel like one machine.

![Rust](https://img.shields.io/badge/rust-1.85%2B-orange)
![Kotlin](https://img.shields.io/badge/kotlin-2.1-blue)
![Status](https://img.shields.io/badge/status-work%20in%20progress-yellow)
[![License](https://img.shields.io/badge/license-MIT-green)](LICENSE)

📖 **Design doc (RU):** [DESIGN.md](DESIGN.md)<br>
🌍 **Other languages:** [RU](README.ru.md)

---

```
  phone (Kotlin agent) ──WSS──▶ murmurs ◀──WSS── murmurd × N laptops
                                dumb relay                Arch / Hyprland
```

The phone is a thin background agent — it only reports events and obeys commands. All logic and all UI live on the laptops, in your own Wayland style: notifications land in your notification daemon, and the phone shows up as a regular MPRIS player — media keys, `playerctl` and bar widgets just work.

## ✨ Features

-   **Notification sync**: phone notifications appear on every live laptop; dismissing anywhere dismisses everywhere — including on the phone itself.
-   **Real-time media control**: what's playing on the phone is mirrored as a native MPRIS player — metadata, position, album art, play/pause/prev/next/seek.
-   **Shared clipboard**: laptop ↔ laptop and laptop → phone automatically; phone → laptop on your explicit action (share sheet / selection menu), delivered to the laptop you're actually sitting at.
-   **End-to-end encryption**: one family key, XChaCha20-Poly1305. The relay sees only ciphertext, device ids and timing.
-   **Revocable devices**: every device has its own auth token — a lost one is kicked out instantly and the family key rotates.
-   **Active-node election**: laptops agree on which one you're using via lock/idle state — no central coordinator, no clipboard echo storms.

## 📦 Packages

| Package | Stack | Role |
|---|---|---|
| `server/` | Rust | `murmurs` — the dumb authenticated relay. Live-only, zero stored content. |
| `laptop/` | Rust | `murmurd` daemon (feature-per-module) + `murmurctl` CLI. All configuration is terminal-only. |
| `mobile/` | Kotlin | Foreground agent: notification/media listeners, share & selection targets. |
| `shared/` | Rust | `murmur-proto` — envelope, crypto, message types shared by server and laptops. |

## 🚀 Quick Start

Everything installs from release binaries — no toolchains needed.

**1. Relay** (any Linux box you own, root):

```console
$ curl -fsSL https://raw.githubusercontent.com/koloideal/murmur/main/install-server.sh | sudo sh
==> bootstrap code: T45K-TP6D      # one-shot, armed while the family is empty
```

Point a TLS reverse proxy (caddy/nginx) at `127.0.0.1:8770` — laptops will use the `wss://` URL.

**2. First laptop** (your user, no root):

```console
$ curl -fsSL https://raw.githubusercontent.com/koloideal/murmur/main/install-client.sh | sh
$ murmurd pair --server wss://murmur.example.org --code T45K-TP6D
bootstrap pairing — verification code: 452690       # verify: murmurs bootstrap-code
$ systemctl --user start murmurd && murmurd doctor
```

The first device to use the bootstrap code becomes the founder and mints the family key locally — the relay never sees it.

**3. Phone:** grab `murmur-android.apk` from [Releases](https://github.com/koloideal/murmur/releases) (Obtainium-friendly). On a laptop run `murmurctl invite`, scan the QR — the invite carries the server URL itself.

**4. More laptops:** `murmurctl invite` on any member, then `murmurd pair --code …` on the new one.

Prefer building yourself? `cargo build --release` in `server/` or `laptop/`, `./gradlew assembleDebug` in `mobile/`.

## 📌 Design

Everything — the pairing flow, the envelope format, presence election, clipboard dedup rules — is specified in [DESIGN.md](DESIGN.md).

Adding a feature to `murmurd` is: one new file with a `feature()` constructor, one line in the registry, one `Message` variant in `murmur-proto`.

## 📄 Why?

KDE Connect needs a shared LAN. Apple Continuity needs Apple. Tailscale needs their coordinator. murmur needs none of that — only a server you already own, and it can't read a single byte of what passes through it.

---

MIT 2025-2026 kolo | made by [kolo](https://t.me/kolo_id)
