//! Presence feature: feeds the shared presence table.
//!
//! Own-state sources, all best-effort and additive:
//! - logind Lock/Unlock where the session supports it ([`crate::logind`]);
//! - locker-process watch for wlroots lockers ([`crate::lockproc`]) —
//!   hyprlock & friends expose no observable lock state, so we watch
//!   `/proc` for configured names;
//! - ext-idle-notify idle/resumed at `cfg.idle_timeout_secs` ([`crate::wlidle`]).
//!
//! Lock sources vote: any `Locked` wins; `Unlocked` only clears its own
//! vote. Idle events are ignored while any vote holds — a locked session
//! stays `locked`, not `idle`.
//!
//! Peers' `Presence` messages land in `shared.presence`; the election
//! itself lives in `state.rs` (other features read it too).

use crate::feature::{Feature, FeatureCtx};
use crate::{lockproc, logind, wlidle};
use murmur_proto::{Message, PresenceState};
use std::time::Duration;
use tracing::{info, warn};

/// How often we re-announce presence — peers that missed a broadcast
/// (they were offline; live-only delivery) converge within one interval.
const HEARTBEAT: Duration = Duration::from_secs(60);

pub fn feature() -> Feature {
    Feature::new(
        "presence",
        &["presence"],
        |ctx| Box::pin(run(ctx)),
    )
}

async fn run(ctx: FeatureCtx) -> anyhow::Result<()> {
    let shared = ctx.shared;
    let mut inbox = ctx.inbox.expect("feature declares wants");

    // Lock votes: [logind, locker-process]. Any true → session locked.
    let mut votes = [false; 2];
    let locked = |votes: &[bool; 2]| votes.iter().any(|v| *v);

    let mut logind_rx = match logind::watch().await {
        Ok(w) => {
            votes[0] = w.initially_locked;
            info!(
                "logind lock tracking up (session locked: {})",
                w.initially_locked
            );
            Some(w.rx)
        }
        Err(e) => {
            warn!("no logind lock tracking: {e:#}");
            None
        }
    };
    let mut proc_rx = lockproc::spawn(&shared.cfg.lock_procs);
    let mut idle_rx = match wlidle::spawn(Duration::from_secs(shared.cfg.idle_timeout_secs)) {
        Ok(rx) => {
            info!("wayland idle tracking up");
            Some(rx)
        }
        Err(e) => {
            warn!("no wayland idle tracking: {e:#}");
            None
        }
    };

    shared.set_local_state(if locked(&votes) {
        PresenceState::Locked
    } else {
        PresenceState::Active
    });

    let mut tick = tokio::time::interval(HEARTBEAT);
    tick.tick().await; // the first tick fires immediately — skip it

    loop {
        tokio::select! {
            m = inbox.recv() => {
                let Some((from, msg)) = m else { return Ok(()) };
                let Message::Presence { state, ts } = (*msg).clone() else { continue };
                shared.note_presence(from, state, ts);
            }
            ev = async { logind_rx.as_mut().unwrap().recv().await },
                  if logind_rx.is_some() => {
                match ev {
                    Some(logind::Lock::Locked) => votes[0] = true,
                    Some(logind::Lock::Unlocked) => votes[0] = false,
                    None => logind_rx = None,
                }
                if ev.is_some() {
                    shared.set_local_state(if locked(&votes) {
                        PresenceState::Locked
                    } else {
                        PresenceState::Active
                    });
                }
            }
            ev = proc_rx.recv() => {
                match ev {
                    Some(logind::Lock::Locked) => votes[1] = true,
                    Some(logind::Lock::Unlocked) => votes[1] = false,
                    None => {}
                }
                if ev.is_some() {
                    shared.set_local_state(if locked(&votes) {
                        PresenceState::Locked
                    } else {
                        PresenceState::Active
                    });
                }
            }
            ev = async { idle_rx.as_mut().unwrap().recv().await },
                  if idle_rx.is_some() => {
                match ev {
                    Some(wlidle::Idle::Idled) if !locked(&votes) => {
                        shared.set_local_state(PresenceState::Idle)
                    }
                    Some(wlidle::Idle::Resumed) if !locked(&votes) => {
                        shared.set_local_state(PresenceState::Active)
                    }
                    Some(_) => {} // ignored — a lock vote is holding
                    None => idle_rx = None,
                }
            }
            _ = tick.tick() => shared.broadcast_presence(),
        }
    }
}

