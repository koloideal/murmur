use crate::config::{Config, Secrets};
use murmur_proto::{Control, DeviceId, Envelope, FamilyKey, Message, PresenceState, Route};
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{broadcast, mpsc};

pub fn now_ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Shared daemon state: identity, key, presence table, outbound queue.
pub struct Shared {
    pub cfg: Config,
    pub id: DeviceId,
    pub key: FamilyKey,
    token_hex: String,
    seq: AtomicU64,
    pub out: mpsc::UnboundedSender<Envelope>,
    /// Cleartext control replies from the server (Devices, Error, ...).
    pub ctl_events: broadcast::Sender<Control>,
    /// device → (last state, ts of last transition). Includes self.
    /// Shared because the active-node election is cross-cutting: clipboard
    /// routing and phone-side pushes both need it.
    pub presence: Mutex<HashMap<DeviceId, (PresenceState, u64)>>,
}

impl Shared {
    pub fn new(
        cfg: Config,
        secrets: Secrets,
        out: mpsc::UnboundedSender<Envelope>,
    ) -> anyhow::Result<Self> {
        let (ctl_events, _) = broadcast::channel(64);
        Ok(Self {
            id: secrets.device_id()?,
            key: secrets.family_key()?,
            token_hex: secrets.auth_token_hex.clone(),
            cfg,
            seq: AtomicU64::new(1),
            out,
            ctl_events,
            presence: Mutex::new(HashMap::new()),
        })
    }

    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::Relaxed)
    }

    /// Seal and enqueue a message for the given route.
    pub fn send(&self, to: Route, msg: &Message) -> anyhow::Result<()> {
        let env = Envelope::seal(self.id, to, self.next_seq(), &self.key, msg)?;
        let _ = self.out.send(env);
        Ok(())
    }

    /// Cleartext control message to the server.
    pub fn send_control(&self, ctl: &murmur_proto::Control) -> anyhow::Result<()> {
        let env = Envelope::control(self.id, Route::Server, self.next_seq(), ctl)?;
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
