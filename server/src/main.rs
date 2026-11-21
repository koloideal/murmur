//! murmur-server: dumb authenticated relay.
//!
//! - Every device connects outbound with WS and says `Control::Hello` first.
//! - `role=auth`: token hash checked against `devices.json`, conn bound to id.
//! - `role=pair`: pending pairing conn; may only exchange Pair* frames until
//!   an authed device registers it.
//! - Frames are routed by `to`: `"*"` fans out, `"<id>"` unicast,
//!   `"@server"` is the control plane (registry ops).
//! - `from` is always overwritten with the authenticated id.
//! - No queues, no storage of payloads. Live-only.

use anyhow::{Context, bail};
use futures_util::{SinkExt, StreamExt};
use murmur_proto::{Control, DeviceId, Envelope, Route, control::DeviceInfo, control::HelloRole};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as Ws;
use tracing::{debug, info, warn};

#[derive(Serialize, Deserialize)]
struct DeviceRec {
    name: String,
    token_hash_hex: String,
    #[serde(default)]
    revoked: bool,
}

struct Conn {
    tx: mpsc::UnboundedSender<String>,
    pairing: bool,
}

struct Shared {
    devices_path: PathBuf,
    /// Persistent registry, mirrored to devices.json.
    devices: Mutex<HashMap<DeviceId, DeviceRec>>,
    /// Live connections.
    conns: Mutex<HashMap<DeviceId, Conn>>,
}

impl Shared {
    fn load(path: PathBuf) -> anyhow::Result<Self> {
        let devices = match std::fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s).context("parse devices.json")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
            Err(e) => return Err(e).context("read devices.json"),
        };
        Ok(Self {
            devices_path: path,
            devices: Mutex::new(devices),
            conns: Mutex::new(HashMap::new()),
        })
    }

    fn persist(&self) {
        let devices = self.devices.lock().unwrap();
        let tmp = self.devices_path.with_extension("tmp");
        match serde_json::to_string_pretty(&*devices) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&tmp, json)
                    .and_then(|_| std::fs::rename(&tmp, &self.devices_path))
                {
                    warn!("persist devices.json: {e}");
                }
            }
            Err(e) => warn!("serialize devices.json: {e}"),
        }
    }

    fn send_to(&self, id: &DeviceId, env: &Envelope) {
        let tx = self.conns.lock().unwrap().get(id).map(|c| c.tx.clone());
        if let Some(tx) = tx {
            let _ = tx.send(env.to_json());
        }
    }

    fn broadcast(&self, except: &DeviceId, env: &Envelope, authed_only: bool) {
        let targets: Vec<_> = self
            .conns
            .lock()
            .unwrap()
            .iter()
            .filter(|(id, c)| *id != except && (!authed_only || !c.pairing))
            .map(|(_, c)| c.tx.clone())
            .collect();
        let json = env.to_json();
        for tx in targets {
            let _ = tx.send(json.clone());
        }
    }

    fn drop_conn(&self, id: &DeviceId) {
        self.conns.lock().unwrap().remove(id);
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let addr: SocketAddr = std::env::var("MURMUR_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8770".into())
        .parse()
        .context("MURMUR_ADDR")?;
    let devices_path =
        PathBuf::from(std::env::var("MURMUR_DEVICES").unwrap_or_else(|_| "devices.json".into()));
    let shared = Arc::new(Shared::load(devices_path.clone())?);

    let listener = TcpListener::bind(addr).await?;
    info!(
        "murmur-server on {addr}, registry {}",
        devices_path.display()
    );

    loop {
        let (stream, peer) = listener.accept().await?;
        let shared = shared.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_conn(stream, shared).await {
                debug!("conn {peer}: {e:#}");
            }
        });
    }
}

