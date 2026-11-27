//! Pairing crypto path — the exact constructions murmurd's pair.rs
//! uses, tested at the layer where correctness lives. A MITM-relay
//! attack on pairing is the one exploit that matters here: everything
//! downstream (family key secrecy) inherits from this handshake.

use base64::Engine;
use murmur_proto::control::PairBlob;
use murmur_proto::{DeviceId, FamilyKey, FamilyKind, Invite, PairKey, sas_code};

/// The inviter seals with FamilyKey::from_raw(dh) exactly like
/// pair_confirm; the joiner opens exactly like join — both sides must
/// derive the same DH or the channel never decrypts.
#[test]
fn dh_channel_roundtrip() {
    let inviter = PairKey::generate();
    let joiner = PairKey::generate();

    // symmetry is THE invariant: both compute shared secrets over the
    // same curve point, from opposite ends
    let dh_i = inviter.dh(&joiner.public);
    let dh_j = joiner.dh(&inviter.public);
    assert_eq!(dh_i, dh_j, "DH outputs must agree — the pairing channel is born here");

    // the exact seal/open construction from pair.rs
    let blob = PairBlob {
        device_id: DeviceId::random(),
        token_hex: "deadbeef".into(),
        family_key_b64: Some(base64::engine::general_purpose::STANDARD.encode([7u8; 32])),
        server_url: Some("wss://example".into()),
        key_epoch: 3,
        family_name: Some("testfam".into()),
        kind: FamilyKind::Serverful,
    };
    let sealed = FamilyKey::from_raw(dh_i).seal(&serde_json::to_vec(&blob).unwrap()).unwrap();
    let opened = FamilyKey::from_raw(dh_j)
        .open(&sealed)
        .expect("joiner opens the inviter's blob");
    let back: PairBlob = serde_json::from_slice(&opened).unwrap();
    assert_eq!(back.token_hex, "deadbeef");
    assert_eq!(back.key_epoch, 3);
}

/// A MITM relaying the handshake sees/creates different pubkeys on
/// both sides — the SAS codes diverge, which is the entire point of
/// showing them to a human.
#[test]
fn sas_detects_mitm() {
    let inviter = PairKey::generate();
    let joiner = PairKey::generate();
    let mitm = PairKey::generate();
    let inviter_id = DeviceId::random();
    let session = DeviceId::random();

    // honest path: symmetric inputs → identical codes either way round
    let a = sas_code(&inviter.public, &joiner.public, &inviter_id, &session);
    let b = sas_code(&joiner.public, &inviter.public, &session, &inviter_id);
    assert_eq!(a, b, "SAS is symmetric by construction");
    assert_eq!(a.len(), 6);

    // MITM path: the joiner sees the attacker's pubkey — its code
    // differs from the inviter's, and the humans hang up
    let attacked = sas_code(&mitm.public, &joiner.public, &inviter_id, &session);
    assert_ne!(a, attacked, "a swapped pubkey MUST change the displayed code");
}

/// A MITM can't open the sealed blob either: its DH with the joiner
/// is a different key than the inviter's DH with the joiner.
#[test]
fn mitm_cannot_open_blob() {
    let inviter = PairKey::generate();
    let joiner = PairKey::generate();
    let mitm = PairKey::generate();

    let dh_honest = inviter.dh(&joiner.public);
    let blob_json = br#"{"device_id":"x"}"#;
    let sealed = FamilyKey::from_raw(dh_honest).seal(blob_json).unwrap();

    // attacker relaying the joiner's side shares a secret with the
    // joiner — but NOT the one the inviter sealed with
    let dh_mitm = mitm.dh(&joiner.public);
    assert!(
        FamilyKey::from_raw(dh_mitm).open(&sealed).is_err(),
        "wrong DH never opens the credentials"
    );
}

/// Invite encoding roundtrips, and the typed short code always lands
/// on the same session the QR bundle carries — the phone scans, the
/// laptop types, both arrive at one rendezvous.
#[test]
fn invite_roundtrip_and_shortcode_consistency() {
    let inviter = DeviceId::random();
    let inv = Invite::new("wss://relay.example".into(), inviter, FamilyKind::Serverful);
    let decoded = Invite::decode(&inv.encode()).expect("encode → decode roundtrip");
    assert_eq!(decoded.session, inv.session);
    assert_eq!(decoded.server_url, inv.server_url);
    assert_eq!(decoded.kind, FamilyKind::Serverful);

    // the human-typeable code inside the bundle derives the same
    // session — this is what makes short codes resolvable at all
    assert_eq!(
        DeviceId::from_code(&inv.code),
        inv.session,
        "typed code and QR bundle must meet at one session"
    );
}

/// DH depends on BOTH halves — inviter mixing up joiners (two pairing
/// sessions crossing) produces no shared secret and no leak.
#[test]
fn crossed_sessions_never_decrypt() {
    let inviter = PairKey::generate();
    let joiner_a = PairKey::generate();
    let joiner_b = PairKey::generate();

    let sealed = FamilyKey::from_raw(inviter.dh(&joiner_a.public))
        .seal(b"credentials for A")
        .unwrap();
    // joiner B's view of the world uses ITS keypair — wrong channel
    let wrong = joiner_b.dh(&inviter.public);
    assert!(FamilyKey::from_raw(wrong).open(&sealed).is_err());
}
