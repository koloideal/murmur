use crate::DeviceId;
use serde::{Deserialize, Serialize};

/// Cleartext control-plane. Travels as Envelope.payload in two cases:
/// - `to == Route::Server` — server handles it directly;
/// - between devices *during pairing only* (the new device has no family key
///   yet). PairData carries its own DH-sealed blob inside.
///
/// `Hello` is also sent as the first raw WS message, before auth.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "c", rename_all = "snake_case")]
pub enum Control {
    /// First message after WS connect. `role = auth` requires `token_hex`;
    /// `role = pair` registers a pending pairing connection.
    Hello {
        device_id: DeviceId,
        name: String,
        role: HelloRole,
        token_hex: Option<String>,
    },
    HelloOk,
    HelloErr {
        reason: String,
    },

    /// Pairing handshake. New device → "*" (relayed to authed conns only).
    PairHello {
        pubkey_b64: String,
    },
    /// Authed device → new device. After this both sides can compute DH+SAS.
    PairAccept {
        pubkey_b64: String,
    },
    /// Either direction, only within a pairing session. `blob_b64` is
    /// XChaCha20-Poly1305 under `FamilyKey::from_raw(dh)` of a JSON blob
    /// `{auth_token_hex, family_key_b64}` sent by the existing device.
    PairData {
        blob_b64: String,
    },

    /// Authed device → @server. Add a device to the registry (pairing result).
    RegisterDevice {
        device_id: DeviceId,
        name: String,
        token_hash_hex: String,
    },
    /// Authed device → @server. Kick a device; its live conn is dropped.
    RevokeDevice {
        device_id: DeviceId,
    },
    /// Authed device → @server.
    ListDevices,
    /// Server → device.
    Devices {
        devices: Vec<DeviceInfo>,
    },

    /// Server → device: generic control error.
    Error {
        reason: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HelloRole {
    Auth,
    Pair,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub device_id: DeviceId,
    pub name: String,
    pub revoked: bool,
    pub online: bool,
}
