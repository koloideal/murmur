use rand::RngCore;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// 8-byte random device identifier, shown/serialized as hex.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceId(pub [u8; 8]);

impl DeviceId {
    /// Server-originated envelopes carry this as `from` — the relay has
    /// no device identity of its own.
    pub const ZERO: Self = Self([0; 8]);

    pub fn random() -> Self {
        let mut b = [0u8; 8];
        rand::rng().fill_bytes(&mut b);
        Self(b)
    }

    /// Pairing session id derived from a short typed code:
    /// `SHA-256(normalized code)[0..8]`. Lets a joiner claim the same
    /// session an inviter opened, knowing only the code.
    pub fn from_code(code: &str) -> Self {
        use sha2::Digest;
        let norm: String = code
            .chars()
            .filter(|c| *c != '-')
            .flat_map(|c| c.to_uppercase())
            .collect();
        let digest = sha2::Sha256::digest(norm.as_bytes());
        let mut b = [0u8; 8];
        b.copy_from_slice(&digest[..8]);
        Self(b)
    }
}

/// Crockford-ish alphabet — no 0/O/1/I/L confusions.
const CODE_ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";

/// A typeable pairing code, `XXXX-XXXX` (~40 bits).
pub fn pair_code() -> String {
    let mut b = [0u8; 8];
    rand::rng().fill_bytes(&mut b);
    let chars: Vec<char> = b
        .iter()
        .map(|v| CODE_ALPHABET[(*v % 31) as usize] as char)
        .collect();
    format!("{}{}{}{}-{}{}{}{}", chars[0], chars[1], chars[2], chars[3], chars[4], chars[5], chars[6], chars[7])
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(self.0))
    }
}

impl fmt::Debug for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeviceId({self})")
    }
}

impl FromStr for DeviceId {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = hex::decode(s)?;
        let arr: [u8; 8] = bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("device id must be 8 bytes"))?;
        Ok(Self(arr))
    }
}

impl Serialize for DeviceId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for DeviceId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Per-device auth token (32 bytes). The server stores only `Sha256(token)`.
#[derive(Clone)]
pub struct AuthToken(pub [u8; 32]);

impl AuthToken {
    pub fn random() -> Self {
        let mut b = [0u8; 32];
        rand::rng().fill_bytes(&mut b);
        Self(b)
    }

    /// Hex digest the server keeps in its registry.
    pub fn hash_hex(&self) -> String {
        use sha2::Digest;
        hex::encode(sha2::Sha256::digest(self.0))
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    pub fn from_hex(s: &str) -> anyhow::Result<Self> {
        let bytes = hex::decode(s)?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("auth token must be 32 bytes"))?;
        Ok(Self(arr))
    }
}
