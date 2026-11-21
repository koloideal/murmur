use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use rand::RngCore;
use sha2::Digest;
use thiserror::Error;
use x25519_dalek::{PublicKey, StaticSecret};

const NONCE_LEN: usize = 24;

#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("ciphertext too short")]
    TooShort,
    #[error("decryption failed")]
    Decrypt,
    #[error("encryption failed")]
    Encrypt,
}

/// Shared symmetric key for the whole device family. 32 bytes.
/// Distributed to a new device over the DH channel during pairing.
#[derive(Clone)]
pub struct FamilyKey(pub [u8; 32]);

impl FamilyKey {
    pub fn random() -> Self {
        let mut b = [0u8; 32];
        rand::rng().fill_bytes(&mut b);
        Self(b)
    }

    /// Sealed layout: `nonce(24) || ciphertext`.
    pub fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let cipher = XChaCha20Poly1305::new(Key::from_slice(&self.0));
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let ct = cipher
            .encrypt(&nonce, plaintext)
            .map_err(|_| CryptoError::Encrypt)?;
        let mut out = Vec::with_capacity(nonce.len() + ct.len());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ct);
        Ok(out)
    }

    pub fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, CryptoError> {
        if sealed.len() < NONCE_LEN {
            return Err(CryptoError::TooShort);
        }
        let (nonce, ct) = sealed.split_at(NONCE_LEN);
        let cipher = XChaCha20Poly1305::new(Key::from_slice(&self.0));
        cipher
            .decrypt(XNonce::from_slice(nonce), ct)
            .map_err(|_| CryptoError::Decrypt)
    }

    /// Same AEAD primitive reused for the pairing DH channel.
    pub fn from_raw(key: [u8; 32]) -> Self {
        Self(key)
    }
}

/// Ephemeral X25519 keypair used in the pairing handshake.
pub struct PairKey {
    secret: StaticSecret,
    pub public: PublicKey,
}

impl PairKey {
    pub fn generate() -> Self {
        let secret = StaticSecret::random_from_rng(OsRng);
        let public = PublicKey::from(&secret);
        Self { secret, public }
    }

    /// DH result used both as SAS input and as AEAD key for PairData.
    pub fn dh(&self, peer: &PublicKey) -> [u8; 32] {
        self.secret.diffie_hellman(peer).to_bytes()
    }

    pub fn public_b64(&self) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(self.public.as_bytes())
    }

    pub fn public_from_b64(s: &str) -> anyhow::Result<PublicKey> {
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD.decode(s)?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("pubkey must be 32 bytes"))?;
        Ok(PublicKey::from(arr))
    }
}

/// Short authentication string shown on both devices during pairing.
/// Six digits from `SHA-256(pkA || pkB || idA || idB)`. A MITM relaying the
/// handshake produces different pubkeys on the two sides → different codes.
pub fn sas_code(
    own_pub: &PublicKey,
    peer_pub: &PublicKey,
    own_id: &crate::DeviceId,
    peer_id: &crate::DeviceId,
) -> String {
    let mut h = sha2::Sha256::new();
    h.update(own_pub.as_bytes());
    h.update(peer_pub.as_bytes());
    h.update(own_id.0);
    h.update(peer_id.0);
    let digest = h.finalize();
    let n = u32::from_be_bytes(digest[0..4].try_into().unwrap()) % 1_000_000;
    format!("{n:06}")
}
