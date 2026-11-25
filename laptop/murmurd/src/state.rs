use crate::config::{Config, Secrets};
use crate::keyring::{Keyring, Roster};
use murmur_proto::{
    Control, DeviceId, Envelope, Message, PairKey, PresenceState, PublicKey, Route,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};
use tokio::sync::{broadcast, mpsc};

pub fn now_ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Pairing state owned by the inviter side. Sessions are the joiner's
/// conn ids: a `PairHello`'s `env.from` is matched against `invites`.
/// Entries expire — an invite nobody finishes within TTL is swept.
#[derive(Default)]
pub struct Pairing {
    /// Outstanding invites: session → our ephemeral DH key.
    pub invites: HashMap<DeviceId, InviteEntry>,
    /// `PairHello` received for one of our invites.
    pub pending: HashMap<DeviceId, Pending>,
    /// Serverless pair sockets staged by session id: `serve_pair`
    /// registers the joiner's socket writer here *before* the
    /// PairHello reaches `pair::handle_ctl`, which moves it into the
    /// fresh `Pending`.
    pub pair_reply: HashMap<DeviceId, mpsc::UnboundedSender<Envelope>>,
}

/// How long an invite/pending stays around without progress.
pub const PAIR_TTL: std::time::Duration = std::time::Duration::from_secs(600);

/// Presence entries older than this stop counting for the active-node
/// election — 2.5× the 60s heartbeat, so one missed beat still counts.
pub const PRESENCE_STALE: u64 = 150;

pub struct InviteEntry {
    pub key: PairKey,
    pub created: std::time::Instant,
}

pub struct Pending {
    /// Our ephemeral key (kept for DH at confirm time).
    pub key: PairKey,
    /// Joiner's pubkey from its `PairHello`.
    pub peer_pub: PublicKey,
    /// Name the joiner asked to be registered under.
    pub name: String,
    /// When the invite was issued (TTL counts from then, not from hello).
    pub created: std::time::Instant,
    /// Serverless pairing: the joiner's direct socket writer — its
    /// PairAccept/PairData ride this instead of the (absent) relay.
    pub reply: Option<mpsc::UnboundedSender<Envelope>>,
}

/// Shared daemon state: identity, key, presence table, outbound queue.
pub struct Shared {
    pub cfg: Config,
    pub id: DeviceId,
    /// Current family key + epoch + a few superseded ones.
    pub keys: RwLock<Keyring>,
    /// Family name from the latest `Devices` push — for status/blobs.
    pub family_name: Mutex<Option<String>>,
    /// Long-term identity key — rotations are sealed to it.
    pub identity: PairKey,
    /// Pinned identities of the other members.
    pub roster: Mutex<Roster>,
    /// secrets.toml as loaded — rewritten when the key rotates.
    pub secrets: Mutex<Secrets>,
    pub last_rekey: Mutex<HashMap<DeviceId, std::time::Instant>>,
    pub rotating: Mutex<()>,
    token_hex: String,
    seq: AtomicU64,
    /// Latency-critical envelopes — commands, control, pairing.
    pub out: mpsc::UnboundedSender<Envelope>,
    /// Replaceable state (`Presence`/`MediaState`/`Clipboard`) tagged
    /// with a slot key — conn keeps only the latest per slot, so a
    /// backlog of stale snapshots drains as one fresh frame.
    pub out_lo: mpsc::UnboundedSender<(u64, Envelope)>,
    /// Live LAN links keyed by peer — a star/mesh fast-path: several
    /// devices can each hold a direct link, unicast picks its peer,
    /// broadcast gets an early copy on every link.
    lan: Mutex<HashMap<DeviceId, mpsc::UnboundedSender<Envelope>>>,
    /// This node's last measured round-trip per peer in ms
    /// (0 = unknown); each peer measures its own on its side.
    lan_rtt: Mutex<HashMap<DeviceId, u64>>,
    /// Peers whose link just died — one-shot redial markers for the
    /// dialer (bounded by peer count, entries expire after a minute).
    lan_dropped: Mutex<HashMap<DeviceId, std::time::Instant>>,
    /// Last-proven dial address per peer — learned when a link
    /// validates (socket remote addr / the peer's advertised
    /// `lan_addr`). On broadcast-filtered networks no fresh beacon
    /// ever arrives, so `lan::redial_loop` uses this map to bring a
    /// dead link back without discovery.
    lan_addrs: Mutex<HashMap<DeviceId, std::net::SocketAddr>>,
    /// Dial requests for the LAN fast-path — fed by UDP beacons and
    /// relay-delivered `LanBeacon`s; `lan::dial_loop` rate-limits,
    /// subnet-checks and connects.
    pub lan_dial: mpsc::UnboundedSender<crate::lan::DialHint>,
    /// Inbound dedup bookkeeping — broadcasts intentionally ride both
    /// LAN and relay, so the second copy must collapse here instead of
    /// duplicating notifications/clips at the features.
    seen: Mutex<Seen>,
    /// Cleartext control frames addressed to us: (sender, frame).
    /// Server replies (Devices, Error) and inbound pair frames.
    pub ctl_events: broadcast::Sender<(DeviceId, Control)>,
    /// device → (last state, ts of last transition). Includes self.
    /// Shared because the active-node election is cross-cutting: clipboard
    /// routing and phone-side pushes both need it.
    pub presence: Mutex<HashMap<DeviceId, (PresenceState, u64)>>,
    /// Inviter-side pairing sessions. Cross-cutting: conn feeds PairHello,
    /// ctl drives invite/accept/confirm.
    pub pair: Mutex<Pairing>,
    /// True between HelloOk and the socket's death — for `murmurctl status`.
    pub online: AtomicBool,
    /// The relay said this device is revoked / unknown.
    pub revoked: AtomicBool,
}

