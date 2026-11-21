use rand::RngCore;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// 8-byte random device identifier, shown/serialized as hex.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceId(pub [u8; 8]);

impl DeviceId {
    pub fn random() -> Self {
        let mut b = [0u8; 8];
        rand::rng().fill_bytes(&mut b);
        Self(b)
    }
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
