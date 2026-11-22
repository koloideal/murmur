//! Pairing feature — brings a new device into the family.
//!
//! Inviter side (this daemon):
//! - `murmurctl invite` → [`Shared::invite`]: session + ephemeral DH key,
//!   invite code printed for the new device.
//! - The joiner's `PairHello` arrives here via `ctl_events`; the session
//!   moves from `invites` to `pending`.
//! - `murmurctl pair` lists pending with SAS codes; `pair accept` sends
//!   `PairAccept` (joiner can now show the same SAS); `pair confirm`
//!   registers the device on the server and ships `PairData`.
//!
//! Joiner side (`murmurd pair --code`): one-shot [`join`] — handshake,
//! print the SAS, wait for `PairData`, hand the secrets to provisioning.

use crate::feature::{Feature, FeatureCtx};
use crate::state::{InviteEntry, PAIR_TTL, Pending, Shared};
use anyhow::{Context, bail};
use futures_util::{SinkExt, StreamExt};
use murmur_proto::control::{HelloRole, PairBlob};
use murmur_proto::{
    AuthToken, Control, DeviceId, Envelope, FamilyKey, Invite, PairKey, Route, sas_code,
};
use std::time::{Duration, Instant};
use tokio::sync::broadcast::error::RecvError;
use tokio_tungstenite::tungstenite::Message as Ws;
use tracing::{info, warn};

/// Joiner gives up if the inviter doesn't confirm within this window.
const JOIN_TIMEOUT: Duration = Duration::from_secs(180);

// ---------- inviter side ----------

pub fn feature() -> Feature {
    Feature::new("pair", None, |ctx| Box::pin(run(ctx)))
}

/// Consume inbound pair frames: a `PairHello` whose `from` matches one of
/// our invites moves the session to `pending` (ctl takes it from there).
/// PairAccept/PairData are answers we never get — we're the inviter.
async fn run(ctx: FeatureCtx) -> anyhow::Result<()> {
    let shared = ctx.shared;
    let mut events = shared.ctl_events.subscribe();
    let mut sweep = tokio::time::interval(Duration::from_secs(60));
    loop {
        tokio::select! {
            ev = events.recv() => {
                match ev {
                    Ok((from, ctl)) => handle_ctl(&shared, from, ctl),
                    // Lagging drops frames, not the feature.
                    Err(RecvError::Lagged(n)) => warn!("pair: missed {n} control frames"),
                    Err(RecvError::Closed) => return Ok(()),
                }
            }
            _ = sweep.tick() => sweep_pairing(&shared),
        }
    }
}

fn handle_ctl(shared: &Shared, from: DeviceId, ctl: Control) {
    let Control::PairHello { pubkey_b64, name } = ctl else {
        return;
    };
    let peer_pub = match PairKey::public_from_b64(&pubkey_b64) {
        Ok(p) => p,
        Err(e) => {
            warn!("{from}: bad pair pubkey: {e}");
            return;
        }
    };
    let mut pair = shared.pair.lock().unwrap();
    match pair.invites.remove(&from) {
        Some(entry) => {
            pair.pending.insert(
                from,
                Pending {
                    key: entry.key,
                    peer_pub,
                    name: name.clone(),
                    created: entry.created,
                },
            );
            info!("pairing pending: {from} ({name}) — `murmurctl pair` to confirm");
        }
        None => tracing::debug!("{from}: PairHello for an unknown session, ignored"),
    }
}

/// Drop invites and pendings that never completed — pairing is a
/// here-and-now ritual, stale entries only confuse `murmurctl pair`.
fn sweep_pairing(shared: &Shared) {
    let mut pair = shared.pair.lock().unwrap();
    let fresh = |t: &Instant| t.elapsed() < PAIR_TTL;
    pair.invites.retain(|_, e| fresh(&e.created));
    pair.pending.retain(|_, p| fresh(&p.created));
}

impl Shared {
    /// `murmurctl invite`: mint a session, keep our DH key, hand back the
    /// code the new device feeds to `murmurd pair --code`.
    pub fn invite(&self) -> Invite {
        let invite = Invite::new(self.cfg.server_url.clone(), self.id);
        self.pair.lock().unwrap().invites.insert(
            invite.session,
            InviteEntry {
                key: PairKey::generate(),
                created: Instant::now(),
            },
        );
        // Register the session with the server so joiners can reach us
        // without knowing our device id (short-code path). Queued if we
        // happen to be offline — flushed on reconnect.
        let _ = self.send_control(&Control::PairOpen {
            session: invite.session,
        });
        invite
    }

