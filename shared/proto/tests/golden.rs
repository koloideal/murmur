//! Golden wire vectors: every Message variant serialized exactly once,
//! frozen in `vectors.json`. The Rust test asserts serialization matches
//! the frozen bytes (wire drift = test failure); the Kotlin side parses
//! the same file in its JVM tests — the two implementations cannot
//! diverge without one of them going red.

use murmur_proto::*;
use serde_json::json;

fn id(b: u8) -> DeviceId {
    DeviceId([b; 8])
}

/// Canonical sample of every variant — stable field values, no timestamps
/// derived from the clock.
fn samples() -> Vec<Message> {
    vec![
        Message::Presence { state: PresenceState::Active, ts: 1_700_000_000 },
        Message::Presence { state: PresenceState::Locked, ts: 1_700_000_001 },
        Message::Notification {
            key: "n1".into(), pkg: "org.telegram".into(),
            app: Some("Telegram".into()),
            title: "hi".into(), body: "there".into(),
            icon_b64: Some("aGk=".into()), actions: vec![], ts: 1_700_000_002,
        },
        Message::NotificationDismiss { key: "n1".into() },
        Message::MediaState {
            player: "spotify".into(), title: "Song".into(), artist: "Band".into(),
            album: "LP".into(), status: PlayStatus::Playing,
            pos_ms: 42_000, dur_ms: 200_000, art_b64: None,
            shuffle: Some(true), loop_status: Some(LoopMode::Track),
            ts: 1_700_000_003,
        },
        Message::MediaCommand { op: MediaOp::Play },
        Message::MediaCommand { op: MediaOp::Seek { pos_ms: 10_000 } },
        Message::Clipboard {
            origin: id(1), hash: 0xdead_beef, mime: "text/plain".into(),
            data_b64: "aGVsbG8=".into(), explicit: true, ts: 1_700_000_004,
        },
        Message::Identity {
            pub_b64: "cHVi".into(), kind: DeviceKind::Phone,
            name: Some("pixel".into()), caps: vec!["clipboard".into(), "media".into()],
        },
        Message::LanBeacon { tcp_port: 17894, kind: DeviceKind::Laptop, lan_addr: Some("192.168.1.5".into()) },
        Message::LanPing { ts: 1_700_000_005 },
        Message::LanPong { ts: 1_700_000_005 },
        Message::KeyRotate {
            epoch: 3, revoked: vec![id(9)],
            keys: vec![SealedKey { device_id: id(2), sealed_b64: "c2VhbGVk".into() }],
        },
    ]
}

#[test]
fn serialization_matches_frozen_vectors() {
    let frozen: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/vectors.json"))
            .expect("tests/vectors.json missing — regenerate with the print test"),
    )
    .unwrap();
    let frozen = frozen.as_array().unwrap();
    let samples = samples();
    assert_eq!(frozen.len(), samples.len(), "vector count drifted");
    for (msg, want) in samples.iter().zip(frozen) {
        let got = serde_json::to_value(msg).unwrap();
        assert_eq!(&got, want, "wire drift on {}", msg.tag());
    }
}

#[test]
fn roundtrip_and_tag_consistency() {
    for msg in samples() {
        let s = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&s).unwrap();
        assert_eq!(back.tag(), msg.tag(), "tag unstable for {s}");
        // the wire tag field equals Message::tag() — dispatch depends on it
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["t"], json!(msg.tag()), "serde tag ≠ Message::tag for {s}");
    }
}

/// `cargo test print_vectors -- --nocapture` — regenerate vectors.json
/// ONLY when the wire format intentionally changes.
#[test]
fn print_vectors() {
    let arr: Vec<_> = samples().iter().map(|m| serde_json::to_value(m).unwrap()).collect();
    println!("{}", serde_json::to_string_pretty(&arr).unwrap());
}
