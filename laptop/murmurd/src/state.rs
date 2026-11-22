use crate::config::{Config, Secrets};
use crate::keyring::{Keyring, Roster};
use murmur_proto::{
    Control, DeviceId, Envelope, Message, PairKey, PresenceState, PublicKey, Route,
};
use std::collections::HashMap;
use std::sync::{Mutex, RwLock};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::{broadcast, mpsc};

pub fn now_ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Pairing state owned by the inviter side. Sessions are the joiner's
/// conn ids: a `PairHello`'s `env.from` is matched against `invites`.
/// Entries expire — an invite nobody finishes within TTL is swept.
#[derive(Default)]
pub struct Pairing {
    /// Outstanding invites: session → our ephemeral DH key.
    pub invites: HashMap<DeviceId, InviteEntry>,
    /// `PairHello` received for one of our invites.
    pub pending: HashMap<DeviceId, Pending>,
}

/// How long an invite/pending stays around without progress.
pub const PAIR_TTL: std::time::Duration = std::time::Duration::from_secs(600);

pub struct InviteEntry {
    pub key: PairKey,
    pub created: std::time::Instant,
}

pub struct Pending {
    /// Our ephemeral key (kept for DH at confirm time).
    pub key: PairKey,
    /// Joiner's pubkey from its `PairHello`.
    pub peer_pub: PublicKey,
    /// Name the joiner asked to be registered under.
    pub name: String,
    /// When the invite was issued (TTL counts from then, not from hello).
    pub created: std::time::Instant,
}

/// Shared daemon state: identity, key, presence table, outbound queue.
pub struct Shared {
    pub cfg: Config,
    pub id: DeviceId,
    /// Current family key + epoch + a few superseded ones.
    pub keys: RwLock<Keyring>,
    /// Long-term identity key — rotations are sealed to it.
    pub identity: PairKey,
    /// Pinned identities of the other members.
    pub roster: Mutex<Roster>,
    /// secrets.toml as loaded — rewritten when the key rotates.
    pub secrets: Mutex<Secrets>,
    pub last_rekey: Mutex<HashMap<DeviceId, std::time::Instant>>,
    pub rotating: Mutex<()>,
    token_hex: String,
    seq: AtomicU64,
    pub out: mpsc::UnboundedSender<Envelope>,
    /// Cleartext control frames addressed to us: (sender, frame).
    /// Server replies (Devices, Error) and inbound pair frames.
    pub ctl_events: broadcast::Sender<(DeviceId, Control)>,
    /// device → (last state, ts of last transition). Includes self.
    /// Shared because the active-node election is cross-cutting: clipboard
    /// routing and phone-side pushes both need it.
    pub presence: Mutex<HashMap<DeviceId, (PresenceState, u64)>>,
    /// Inviter-side pairing sessions. Cross-cutting: conn feeds PairHello,
    /// ctl drives invite/accept/confirm.
    pub pair: Mutex<Pairing>,
    /// True between HelloOk and the socket's death — for `murmurctl status`.
    pub online: AtomicBool,
    /// The relay said this device is revoked / unknown.
    pub revoked: AtomicBool,
}

impl Shared {
    pub fn new(
        cfg: Config,
        mut secrets: Secrets,
        out: mpsc::UnboundedSender<Envelope>,
    ) -> anyhow::Result<Self> {
        let (ctl_events, _) = broadcast::channel(64);
        let identity = crate::keyring::load_identity(&mut secrets)?;
        Ok(Self {
            id: secrets.device_id()?,
            keys: RwLock::new(Keyring::load(&secrets)?),
            identity,
            roster: Mutex::new(crate::keyring::load_roster()),
            last_rekey: Mutex::new(HashMap::new()),
            rotating: Mutex::new(()),
            token_hex: secrets.auth_token_hex.clone(),
            secrets: Mutex::new(secrets),
            cfg,
            seq: AtomicU64::new(1),
            out,
            ctl_events,
            presence: Mutex::new(HashMap::new()),
            pair: Mutex::new(Pairing::default()),
            online: AtomicBool::new(false),
            revoked: AtomicBool::new(false),
        })
    }

    pub fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::Relaxed)
    }

    /// Seal and enqueue a message for the given route.
    pub fn send(&self, to: Route, msg: &Message) -> anyhow::Result<()> {
        let env = Envelope::seal(self.id, to, self.next_seq(), &self.keys.read().unwrap().current, msg)?;
        let _ = self.out.send(env);
        Ok(())
    }

    /// Cleartext control message to the server.
    pub fn send_control(&self, ctl: &Control) -> anyhow::Result<()> {
        self.send_control_to(Route::Server, ctl)
    }

    /// Cleartext control message to any route — pairing frames go unicast
    /// to a session conn (`Route::Device(session)`).
    pub fn send_control_to(&self, to: Route, ctl: &Control) -> anyhow::Result<()> {
        let env = Envelope::control(self.id, to, self.next_seq(), ctl)?;
        let _ = self.out.send(env);
        Ok(())
    }

    /// Active-node election: newest `active` wins; ties → smaller DeviceId.
    /// Deterministic across nodes given the same presence table.
    pub fn active_node(&self) -> Option<DeviceId> {
        self.presence
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, (s, _))| *s == PresenceState::Active)
            .max_by(|(a_id, (_, a_ts)), (b_id, (_, b_ts))| {
                a_ts.cmp(b_ts).then(b_id.cmp(a_id)) // later ts wins; smaller id wins ties
            })
            .map(|(id, _)| *id)
    }

    pub fn is_active(&self) -> bool {
        self.active_node() == Some(self.id)
    }

    pub fn token_hex(&self) -> String {
        self.token_hex.clone()
    }

    /// Update own state and remember it for the next broadcast.
    pub fn set_local_state(&self, state: PresenceState) {
        self.presence
            .lock()
            .unwrap()
            .insert(self.id, (state, now_ts()));
        self.broadcast_presence();
    }

    /// Re-announce current presence state to everyone.
    pub fn broadcast_presence(&self) {
        let state = self
            .presence
            .lock()
            .unwrap()
            .get(&self.id)
            .map(|(s, _)| *s)
            .unwrap_or(PresenceState::Active);
        let _ = self.send(
            Route::All,
            &Message::Presence {
                state,
                ts: now_ts(),
            },
        );
    }

    /// Called on disconnect. Presence of remote peers is retained; they age
    /// out implicitly because our election only considers `active` states —
    /// a disconnected peer simply stops updating. TODO: mark peers stale.
    pub fn note_presence_offline(&self) {}

    pub fn note_presence(&self, id: DeviceId, state: PresenceState, ts: u64) {
        let mut m = self.presence.lock().unwrap();
        if m.get(&id).is_none_or(|(_, old)| ts > *old) {
            m.insert(id, (state, ts));
        }
    }
}
