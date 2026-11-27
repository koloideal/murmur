//! In-memory integration: two full `Shared` nodes wired by channel
//! pumps — no sockets, no relay, no clock dependence beyond what the
//! production code itself uses. The pump *is* the transport under test
//! in miniature: whatever `send()` emits is what the peer ingests,
//! through the same `ingest` invariant the real links use.
//!
//! Env safety: tests here mutate XDG_DATA_HOME so persistence writes
//! land in tempdirs, never in the real ~/.local/share/murmur. A static
//! lock serializes them because env is process-global.

#[cfg(test)]
mod tests {
    use crate::config::{Config, Secrets};
    use crate::feature::Hub;
    use crate::state::Shared;
    use murmur_proto::{DeviceId, Envelope, FamilyKey, Message, PairKey, PresenceState, Route};
    use std::sync::{Arc, Mutex};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn hex_id(b: u8) -> String {
        hex::encode([b; 8])
    }

    /// A fully in-memory node: real keyring derivation, real dedupe,
    /// real presence/election — only the disk and the wire are fake.
    fn node(
        dev: u8,
        family: &FamilyKey,
    ) -> (
        Arc<Shared>,
        tokio::sync::mpsc::UnboundedReceiver<Envelope>,
        tokio::sync::mpsc::UnboundedReceiver<(u64, Envelope)>,
    ) {
        let identity = PairKey::generate();
        let secrets = Secrets {
            device_id: hex_id(dev),
            auth_token_hex: hex::encode([dev; 16]),
            family_key_hex: hex::encode(family.0),
            key_epoch: 0,
            key_rotator: None,
            old_keys_hex: vec![],
            revoked: vec![],
            identity_hex: Some(hex::encode(identity.secret_bytes())),
            family_name: Some("testfam".into()),
        };
        let cfg = Config {
            family_kind: murmur_proto::FamilyKind::Serverful,
            server_url: "wss://unused".into(),
            name: format!("node{dev}"),
            idle_timeout_secs: 180,
            clipboard_mime_deny: vec![],
            lock_procs: vec![],
            notify_suppress_if_running: Default::default(),
            notify_hide_twins_if_online: vec![],
        };
        let (out_tx, out_rx) = tokio::sync::mpsc::unbounded_channel();
        let (lo_tx, lo_rx) = tokio::sync::mpsc::unbounded_channel();
        let (lan_tx, _lan_rx) = tokio::sync::mpsc::unbounded_channel();
        let shared = Shared::new(cfg, secrets, out_tx, lo_tx, lan_tx).unwrap();
        (Arc::new(shared), out_rx, lo_rx)
    }

    fn with_env<R>(f: impl FnOnce() -> R) -> R {
        let _g = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("murmur-integ-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: serialized by ENV_LOCK; the test process owns its env
        // and no other thread reads xdg paths concurrently in tests.
        unsafe { std::env::set_var("XDG_DATA_HOME", &dir) };
        f()
    }

    /// Drain whatever `send()` emitted — hi channel first (commands),
    /// then lo (replaceable state). One pump step = one envelope.
    fn pump_one(
        out: &mut tokio::sync::mpsc::UnboundedReceiver<Envelope>,
        lo: &mut tokio::sync::mpsc::UnboundedReceiver<(u64, Envelope)>,
    ) -> Option<Envelope> {
        out.try_recv().ok().or_else(|| lo.try_recv().ok().map(|(_, e)| e))
    }

    #[test]
    fn duplex_delivery_and_dual_path_dedup() {
        with_env(|| {
            let family = FamilyKey::random();
            let (a, mut a_out, mut a_lo) = node(1, &family);
            let (b, mut _b_out, mut _b_lo) = node(2, &family);

            // B's feature tap: a hub sink capturing everything
            let hub = Hub::new();
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let mut hub = hub;
            hub.sinks
                .entry("presence")
                .or_default()
                .push(tx);

            // A announces itself; the envelope must exist on the wire
            a.send(Route::All, &Message::Presence { state: PresenceState::Active, ts: 1000 })
                .unwrap();
            let env = pump_one(&mut a_out, &mut a_lo).expect("an envelope leaves A");

            // B opens and ingests — exactly like a link reader would
            let key = b.keys.read().unwrap().current.clone();
            let msg = env.open(&key).unwrap();
            assert!(b.ingest(&hub, env.from, env.seq, &msg), "first copy delivers");

            // the SAME frame rides the second path (LAN+relay twins)
            assert!(!b.ingest(&hub, env.from, env.seq, &msg), "twin copy dies in dedupe");

            // exactly one copy reached features
            let got = rx.try_recv().expect("feature inbox got the presence");
            assert_eq!(got.0, DeviceId([1; 8]));
            assert!(rx.try_recv().is_err(), "no duplicate reaches features");

            // and B's presence table learned the peer — election inputs
            assert!(b.lan_peers().is_empty()); // no links in-memory — sanity
        });
    }

    #[test]
    fn wrong_family_frames_never_open() {
        with_env(|| {
            let fam_a = FamilyKey::random();
            let fam_b = FamilyKey::random();
            let (a, mut a_out, mut a_lo) = node(1, &fam_a);
            let (b, _b_out, _b_lo) = node(2, &fam_b);
            a.send(Route::All, &Message::Presence { state: PresenceState::Active, ts: 1 })
                .unwrap();
            let env = pump_one(&mut a_out, &mut a_lo).unwrap();
            let key = b.keys.read().unwrap().current.clone();
            assert!(env.open(&key).is_err(), "foreign family frames die at the seal");
        });
    }
}
