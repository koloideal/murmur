//! Property tests: `Envelope::from_json` is the choke point every
//! transport feeds — relay WS, client WS, LAN links, UDP beacons. It
//! must never panic, never accept an over-cap frame, and round-trip
//! everything `seal` produces.

use murmur_proto::{
    DeviceId, Envelope, FamilyKey, MAX_ENVELOPE_JSON, MAX_PAYLOAD, Message, Route,
};
use proptest::prelude::*;

proptest! {
    /// Arbitrary garbage: parse or reject — a malformed frame must
    /// never take the process down.
    #[test]
    fn from_json_never_panics_on_garbage(bytes in proptest::collection::vec(any::<u8>(), 0..8192)) {
        let s = String::from_utf8_lossy(&bytes);
        let _ = Envelope::from_json(&s);
    }

    /// Mutated *valid* envelopes: corruption lands in field values, not
    /// just syntax — flipped bytes inside the b64 payload, truncated
    /// tails, a swapped seq. Parse or reject, still never panic.
    #[test]
    fn from_json_never_panics_on_mutations(
        seq in any::<u64>(),
        ts in any::<u64>(),
        edits in proptest::collection::vec((any::<usize>(), any::<u8>()), 1..32),
    ) {
        let key = FamilyKey::random();
        let env = Envelope::seal(
            DeviceId::random(),
            Route::All,
            seq,
            &key,
            &Message::LanPing { ts },
        )
        .unwrap();
        let mut json = env.to_json().into_bytes();
        for (pos, b) in edits {
            if json.is_empty() {
                break;
            }
            let idx = pos % json.len();
            json[idx] = b;
        }
        let s = String::from_utf8_lossy(&json);
        let _ = Envelope::from_json(&s);
    }

    /// Anything past the wire cap is rejected before serde ever sees it.
    #[test]
    fn over_cap_json_rejected(extra in 1usize..4096) {
        let s = "x".repeat(MAX_ENVELOPE_JSON + extra);
        prop_assert!(Envelope::from_json(&s).is_err());
    }

    /// The happy path: seal → json → parse → open returns our message.
    #[test]
    fn seal_open_roundtrip(seq in any::<u64>(), ts in any::<u64>()) {
        let key = FamilyKey::random();
        let env = Envelope::seal(
            DeviceId::random(),
            Route::All,
            seq,
            &key,
            &Message::LanPing { ts },
        )
        .unwrap();
        let parsed = Envelope::from_json(&env.to_json()).unwrap();
        let ok = matches!(parsed.open(&key), Ok(Message::LanPing { ts: t }) if t == ts);
        prop_assert!(ok);
    }
}

// Fewer cases — each iteration serializes a multi-MB message.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]

    /// Over-cap *payloads* refuse to seal — a feature that overfills a
    /// message gets an error instead of pushing a giant frame.
    #[test]
    fn over_cap_payload_refuses_to_seal(extra in 1usize..8192) {
        let key = FamilyKey::random();
        let msg = Message::Clipboard {
            origin: DeviceId::random(),
            hash: 0,
            mime: "text/plain".into(),
            data_b64: "A".repeat(MAX_PAYLOAD + extra),
            explicit: false,
            ts: 0,
        };
        prop_assert!(Envelope::seal(DeviceId::random(), Route::All, 1, &key, &msg).is_err());
    }
}
