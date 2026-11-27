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

use std::collections::HashMap;

/// Pairing state owned by the inviter side. Sessions are the joiner's
/// conn ids: a `PairHello`'s `env.from` is matched against `invites`.
/// Entries expire — an invite nobody finishes within TTL is swept.
#[derive(Default)]
pub struct Pairing {
    /// Outstanding invites: session → our ephemeral DH key.
    pub invites: HashMap<DeviceId, InviteEntry>,
    /// `PairHello` received for one of our invites.
    pub pending: HashMap<DeviceId, Pending>,
    /// Serverless pair sockets staged by session id: `serve_pair`
    /// registers the joiner's socket writer here *before* the
    /// PairHello reaches `pair::handle_ctl`, which moves it into the
    /// fresh `Pending`.
    pub pair_reply: HashMap<DeviceId, mpsc::UnboundedSender<Envelope>>,
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
    /// Serverless pairing: the joiner's direct socket writer — its
    /// PairAccept/PairData ride this instead of the (absent) relay.
    pub reply: Option<mpsc::UnboundedSender<Envelope>>,
}

/// Shared daemon state: identity, key, presence table, outbound queue.

use crate::feature::{Feature, FeatureCtx};
use crate::state::Shared;
use anyhow::{Context, bail};
use futures_util::{SinkExt, StreamExt};
use murmur_proto::control::{HelloRole, PairBlob};
use murmur_proto::{
    AuthToken, Control, DeviceId, Envelope, FamilyKey, FamilyKind, Invite, PairKey, PublicKey,
    Route, sas_code,
};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::mpsc;
use tokio::sync::broadcast::error::RecvError;
use tokio_tungstenite::tungstenite::Message as Ws;
use tracing::{info, warn};

/// Joiner gives up if the inviter doesn't confirm within this window.
const JOIN_TIMEOUT: Duration = Duration::from_secs(180);
/// How long a typed short code waits for its LAN pair beacon.
const BEACON_WAIT: Duration = Duration::from_secs(120);

// ---------- inviter side ----------

