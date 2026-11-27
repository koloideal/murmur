//! murmur wire protocol: envelope routing, family-key crypto, message types.
//!
//! Two layers of messages:
//! - [`Control`]: cleartext control-plane between a device and the server
//!   (hello/auth, device registry ops, pairing handshake).
//! - [`Message`]: end-to-end payload, always inside an [`Envelope`].payload
//!   sealed with the family key. The server never sees it.

pub mod control;
pub mod crypto;
pub mod envelope;
pub mod id;
pub mod invite;
pub mod msg;
pub mod rekey;

pub use control::Control;
pub use crypto::{FamilyKey, PairKey, sas_code};
pub use envelope::{Envelope, Route};
pub use id::{AuthToken, DeviceId, pair_code};
pub use invite::Invite;
pub use msg::{DeviceKind, LoopMode, MediaOp, Message, PlayStatus, PresenceState, SealedKey};
pub use x25519_dalek::PublicKey;

/// What kind of family this device belongs to — chosen once at family
/// creation and carried by invites, never inferred from the absence
/// of a server URL.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FamilyKind {
    /// A relay routes envelopes — works from anywhere.
    #[default]
    Serverful,
    /// Direct LAN links only — no relay exists anywhere in this
    /// family's world.
    Serverless,
}

/// Protocol version. Bump on incompatible wire changes.
pub const PROTO_VERSION: u8 = 1;

/// Route used for control-plane envelopes (`to` field).
pub const SERVER_ROUTE: &str = "@server";

/// Hard cap on one sealed payload — 4 MiB of ciphertext. Anything
/// larger is a bug or a DoS, never legitimate traffic: features cap
/// their own data well below this (see [`MAX_CLIP`]). Enforced at
/// [`Envelope::seal`], so a feature that overfills a message gets an
/// error instead of pushing a giant frame through the family.
pub const MAX_PAYLOAD: usize = 4 * 1024 * 1024;

/// Hard cap on one JSON-serialized envelope on the wire — the b64
/// payload inflates ~4/3 plus the envelope skeleton. Enforced at every
/// ingress: relay WS (`max_message_size`), client WS, LAN links,
/// `Envelope::from_json`.
pub const MAX_ENVELOPE_JSON: usize = 6 * 1024 * 1024;

/// Largest clipboard payload a feature may sync — bounded under
/// [`MAX_PAYLOAD`] because the data is b64'd inside the message before
/// sealing. A bigger clip is dropped with a log, never truncated.
pub const MAX_CLIP: usize = 2 * 1024 * 1024;