impl Shared {
    pub fn new(
        cfg: Config,
        mut secrets: Secrets,
        out: mpsc::UnboundedSender<Envelope>,
        out_lo: mpsc::UnboundedSender<(u64, Envelope)>,
        lan_dial: mpsc::UnboundedSender<crate::lan::DialHint>,
    ) -> anyhow::Result<Self> {
        let (ctl_events, _) = broadcast::channel(64);
        let identity = crate::keyring::load_identity(&mut secrets)?;
        Ok(Self {
            id: secrets.device_id()?,
            keys: RwLock::new(Keyring::load(&secrets)?),
            family_name: Mutex::new(secrets.family_name.clone()),
            identity,
            roster: Mutex::new(crate::keyring::load_roster()),
            last_rekey: Mutex::new(HashMap::new()),
            rotating: Mutex::new(()),
            token_hex: secrets.auth_token_hex.clone(),
            secrets: Mutex::new(secrets),
            cfg,
            // Seeded from the clock: a restart must not reuse numbers a
            // peer's dedupe window still holds, or our first frames
            // after the restart are dropped as duplicates.
            seq: AtomicU64::new(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(1),
            ),
            out,
            out_lo,
            lan: Mutex::new(HashMap::new()),
            lan_rtt: Mutex::new(HashMap::new()),
            lan_dropped: Mutex::new(HashMap::new()),
            lan_addrs: Mutex::new(load_lan_addrs()),
            lan_dial,
            seen: Mutex::new(Seen::default()),
            ctl_events,
            presence: Mutex::new(HashMap::new()),
            pair: Mutex::new(Pairing::default()),
            online: AtomicBool::new(false),
            revoked: AtomicBool::new(false),
        })
    }

