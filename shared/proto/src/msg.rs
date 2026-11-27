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
        /// What the notification lets the user do, mirrored from the
        /// phone's action list: tapping the body fires `contentIntent`
        /// (always true — hence implicit), reply actions are the ones
        /// carrying a RemoteInput. Laptops render these as freedesktop
        /// actions; old phones send nothing and the features degrade
        /// to read-only mirroring.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        actions: Vec<NotifAction>,
        ts: u64,
    },

    /// Laptop → phone: the user clicked the notification body —
    /// fire its contentIntent exactly as a tap on the phone would.
    NotificationInvoke { key: String },

    /// Laptop → phone: fire a non-reply action button by its index
    /// in the original `actions` list.
    NotificationAction { key: String, index: u32 },

    /// Laptop → phone: inline reply text for the action that carries
    /// the RemoteInput. The phone fills the input and fires the same
    /// PendingIntent the shade's inline field would have.
    NotificationReply { key: String, text: String },

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
        /// Phone-reported shuffle state — `Some` when the media session
        /// exposes it (API 21+); laptops surface it via MPRIS `Shuffle`.
        #[serde(default)]
        shuffle: Option<bool>,
        /// Phone-reported repeat mode — `Some` when exposed; laptops
        /// surface it via MPRIS `LoopStatus`.
        #[serde(default)]
        loop_status: Option<LoopMode>,
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

    /// The clipboard was emptied on `origin` — user ran wl-copy --clear,
    /// the island's clear gesture, or the source app exited taking its
    /// unpersisted selection with it. Same gating as the content it
    /// removes: phone-origin clears apply only on the clip target,
    /// laptop-origin ones are peer sync and apply everywhere.
    ClipboardClear {
        origin: DeviceId,
        ts: u64,
    },

    /// Authed device → "*" after every (re)connect: its long-term X25519
    /// identity pubkey. Peers pin the first key per device (TOFU), and
    /// only from frames that opened under the *current* family key.
    /// `name` is the roster gossip the LAN-only (serverless) world
    /// needs — a relay family gets names from `Control::Devices`
    /// instead. Absent in old frames.
    Identity {
        pub_b64: String,
        kind: DeviceKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// Feature capabilities the sender speaks ("clipboard", "media",
        /// "notify", ...). Peers check before sending feature traffic;
        /// mixed-version families stop silently dropping messages.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        caps: Vec<String>,
    },

    /// LAN fast-path beacon: sealed envelope over UDP broadcast *and*
    /// periodically via the relay. Only a family member can seal a
    /// frame that opens — the payload carries just the sender's TCP
    /// listener port and LAN address. `kind` lets receivers skip
    /// dialing non-peers of interest (e.g. laptops dial phones only).
    /// `lan_addr` is required for the relay path — a relay-delivered
    /// beacon carries no source IP to dial back to.
    LanBeacon {
        tcp_port: u16,
        kind: DeviceKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lan_addr: Option<String>,
    },

    /// LAN-link keepalive: each side pings over the link; the peer
    /// echoes `LanPong` with the same timestamp — RTT is measured by
    /// the pinger on its own clock (no cross-device skew). Handled
    /// inside the transport, never dispatched to features.
    LanPing { ts: u64 },
    LanPong { ts: u64 },

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

impl Message {
    /// The serde `"t"` tag of this variant — the hub dispatches on it and
    /// features declare consumption in these terms. Compile-enforced:
    /// a new variant fails to compile until its tag is named here.
    pub fn tag(&self) -> &'static str {
        match self {
            Message::Presence { .. } => "presence",
            Message::Notification { .. } => "notification",
            Message::NotificationDismiss { .. } => "notification_dismiss",
            Message::NotificationInvoke { .. } => "notification_invoke",
            Message::NotificationAction { .. } => "notification_action",
            Message::NotificationReply { .. } => "notification_reply",
            Message::MediaState { .. } => "media_state",
            Message::MediaCommand { .. } => "media_command",
            Message::Clipboard { .. } => "clipboard",
            Message::ClipboardClear { .. } => "clipboard",
            Message::Identity { .. } => "identity",
            Message::LanBeacon { .. } => "lan_beacon",
            Message::LanPing { .. } => "lan_ping",
            Message::LanPong { .. } => "lan_pong",
            Message::KeyRotate { .. } => "key_rotate",
        }
    }
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

/// One thing a phone notification offers to do. `index` is positional
/// in the phone's own action array — it round-trips in
/// `NotificationAction`/`NotificationReply` so the phone fires the
/// right PendingIntent without re-scanning titles.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NotifAction {
    pub index: u32,
    pub title: String,
    /// "reply" — carries a RemoteInput, text entry is meaningful;
    /// "action" — plain button (mark-as-read, archive, …).
    pub kind: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayStatus {
    Playing,
    Paused,
    Stopped,
}

/// MPRIS `LoopStatus`, snake_case on the wire: `"none" | "track" | "playlist"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopMode {
    None,
    Track,
    Playlist,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum MediaOp {
    Play,
    Pause,
    Next,
    Previous,
    Seek { pos_ms: u64 },
    /// `{"op":"shuffle","on":bool}` — phone sets its session shuffle.
    Shuffle { on: bool },
    /// `{"op":"loop","mode":"none"|"track"|"playlist"}` — phone repeat.
    Loop { mode: LoopMode },
}
