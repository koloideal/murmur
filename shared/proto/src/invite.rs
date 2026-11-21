//! Invite bundle: what a new device needs to start pairing — where the
//! server is and which device invited it. Encoded as a single opaque
//! string (base64url of JSON) that travels over a QR code, a paste, or a
//! `murmurd pair --code` argument. Contains no secrets: worst case someone
//! shows up as an extra pending pairing that gets SAS-rejected.

use crate::DeviceId;
use anyhow::Context;
use base64::Engine;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Invite {
    /// Server the joiner should connect to.
    #[serde(rename = "s")]
    pub server_url: String,
    /// Device that issued the invite — target of `PairHello`.
    #[serde(rename = "i")]
    pub inviter: DeviceId,
    /// Session id: the joiner claims it as its conn id (`role=pair`
    /// Hello's `device_id`), the inviter matches `PairHello`s against it.
    #[serde(rename = "p")]
    pub session: DeviceId,
}

impl Invite {
    pub fn new(server_url: String, inviter: DeviceId) -> Self {
        Self {
            server_url,
            inviter,
            session: DeviceId::random(),
        }
    }

    pub fn encode(&self) -> String {
        let json = serde_json::to_vec(self).expect("invite is serializable");
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json)
    }

    pub fn decode(s: &str) -> anyhow::Result<Self> {
        let json = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(s.trim())
            .context("invite code is not base64url")?;
        Ok(serde_json::from_slice(&json)?)
    }
}
