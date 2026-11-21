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

pub fn feature() -> Feature {
    Feature::new(
        "presence",
        Some(|m| matches!(m, Message::Presence { .. })),
        |ctx| Box::pin(run(ctx)),
    )
}

async fn run(ctx: FeatureCtx) -> anyhow::Result<()> {
    let mut inbox = ctx.inbox.expect("feature declares wants");

    // TODO(wayland+zbus): replace this optimistic `Active` with real
    // lock/idle wiring; the select! will gain two more branches then.
    ctx.shared.set_local_state(PresenceState::Active);

    while let Some((from, msg)) = inbox.recv().await {
        let Message::Presence { state, ts } = msg else { continue };
        ctx.shared.note_presence(from, state, ts);
    }
    Ok(())
}
