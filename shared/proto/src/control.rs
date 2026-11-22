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

    /// Authed device → @server: "I issued an invite for this session —
    /// forward any `PairHello` arriving from it to me". Lets a joiner
    /// reach the inviter without knowing its device id (short-code path).
    PairOpen {
        session: DeviceId,
    },
    /// Joiner → @server (forwarded to the inviter). `from` is the session
    /// id the joiner claimed as its conn id; the inviter replies with
    /// `to = session`.
    PairHello {
        pubkey_b64: String,
        /// Name the new device wants to be registered under.
        name: String,
    },
    /// Inviter → joiner (`to = session`). After this both sides have both
    /// pubkeys and can compute DH + SAS for human verification.
    PairAccept {
        pubkey_b64: String,
    },
    /// Joiner → @server, bootstrap flow only: "the SAS matched the relay
    /// log — hand me credentials". Normal pairing doesn't use this; the
    /// inviter decides.
    PairConfirm,
    /// Inviter → joiner, only after SAS confirmation. `blob_b64` is
    /// XChaCha20-Poly1305 under `FamilyKey::from_raw(dh)` of a [`PairBlob`].
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
    /// Authed device → @server. Rename a registry entry — only self-
    /// renames are honoured (`device_id` must equal the sender's id).
    RenameDevice {
        device_id: DeviceId,
        name: String,
    },
    /// Authed device → @server.
    ListDevices,
    /// Server → device.
    Devices {
        devices: Vec<DeviceInfo>,
        /// Family name, when the admin gave the relay one
        /// (`--family-name`). Older relays omit it.
        #[serde(default)]
        family_name: Option<String>,
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
    /// Server-side WS ping round-trip, millis — the per-device latency
    /// the dashboards display. `None` while offline or unmeasured.
    #[serde(default)]
    pub rtt_ms: Option<u64>,
}

/// The DH-sealed payload inside `Control::PairData`: everything the new
/// device needs to join the family.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PairBlob {
    pub device_id: DeviceId,
    pub token_hex: String,
    /// `None` in the bootstrap flow — the founder generates the family
    /// key locally; the relay must never see it.
    #[serde(default)]
    pub family_key_b64: Option<String>,
    /// `None` in the bootstrap flow — the joiner keeps the URL it
    /// connected to; the relay doesn't know its own public name.
    #[serde(default)]
    pub server_url: Option<String>,
    /// Rotation epoch of `family_key_b64` — the joiner starts there so
    /// it never accepts an older rotation. Absent on bootstrap (0).
    #[serde(default)]
    pub key_epoch: u64,
}