pub fn feature() -> Feature {
    Feature::new("pair", &[], |ctx| Box::pin(run(ctx)))
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
            // A serverless PairHello arrived over a direct socket —
            // its reply channel was staged by `serve_pair`.
            let reply = pair.pair_reply.remove(&from);
            pair.pending.insert(
                from,
                Pending {
                    key: entry.key,
                    peer_pub,
                    name: name.clone(),
                    created: entry.created,
                    reply,
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
    // Staged sockets die with their session — but a PairHello can be
    // sitting in ctl_events *between* the two maps, so keep a staged
    // channel while the session is still a live invite too.
    let live: Vec<DeviceId> = pair
        .invites
        .keys()
        .chain(pair.pending.keys())
        .copied()
        .collect();
    pair.pair_reply.retain(|s, _| live.contains(s));
}

// ---------- serverless invite listener ----------

/// A serverless invite's rendezvous: an ephemeral TCP listener that
/// exists only while the invite does. Each accepted socket gets its
/// own `serve_pair` — a straggler can't block a second attempt.
async fn pair_listen(shared: Arc<Shared>, listener: TcpListener) {
    let deadline = Instant::now() + PAIR_TTL;
    loop {
        match tokio::time::timeout_at(deadline.into(), listener.accept()).await {
            Ok(Ok((s, _))) => {
                let _ = s.set_nodelay(true);
                tokio::spawn(serve_pair(shared.clone(), s));
            }
            Ok(Err(e)) => {
                warn!("pair-listen accept: {e}");
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            Err(_) => break, // invite expired — free the port
        }
    }
}

/// One inbound joiner socket: newline-JSON envelopes carrying cleartext
/// `Control` frames — the same shape the relay forwards in a serverful
/// family. A `PairHello` registers this socket as the session's reply
/// channel *before* the frame hits `ctl_events`, so `pair_accept`/
/// `pair_confirm` answers land back on the wire that asked.
async fn serve_pair(shared: Arc<Shared>, s: TcpStream) {
    let (r, mut w) = s.into_split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Envelope>();
    tokio::spawn(async move {
        while let Some(env) = rx.recv().await {
            let mut line = env.to_json();
            line.push('\n');
            if w.write_all(line.as_bytes()).await.is_err() {
                break;
            }
        }
    });
    let mut br = BufReader::new(r);
    let mut line = String::new();
    loop {
        line.clear();
        match tokio::time::timeout(JOIN_TIMEOUT, br.read_line(&mut line)).await {
            Ok(Ok(0)) | Ok(Err(_)) | Err(_) => break,
            Ok(Ok(_)) => {}
        }
        let Ok(env) = Envelope::from_json(line.trim_end()) else {
            continue;
        };
        let Ok(ctl) = env.control_payload() else {
            continue;
        };
        if let Control::PairHello { .. } = ctl {
            shared
                .pair
                .lock()
                .unwrap()
                .pair_reply
                .insert(env.from, tx.clone());
        }
        let _ = shared.ctl_events.send((env.from, ctl));
    }
}

/// While a serverless invite lives it advertises itself on the LAN —
/// the only way a *typed* short code can find an inviter with no relay
/// to resolve the session. Cleartext JSON datagrams on the beacon port;
/// they carry no secrets, worst case a stranger pairs in and gets
/// SAS-rejected — same threat model as the invite QR itself.
async fn pair_beacon(session: DeviceId, addr: String) {
    let Ok(udp) = UdpSocket::bind("0.0.0.0:0").await else {
        return;
    };
    let _ = udp.set_broadcast(true);
    let payload = serde_json::json!({
        "t": "murmur-pair",
        "s": session.to_string(),
        "a": addr,
    })
    .to_string();
    let deadline = Instant::now() + PAIR_TTL;
    while Instant::now() < deadline {
        let _ = udp
            .send_to(payload.as_bytes(), ("255.255.255.255", crate::lan::BEACON_PORT))
            .await;
        for b in crate::lan::broadcast_addrs() {
            let _ = udp
                .send_to(payload.as_bytes(), (b, crate::lan::BEACON_PORT))
                .await;
        }
        tokio::time::sleep(Duration::from_millis(1500)).await;
    }
}

/// Resolve a typed short code to a `host:port` by listening for the
/// inviter's pair beacon — the LAN-side stand-in for the relay's
/// `PairOpen` session lookup.
async fn pair_beacon_wait(session: DeviceId) -> anyhow::Result<String> {
    // SO_REUSEADDR: a running daemon already holds the beacon port.
    let s2 = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )?;
    s2.set_reuse_address(true)?;
    s2.set_broadcast(true)?;
    s2.bind(&format!("0.0.0.0:{}", crate::lan::BEACON_PORT)
        .parse::<std::net::SocketAddr>()?
        .into())?;
    s2.set_nonblocking(true)?;
    let udp = UdpSocket::from_std(s2.into())?;
    let want = session.to_string();
    let mut buf = [0u8; 2048];
    tokio::time::timeout(BEACON_WAIT, async {
        loop {
            let (n, _) = udp.recv_from(&mut buf).await?;
            let Ok(v) = serde_json::from_slice::<serde_json::Value>(&buf[..n]) else {
                continue;
            };
            if v["t"] == "murmur-pair" && v["s"] == want {
                if let Some(a) = v["a"].as_str() {
                    return anyhow::Ok(a.to_owned());
                }
            }
        }
    })
    .await
    .context("no invite answering on this lan — is `murmurctl invite` running there?")?
}

impl Shared {
    /// `murmurctl invite`: mint a session, keep our DH key, hand back the
    /// code the new device feeds to `murmurd pair --code` (or scans as
    /// a QR on the phone). In a serverless family the invite carries a
    /// direct `host:port` the joiner dials — the listener lives as long
    /// as the invite does.
    pub fn invite(self: &Arc<Self>) -> anyhow::Result<Invite> {
        let url = match self.cfg.family_kind {
            FamilyKind::Serverful => self.cfg.server_url.clone(),
            FamilyKind::Serverless => {
                let ip = crate::lan::lan_ipv4()
                    .context("serverless invite needs a LAN address to listen on")?;
                // Fixed port — a host firewall can allow it once; only
                // one live invite per machine.
                let l = std::net::TcpListener::bind((ip, crate::lan::PAIR_PORT))
                    .context("pair port busy — is another invite live?")?;
                l.set_nonblocking(true)?;
                let listener = tokio::net::TcpListener::from_std(l)?;
                tokio::spawn(pair_listen(self.clone(), listener));
                format!("{ip}:{}", crate::lan::PAIR_PORT)
            }
        };
        let invite = Invite::new(url, self.id, self.cfg.family_kind);
        self.pair.lock().unwrap().invites.insert(
            invite.session,
            InviteEntry {
                key: PairKey::generate(),
                created: Instant::now(),
            },
        );
        // Register the session with the server so joiners can reach us
        // without knowing our device id (short-code path). Queued if we
        // happen to be offline — flushed on reconnect. Serverless has
        // no server — the direct listener is the rendezvous, and the
        // pair beacon makes a typed short code resolvable on the LAN.
        if self.cfg.family_kind == FamilyKind::Serverful {
            let _ = self.send_control(&Control::PairOpen {
                session: invite.session,
            });
        } else {
            tokio::spawn(pair_beacon(invite.session, invite.server_url.clone()));
        }
        Ok(invite)
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
        self.send_pair_frame(session, &Control::PairAccept { pubkey_b64 })
    }

    /// Pair frames take the joiner's own transport: a direct socket
    /// for a serverless session, the relay otherwise.
    fn send_pair_frame(&self, session: DeviceId, ctl: &Control) -> anyhow::Result<()> {
        let reply = self
            .pair
            .lock()
            .unwrap()
            .pending
            .get(&session)
            .and_then(|p| p.reply.clone());
        match reply {
            Some(tx) => {
                let env = Envelope::control(
                    self.id,
                    Route::Device(session),
                    self.next_seq(),
                    ctl,
                )?;
                tx.send(env).map_err(|_| anyhow::anyhow!("pair socket closed"))?;
            }
            None => self.send_control_to(Route::Device(session), ctl)?,
        }
        Ok(())
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
        // Serverful families register the member on the relay;
        // serverless ones mint the id locally — roster gossip
        // distributes it (there is no server to ask).
        if self.cfg.family_kind == FamilyKind::Serverful {
            self.send_control(&Control::RegisterDevice {
                device_id,
                name: pending.name.clone(),
                token_hash_hex: token.hash_hex(),
            })?;
        }

        let blob = PairBlob {
            device_id,
            token_hex: token.to_hex(),
            family_key_b64: Some({
                use base64::Engine;
                base64::engine::general_purpose::STANDARD.encode(self.keys.read().unwrap().current.0)
            }),
            server_url: Some(self.cfg.server_url.clone()),
            key_epoch: self.keys.read().unwrap().epoch,
            family_name: self.family_name.lock().unwrap().clone(),
            kind: self.cfg.family_kind,
        };
        let dh = pending.key.dh(&pending.peer_pub);
        let blob_b64 = {
            use base64::Engine;
            let sealed = FamilyKey::from_raw(dh).seal(&serde_json::to_vec(&blob)?)?;
            base64::engine::general_purpose::STANDARD.encode(sealed)
        };
        // `pending` is already out of the map — send its reply channel
        // along instead of re-entering `send_pair_frame`'s lookup.
        let env = Envelope::control(
            self.id,
            Route::Device(session),
            self.next_seq(),
            &Control::PairData { blob_b64 },
        )?;
        match pending.reply {
            Some(tx) => {
                info!("pair: sending PairData to {session} over its direct socket");
                tx.send(env.clone())
                    .map_err(|_| anyhow::anyhow!("pair socket closed"))?;
                // One frame over sleepy-wifi TCP can stall for minutes —
                // resend a few times; the joiner finishes on the first
                // PairData it reads and drops the socket, ending the loop.
                let env = env.clone();
                tokio::spawn(async move {
                    for _ in 0..10 {
                        tokio::time::sleep(Duration::from_secs(2)).await;
                        if tx.send(env.clone()).is_err() {
                            break;
                        }
                    }
                });
            }
            None => {
                let _ = self.out.send(env);
            }
        }
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
    pub key_epoch: u64,
    pub kind: murmur_proto::FamilyKind,
    pub family_name: Option<String>,
}

/// `murmurd pair --code <invite|XXXX-XXXX> [--server URL]`: claim the
/// session, exchange pubkeys, show the SAS, wait for `PairData`.
/// A full invite carries the endpoint; a short code derives the
/// session and resolves it via `--server` or a LAN pair beacon.
pub async fn join(code: &str, name: &str, server: Option<&str>) -> anyhow::Result<Paired> {
    let (session, server_url, kind) = match Invite::decode(code) {
        Ok(inv) => (inv.session, inv.server_url, inv.kind),
        Err(_) => {
            // Not a bundle — the typeable XXXX-XXXX form. With --server
            // it's a relay-family join; bare, it means "find the inviter
            // on the LAN" — the serverless short-code path.
            let session = DeviceId::from_code(code.trim());
            let Some(server_url) = server.map(str::to_owned) else {
                return join_lan_code(session, name).await;
            };
            (session, server_url, FamilyKind::Serverful)
        }
    };
    if kind == FamilyKind::Serverless {
        return join_direct(&server_url, session, name).await;
    }
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
    eprintln!("  › pair session {session} — waiting for the other side");

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
                        eprintln!("  │    sudo murmurs bootstrap-code");
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
                        eprintln!("  │  confirm it on the inviter — `murmurctl pair` there");
                    }
                }
                Control::PairReject { reason } => {
                    bail!("pairing rejected: {reason}")
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
                        key_epoch: blob.key_epoch,
                        kind: blob.kind,
                        family_name: blob.family_name,
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

/// `murmurd pair --code XXXX-XXXX` with no server: the inviter's LAN
/// pair beacon tells us where to dial — the serverless stand-in for
/// the relay's session lookup.
async fn join_lan_code(session: DeviceId, name: &str) -> anyhow::Result<Paired> {
    eprintln!("  › listening for a `{session}` invite on the lan…");
    let addr = pair_beacon_wait(session).await?;
    eprintln!("  ✓ invite answered at {addr}");
    join_direct(&addr, session, name).await
}

/// Joiner side of a serverless invite: `addr` is the inviter's
/// ephemeral `host:port`. Newline-JSON envelopes — the same `Control`
/// frames as the relay path, no `Hello` handshake (there is no
/// server to introduce us; the session id in `from` is the
/// introduction).
async fn join_direct(addr: &str, session: DeviceId, name: &str) -> anyhow::Result<Paired> {
    let key = PairKey::generate();
    let s = TcpStream::connect(addr).await?;
    let _ = s.set_nodelay(true);
    let (r, mut w) = s.into_split();
    eprintln!("  ✓ connected to {addr} — direct");
    eprintln!("  › pair session {session} — waiting for the other side");

    let hello = Envelope::control(
        session,
        Route::Server,
        1,
        &Control::PairHello {
            pubkey_b64: key.public_b64(),
            name: name.into(),
        },
    )?;
    w.write_all(format!("{}\n", hello.to_json()).as_bytes())
        .await?;

    tokio::time::timeout(JOIN_TIMEOUT, async {
        let mut dh_key: Option<FamilyKey> = None;
        let mut br = BufReader::new(r);
        let mut line = String::new();
        loop {
            line.clear();
            if br.read_line(&mut line).await? == 0 {
                bail!("connection closed before PairData");
            }
            let Ok(env) = Envelope::from_json(line.trim_end()) else {
                continue;
            };
            match env.control_payload()? {
                Control::PairAccept { pubkey_b64 } => {
                    let peer = PairKey::public_from_b64(&pubkey_b64)?;
                    dh_key = Some(FamilyKey::from_raw(key.dh(&peer)));
                    let sas = sas_code(&key.public, &peer, &session, &env.from);
                    eprintln!("  │");
                    eprintln!("  │  verification code   {sas}");
                    eprintln!("  │");
                    eprintln!("  │  confirm it on the inviter — `murmurctl pair` there");
                }
                Control::PairReject { reason } => {
                    bail!("pairing rejected: {reason}")
                }
                Control::PairData { blob_b64 } => {
                    use base64::Engine;
                    let dk = dh_key.context("PairData before PairAccept")?;
                    let ct = base64::engine::general_purpose::STANDARD.decode(blob_b64)?;
                    let blob: PairBlob = serde_json::from_slice(&dk.open(&ct)?)?;
                    return anyhow::Ok(Paired {
                        // No relay exists to fall back to — the invite's
                        // `host:port` was a one-shot rendezvous, not a
                        // server.
                        server_url: blob.server_url.unwrap_or_default(),
                        device_id: blob.device_id,
                        token_hex: blob.token_hex,
                        family_key_hex: match &blob.family_key_b64 {
                            Some(b64) => {
                                let raw =
                                    base64::engine::general_purpose::STANDARD.decode(b64)?;
                                hex::encode(raw)
                            }
                            None => hex::encode(FamilyKey::random().0),
                        },
                        key_epoch: blob.key_epoch,
                        kind: blob.kind,
                        family_name: blob.family_name,
                    });
                }
                _ => {}
            }
        }
    })
    .await
    .context("pairing timed out — the inviter didn't confirm in time")?
}