    pub fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::Relaxed)
    }

    /// Seal and enqueue a message for the given route. A live LAN link
    /// gets an *early copy* of everything that can reach its peer —
    /// unicast to it, or a broadcast it is a member of. The frame still
    /// rides the relay too: LAN is a latency fast-path, not a hard
    /// dependency (a silently dead link must not eat the command).
    /// `dedupe` on the receiving side collapses the second copy.
    /// Outbound: commands via the hi-priority channel, replaceable
    /// state via the coalesced lo channel.
    pub fn send(&self, to: Route, msg: &Message) -> anyhow::Result<()> {
        let env = Envelope::seal(
            self.id,
            to.clone(),
            self.next_seq(),
            &self.keys.read().unwrap().current,
            msg,
        )?;
        {
            let lan = self.lan.lock().unwrap();
            match &to {
                // Broadcast rides every live link — each peer gets its
                // early copy; the relay copy follows as always.
                Route::All => {
                    for tx in lan.values() {
                        let _ = tx.send(env.clone());
                    }
                }
                Route::Device(d) => {
                    if let Some(tx) = lan.get(d) {
                        let _ = tx.send(env.clone());
                    }
                }
                _ => {}
            }
        }
        if self.has_relay() {
            match replaceable_slot(&to, msg) {
                Some(slot) => {
                    let _ = self.out_lo.send((slot, env));
                }
                None => {
                    let _ = self.out.send(env);
                }
            }
        }
        Ok(())
    }

    /// Serverless families have no relay consumer — outbound queues
    /// would grow forever.
    pub(crate) fn has_relay(&self) -> bool {
        self.cfg.family_kind == murmur_proto::FamilyKind::Serverful
    }

    /// Link bookkeeping for [`crate::lan`]. Called on every valid
    /// frame — a no-op while the map already points at this channel,
    /// so it never tramples the link's measured RTT.
    pub fn lan_adopt(&self, peer: DeviceId, tx: mpsc::UnboundedSender<Envelope>) {
        {
            let mut lan = self.lan.lock().unwrap();
            if matches!(lan.get(&peer), Some(t) if t.same_channel(&tx)) {
                return;
            }
            lan.insert(peer, tx);
            self.lan_rtt.lock().unwrap().insert(peer, 0);
        }
        // The link-up is a rendezvous: announce so the peer pins our
        // identity + name, and re-announce presence — the heartbeat's
        // 60s window would otherwise leave the peer rejecting clips
        // from an "inactive" origin. In a serverless family it's the
        // *only* rendezvous — there is no relay connect to hang this on.
        self.announce_identity();
        self.broadcast_presence();
    }
    /// Push a pre-built envelope down a live link — for frames that
    /// must ride a specific key (a re-key sealed under the peer's old
    /// key), not the current-key seal `send` always applies.
    pub(crate) fn lan_send(&self, peer: &DeviceId, env: Envelope) {
        if let Some(tx) = self.lan.lock().unwrap().get(peer) {
            let _ = tx.send(env);
        }
    }

    /// Live peers — the ctl/island "on lan" truth.
    pub fn lan_peers(&self) -> Vec<DeviceId> {
        self.lan.lock().unwrap().keys().copied().collect()
    }
    pub fn lan_rtt(&self, peer: &DeviceId) -> u64 {
        self.lan_rtt.lock().unwrap().get(peer).copied().unwrap_or(0)
    }
    pub fn lan_rtt_set(&self, peer: DeviceId, ms: u64) {
        self.lan_rtt.lock().unwrap().insert(peer, ms);
    }
    /// Tear down whichever peer slot still points at `tx` — a dying
    /// old link must not clear a newer one adopted under the same peer.
    /// A genuinely lost peer (slot removed, not merely replaced) gets
    /// a redial marker so the next beacon reconnects without waiting
    /// out the rate limit.
    pub fn lan_clear(&self, tx: &mpsc::UnboundedSender<Envelope>) {
        let mut g = self.lan.lock().unwrap();
        let removed: Vec<DeviceId> = g
            .iter()
            .filter(|(_, t)| t.same_channel(tx))
            .map(|(p, _)| *p)
            .collect();
        g.retain(|_, t| !t.same_channel(tx));
        let remaining: std::collections::HashSet<DeviceId> = g.keys().copied().collect();
        drop(g);
        self.lan_rtt
            .lock()
            .unwrap()
            .retain(|p, _| remaining.contains(p));
        if !removed.is_empty() {
            let now = std::time::Instant::now();
            let mut d = self.lan_dropped.lock().unwrap();
            for p in removed {
                d.insert(p, now);
            }
            d.retain(|_, t| now.duration_since(*t) < std::time::Duration::from_secs(60));
        }
    }

    /// Consume the "just lost this peer" marker — the dialer skips the
    /// rate limit once so a broken fast path comes back on the very
    /// next beacon instead of up to DIAL_EVERY + beacon cadence later.
    pub fn lan_redial_pending(&self, peer: &DeviceId) -> bool {
        self.lan_dropped.lock().unwrap().remove(peer).is_some()
    }

    /// Remember where this peer was last reachable — refreshed on
    /// every validated link and beacon-advertised `lan_addr`.
    /// Persisted: a daemon restart on a broadcast-filtered network
    /// would otherwise lose the only rendezvous it has.
    pub fn lan_addr_set(&self, peer: DeviceId, addr: std::net::SocketAddr) {
        {
            let mut m = self.lan_addrs.lock().unwrap();
            if m.get(&peer) == Some(&addr) {
                return;
            }
            m.insert(peer, addr);
        }
        save_lan_addrs(&self.lan_addrs.lock().unwrap());
    }

    /// Snapshot for the redial loop and `murmurctl status`.
    pub fn lan_addrs(&self) -> HashMap<DeviceId, std::net::SocketAddr> {
        self.lan_addrs.lock().unwrap().clone()
    }

    /// True on first sight of this envelope — call only *after* a
    /// successful `open()` so forged LAN frames can't poison the table.
    ///
    /// `seq` restarts at 1 on every sender restart, so "seen before"
    /// can't be decided by the window alone: a frame landing far below
    /// the sender's high-water mark is a fresh post-restart frame, and
    /// the stored history would swallow it. Gap detection resets the
    /// sender's window; stragglers just above/below the mark still
    /// dedupe correctly.
    pub fn dedupe(&self, from: DeviceId, seq: u64) -> bool {
        /// This far below the high-water mark can't be an in-flight
        /// duplicate — the LAN early copy is never that late.
        const RESTART_GAP: u64 = 64;
        let mut g = self.seen.lock().unwrap();
        let last = g.last.get(&from).copied().unwrap_or(0);
        let last = if seq <= last && last - seq > RESTART_GAP {
            g.set.retain(|(f, _)| *f != from);
            g.order.retain(|(f, _)| *f != from);
            seq
        } else {
            last.max(seq)
        };
        g.last.insert(from, last);
        if !g.set.insert((from, seq)) {
            return false;
        }
        g.order.push_back((from, seq));
        if g.order.len() > 4096 {
            if let Some(old) = g.order.pop_front() {
                g.set.remove(&old);
            }
        }
        true
    }

    /// Cleartext control message to the server.
    pub fn send_control(&self, ctl: &Control) -> anyhow::Result<()> {
        self.send_control_to(Route::Server, ctl)
    }

    /// Cleartext control message to any route — pairing frames go unicast
    /// to a session conn (`Route::Device(session)`).
    pub fn send_control_to(&self, to: Route, ctl: &Control) -> anyhow::Result<()> {
        // Serverless: no relay exists to carry control frames — a
        // no-op, not an error. Pair frames ride their own socket via
        // `pair_reply` and never come through here.
        if !self.has_relay() {
            return Ok(());
        }
        let env = Envelope::control(self.id, to, self.next_seq(), ctl)?;
        let _ = self.out.send(env);
        Ok(())
    }

    /// Active-node election: newest `active` wins; ties → smaller DeviceId.
    /// Deterministic across nodes given the same presence table.
    /// Entries older than PRESENCE_STALE are ignored: delivery is
    /// live-only, so a peer silent that long is dead — without the
    /// cutoff a crashed "active" laptop wins the election forever and
    /// explicit pushes/commands route to a ghost.
    pub fn active_node(&self) -> Option<DeviceId> {
        let cutoff = now_ts().saturating_sub(PRESENCE_STALE);
        self.presence
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, (s, ts))| *s == PresenceState::Active && *ts >= cutoff)
            .max_by(|(a_id, (_, a_ts)), (b_id, (_, b_ts))| {
                a_ts.cmp(b_ts).then(b_id.cmp(a_id)) // later ts wins; smaller id wins ties
            })
            .map(|(id, _)| *id)
    }

    pub fn is_active(&self) -> bool {
        self.active_node() == Some(self.id)
    }

    /// The mirror path is alive: a direct LAN link, or another device
    /// whose presence broadcast is still fresh. Used by the notify
    /// feature to hide desktop twins' own popups.
    pub fn peer_online(&self) -> bool {
        if !self.lan_peers().is_empty() {
            return true;
        }
        let cutoff = now_ts().saturating_sub(PRESENCE_STALE);
        self.presence
            .lock()
            .unwrap()
            .iter()
            .any(|(id, (_, ts))| *id != self.id && *ts >= cutoff)
    }

    pub fn token_hex(&self) -> String {
        self.token_hex.clone()
    }

    /// Update own state and remember it for the next broadcast.
    pub fn set_local_state(&self, state: PresenceState) {
        self.presence
            .lock()
            .unwrap()
            .insert(self.id, (state, now_ts()));
        self.broadcast_presence();
    }

    /// Re-announce current presence state to everyone. The wire ts is
    /// fresh; refresh our own table row too or it ages past the stale
    /// cutoff and our election drifts from what the peers compute.
    pub fn broadcast_presence(&self) {
        let ts = now_ts();
        let state = {
            let mut m = self.presence.lock().unwrap();
            let s = m
                .get(&self.id)
                .map(|(s, _)| *s)
                .unwrap_or(PresenceState::Active);
            m.insert(self.id, (s, ts));
            s
        };
        let _ = self.send(Route::All, &Message::Presence { state, ts });
        tracing::debug!("presence → broadcast {state:?} ts={ts}");
    }

    /// Called on disconnect. Peer rows are kept — a LAN peer is still
    /// legitimately alive while the relay is gone. `active_node`'s
    /// stale cutoff ages out whatever stops broadcasting.
    pub fn note_presence_offline(&self) {}

    pub fn note_presence(&self, id: DeviceId, state: PresenceState, ts: u64) {
        let mut m = self.presence.lock().unwrap();
        if m.get(&id).is_none_or(|(_, old)| ts > *old) {
            m.insert(id, (state, ts));
        }
    }
}

