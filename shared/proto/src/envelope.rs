use crate::{DeviceId, FamilyKey, Message, PROTO_VERSION, SERVER_ROUTE};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Where a frame goes. Serialized inside the `to` field:
/// `"*"` = all live family devices except sender, `"@server"` = control-plane,
/// `"<device-hex>"` = a single device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    All,
    Server,
    Device(DeviceId),
}

impl Route {
    pub fn device(id: DeviceId) -> Self {
        Self::Device(id)
    }
}

impl Serialize for Route {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Route::All => s.serialize_str("*"),
            Route::Server => s.serialize_str(SERVER_ROUTE),
            Route::Device(id) => s.serialize_str(&id.to_string()),
        }
    }
}

impl<'de> Deserialize<'de> for Route {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        match s.as_str() {
            "*" => Ok(Route::All),
            SERVER_ROUTE => Ok(Route::Server),
            _ => s
                .parse()
                .map(Route::Device)
                .map_err(serde::de::Error::custom),
        }
    }
}

mod b64 {
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        base64::engine::general_purpose::STANDARD
            .decode(&s)
            .map_err(serde::de::Error::custom)
    }
}

/// The only thing the server ever sees. `payload` is either
/// - a cleartext JSON [`crate::Control`] when `to == Route::Server` or during
///   pairing, or
/// - `FamilyKey::seal(serde_json(Message))` for all device traffic.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Envelope {
    pub v: u8,
    pub from: DeviceId,
    pub to: Route,
    pub seq: u64,
    #[serde(with = "b64")]
    pub payload: Vec<u8>,
}

impl Envelope {
    /// Wrap and encrypt an E2E message for `to`.
    pub fn seal(
        from: DeviceId,
        to: Route,
        seq: u64,
        key: &FamilyKey,
        msg: &Message,
    ) -> anyhow::Result<Self> {
        let pt = serde_json::to_vec(msg)?;
        Ok(Self {
            v: PROTO_VERSION,
            from,
            to,
            seq,
            payload: key.seal(&pt)?,
        })
    }

    /// Wrap a cleartext control message.
    pub fn control(
        from: DeviceId,
        to: Route,
        seq: u64,
        ctl: &crate::Control,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            v: PROTO_VERSION,
            from,
            to,
            seq,
            payload: serde_json::to_vec(ctl)?,
        })
    }

    /// Decrypt payload into an E2E message.
    pub fn open(&self, key: &FamilyKey) -> anyhow::Result<Message> {
        let pt = key.open(&self.payload)?;
        Ok(serde_json::from_slice(&pt)?)
    }

    /// Parse payload as a cleartext control message.
    pub fn control_payload(&self) -> anyhow::Result<crate::Control> {
        Ok(serde_json::from_slice(&self.payload)?)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("envelope is serializable")
    }

    pub fn from_json(s: &str) -> anyhow::Result<Self> {
        Ok(serde_json::from_str(s)?)
    }
}
