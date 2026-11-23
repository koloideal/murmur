use crate::config::{Config, Secrets};
use crate::keyring::{Keyring, Roster};
use murmur_proto::{
    Control, DeviceId, Envelope, Message, PairKey, PresenceState, PublicKey, Route,
};
use std::collections::HashMap;
use std::sync::{Mutex, RwLock};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
            family_name: Mutex::new(None),
            identity,
            roster: Mutex::new(crate::keyring::load_roster()),
            last_rekey: Mutex::new(HashMap::new()),
            rotating: Mutex::new(()),
            token_hex: secrets.auth_token_hex.clone(),
            secrets: Mutex::new(secrets),
            cfg,
            seq: AtomicU64::new(1),
            out,
            out_lo,
            lan: Mutex::new(HashMap::new()),
            lan_rtt: Mutex::new(HashMap::new()),
            lan_dropped: Mutex::new(HashMap::new()),
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
        let env = Envelope::seal(self.id, to.clone(), self.next_seq(), &self.keys.read().unwrap().current, msg)?;
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
    fn has_relay(&self) -> bool {
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
        // identity + name. In a serverless family it's the *only*
        // rendezvous — there is no relay connect to hang this on.
        self.announce_identity();
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
    pub fn active_node(&self) -> Option<DeviceId> {
        self.presence
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, (s, _))| *s == PresenceState::Active)
            .max_by(|(a_id, (_, a_ts)), (b_id, (_, b_ts))| {
                a_ts.cmp(b_ts).then(b_id.cmp(a_id)) // later ts wins; smaller id wins ties
            })
            .map(|(id, _)| *id)
    }

    pub fn is_active(&self) -> bool {
        self.active_node() == Some(self.id)
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

    /// Re-announce current presence state to everyone.
    pub fn broadcast_presence(&self) {
        let state = self
            .presence
            .lock()
            .unwrap()
            .get(&self.id)
            .map(|(s, _)| *s)
            .unwrap_or(PresenceState::Active);
        let _ = self.send(
            Route::All,
            &Message::Presence {
                state,
                ts: now_ts(),
            },
        );
    }

    /// Called on disconnect. Presence of remote peers is retained; they age
    /// out implicitly because our election only considers `active` states —
    /// a disconnected peer simply stops updating. TODO: mark peers stale.
    pub fn note_presence_offline(&self) {}

    pub fn note_presence(&self, id: DeviceId, state: PresenceState, ts: u64) {
        let mut m = self.presence.lock().unwrap();
        if m.get(&id).is_none_or(|(_, old)| ts > *old) {
            m.insert(id, (state, ts));
        }
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
