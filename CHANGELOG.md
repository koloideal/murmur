# Changelog

All notable changes to murmur. Dates are tag dates; items are the
commits that shipped in each release.

## [Unreleased]

### Added
- `murmurd join` is the new name of `murmurd pair` (kept as a hidden
  alias) — joins the family; `murmurctl pair` stays the inviter-side
  accept/confirm, so the two commands no longer collide.
- Multi-hop LAN mesh: first-seen link frames flood onward to a node's
  other links, so members reach each other through a transit peer
  (phone ⇄ usb/wifi-laptop ⇄ second laptop). Sealed envelopes ride
  as-is; dedupe makes loops terminate.
- Pair-time peer seeds: the joiner persists the inviter's address as
  `lan_seeds` in config.toml and the daemon dials it at startup; the
  inviter remembers the joiner's pair-socket address and nudges the
  dialer through its boot window. A fresh join links even where
  discovery broadcast can never pass (host firewalls, AP isolation).
- `murmurd join --code CODE --server HOST[:PORT]` — direct dial to the
  inviter's pair listener, bypassing LAN broadcast entirely (AP
  isolation, filtered subnets). `tcp://` prefix and bare host accepted.
- `murmurctl invite` prints the direct-dial fallback for serverless.
- Beacon wait heartbeats every 15 s with the fallback hint instead of
  a silent 2-minute hang; connect to the inviter has a 10 s timeout.
- Transport failover candidate sets: each peer remembers up to four
  dial addresses — the validated link's real remote addr sits at the
  front, beacon/pair hints queue behind as alternates
  (`lan_addrs.json` list form; the old flat `{id: "addr"}` still
  loads). Losing a link emits dial hints for the whole set at once,
  connects race in parallel in their own tasks, and the rate limit
  moved to per (peer, addr) — a dead transport can't pace the live
  one. Verified live: killing `adb`'s transport dropped the phone
  link instantly and the restored forward re-linked 5 s later with
  no user action.
- `FLOWS.md` at the repo root pins every user flow — pairing,
  transport failover, transit, multi-laptop-one-phone, revocation —
  for serverless and serverful families, with the live-run matrix.

