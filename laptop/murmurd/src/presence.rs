//! Presence feature: feeds the shared presence table.
//!
//! Two jobs:
//! - ingest peer `Presence` messages into `shared.presence` (the table the
//!   active-node election reads);
//! - watch local lock/idle and re-broadcast our own state.
//!
//! Local sources (TODO, join via select!):
//! - zbus: org.freedesktop.login1 Session LockedHint
//! - wayland: ext_idle_notifier_v1 → idle/resumed at cfg.idle_timeout_secs

use crate::feature::{Feature, FeatureCtx};
use murmur_proto::{Message, PresenceState};
use std::time::Duration;

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
    let mut inbox = ctx.inbox.expect("feature declares wants");
    let mut tick = tokio::time::interval(HEARTBEAT);
    tick.tick().await; // the first tick fires immediately — skip it

    // TODO(wayland+zbus): replace this optimistic `Active` with real
    // lock/idle wiring; the select! will gain two more branches then.
    ctx.shared.set_local_state(PresenceState::Active);

    loop {
        tokio::select! {
            m = inbox.recv() => {
                let Some((from, msg)) = m else { return Ok(()) };
                let Message::Presence { state, ts } = msg else { continue };
                ctx.shared.note_presence(from, state, ts);
            }
            _ = tick.tick() => ctx.shared.broadcast_presence(),
        }
    }
}