    /// Pending joiners with their SAS codes (computable once both pubkeys
    /// are known — no need to wait for `accept`).
    pub fn pair_pending(&self) -> Vec<serde_json::Value> {
        self.pair
            .lock()
            .unwrap()
            .pending
            .iter()
            .map(|(session, p)| {
                serde_json::json!({
                    "session": session.to_string(),
                    "name": p.name,
                    "sas": sas_code(&p.key.public, &p.peer_pub, &self.id, session),
                })
            })
            .collect()
    }

    /// `murmurctl pair accept <session>`: send our pubkey so the joiner
    /// can show the same SAS. Human verification happens after this.
    pub fn pair_accept(&self, session: DeviceId) -> anyhow::Result<()> {
        let pubkey_b64 = {
            let pair = self.pair.lock().unwrap();
            pair.pending
                .get(&session)
                .map(|p| p.key.public_b64())
                .with_context(|| format!("no pending pairing {session}"))?
        };
        self.send_control_to(Route::Device(session), &Control::PairAccept { pubkey_b64 })
    }

    /// `murmurctl pair confirm <session>`: register the joiner on the
    /// server and ship its credentials over the DH-sealed channel.
    pub fn pair_confirm(&self, session: DeviceId) -> anyhow::Result<DeviceId> {
        let pending = self
            .pair
            .lock()
            .unwrap()
            .pending
            .remove(&session)
            .with_context(|| format!("no pending pairing {session}"))?;

        let device_id = DeviceId::random();
        let token = AuthToken::random();
        self.send_control(&Control::RegisterDevice {
            device_id,
            name: pending.name,
            token_hash_hex: token.hash_hex(),
        })?;

        let blob = PairBlob {
            device_id,
            token_hex: token.to_hex(),
            family_key_b64: Some({
                use base64::Engine;
                base64::engine::general_purpose::STANDARD.encode(self.key.0)
            }),
            server_url: Some(self.cfg.server_url.clone()),
        };
        let dh = pending.key.dh(&pending.peer_pub);
        let blob_b64 = {
            use base64::Engine;
            let sealed = FamilyKey::from_raw(dh).seal(&serde_json::to_vec(&blob)?)?;
            base64::engine::general_purpose::STANDARD.encode(sealed)
        };
        self.send_control_to(Route::Device(session), &Control::PairData { blob_b64 })?;
        Ok(device_id)
    }
}

// ---------- joiner side ----------

/// What `join` hands to provisioning.
pub struct Paired {
    pub server_url: String,
    pub device_id: DeviceId,
    pub token_hex: String,
    pub family_key_hex: String,
}