async fn handle_conn(stream: TcpStream, shared: Arc<Shared>) -> anyhow::Result<()> {
    let ws = tokio_tungstenite::accept_async(stream).await?;
    let (mut sink, mut source) = ws.split();

    // First message must be a Hello envelope.
    let hello = match source.next().await {
        Some(Ok(Ws::Text(t))) => {
            serde_json::from_str::<Control>(&t).context("first message must be Control::Hello")?
        }
        _ => bail!("closed before hello"),
    };

    let (device_id, pairing) = match hello {
        Control::Hello {
            device_id,
            name,
            role,
            token_hex,
        } => match role {
            HelloRole::Auth => {
                let token = token_hex
                    .and_then(|h| murmur_proto::AuthToken::from_hex(&h).ok())
                    .context("missing token")?;
                let ok = {
                    let devices = shared.devices.lock().unwrap();
                    matches!(devices.get(&device_id), Some(r) if !r.revoked && r.token_hash_hex == token.hash_hex())
                };
                if !ok {
                    send_ctl(
                        &mut sink,
                        &Control::HelloErr {
                            reason: "auth failed".into(),
                        },
                    )
                    .await?;
                    bail!("auth failed for {device_id}");
                }
                info!("device {device_id} ({name}) authed");
                (device_id, false)
            }
            HelloRole::Pair => {
                info!("pairing conn from {device_id} ({name})");
                (device_id, true)
            }
        },
        _ => bail!("expected Hello"),
    };
    send_ctl(&mut sink, &Control::HelloOk).await?;

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    shared
        .conns
        .lock()
        .unwrap()
        .insert(device_id, Conn { tx, pairing });

    // Writer: everything queued for this conn goes out as text frames.
    let writer = tokio::spawn(async move {
        while let Some(json) = rx.recv().await {
            if sink.send(Ws::Text(json.into())).await.is_err() {
                break;
            }
        }
    });

    // Reader: route by `to`.
    let read_res = async {
        while let Some(msg) = source.next().await {
            let Ws::Text(t) = msg? else { continue };
            let mut env = match Envelope::from_json(&t) {
                Ok(e) => e,
                Err(e) => {
                    warn!("{device_id}: bad envelope: {e}");
                    continue;
                }
            };
            env.from = device_id; // never trust the client's claim

            match &env.to {
                Route::Server => handle_control(&shared, device_id, pairing, &env).await?,
                Route::Device(dst) => shared.send_to(dst, &env),
                Route::All => {
                    if pairing {
                        // Pairing conns may only broadcast cleartext pair frames.
                        match env.control_payload() {
                            Ok(Control::PairHello { .. }) | Ok(Control::PairData { .. }) => {
                                shared.broadcast(&device_id, &env, true)
                            }
                            _ => warn!("{device_id}: pairing conn tried non-pair broadcast"),
                        }
                    } else {
                        shared.broadcast(&device_id, &env, false);
                    }
                }
            }
        }
        anyhow::Ok(())
    }
    .await;

    shared.drop_conn(&device_id);
    writer.abort();
    info!("device {device_id} disconnected");
    read_res
}

async fn handle_control(
    shared: &Arc<Shared>,
    from: DeviceId,
    pairing: bool,
    env: &Envelope,
) -> anyhow::Result<()> {
    let ctl = match env.control_payload() {
        Ok(c) => c,
        Err(e) => {
            warn!("{from}: bad control payload: {e}");
            return Ok(());
        }
    };
    if pairing {
        warn!("{from}: pairing conn sent server op");
        return Ok(());
    }
    match ctl {
        Control::RegisterDevice {
            device_id,
            name,
            token_hash_hex,
        } => {
            info!("{from} registered device {device_id} ({name})");
            shared.devices.lock().unwrap().insert(
                device_id,
                DeviceRec {
                    name,
                    token_hash_hex,
                    revoked: false,
                },
            );
            shared.persist();
        }
        Control::RevokeDevice { device_id } => {
            info!("{from} revoked device {device_id}");
            if let Some(rec) = shared.devices.lock().unwrap().get_mut(&device_id) {
                rec.revoked = true;
            }
            shared.persist();
            shared.drop_conn(&device_id); // live conn dies immediately
        }
        Control::ListDevices => {
            let devices = {
                let devs = shared.devices.lock().unwrap();
                let conns = shared.conns.lock().unwrap();
                devs.iter()
                    .map(|(id, r)| DeviceInfo {
                        device_id: *id,
                        name: r.name.clone(),
                        revoked: r.revoked,
                        online: conns.contains_key(id),
                    })
                    .collect()
            };
            let reply = Envelope::control(
                DeviceId([0; 8]),
                Route::device(from),
                0,
                &Control::Devices { devices },
            )?;
            shared.send_to(&from, &reply);
        }
        other => debug!("{from}: unhandled control {other:?}"),
    }
    Ok(())
}

async fn send_ctl(
    sink: &mut futures_util::stream::SplitSink<tokio_tungstenite::WebSocketStream<TcpStream>, Ws>,
    ctl: &Control,
) -> anyhow::Result<()> {
    sink.send(Ws::Text(serde_json::to_string(ctl)?.into()))
        .await?;
    Ok(())
}
