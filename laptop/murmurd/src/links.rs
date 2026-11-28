//! Link registry — the LinkStore: which peers hold live direct links,
//! on which transport, with what round-trip. Everything a transport
//! needs to *manage* links lives here; how a link is wired up lives
//! in `lan.rs`. Methods are a partial impl of `Shared` — the registry
//! fields themselves sit next to the other shared state.

use crate::state::Shared;
use murmur_proto::{DeviceId, Envelope, Route};
use std::collections::HashMap;
use tokio::sync::mpsc;

/// `lan_addrs.json` — remembered peer dial addresses, keyed by hex id.
/// Both directions stay stringly: DeviceId/SocketAddr parse cleanly
/// and a hand-editable file beats a schema for two fields.
pub(crate) fn load_lan_addrs() -> HashMap<DeviceId, std::net::SocketAddr> {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdoptOutcome {
    /// Freshly registered or replaced an older link.
    Adopted,
    /// This channel was already the registered one — nothing changed.
    Kept,
    /// Lost the simultaneous-dial tie-break — close the socket.
    Rejected,
}

impl Shared {
    /// Link bookkeeping for [`crate::lan`]. Called on every valid
    /// frame — a no-op while the map already points at this channel,
    /// so it never tramples the link's measured RTT.
    /// Adopt a validated link. Simultaneous dials (both sides redial
    /// remembered addresses) briefly hold two sockets per pair; the
    /// tie-break keeps only the one dialed by the smaller device id —
    /// both sides compute it independently and close the same loser,
    /// so the flap of crossing close-orders can't happen. Returns
    /// [AdoptOutcome::Rejected] when this link loses: the caller closes
    /// the socket instead of letting its reader live on unregistered.
    pub fn lan_adopt(
        &self,
        peer: DeviceId,
        tx: mpsc::UnboundedSender<Envelope>,
        transport: crate::lan::Transport,
        dir: crate::lan::LinkDir,
    ) -> AdoptOutcome {
        {
            let mut lan = self.lan.lock().unwrap();
            if matches!(lan.get(&peer), Some((t, _, _)) if t.same_channel(&tx)) {
                return AdoptOutcome::Kept;
            }
            let winner = crate::lan::winner_dir(self.id, peer);
            if let Some((_, _, old_dir)) = lan.get(&peer) {
                let old_wins = *old_dir == winner;
                let new_wins = dir == winner;
                if old_wins && !new_wins {
                    return AdoptOutcome::Rejected;
                }
            }
            lan.insert(peer, (tx, transport, dir));
            self.lan_rtt.lock().unwrap().insert(peer, 0);
        }
        // The link-up is a rendezvous: announce so the peer pins our
        // identity + name, and re-announce presence — the heartbeat's
        // 60s window would otherwise leave the peer rejecting clips
        // from an "inactive" origin. In a serverless family it's the
        // *only* rendezvous — there is no relay connect to hang this on.
        self.announce_identity();
        self.broadcast_presence();
        AdoptOutcome::Adopted
    }
    /// Push a pre-built envelope down a live link — for frames that
    /// must ride a specific key (a re-key sealed under the peer's old
    /// key), not the current-key seal `send` always applies.
    pub(crate) fn lan_send(&self, peer: &DeviceId, env: Envelope) {
        if let Some((tx, ..)) = self.lan.lock().unwrap().get(peer) {
            let _ = tx.send(env);
        }
    }

    /// Transport tag per linked peer — ctl shows how the link rides.
    pub fn lan_transports(&self) -> Vec<(DeviceId, crate::lan::Transport)> {
        self.lan
            .lock()
            .unwrap()
            .iter()
            .map(|(p, (_, t, _))| (*p, *t))
            .collect()
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
            .filter(|(_, (t, ..))| t.same_channel(tx))
            .map(|(p, _)| *p)
            .collect();
        g.retain(|_, (t, ..)| !t.same_channel(tx));
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

    /// Flood an inbound link frame to every *other* live link — the
    /// serverless mesh is multi-hop: a phone USB-linked to one laptop
    /// must still reach a second laptop on wifi. The sealed envelope
    /// rides as-is (`from` stays the origin — no re-wrap needed), the
    /// caller forwards only first-seen frames (dedupe already ran),
    /// and the arrival socket plus the origin's own link are skipped.
    /// Loops terminate: every node forwards each (from, seq) at most
    /// once, later copies die in dedupe.
    pub(crate) fn lan_forward(
        &self,
        env: &Envelope,
        arrived: &mpsc::UnboundedSender<Envelope>,
    ) {
        match &env.to {
            // Addressed to us — dispatched already; Route::Server is
            // relay business a LAN flood can never complete.
            Route::Device(d) if *d == self.id => return,
            Route::Server => return,
            _ => {}
        }
        let lan = self.lan.lock().unwrap();
        for (peer, (tx, ..)) in lan.iter() {
            if *peer == env.from || tx.same_channel(arrived) {
                continue;
            }
            let _ = tx.send(env.clone());
        }
    }
}