/// `murmurd pair --code <invite|XXXX-XXXX> [--server URL]`: claim the
/// session, exchange pubkeys, show the SAS, wait for `PairData`.
/// A full invite carries server+session; a short code derives the
/// session and needs `--server`.
pub async fn join(code: &str, name: &str, server: Option<&str>) -> anyhow::Result<Paired> {
    let (session, server_url) = match Invite::decode(code) {
        Ok(inv) => (inv.session, inv.server_url),
        Err(_) => {
            // Not a bundle — try the typeable XXXX-XXXX form.
            let session = DeviceId::from_code(code.trim());
            let server_url = server
                .map(str::to_owned)
                .context("short invite codes need --server ws(s)://…")?;
            (session, server_url)
        }
    };
    let key = PairKey::generate();

    let (ws, _) = tokio_tungstenite::connect_async(&server_url).await?;
    let (mut sink, mut source) = ws.split();

    sink.send(Ws::Text(
        serde_json::to_string(&Control::Hello {
            device_id: session,
            name: name.into(),
            role: HelloRole::Pair,
            token_hex: None,
        })?
        .into(),
    ))
    .await?;
    match source.next().await {
        Some(Ok(Ws::Text(t))) => match serde_json::from_str::<Control>(&t) {
            Ok(Control::HelloOk) => {}
            Ok(Control::HelloErr { reason }) => bail!("pair conn rejected: {reason}"),
            other => bail!("unexpected hello reply: {other:?}"),
        },
        _ => bail!("no hello reply"),
    }
    eprintln!("  ✓ connected to {server_url}");
    eprintln!("  › pair session {session}");

    // PairHello goes to @server — it forwards to whoever PairOpen'd
    // this session, so the joiner never needs the inviter's id.
    let hello = Envelope::control(
        session,
        Route::Server,
        1,
        &Control::PairHello {
            pubkey_b64: key.public_b64(),
            name: name.into(),
        },
    )?;
    sink.send(Ws::Text(hello.to_json().into())).await?;
    eprintln!("  › waiting for the inviter — `murmurctl pair` there");

    tokio::time::timeout(JOIN_TIMEOUT, async {
        let mut dh_key: Option<FamilyKey> = None;
        while let Some(msg) = source.next().await {
            let Ws::Text(t) = msg? else { continue };
            let env = Envelope::from_json(&t)?;
            match env.control_payload()? {
                Control::PairAccept { pubkey_b64 } => {
                    let peer = PairKey::public_from_b64(&pubkey_b64)?;
                    dh_key = Some(FamilyKey::from_raw(key.dh(&peer)));
                    // env.from is the inviter's device id (stamped by
                    // the server) — the SAS needs it, and on the
                    // short-code path it's the only way to learn it.
                    // DeviceId::ZERO means the relay itself answered:
                    // bootstrap pairing, SAS lives in the relay log and
                    // the joiner confirms right here.
                    let sas = sas_code(&key.public, &peer, &session, &env.from);
                    if env.from == DeviceId::ZERO {
                        eprintln!("  │");
                        eprintln!("  │  verification code   {sas}");
                        eprintln!("  │");
                        eprintln!("  │  get it on the relay:");
                        eprintln!("  │    sudo murmur-server bootstrap-code");
                        eprintln!("  │");
                        eprint!("  ? codes match? [y/N] ");
                        use std::io::Write;
                        std::io::stderr().flush()?;
                        // SAS is a human check — read the real terminal, not
                        // stdin: under `curl | sh` stdin is the dead pipe.
                        let mut line = String::new();
                        match std::fs::File::open("/dev/tty") {
                            Ok(f) => {
                                use std::io::BufRead;
                                std::io::BufReader::new(f).read_line(&mut line)?;
                            }
                            Err(_) => {
                                std::io::stdin().read_line(&mut line)?;
                            }
                        }
                        if line.is_empty() {
                            eprintln!();
                        }
                        if !line.trim().eq_ignore_ascii_case("y") {
                            bail!("pairing aborted — SAS not confirmed");
                        }
                        let confirm =
                            Envelope::control(session, Route::Server, 2, &Control::PairConfirm)?;
                        sink.send(Ws::Text(confirm.to_json().into())).await?;
                        eprintln!("  ✓ confirmed — receiving credentials");
                    } else {
                        eprintln!("  │");
                        eprintln!("  │  verification code   {sas}");
                        eprintln!("  │");
                        eprintln!("  │  the inviter confirms it — pairing completes there");
                    }
                }
                Control::PairData { blob_b64 } => {
                    use base64::Engine;
                    let dk = dh_key.context("PairData before PairAccept")?;
                    let ct = base64::engine::general_purpose::STANDARD.decode(blob_b64)?;
                    let blob: PairBlob = serde_json::from_slice(&dk.open(&ct)?)?;
                    return anyhow::Ok(Paired {
                        // Bootstrap blobs leave these empty: the joiner
                        // keeps the URL it dialed and mints the family
                        // key itself — the relay never sees it.
                        server_url: blob.server_url.unwrap_or(server_url.clone()),
                        device_id: blob.device_id,
                        token_hex: blob.token_hex,
                        family_key_hex: match &blob.family_key_b64 {
                            Some(b64) => {
                                let raw = base64::engine::general_purpose::STANDARD.decode(b64)?;
                                hex::encode(raw)
                            }
                            None => hex::encode(FamilyKey::random().0),
                        },
                    });
                }
                _ => {}
            }
        }
        bail!("connection closed before PairData")
    })
    .await
    .context("pairing timed out — the inviter didn't confirm in time")?
}
