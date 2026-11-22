//! Family-key rotation that a revoked device can't follow.
//!
//! Every device holds a long-term X25519 *identity* key and announces the
//! public half ([`crate::Message::Identity`]) under the family key; peers
//! pin the first one they see (TOFU). A rotation seals the new family key
//! once per remaining member under `KDF(DH(rotator, member))` — the relay
//! can't open it (no identity secrets), and neither can the revoked
//! device, even if the relay hands it every frame: it only ever had the
//! old family key, never a member's identity secret.
//!
//! Sealed plaintext = `epoch (u64 BE) ‖ key (32)`, so an old entry can't
//! be replayed under a newer epoch.

use crate::{FamilyKey, PairKey, PublicKey};
use base64::Engine;
use sha2::Digest;

const DOMAIN: &[u8] = b"murmur/rekey/v1";

/// AEAD key for one rotator→member wrap.
fn wrap_key(dh: [u8; 32]) -> FamilyKey {
    let mut h = sha2::Sha256::new();
    h.update(DOMAIN);
    h.update(dh);
    FamilyKey(h.finalize().into())
}

/// Seal `new_key` at `epoch` for `member`, from `identity`.
pub fn seal_for(
    identity: &PairKey,
    member: &PublicKey,
    epoch: u64,
    new_key: &FamilyKey,
) -> anyhow::Result<String> {
    let mut pt = Vec::with_capacity(40);
    pt.extend_from_slice(&epoch.to_be_bytes());
    pt.extend_from_slice(&new_key.0);
    let ct = wrap_key(identity.dh(member)).seal(&pt)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(ct))
}

/// Open an entry sealed for us by `rotator`; checks the epoch binding.
pub fn open_from(
    identity: &PairKey,
    rotator: &PublicKey,
    epoch: u64,
    sealed_b64: &str,
) -> anyhow::Result<FamilyKey> {
    let ct = base64::engine::general_purpose::STANDARD.decode(sealed_b64)?;
    let pt = wrap_key(identity.dh(rotator)).open(&ct)?;
    anyhow::ensure!(pt.len() == 40, "bad rekey payload");
    anyhow::ensure!(pt[..8] == epoch.to_be_bytes(), "rekey epoch mismatch");
    Ok(FamilyKey(pt[8..].try_into().unwrap()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_isolation() {
        let rot = PairKey::generate();
        let member = PairKey::generate();
        let outsider = PairKey::generate();
        let k = FamilyKey::random();
        let s = seal_for(&rot, &member.public, 7, &k).unwrap();
        assert_eq!(open_from(&member, &rot.public, 7, &s).unwrap().0, k.0);
        assert!(open_from(&member, &rot.public, 8, &s).is_err());
        assert!(open_from(&outsider, &rot.public, 7, &s).is_err());
    }

    #[test]
    fn identity_secret_roundtrip() {
        let a = PairKey::generate();
        let b = PairKey::from_secret(a.secret_bytes());
        assert_eq!(a.public.as_bytes(), b.public.as_bytes());
    }
}