/// `lan_addrs.json` — remembered peer dial addresses, keyed by hex id.
/// Both directions stay stringly: DeviceId/SocketAddr parse cleanly
/// and a hand-editable file beats a schema for two fields.
fn load_lan_addrs() -> HashMap<DeviceId, std::net::SocketAddr> {
    std::fs::read_to_string(crate::config::data_path("lan_addrs.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<HashMap<String, String>>(&s).ok())
        .map(|m| {
            m.iter()
                .filter_map(|(id, a)| {
                    Some((id.parse::<DeviceId>().ok()?, a.parse::<std::net::SocketAddr>().ok()?))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn save_lan_addrs(m: &HashMap<DeviceId, std::net::SocketAddr>) {
    let flat: HashMap<String, String> = m
        .iter()
        .map(|(id, a)| (id.to_string(), a.to_string()))
        .collect();
    if let Ok(j) = serde_json::to_string(&flat) {
        let _ = std::fs::write(crate::config::data_path("lan_addrs.json"), j);
    }
}

/// Coalescing slot for replaceable state messages — `None` means the
/// message is latency-critical and goes to the hi channel as-is.
fn replaceable_slot(to: &Route, msg: &Message) -> Option<u64> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let tag: u8 = match msg {
        Message::Presence { .. } => 1,
        Message::MediaState { .. } => 2,
        Message::Clipboard { .. } => 3,
        // Stale beacons queued during a relay outage coalesce to the
        // freshest one on reconnect.
        Message::LanBeacon { .. } => 4,
        _ => return None,
    };
    let mut h = DefaultHasher::new();
    tag.hash(&mut h);
    format!("{to:?}").hash(&mut h);
    Some(h.finish())
}

/// Dedup state for [`Shared::dedupe`]: per-sender high-water marks plus
/// a bounded FIFO window of recent (sender, seq) pairs.
#[derive(Default)]
struct Seen {
    last: HashMap<DeviceId, u64>,
    set: std::collections::HashSet<(DeviceId, u64)>,
    order: std::collections::VecDeque<(DeviceId, u64)>,
}
