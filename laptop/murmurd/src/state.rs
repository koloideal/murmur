use crate::config::{Config, Secrets};
use crate::keyring::{Keyring, Roster};
use crate::links::load_lan_addrs;
use crate::pair::Pairing;
use murmur_proto::{
    Control, DeviceId, Envelope, Message, PairKey, PresenceState, Route,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use tokio::sync::{broadcast, mpsc};

pub fn now_ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

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
    pub(crate) lan: Mutex<HashMap<DeviceId, (mpsc::UnboundedSender<Envelope>, crate::lan::Transport)>>,
    /// This node's last measured round-trip per peer in ms
    /// (0 = unknown); each peer measures its own on its side.
    pub(crate) lan_rtt: Mutex<HashMap<DeviceId, u64>>,
    /// Last clipboard sync event — ("in" origin id | "out", unix ts) —
    /// the island's clip widget shows direction instead of content.
    pub last_clip: Mutex<Option<(String, String, u64)>>,
    /// Self-status hooks of wired features — ctl renders the registry
    /// instead of knowing each feature's internals by name.
    pub feature_status: RwLock<Vec<(&'static str, fn(&Arc<Shared>) -> serde_json::Value)>>,
    /// Peers whose link just died — one-shot redial markers for the
    /// dialer (bounded by peer count, entries expire after a minute).
    pub(crate) lan_dropped: Mutex<HashMap<DeviceId, std::time::Instant>>,
    /// Last-proven dial address per peer — learned when a link
    /// validates (socket remote addr / the peer's advertised
    /// `lan_addr`). On broadcast-filtered networks no fresh beacon
    /// ever arrives, so `lan::redial_loop` uses this map to bring a
    /// dead link back without discovery.
    pub(crate) lan_addrs: Mutex<HashMap<DeviceId, std::net::SocketAddr>>,
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
            last_clip: Mutex::new(None),
            feature_status: RwLock::new(Vec::new()),
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
                    for (tx, _) in lan.values() {
                        let _ = tx.send(env.clone());
                    }
                }
                Route::Device(d) => {
                    if let Some((tx, _)) = lan.get(d) {
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

    /// The inbound invariant every transport shares: dedupe BEFORE
    /// dispatch, or a dual-path frame (LAN early copy + relay copy)
    /// reaches features twice. Transports open envelopes themselves;
    /// what happens after opening is exactly this.
    pub(crate) fn ingest(
        &self,
        hub: &crate::feature::Hub,
        from: DeviceId,
        seq: u64,
        msg: &Message,
    ) -> bool {
        if !self.dedupe(from, seq) {
            return false;
        }
        hub.dispatch(from, msg);
        true
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
    pub fn token_hex(&self) -> String {
        self.token_hex.clone()
    }

    pub fn dedupe(&self, from: DeviceId, seq: u64) -> bool {
        self.seen.lock().unwrap().check(from, seq)
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

impl Seen {
    /// True = first sighting, deliver; false = duplicate, drop.
    /// High-water marks per sender plus a bounded FIFO window —
    /// broadcasts intentionally ride both the LAN fast-path and the
    /// relay, so the second copy must die here.
    fn check(&mut self, from: DeviceId, seq: u64) -> bool {
        /// This far below the high-water mark can't be an in-flight
        /// duplicate — the LAN early copy is never that late; it's a
        /// sender restart reusing low numbers, so the window resets.
        const RESTART_GAP: u64 = 64;
        let last = self.last.get(&from).copied().unwrap_or(0);
        let last = if seq <= last && last - seq > RESTART_GAP {
            self.set.retain(|(f, _)| *f != from);
            self.order.retain(|(f, _)| *f != from);
            seq
        } else {
            last.max(seq)
        };
        self.last.insert(from, last);
        if !self.set.insert((from, seq)) {
            return false;
        }
        self.order.push_back((from, seq));
        if self.order.len() > 4096 {
            if let Some(old) = self.order.pop_front() {
                self.set.remove(&old);
            }
        }
        true
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn dev(b: u8) -> DeviceId {
        DeviceId([b; 8])
    }

    // ---- dedup window -------------------------------------------------

    #[test]
    fn dedupe_first_true_dupe_false() {
        let mut s = Seen::default();
        assert!(s.check(dev(1), 10));
        assert!(!s.check(dev(1), 10)); // the LAN+relay twin dies here
        assert!(s.check(dev(1), 11));
        assert!(s.check(dev(2), 10)); // same seq, different sender — fine
    }

    #[test]
    fn dedupe_out_of_order_within_window() {
        let mut s = Seen::default();
        assert!(s.check(dev(1), 100));
        // slightly late early-copy — still recognized as a dupe target
        assert!(s.check(dev(1), 98));
        assert!(!s.check(dev(1), 98));
    }

    #[test]
    fn dedupe_restart_gap_resets() {
        let mut s = Seen::default();
        assert!(s.check(dev(1), 1000));
        // sender restarted, seq back to small numbers — must not be
        // dropped as duplicates or the rebooted peer goes deaf
        assert!(s.check(dev(1), 1));
        assert!(s.check(dev(1), 2));
        assert!(!s.check(dev(1), 2));
    }

    #[test]
    fn dedupe_window_bounded_and_recent_dupes_survive() {
        let mut s = Seen::default();
        for i in 0..5000u64 {
            assert!(s.check(dev(1), i), "seq {i} first sighting");
        }
        // memory is bounded at the 4096 cap even under a long session
        assert!(s.order.len() <= 4096);
        // recent sequences are still inside the window — dupes die
        assert!(!s.check(dev(1), 4999));
        // a replay of ancient seq 0 is indistinguishable from a sender
        // restart (gap > RESTART_GAP) — the restart path accepts it.
        // Correct-by-design: a frame that old can never be in flight.
        assert!(s.check(dev(1), 0));
    }
}
