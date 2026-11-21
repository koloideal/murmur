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

pub use control::Control;
pub use crypto::{FamilyKey, PairKey, sas_code};
pub use envelope::{Envelope, Route};
pub use id::{AuthToken, DeviceId, pair_code};
pub use invite::Invite;
pub use msg::{MediaOp, Message, PlayStatus, PresenceState};
pub use x25519_dalek::PublicKey;

/// Protocol version. Bump on incompatible wire changes.
pub const PROTO_VERSION: u8 = 1;

/// Route used for control-plane envelopes (`to` field).
pub const SERVER_ROUTE: &str = "@server";
