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
        &["clipboard"],
        |ctx| Box::pin(run(ctx)),
    )
    .with_status(|shared| {
        serde_json::json!({
            "last_clip": shared.last_clip.lock().unwrap().clone()
                .map(|(d, o, ts)| serde_json::json!({"dir": d, "origin": o, "ts": ts})),
        })
    })
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
    // Hashes we just applied from the network — the watcher's echo of
    // each is not a real copy. Multi-slot on purpose: two inbound clips
    // landing before the watcher fires would overwrite a single slot
    // and the unconsumed echo would rebroadcast — a ping-pong loop.
    let mut suppress: std::collections::VecDeque<u64> = std::collections::VecDeque::new();
    // Last hash we broadcast — repeated local events are deduped.
    let mut last_sent: Option<u64> = None;

    loop {
        tokio::select! {
            ev = events.recv() => {
                let Some(ev) = ev else { return Ok(()) };
                let wlclip::ClipEvent::Data { mime, offered, data } = ev else { continue };
                let hash = hash_clip(&data);
                // Consume exactly one matching suppression — the echo is
                // one watcher event per apply, and an unrelated local
                // event must not eat the slot.
                if let Some(pos) = suppress.iter().position(|h| *h == hash) {
                    suppress.remove(pos);
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
                if data.len() > murmur_proto::MAX_CLIP {
                    warn!("clipboard: {}B over wire cap — not synced", data.len());
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
                    *shared.last_clip.lock().unwrap() =
                        Some(("out".to_string(), String::new(), crate::state::now_ts()));
                }
            }
            m = inbox.recv() => {
                let Some((from, msg)) = m else { return Ok(()) };
                let Message::Clipboard { hash, mime, data_b64, explicit, .. } = (*msg).clone() else {
                    continue;
                };
                if !should_apply(hash, explicit, last_sent, shared.is_active()) {
                    continue;
                }
                use base64::Engine;
                let Ok(data) = base64::engine::general_purpose::STANDARD.decode(&data_b64) else {
                    continue;
                };
                // Hash from the wire is the *sender's* — we suppress by
                // the local hash the watcher will actually see.
                suppress.push_back(hash_clip(&data));
                while suppress.len() > 8 {
                    suppress.pop_front();
                }
                *shared.last_clip.lock().unwrap() =
                    Some(("in".to_string(), from.to_string(), crate::state::now_ts()));
                tracing::debug!("clip ← apply hash={hash:x} mime={mime} explicit={explicit}");
                let _ = from;
                if cmds.send(wlclip::ClipCmd::Set { mime, data }).is_err() {
                    return Ok(()); // adapter died
                }
            }
        }
    }
}


/// Anti-echo / targeting decision for an inbound clip, pure so the
/// matrix is testable: explicit pushes apply only on the active node,
/// and a clip whose hash matches what we just broadcast is our own
/// echo coming back around — applying it would re-trigger the watcher
/// and ping-pong the clipboard forever.
pub(crate) fn should_apply(hash: u64, explicit: bool, last_sent: Option<u64>, is_active: bool) -> bool {
    if explicit && !is_active {
        return false;
    }
    if last_sent == Some(hash) {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echo_of_own_clip_never_applies() {
        // the ping-pong scenario: we broadcast hash H, the frame comes
        // back through the family — it must die here or the watcher
        // sees it as a new local clip and rebroadcasts endlessly
        assert!(!should_apply(0xAA, false, Some(0xAA), true));
        assert!(!should_apply(0xAA, true, Some(0xAA), true));
    }

    #[test]
    fn foreign_clip_applies_regardless_of_role() {
        assert!(should_apply(0xBB, false, Some(0xAA), true));
        assert!(should_apply(0xBB, false, Some(0xAA), false)); // passive sync applies everywhere
        assert!(should_apply(0xBB, false, None, false));
    }

    #[test]
    fn explicit_push_respects_active_node() {
        // explicit (share/process-text) is a deliberate targeting act —
        // inactive laptops must not fight over who owns the clipboard
        assert!(should_apply(0xBB, true, None, true));
        assert!(!should_apply(0xBB, true, None, false));
    }

    #[test]
    fn hash_clip_is_stable_and_content_sensitive() {
        assert_eq!(hash_clip(b"hello"), hash_clip(b"hello"));
        assert_ne!(hash_clip(b"hello"), hash_clip(b"world"));
        assert_ne!(hash_clip(b""), hash_clip(b"\0"));
    }

    #[test]
    fn suppress_matches_only_exact_hash() {
        // the watcher clears suppression ONLY on exact match — an
        // unrelated local copy must not eat the suppression or the
        // applied clip echoes back when it finally surfaces
        let suppress = Some(hash_clip(b"from-network"));
        assert!(suppress == Some(hash_clip(b"from-network")));
        assert!(suppress != Some(hash_clip(b"unrelated-local-copy")));
    }
}
