//! Family-key lifecycle: identity key, member roster, rotation.
//!
//! - Every daemon has a long-term identity key and announces it after
//!   each connect; peers' identities are pinned on first sight (TOFU)
//!   in `roster.json`.
//! - When the relay reports a pinned member as revoked, the laptops
//!   rotate: the lowest-id online laptop goes first, the others wait
//!   their turn and stand down once the rotation reaches them.
//! - A member caught sending on an old key (offline during rotation)
//!   gets the current key re-sealed to its identity, unicast.
//!
//! See [`murmur_proto::rekey`] for why the revoked device can't follow.

use crate::config;
use crate::feature::{Feature, FeatureCtx};
use crate::state::Shared;
use murmur_proto::{
    Control, DeviceId, DeviceKind, Envelope, FamilyKey, Message, PairKey, PublicKey, Route,
    SealedKey, control::DeviceInfo, rekey,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast::error::RecvError;
use tracing::{info, warn};

/// How many superseded keys we keep to recognise stale members.
const OLD_KEYS: usize = 4;
/// At most one re-key per stale member per this window.
const REKEY_EVERY: Duration = Duration::from_secs(30);
/// Head start each rank of laptop gives the one before it.
const ROTATE_STAGGER: Duration = Duration::from_secs(6);
/// Everyone waits at least this long — a laptop that just reconnected
/// is usually about to be re-keyed by a peer that already rotated.
const ROTATE_GRACE: Duration = Duration::from_secs(3);

pub struct Keyring {
    pub current: FamilyKey,
    pub epoch: u64,
    rotator: Option<DeviceId>,
    old: Vec<FamilyKey>,
    /// Ever excluded by a rotation — travels with every KeyRotate.
    pub revoked: Vec<DeviceId>,
}

impl Keyring {
    pub fn load(s: &config::Secrets) -> anyhow::Result<Self> {
        let old = s
            .old_keys_hex
            .iter()
            .filter_map(|h| hex::decode(h).ok()?.try_into().ok().map(FamilyKey))
            .collect();
        Ok(Self {
            current: s.family_key()?,
            epoch: s.key_epoch,
            rotator: s.key_rotator.as_deref().and_then(|r| r.parse().ok()),
            old,
            revoked: s.revoked.iter().filter_map(|r| r.parse().ok()).collect(),
        })
    }

    pub fn old(&self) -> &[FamilyKey] {
        &self.old
    }

    fn install(&mut self, key: FamilyKey, epoch: u64, rotator: DeviceId, revoked: &[DeviceId]) {
        for r in revoked {
            if !self.revoked.contains(r) {
                self.revoked.push(*r);
            }
        }
        let prev = std::mem::replace(&mut self.current, key);
        self.old.retain(|k| k.0 != prev.0);
        self.old.insert(0, prev);
        self.old.truncate(OLD_KEYS);
        self.epoch = epoch;
        self.rotator = Some(rotator);
    }

    /// Newer epoch wins; on a tie (two laptops rotated at once) the
    /// smaller rotator id wins, so everyone converges on one key.
    fn supersedes(&self, epoch: u64, rotator: DeviceId) -> bool {
        epoch > self.epoch
            || (epoch == self.epoch && self.rotator.is_some_and(|cur| rotator < cur))
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Member {
    pub pub_b64: String,
    pub kind: DeviceKind,
}

impl Member {
    fn public(&self) -> Option<PublicKey> {
        PairKey::public_from_b64(&self.pub_b64).ok()
    }
}

pub type Roster = BTreeMap<DeviceId, Member>;

pub fn load_identity(s: &mut config::Secrets) -> anyhow::Result<PairKey> {
    if let Some(h) = &s.identity_hex {
        let b: [u8; 32] = hex::decode(h)?
            .try_into()
            .map_err(|_| anyhow::anyhow!("identity key must be 32 bytes"))?;
        return Ok(PairKey::from_secret(b));
    }
    let k = PairKey::generate();
    s.identity_hex = Some(hex::encode(k.secret_bytes()));
    s.save()?;
    info!("generated identity key");
    Ok(k)
}

pub fn load_roster() -> Roster {
    std::fs::read_to_string(config::data_path("roster.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

impl Shared {
    fn persist_keys(&self) {
        let k = self.keys.read().unwrap();
        let mut s = self.secrets.lock().unwrap();
        s.family_key_hex = hex::encode(k.current.0);
        s.key_epoch = k.epoch;
        s.key_rotator = k.rotator.map(|r| r.to_string());
        s.old_keys_hex = k.old.iter().map(|o| hex::encode(o.0)).collect();
        s.revoked = k.revoked.iter().map(|r| r.to_string()).collect();
        if let Err(e) = s.save() {
            warn!("persist secrets: {e:#}");
        }
    }

    fn persist_roster(&self) {
        let json = serde_json::to_string_pretty(&*self.roster.lock().unwrap());
        if let Err(e) = json
            .map_err(anyhow::Error::from)
            .and_then(|j| Ok(std::fs::write(config::data_path("roster.json"), j)?))
        {
            warn!("persist roster: {e:#}");
        }
    }

    /// Tell the family who we are. Runs after every authed connect.
    pub fn announce_identity(&self) {
        let _ = self.send(
            Route::All,
            &Message::Identity {
                pub_b64: self.identity.public_b64(),
                kind: DeviceKind::Laptop,
            },
        );
    }

    fn pin(&self, id: DeviceId, pub_b64: String, kind: DeviceKind) {
        if id == self.id
            || PairKey::public_from_b64(&pub_b64).is_err()
            || self.keys.read().unwrap().revoked.contains(&id)
        {
            return;
        }
        let mut r = self.roster.lock().unwrap();
        match r.get(&id) {
            Some(m) if m.pub_b64 == pub_b64 => return,
            Some(_) => {
                warn!("{id} announced a different identity key — keeping the pinned one");
                return;
            }
            None => {
                info!("pinned identity of {id} ({kind:?})");
                r.insert(id, Member { pub_b64, kind });
            }
        }
        drop(r);
        self.persist_roster();
        // A newcomer missed our own announcement — introduce ourselves.
        let _ = self.send(
            Route::Device(id),
            &Message::Identity {
                pub_b64: self.identity.public_b64(),
                kind: DeviceKind::Laptop,
            },
        );
    }

    /// Mint a new family key, seal it to every pinned member except
    /// `revoked`, broadcast under the *current* key, then switch.
    pub fn rotate(&self, revoked: Vec<DeviceId>) -> anyhow::Result<u64> {
        let new = FamilyKey::random();
        let epoch = self.keys.read().unwrap().epoch + 1;
        let keys = self.sealed_for(
            self.roster
                .lock()
                .unwrap()
                .iter()
                .filter(|(id, _)| !revoked.contains(id))
                .map(|(id, m)| (*id, m.clone()))
                .collect(),
            epoch,
            &new,
        );
        let n = keys.len();
        let mut all_revoked = self.keys.read().unwrap().revoked.clone();
        all_revoked.extend(revoked.iter().filter(|r| !all_revoked.contains(r)).copied().collect::<Vec<_>>());
        self.send(Route::All, &Message::KeyRotate { epoch, revoked: all_revoked, keys })?;
        self.keys.write().unwrap().install(new, epoch, self.id, &revoked);
        self.persist_keys();
        self.forget(&revoked);
        info!("rotated family key → epoch {epoch} ({n} member(s), revoked {revoked:?})");
        Ok(epoch)
    }

    fn sealed_for(&self, members: Vec<(DeviceId, Member)>, epoch: u64, key: &FamilyKey) -> Vec<SealedKey> {
        members
            .into_iter()
            .filter_map(|(id, m)| {
                let sealed_b64 = rekey::seal_for(&self.identity, &m.public()?, epoch, key).ok()?;
                Some(SealedKey { device_id: id, sealed_b64 })
            })
            .collect()
    }

    fn forget(&self, ids: &[DeviceId]) {
        if ids.is_empty() {
            return;
        }
        self.roster.lock().unwrap().retain(|id, _| !ids.contains(id));
        self.presence.lock().unwrap().retain(|id, _| !ids.contains(id));
        self.persist_roster();
    }

    fn apply_rotation(&self, from: DeviceId, epoch: u64, revoked: Vec<DeviceId>, keys: Vec<SealedKey>) {
        if from == self.id || !self.keys.read().unwrap().supersedes(epoch, from) {
            return;
        }
        let Some(rotator) = self.roster.lock().unwrap().get(&from).and_then(Member::public) else {
            warn!("key rotation from unpinned {from} — ignored");
            return;
        };
        let Some(mine) = keys.iter().find(|k| k.device_id == self.id) else {
            warn!("key rotation from {from} has no entry for us");
            return;
        };
        match rekey::open_from(&self.identity, &rotator, epoch, &mine.sealed_b64) {
            Ok(key) => {
                self.keys.write().unwrap().install(key, epoch, from, &revoked);
                self.persist_keys();
                self.forget(&revoked);
                info!("family key rotated by {from} → epoch {epoch}");
                // Peers that pinned us while we were away still need us.
                self.announce_identity();
            }
            Err(e) => warn!("key rotation from {from} didn't open: {e:#}"),
        }
    }

    /// conn found a frame from `from` that only an old key opens: it
    /// missed a rotation. Re-seal the current key to it, wrapped in the
    /// old key it can still read. Nothing else from that frame is used.
    pub fn on_stale(&self, from: DeviceId, old: &FamilyKey) {
        let Some(member) = self.roster.lock().unwrap().get(&from).cloned() else {
            return;
        };
        {
            let mut last = self.last_rekey.lock().unwrap();
            if last.get(&from).is_some_and(|t| t.elapsed() < REKEY_EVERY) {
                return;
            }
            last.insert(from, Instant::now());
        }
        let (epoch, cur, revoked) = {
            let k = self.keys.read().unwrap();
            (k.epoch, k.current.clone(), k.revoked.clone())
        };
        let keys = self.sealed_for(vec![(from, member)], epoch, &cur);
        let msg = Message::KeyRotate { epoch, revoked, keys };
        match Envelope::seal(self.id, Route::Device(from), self.next_seq(), old, &msg) {
            Ok(env) => {
                info!("{from} is on an old key — re-keyed it to epoch {epoch}");
                let _ = self.out.send(env);
            }
            Err(e) => warn!("re-key {from}: {e:#}"),
        }
    }

    /// Relay's device list → rotate if a pinned member got revoked.
    /// Laptops go in id order, each waiting its rank × stagger, and
    /// re-check before acting: the first rotation un-pins the revoked.
    async fn on_devices(self: &Arc<Self>, devices: &[DeviceInfo]) {
        let pinned_revoked = |r: &Roster| -> Vec<DeviceId> {
            devices
                .iter()
                .filter(|d| d.revoked && r.contains_key(&d.device_id))
                .map(|d| d.device_id)
                .collect()
        };
        if pinned_revoked(&self.roster.lock().unwrap()).is_empty() {
            return;
        }
        let rank = {
            let r = self.roster.lock().unwrap();
            devices
                .iter()
                .filter(|d| {
                    d.online
                        && !d.revoked
                        && d.device_id < self.id
                        && r.get(&d.device_id).is_some_and(|m| m.kind == DeviceKind::Laptop)
                })
                .count() as u32
        };
        tokio::time::sleep(ROTATE_GRACE + ROTATE_STAGGER * rank).await;
        // Several Devices pushes can race here; check+rotate is atomic.
        let _g = self.rotating.lock().unwrap();
        let todo = pinned_revoked(&self.roster.lock().unwrap());
        if todo.is_empty() {
            return; // someone ahead of us already rotated
        }
        if let Err(e) = self.rotate(todo) {
            warn!("rotation failed: {e:#}");
        }
    }
}

pub fn feature() -> Feature {
    Feature::new(
        "keyring",
        Some(|m| matches!(m, Message::Identity { .. } | Message::KeyRotate { .. })),
        |ctx| Box::pin(run(ctx)),
    )
}

async fn run(ctx: FeatureCtx) -> anyhow::Result<()> {
    let shared = ctx.shared;
    let mut inbox = ctx.inbox.expect("keyring wants messages");
    let mut ctl = shared.ctl_events.subscribe();
    loop {
        tokio::select! {
            m = inbox.recv() => match m {
                Some((from, Message::Identity { pub_b64, kind })) => shared.pin(from, pub_b64, kind),
                Some((from, Message::KeyRotate { epoch, revoked, keys })) => {
                    shared.apply_rotation(from, epoch, revoked, keys)
                }
                Some(_) => {}
                None => return Ok(()),
            },
            c = ctl.recv() => match c {
                Ok((_, Control::Devices { devices, .. })) => {
                    let s = shared.clone();
                    tokio::spawn(async move { s.on_devices(&devices).await });
                }
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return Ok(()),
            },
        }
    }
}
