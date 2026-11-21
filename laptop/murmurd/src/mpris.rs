//! MPRIS feature: the phone appears as a normal system player.
//!
//! Serves org.mpris.MediaPlayer2.murmur (+ .Player) on the session bus.
//! `MediaState` → Metadata/PlaybackStatus/Position (extrapolated locally,
//! Seeked on updates). MPRIS methods → `MediaCommand` to the phone.
//! Artwork bytes → ~/.cache/murmur/art.<hash> → file:// mpris:artUrl.
//!
//! Free wins: media keys, playerctl, waybar/swaync widgets.

use crate::feature::{Feature, FeatureCtx};
use murmur_proto::Message;
use tracing::debug;

pub fn feature() -> Feature {
    Feature::new(
        "mpris",
        Some(|m| matches!(m, Message::MediaState { .. })),
        |ctx| Box::pin(run(ctx)),
    )
}

async fn run(ctx: FeatureCtx) -> anyhow::Result<()> {
    let mut inbox = ctx.inbox.expect("feature declares wants");

    // TODO(zbus): object server; last MediaState + extrapolation clock and
    // artwork cache live here. MediaCommand is outbound-only — goes through
    // ctx.shared.send to the phone.
    while let Some((from, msg)) = inbox.recv().await {
        let Message::MediaState {
            player,
            title,
            status,
            ..
        } = msg
        else {
            continue;
        };
        debug!("mpris: {player} {title:?} {status:?} from {from} (TODO: update MPRIS)");
    }
    Ok(())
}
