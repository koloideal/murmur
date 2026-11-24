//! Clipboard feature: bidirectional sync through ext-data-control
//! ([`crate::wlclip`]).
//!
//! Outbound (local → family): a local `ClipEvent::Data` is hashed; if it
//! matches `suppress` (we just applied it from the network ourselves —
//! echo) or `last_sent` (repeat event), it's dropped. Otherwise it's
//! broadcast to `*`, phones included — the phone decides by itself
//! whether we're the active node.
//!
//! Inbound (family → local): `explicit` clips (phone share menu /
//! process-text) are applied only on the active laptop; laptop clips
//! apply everywhere. Applied clips land in `suppress` so the watcher
//! doesn't rebroadcast them.
//!
//! Protection: offers containing a denylisted mime
//! (`clipboard_mime_deny` — password-manager markers) are never sent.

use crate::feature::{Feature, FeatureCtx};
use crate::wlclip;
use murmur_proto::{Message, Route};
use std::collections::HashSet;
use tracing::{info, warn};

/// FNV-1a — dedup needs stable, cheap, non-cryptographic hashing.
pub fn hash_clip(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x100000001b3)
    })
}

pub fn feature() -> Feature {
    Feature::new(
        "clipboard",
        Some(|m| matches!(m, Message::Clipboard { .. })),
        |ctx| Box::pin(run(ctx)),
    )
}

async fn run(ctx: FeatureCtx) -> anyhow::Result<()> {
    let shared = ctx.shared;
    let mut inbox = ctx.inbox.expect("feature declares wants");
    let (mut events, cmds) = match wlclip::spawn() {
        Ok(pair) => {
            info!("clipboard tracking up");
            pair
        }
        Err(e) => {
            // No wayland or no data-control — feature idles.
            warn!("clipboard feature disabled: {e:#}");
            while inbox.recv().await.is_some() {}
            return Ok(());
        }
    };

    let deny: HashSet<String> = shared.cfg.clipboard_mime_deny.iter().cloned().collect();
    // Hash we just applied from the network — next local event with it
    // is echo, not a real copy.
    let mut suppress: Option<u64> = None;
    // Last hash we broadcast — repeated local events are deduped.
    let mut last_sent: Option<u64> = None;

    loop {
        tokio::select! {
            ev = events.recv() => {
                let Some(ev) = ev else { return Ok(()) };
                let wlclip::ClipEvent::Data { mime, offered, data } = ev else { continue };
                let hash = hash_clip(&data);
                // Only clear suppress on a match — an unrelated local
                // event arriving before the echo must not consume it.
                if suppress == Some(hash) {
                    suppress = None;
                    continue; // echo of our own apply
                }
                if last_sent == Some(hash) {
                    continue; // repeated event for the same content
                }
                // Denylist is offer-level: password managers tag their
                // clips with a hint mime *alongside* text/plain — deny if
                // it appears anywhere in the offer.
                if let Some(bad) = offered.iter().find(|m| deny.contains(*m)) {
                    info!("clipboard: offer tagged {bad} not synced");
                    continue;
                }
                tracing::debug!("clip → broadcast hash={hash:x} mime={mime}");
                use base64::Engine;
                let msg = Message::Clipboard {
                    origin: shared.id,
                    hash,
                    mime,
                    data_b64: base64::engine::general_purpose::STANDARD.encode(&data),
                    explicit: false,
                    ts: crate::state::now_ts(),
                };
                if shared.send(Route::All, &msg).is_ok() {
                    last_sent = Some(hash);
                }
            }
            m = inbox.recv() => {
                let Some((from, msg)) = m else { return Ok(()) };
                let Message::Clipboard { hash, mime, data_b64, explicit, .. } = msg else {
                    continue;
                };
                if explicit && !shared.is_active() {
                    continue; // explicit pushes go to the active node only
                }
                if last_sent == Some(hash) {
                    continue; // our own clip came back around
                }
                use base64::Engine;
                let Ok(data) = base64::engine::general_purpose::STANDARD.decode(&data_b64) else {
                    continue;
                };
                // Hash from the wire is the *sender's* — we suppress by
                // the local hash the watcher will actually see.
                suppress = Some(hash_clip(&data));
                tracing::debug!("clip ← apply hash={hash:x} mime={mime} explicit={explicit}");
                let _ = from;
                if cmds.send(wlclip::ClipCmd::Set { mime, data }).is_err() {
                    return Ok(()); // adapter died
                }
            }
        }
    }
}
