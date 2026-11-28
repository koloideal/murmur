//! Link registry — the LinkStore: which peers hold live direct links,
//! on which transport, with what round-trip. Everything a transport
//! needs to *manage* links lives here; how a link is wired up lives
//! in `lan.rs`. Methods are a partial impl of `Shared` — the registry
//! fields themselves sit next to the other shared state.

use crate::state::Shared;
use murmur_proto::{DeviceId, Envelope, Route};
use std::collections::HashMap;
use tokio::sync::mpsc;

/// `lan_addrs.json` — remembered dial *candidates* per peer, ordered:
/// front is the address a validated link last proved, the rest are
/// alternates gathered from hints (beacon adverts, pair seeds).
/// Transport switching needs the whole set remembered — betting on a
/// single rendezvous means a dead one strands the peer.
/// Back-compat: the old `{id: "addr"}` flat form loads as a one-entry
/// list.
pub(crate) fn load_lan_addrs() -> HashMap<DeviceId, Vec<std::net::SocketAddr>> {
    std::fs::read_to_string(crate::config::data_path("lan_addrs.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<HashMap<String, serde_json::Value>>(&s).ok())
        .map(|m| {
            m.iter()
                .filter_map(|(id, v)| {
                    let id = id.parse::<DeviceId>().ok()?;
                    let one = |s: &str| s.parse::<std::net::SocketAddr>().ok();
                    let addrs = match v {
                        serde_json::Value::String(a) => one(a).into_iter().collect(),
                        serde_json::Value::Array(arr) => arr
                            .iter()
                            .filter_map(|a| a.as_str().and_then(one))
                            .collect(),
                        _ => Vec::new(),
                    };
                    (!addrs.is_empty()).then_some((id, addrs))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn save_lan_addrs(m: &HashMap<DeviceId, Vec<std::net::SocketAddr>>) {
    let flat: HashMap<String, Vec<String>> = m
        .iter()
        .map(|(id, a)| (id.to_string(), a.iter().map(|x| x.to_string()).collect()))
        .collect();
    if let Ok(j) = serde_json::to_string(&flat) {
        let _ = std::fs::write(crate::config::data_path("lan_addrs.json"), j);
    }
}

/// Candidates per peer — enough for proven + a few alternates without
/// the set growing into a dial storm.
const LAN_ADDR_CAP: usize = 4;

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
                // A lost link re-dials NOW, not on the next 10s tick —
                // the whole candidate set goes out front-first so the
                // proven transport gets the rate-limit exemption and
                // alternates race it in parallel. Transport switching
                // is only seamless if the fallback starts immediately.
                if let Some(kind) = self.roster.lock().unwrap().get(&p).map(|m| m.kind) {
                    for a in self.lan_addrs.lock().unwrap().get(&p).cloned().unwrap_or_default()
                    {
                        let _ = self.lan_dial.send(crate::lan::DialHint {
                            peer: Some(p),
                            addr: a.ip(),
                            port: a.port(),
                            kind,
                            trusted: true,
                            transport: crate::lan::Transport::Lan,
                        });
                    }
                }
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

    /// A proven rendezvous — a validated link's real remote addr.
    /// Moves to the front of the candidate list; alternates survive
    /// behind it for transport failover.
    /// Persisted: a daemon restart on a broadcast-filtered network
    /// would otherwise lose every rendezvous it has.
    pub fn lan_addr_set(&self, peer: DeviceId, addr: std::net::SocketAddr) {
        {
            let mut m = self.lan_addrs.lock().unwrap();
            if m.get(&peer).and_then(|v| v.first()) == Some(&addr) {
                return;
            }
            let v = m.entry(peer).or_default();
            v.retain(|a| *a != addr);
            v.insert(0, addr);
            v.truncate(LAN_ADDR_CAP);
            save_lan_addrs(&m);
        }
    }

    /// A tentative rendezvous — a beacon's advertised addr or a
    /// pair-time seed. Appends as an alternate: never reorders the
    /// proven front (a usb-linked phone still advertises its wifi
    /// addr — correct to learn as a fallback, wrong to promote).
    pub fn lan_addr_hint(&self, peer: DeviceId, addr: std::net::SocketAddr) {
        let mut m = self.lan_addrs.lock().unwrap();
        let v = m.entry(peer).or_default();
        if v.contains(&addr) {
            return;
        }
        if v.len() < LAN_ADDR_CAP {
            v.push(addr);
        } else {
            *v.last_mut().unwrap() = addr; // freshest hint evicts the oldest alternate
        }
        save_lan_addrs(&m);
    }

    /// Snapshot for the redial loop and `murmurctl status` — ordered,
    /// proven addr first.
    pub fn lan_addrs(&self) -> HashMap<DeviceId, Vec<std::net::SocketAddr>> {
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
