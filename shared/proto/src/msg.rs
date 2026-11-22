use crate::DeviceId;
use serde::{Deserialize, Serialize};

/// E2E payload — always inside the sealed Envelope.payload.
/// Serde enum tagged by `"t"`. New features = new variants.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Message {
    /// Node presence broadcast. Everyone keeps a {device → (state, ts)} map
    /// and independently computes the active node: latest `active` wins,
    /// ties broken by smaller DeviceId.
    Presence { state: PresenceState, ts: u64 },

    /// Phone → all laptops.
    Notification {
        key: String,
        pkg: String,
        /// Human app name ("Telegram") — the laptop's notification
        /// daemon groups and labels by it. Older phones omit it.
        #[serde(default)]
        app: Option<String>,
        title: String,
        body: String,
        icon_b64: Option<String>,
        ts: u64,
    },

    /// Any → all. Dismissed locally somewhere → dismissed everywhere,
    /// including on the phone.
    NotificationDismiss { key: String },

    /// Phone → all laptops. Sent on metadata/status changes, on seeks and
    /// ~every 20s while playing; laptops extrapolate position between updates.
    MediaState {
        player: String,
        title: String,
        artist: String,
        album: String,
        status: PlayStatus,
        pos_ms: u64,
        dur_ms: u64,
        art_b64: Option<String>,
        ts: u64,
    },

    /// Laptop (any, via MPRIS) → phone. `op` is flattened so the wire is
    /// `{"t":"media_command","op":"play"}` / `{"op":"seek","pos_ms":N}`
    /// — flat fields the Android side can model as `op` + `pos_ms?`.
    MediaCommand {
        #[serde(flatten)]
        op: MediaOp,
    },

    /// Clipboard contents. `origin` = device that produced the data (for
    /// loop diagnostics), `hash` = FNV/xx of plaintext for dedup & anti-echo.
    /// `explicit` = pushed by a deliberate user action (phone share /
    /// process-text) rather than passive sync — laptops apply such clips
    /// only when they're the active node. Laptops broadcast with
    /// `explicit=false` (peer sync); the phone applies only clips that
    /// came from the active node.
    Clipboard {
        origin: DeviceId,
        hash: u64,
        mime: String,
        data_b64: String,
        explicit: bool,
        ts: u64,
    },

    /// Authed device → "*" after every (re)connect: its long-term X25519
    /// identity pubkey. Peers pin the first key per device (TOFU), and
    /// only from frames that opened under the *current* family key.
    Identity { pub_b64: String, kind: DeviceKind },

    /// New family key after a revocation (or `murmurctl rotate`), sealed
    /// once per remaining member to its identity key — see
    /// [`crate::rekey`]. `revoked` are dropped from everyone's roster.
    /// Also re-sent unicast to a member caught talking on an old key.
    KeyRotate {
        epoch: u64,
        #[serde(default)]
        revoked: Vec<DeviceId>,
        keys: Vec<SealedKey>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SealedKey {
    pub device_id: DeviceId,
    pub sealed_b64: String,
}

/// What a device is — rotation is only ever run by laptops; the phone
/// stays a thin agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    Laptop,
    Phone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresenceState {
    Active,
    Idle,
    Locked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayStatus {
    Playing,
    Paused,
    Stopped,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum MediaOp {
    Play,
    Pause,
    Next,
    Previous,
    Seek { pos_ms: u64 },
}