### Fixed
- Android: transit-reachable members no longer show as offline in
  the device list — `lastHeard` marks any valid frame within 30 s
  (the phone's flooded pings), same rule the daemon's `devices`
  table already used.
- Second `murmurctl invite` inside the invite TTL died with
  "pair port busy": the serverless pair listener is now bound once at
  daemon start and lives permanently.
- `murmurd pair` couldn't listen for the pair beacon while the daemon
  was running — the daemon's beacon socket lacked SO_REUSEADDR, so the
  second bind failed with EADDRINUSE.
- Unconfigured installs crash-looped: the user unit now carries
  `ConditionPathExists` on config.toml and skips quietly until paired.
- install-client: skips the join flow entirely when config.toml
  already exists (provision would refuse the overwrite anyway), the
  relay question accepts the inviter's ip[:port], and declining the
  firewall offer now names what breaks (discovery, phone links,
  serverless pairing).

## [0.5.0] — 2026-11-27

### Added
- Wire caps everywhere: 4 MiB sealed payload, 6 MiB JSON envelope —
  enforced at `Envelope::seal`, `from_json`, WS configs on relay and
  laptop, and LAN link readers. Oversized data drops with a log, never
  truncated.
- Protocol-mismatch UX: `Hello` carries `proto_v`, the relay rejects
  skewed clients at connect, mismatched frames warn and surface as
  `proto_mismatch` in `murmurctl status`.
- `wlr-data-control-v1` fallback — clipboard works on older wlroots and
  Plasma that predate `ext-data-control-v1`.
- `murmurd doctor` now probes the session for data-control and
  idle-notify globals and names what's missing.
- Android: received `image/png` clips land as FileProvider-backed
  content URIs — pasting actually works.
- Android: shares made while the service is down are queued and
  delivered on first connectivity instead of being lost.
- Tests: proptest coverage for envelope parsing (garbage/mutation/cap/
  roundtrip) and v0.3-era config+secrets migration fixtures.
- README "Scope & limits": compositor matrix, IPv4-only discovery,
  live-only semantics with explicit payload caps.
- `murmurctl uninstall` — removes every artifact the installers can
  leave: daemon unit, island bits, pairing secrets, completions, PATH
  markers, binaries. `--yes` skips the confirmation.
- Crate versions now track the release (0.5.0 via workspace package).

### Fixed
- install-client: `have_tty` actually opened /dev/tty instead of checking
  permission bits — prompts silently ate defaults (no join, no island)
  on terminals without a controlling tty.
- install-client: a failed murmurt download no longer kills the install
  before the pairing prompts (and `dl` no longer masks curl's status).
- install-client: join prompt defaults to yes; `--serverless` recognized
  anywhere in args; names with spaces survive; PATH lands in fish and
  .bash_profile; murmurt gets completions; zsh gets compinit.
- install-server: the summary endpoint prints `wss://` only when a proxy
  was actually configured; nginx works on conf.d layouts; caddy reload
  failures surface; bootstrap code comes from `murmurs bootstrap-code`
  (journald scrape kept as fallback).

## [0.4.0] — 2026-11-25
- murmurt: self-contained Rust installer for the waybar island
- island-hover: find the media label by content, not tree position
- cli: consistent styled help, shell completions, murmurd run verb
- murmurt: positional jsonc merging, validation gate, css guard
- polish: ascii-art binary tags, dial reachability probe, doctor fix-hints
- murmurctl dial: interactive probe with verified link outcome
- horizontal swipe seek + track switching on the island media widget
- throttled swipe dispatcher; clipboard widget auto-hides after 10s
- usb-tethering fast path: auto-dial the tethering gateway
- feature framework v2: tag dispatch, Arc inbox, self-status, capabilities
- test infrastructure: golden wire vectors, election/dedup, anti-echo
  matrix, in-memory full-duplex integration, key rotation e2e
- typed ctl errors + scriptable exit codes; state.rs split into stores
- android: feature hub ports the murmurd pattern to Kotlin
- ci: tests gate the release on tag push
- pairing crypto + registry security invariants under test
- readme rewrite around the technical strengths

## [0.3.11] — 2026-11-24
- pair: LAN pair beacon — short codes resolve in a serverless family
- mobile: grant gate → split screen → pairing flow, copyable invite

## [0.3.10] — 2026-11-24
- mobile: family-kind split screen; staged pair socket sweep

## [0.3.9] — 2026-11-24
- serverless families: installer path, direct-TCP invite + LAN roster
  gossip, explicit `serverful|serverless` kind carried end-to-end
- lan: priority channel for keepalives (honest RTT), instant redial,
  multi-peer links, remembered-peer redial, `murmurctl dial`
- clipboard: laptop→phone sync and serverless push fixes
- notify: murmur tag on mirrored popups, desktop-twin suppression,
  sender avatar, urgency override rules
- contrib: swaync popup theme + murmur tag artwork
- mobile: rename propagation via Identity gossip, own row in family card
- audit fixes: stale-key recovery on LAN, presence aging, family_name
- pair: fixed port 17895; installer opens the firewall

## [0.3.8] — 2026-11-23
- lan: single-writer queue on the phone — link flaps on media events

## [0.3.7] — 2026-11-23
- lan: transport ping/pong, real LAN RTT, drop diagnostics

## [0.3.6] — 2026-11-23
- lan: adopt dialed links immediately; on-lan/on-wan indicators

## [0.3.5] — 2026-11-23
- lan: relay-borne beacon discovery — works behind client-isolating APs

## [0.3.4] — 2026-11-23
- lan: directed-broadcast beacons survive VPNs and odd Android routing

## [0.3.3] — 2026-11-23
- lan reliability: dual-path frames, mobile dedup, resilient sockets
- shuffle/repeat MPRIS control end-to-end + LAN fast-path, hi/lo
  outbound coalescing
- contrib: waybar island reference integration (scripts, configs, README)

## [0.3.2] — 2026-11-23 / [0.3.1], [0.3.0] — 2026-11-22
- rename murmur-server → murmurs; installer-style UI everywhere
- bootstrap PairBlob carries family_name; android UX fixes

## [0.2.x] — 2026-11-22
- security: automatic family-key rotation on revoke, per-member sealed
- pairing: relay answers unknown/expired PairHello with pair_reject
- android: CameraX+MLKit QR scanner, rotation survival, signed CI APK
- server cli: framed UI, status layout, dup-name warning, bind hints

## [0.1.x] — 2026-11-22
- initial release: relay (murmurs), laptop daemon (murmurd), Android
  agent; pairing via DH+SAS; presence election; notifications and
  clipboard sync; interactive installers for server and clients
