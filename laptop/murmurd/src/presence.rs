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
        Some(|m| matches!(m, Message::Presence { .. })),
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
                let Message::Presence { state, ts } = msg else { continue };
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
