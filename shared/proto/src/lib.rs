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
