//! Crypto and rotation integration at the proto level: envelope
//! seal/open roundtrips, wrong-key rejection, and the rekey path with
//! its epoch binding — the mechanics the family-key rotation E2E
//! depends on, without needing any network.

use murmur_proto::*;

fn id(b: u8) -> DeviceId { DeviceId([b; 8]) }

fn sample() -> Message {
    Message::Clipboard {
        origin: id(1), hash: 42, mime: "text/plain".into(),
        data_b64: "eA==".into(), explicit: false, ts: 1_700_000_000,
    }
}

#[test]
fn envelope_roundtrip_and_json_survival() {
    let key = FamilyKey::random();
    let env = Envelope::seal(id(1), Route::All, 7, &key, &sample()).unwrap();
    // the wire path is JSON — every byte of the envelope survives it
    let wire = env.to_json();
    let back = Envelope::from_json(&wire).unwrap();
    let msg = back.open(&key).unwrap();
    assert_eq!(msg.tag(), "clipboard");
}

#[test]
fn wrong_key_cannot_open() {
    let key = FamilyKey::random();
    let other = FamilyKey::random();
    let env = Envelope::seal(id(1), Route::All, 7, &key, &sample()).unwrap();
    assert!(env.open(&other).is_err(), "wrong family key must never open");
}

#[test]
fn rekey_roundtrip_and_epoch_binding() {
    let rotator = PairKey::generate();
    let member = PairKey::generate();
    let new_key = FamilyKey::random();

    let sealed = rekey::seal_for(&rotator, &member.public, 5, &new_key).unwrap();
    let opened = rekey::open_from(&member, &rotator.public, 5, &sealed).unwrap();
    assert_eq!(opened.0, new_key.0, "member must recover the exact family key");

    // epoch is bound into the payload — replaying an old rotation at a
    // newer epoch must fail, or a MitM could roll the family backwards
    assert!(rekey::open_from(&member, &rotator.public, 6, &sealed).is_err());
}

#[test]
fn rekey_wrong_recipient_cannot_open() {
    let rotator = PairKey::generate();
    let member = PairKey::generate();
    let bystander = PairKey::generate();
    let new_key = FamilyKey::random();

    let sealed = rekey::seal_for(&rotator, &member.public, 5, &new_key).unwrap();
    // the revoked/eavesdropping device gets nothing from the sealed entry
    assert!(rekey::open_from(&bystander, &rotator.public, 5, &sealed).is_err());
}