// ---------------------------------------------------------------------------
// Election — pure functions plus the Shared accessors other features
// read. Lives here rather than state.rs because presence semantics
// (recording, merging, electing) are one cohesive topic.
// ---------------------------------------------------------------------------

use crate::state::{now_ts, Shared};
use murmur_proto::{DeviceId, Route};
use std::collections::HashMap;

/// Presence entries older than this stop counting for the active-node
/// election — 2.5× the 60s heartbeat, so one missed beat still counts.
pub const PRESENCE_STALE: u64 = 150;

/// Pure active-node election — the function every node computes
/// independently over its presence map. Latest active timestamp wins,
/// ties break toward the smaller DeviceId; entries past the staleness
/// cutoff don't count. Pure so the convergence tests can hammer it.
/// Presence merge rule: a fresher timestamp overwrites, stale never
/// does — the table is order-independent *by construction*, which is
/// what makes the distributed election converge at all.
pub(crate) fn presence_merge(
    map: &mut HashMap<DeviceId, (PresenceState, u64)>,
    id: DeviceId,
    state: PresenceState,
    ts: u64,
) {
    if map.get(&id).is_none_or(|(_, old)| ts > *old) {
        map.insert(id, (state, ts));
    }
}

pub(crate) fn elect_active(
    map: &HashMap<DeviceId, (PresenceState, u64)>,
    now: u64,
) -> Option<DeviceId> {
    let cutoff = now.saturating_sub(PRESENCE_STALE);
    map.iter()
        .filter(|(_, (s, ts))| *s == PresenceState::Active && *ts >= cutoff)
        .max_by(|(a_id, (_, a_ts)), (b_id, (_, b_ts))| {
            a_ts.cmp(b_ts).then(b_id.cmp(a_id)) // later ts wins; smaller id wins ties
        })
        .map(|(id, _)| *id)
}


/// Clipboard-target election: who clipboard traffic follows. Unlike
/// [`elect_active`] this never answers "nobody" while a live laptop
/// exists — the user's real flow is phone-in-hand with the laptop
/// untouched for minutes, and dropping their deliberate shares because
/// no node is `active` was exactly the reported "stops in both
/// directions". Tiers: active beats idle beats locked (engagement
/// outranks recency); within a tier freshest ts wins, ties → smaller
/// id. Same inputs on every node → same target, so exactly one laptop
/// applies an explicit push.
pub(crate) fn elect_clip_target(
    map: &HashMap<DeviceId, (PresenceState, u64)>,
    now: u64,
) -> Option<DeviceId> {
    let cutoff = now.saturating_sub(PRESENCE_STALE);
    let rank = |s: &PresenceState| match s {
        PresenceState::Active => 2u8,
        PresenceState::Idle => 1,
        PresenceState::Locked => 0,
    };
    map.iter()
        .filter(|(_, (_, ts))| *ts >= cutoff)
        .max_by(|(a_id, (a_s, a_ts)), (b_id, (b_s, b_ts))| {
            rank(a_s)
                .cmp(&rank(b_s))
                .then(a_ts.cmp(b_ts))
                .then(b_id.cmp(a_id))
        })
        .map(|(id, _)| *id)
}

impl Shared {
    /// Active-node election: newest `active` wins; ties → smaller DeviceId.
    /// Deterministic across nodes given the same presence table.
    /// Entries older than PRESENCE_STALE are ignored: delivery is
    /// live-only, so a peer silent that long is dead — without the
    /// cutoff a crashed "active" laptop wins the election forever and
    /// explicit pushes/commands route to a ghost.
    pub fn active_node(&self) -> Option<DeviceId> {
        elect_active(&self.presence.lock().unwrap(), now_ts())
    }

    pub fn is_active(&self) -> bool {
        self.active_node() == Some(self.id)
    }

    /// Am I the laptop clipboard traffic should follow right now —
    /// the tiered fallback election, so clipboard never dies just
    /// because the user is looking at their phone instead of typing.
    pub fn is_clip_target(&self) -> bool {
        elect_clip_target(&self.presence.lock().unwrap(), now_ts()) == Some(self.id)
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
        presence_merge(&mut self.presence.lock().unwrap(), id, state, ts);
    }
}

#[cfg(test)]
mod election_tests {
    use super::*;

    fn dev(b: u8) -> DeviceId {
        DeviceId([b; 8])
    }

    fn apply(events: &[(u8, PresenceState, u64)]) -> HashMap<DeviceId, (PresenceState, u64)> {
        let mut m = HashMap::new();
        for (d, st, ts) in events {
            presence_merge(&mut m, dev(*d), *st, *ts);
        }
        m
    }

