//! Clipboard feature: sync via ext-data-control-v1 (no polling).
//!
//! Local offer → hash → skip if it matches what we just applied ourselves
//! (expected) or what we last sent → skip denied MIMEs and sensitive-flagged
//! offers → broadcast `Clipboard` to "*"; if this node is the active one,
//! also send to the phone.
//!
//! Inbound `Clipboard`: laptop-origin → apply on every node; phone-origin →
//! apply only on the active node. Dedup state (`last`, `expected`) is
//! private to this feature.

use crate::feature::{Feature, FeatureCtx};
use murmur_proto::Message;
use tracing::debug;

/// FNV-1a over clipboard bytes — cheap dedup key, no crypto needed.
pub fn hash_clip(data: &[u8]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

pub fn feature() -> Feature {
    Feature::new(
        "clipboard",
        Some(|m| matches!(m, Message::Clipboard { .. })),
        |ctx| Box::pin(run(ctx)),
    )
}

async fn run(ctx: FeatureCtx) -> anyhow::Result<()> {
    let mut inbox = ctx.inbox.expect("feature declares wants");
    let mut last: Option<u64> = None; // last hash sent or applied (anti-echo)

    // TODO(wayland): ext_data_control_manager_v1 → data device. On offer:
    //   read mimes; skip denied (cfg.clipboard_mime_deny) and sensitive;
    //   pipe-fd read; hash; compare vs expected/last; broadcast (+phone if
    //   ctx.shared.is_active()).
    while let Some((from, msg)) = inbox.recv().await {
        let Message::Clipboard {
            hash, mime, origin, ..
        } = msg
        else {
            continue;
        };
        if Some(hash) == last {
            continue; // our own echo
        }
        // Phone pushes apply only where the user sits; laptop pushes everywhere.
        // Phones never appear in the presence table — that distinguishes them.
        // Edge case: a laptop that hasn't announced presence yet looks the same;
        // acceptable — its clipboard would just wait for the election.
        let phone_origin = !ctx.shared.presence.lock().unwrap().contains_key(&origin);
        if phone_origin && !ctx.shared.is_active() {
            debug!("clipboard: ignoring push from {from} — not the active node");
            continue;
        }
        last = Some(hash);
        debug!(
            "clipboard: apply {mime} from {from} (origin {origin}) — TODO: set via data-control"
        );
    }
    Ok(())
}
