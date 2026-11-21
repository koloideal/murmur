//! Notifications feature: phone notifications ⇄ swaync.
//!
//! `Notification{key,…}` → org.freedesktop.Notifications.Notify; the local
//! nid is kept in a private map (key ↔ nid). swaync NotificationClosed(nid)
//! → `NotificationDismiss{key}` broadcast → the phone cancels and other
//! laptops close theirs. Remote-initiated closes mark the nid first so the
//! resulting NotificationClosed doesn't echo back out (anti-loop).

use crate::feature::{Feature, FeatureCtx};
use murmur_proto::Message;
use tracing::debug;

pub fn feature() -> Feature {
    Feature::new(
        "notify",
        Some(|m| {
            matches!(
                m,
                Message::Notification { .. } | Message::NotificationDismiss { .. }
            )
        }),
        |ctx| Box::pin(run(ctx)),
    )
}

async fn run(ctx: FeatureCtx) -> anyhow::Result<()> {
    let mut inbox = ctx.inbox.expect("feature declares wants");

    // TODO(zbus): session-bus conn; Notify/CloseNotification calls;
    // match-listener on NotificationClosed; HashMap<String, u32> key↔nid and
    // HashSet<u32> remote-closed live here, owned by this feature.
    while let Some((from, msg)) = inbox.recv().await {
        match msg {
            Message::Notification { key, title, .. } => {
                debug!("notify: {key} {title:?} from {from} (TODO: post to swaync)")
            }
            Message::NotificationDismiss { key } => {
                debug!("notify: dismiss {key} from {from} (TODO: close local nid)")
            }
            _ => {}
        }
    }
    Ok(())
}