    // ---- election convergence ----------------------------------------

    /// The same event multiset in any order yields the same winner —
    /// the property the whole distributed election rests on.
    #[test]
    fn election_converges_under_permutations() {
        let events = vec![
            (1u8, PresenceState::Active, 100u64),
            (1, PresenceState::Active, 200),   // same node, newer beat
            (2, PresenceState::Active, 150),
            (2, PresenceState::Idle, 250),     // goes idle — out of the race
            (3, PresenceState::Active, 200),   // ts tie with dev1 — smaller id wins
        ];
        let now = 300; // everything fresh: 300 - 150 < PRESENCE_STALE window check below
        // ensure events are inside the staleness window for `now`
        let now = if PRESENCE_STALE > 200 { now } else { 100 + PRESENCE_STALE - 1 };

        let reference = elect_active(&apply(&events), now);

        // permute by rotation+reversal — cheap but covers reordering classes
        for rot in 0..events.len() {
            let mut p: Vec<_> = events.clone();
            p.rotate_left(rot);
            assert_eq!(elect_active(&apply(&p), now), reference, "rotation {rot}");
            p.reverse();
            assert_eq!(elect_active(&apply(&p), now), reference, "reversed rotation {rot}");
        }
        // dev1: ts 200, active, smaller than dev3 — the winner
        assert_eq!(reference, Some(dev(1)));
    }

    #[test]
    fn election_rules() {
        let now = 1_000_000;
        let fresh = now - PRESENCE_STALE + 10;
        // stale entries don't count
        let mut m = HashMap::new();
        m.insert(dev(1), (PresenceState::Active, now - PRESENCE_STALE - 1));
        assert_eq!(elect_active(&m, now), None);
        // idle/locked never win
        m.insert(dev(1), (PresenceState::Idle, fresh));
        m.insert(dev(2), (PresenceState::Locked, fresh));
        assert_eq!(elect_active(&m, now), None);
        // later ts beats earlier
        m.insert(dev(3), (PresenceState::Active, fresh));
        m.insert(dev(4), (PresenceState::Active, fresh + 5));
        assert_eq!(elect_active(&m, now), Some(dev(4)));
        // tie → smaller id
        let mut t = HashMap::new();
        t.insert(dev(9), (PresenceState::Active, fresh));
        t.insert(dev(5), (PresenceState::Active, fresh));
        assert_eq!(elect_active(&t, now), Some(dev(5)));
    }

    // ---- clip-target fallback election -------------------------------

    /// The user's real flow: phone in hand, laptop untouched for three
    /// minutes — presence flips idle and every explicit share was
    /// silently dropped. The clip target must never be "nobody" while
    /// a live laptop exists: active first, then idle, then locked.
    #[test]
    fn clip_target_falls_back_past_active() {
        let now = 1_000_000;
        let fresh = now - PRESENCE_STALE + 10;
        let mut m = HashMap::new();
        // no active node at all — an idle laptop still takes the traffic
        m.insert(dev(1), (PresenceState::Idle, fresh));
        assert_eq!(elect_clip_target(&m, now), Some(dev(1)));
        // locked is the last resort, not a drop
        let mut l = HashMap::new();
        l.insert(dev(2), (PresenceState::Locked, fresh));
        assert_eq!(elect_clip_target(&l, now), Some(dev(2)));
        // active still beats a fresher idle — engagement outranks recency
        let mut x = HashMap::new();
        x.insert(dev(3), (PresenceState::Active, fresh - 5));
        x.insert(dev(4), (PresenceState::Idle, fresh));
        assert_eq!(elect_clip_target(&x, now), Some(dev(3)));
        // staleness applies to every tier — a dead laptop wins nothing
        let mut s = HashMap::new();
        s.insert(dev(5), (PresenceState::Idle, now - PRESENCE_STALE - 1));
        assert_eq!(elect_clip_target(&s, now), None);
    }

    #[test]
    fn merge_never_goes_backwards() {
        let mut m = HashMap::new();
        presence_merge(&mut m, dev(1), PresenceState::Active, 100);
        presence_merge(&mut m, dev(1), PresenceState::Idle, 90); // stale — ignored
        assert_eq!(m[&dev(1)], (PresenceState::Active, 100));
        presence_merge(&mut m, dev(1), PresenceState::Idle, 110); // newer — wins
        assert_eq!(m[&dev(1)], (PresenceState::Idle, 110));
    }

}
