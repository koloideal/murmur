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
        title: String,
        body: String,
        icon_b64: Option<String>,
        ts: u64,
    },

    /// Any → all. Dismissed locally somewhere → dismissed everywhere,
    /// including on the phone.
    NotificationDismiss { key: String },

    /// Phone → all laptops. Sent on metadata/status changes, on seeks and
    /// ~every 5s while playing; laptops extrapolate position between updates.
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

    /// Laptop (any, via MPRIS) → phone.
    MediaCommand { op: MediaOp },

    /// Clipboard contents. `origin` = device that produced the data (for
    /// loop diagnostics), `hash` = FNV/xx of plaintext for dedup & anti-echo.
    /// Routing rules: laptops broadcast to "*"; only the active laptop also
    /// targets the phone. Phone pushes to "*" but only the active laptop applies.
    Clipboard {
        origin: DeviceId,
        hash: u64,
        mime: String,
        data_b64: String,
        ts: u64,
    },

    /// Authed device → "*". New family key after a device was revoked.
    /// Everyone switches immediately; the revoked device can't read this
    /// because it doesn't have the new key — it only sees ciphertext.
    KeyRotate { new_key_b64: String },
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
