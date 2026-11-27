//! Reconnecting WSS client. Sends Hello, drains the outbound envelope queue,
//! decrypts inbound envelopes and dispatches them through the feature hub.

use crate::feature::Hub;
use crate::state::Shared;
use futures_util::{SinkExt, StreamExt};
use murmur_proto::{Control, control::HelloRole};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as Ws;
use tracing::{debug, info, warn};

const BACKOFF_MAX: Duration = Duration::from_secs(15);
const REVOKED_RETRY: Duration = Duration::from_secs(1800);

pub async fn run(
    shared: Arc<Shared>,
    hub: Arc<Hub>,
    mut out_rx: mpsc::UnboundedReceiver<murmur_proto::Envelope>,
    mut lo_rx: mpsc::UnboundedReceiver<(u64, murmur_proto::Envelope)>,
) -> anyhow::Result<()> {
    let mut backoff = Duration::from_secs(1);
    loop {
        match connect_once(&shared, &hub, &mut out_rx, &mut lo_rx).await {
            Ok(()) => info!("connection closed cleanly"),
            Err(e) => warn!("connection lost: {e:#}"),
        }
        shared.online.store(false, std::sync::atomic::Ordering::Relaxed);
        shared.note_presence_offline();
        // Revoked is final until re-paired — check back rarely instead
        // of hammering the relay every minute.
        let revoked = shared.revoked.load(std::sync::atomic::Ordering::Relaxed);
        if revoked {
            warn!("this device was revoked — re-pair it: murmurd pair --code …");
        }
        tokio::time::sleep(if revoked { REVOKED_RETRY } else { backoff }).await;
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

async fn connect_once(
    shared: &Arc<Shared>,
    hub: &Hub,
    out_rx: &mut mpsc::UnboundedReceiver<murmur_proto::Envelope>,
    lo_rx: &mut mpsc::UnboundedReceiver<(u64, murmur_proto::Envelope)>,
) -> anyhow::Result<()> {
    let (ws, _) = tokio_tungstenite::connect_async(&shared.cfg.server_url).await?;
    let (mut sink, mut source) = ws.split();

    let hello = serde_json::to_string(&Control::Hello {
        device_id: shared.id,
        name: shared.cfg.name.clone(),
        role: HelloRole::Auth,
        token_hex: Some(shared.token_hex()),
    })?;
    sink.send(Ws::Text(hello.into())).await?;

    match source.next().await {
        Some(Ok(Ws::Text(t))) => match serde_json::from_str::<Control>(&t) {
            Ok(Control::HelloOk) => {
                shared.revoked.store(false, std::sync::atomic::Ordering::Relaxed);
                shared.online.store(true, std::sync::atomic::Ordering::Relaxed);
                info!("authed to {}", shared.cfg.server_url);
            }
            Ok(Control::HelloErr { reason }) => {
                if reason == "revoked" || reason == "unknown device" {
                    shared.revoked.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                anyhow::bail!("auth rejected: {reason}")
            }
            other => anyhow::bail!("unexpected hello reply: {other:?}"),
        },
        _ => anyhow::bail!("no hello reply"),
    }

    // Reannounce presence + identity after every (re)connect, and pull
    // the device list — a revocation that happened while we were away
    // still has to trigger a rotation.
    shared.broadcast_presence();
    shared.announce_identity();
    let _ = shared.send_control(&Control::ListDevices);

    loop {
        tokio::select! {
            // Commands/control always beat queued state — biased select
            // polls the hi channel first.
            biased;
            env = out_rx.recv() => {
                let Some(env) = env else { return Ok(()) };
                sink.send(Ws::Text(env.to_json().into())).await?;
            }
            // Replaceable state: drain the backlog and send only the
            // newest frame per slot — a queue of stale snapshots
            // collapses to one fresh send.
            env = lo_rx.recv() => {
                let Some((slot, env)) = env else { return Ok(()) };
                let mut latest = std::collections::HashMap::new();
                latest.insert(slot, env);
                while let Ok((s, e)) = lo_rx.try_recv() {
                    latest.insert(s, e);
                }
                for (_, e) in latest {
                    sink.send(Ws::Text(e.to_json().into())).await?;
                }
            }
            // Inbound: WS → decrypt → feature hub.
            msg = source.next() => {
                let Some(msg) = msg else { return Ok(()) };
                let Ws::Text(t) = msg? else { continue };
                let env = match murmur_proto::Envelope::from_json(&t) {
                    Ok(e) => e,
                    Err(e) => {
                        debug!("bad envelope: {e}");
                        continue;
                    }
                };
                // Cleartext control first: server replies (Devices, Error)
                // and unicast pair frames. Anything that doesn't parse is
                // an E2E payload.
                match env.control_payload() {
                    Ok(ctl) => {
                        if let Control::Devices { family_name, .. } = &ctl {
                            *shared.family_name.lock().unwrap() = family_name.clone();
                            let mut s = shared.secrets.lock().unwrap();
                            if s.family_name != *family_name {
                                s.family_name = family_name.clone();
                                let _ = s.save();
                            }
                        }
                        let _ = shared.ctl_events.send((env.from, ctl));
                    }
                    Err(_) => {
                        let opened = env.open(&shared.keys.read().unwrap().current);
                        match opened {
                            Ok(msg) => {
                                debug!("{} → {msg:?}", env.from);
                                // A beacon that arrived via the relay
                                // is still a dial hint — it carries the
                                // sender's LAN address explicitly. Any
                                // kind feeds the dialer; the direction
                                // rule lives in dial_loop.
                                if let murmur_proto::Message::LanBeacon {
                                    tcp_port,
                                    kind,
                                    lan_addr: Some(ip),
                                } = &msg
                                {
                                    if let Ok(ip) = ip.parse() {
                                        let _ = shared.lan_dial.send(crate::lan::DialHint {
                                            peer: Some(env.from),
                                            addr: ip,
                                            port: *tcp_port,
                                            kind: *kind,
                                            trusted: false,
                                            transport: crate::lan::Transport::Lan,
                                        });
                                    }
                                }
                                // Dedupe before dispatch — one shared
                                // invariant for every transport.
                                shared.ingest(&hub, env.from, env.seq, &msg);
                            }
                            // An old key opening it = the sender missed a
                            // rotation. Never dispatch it — just re-key.
                            Err(e) => {
                                let old = shared
                                    .keys
                                    .read()
                                    .unwrap()
                                    .old()
                                    .iter()
                                    .find(|k| env.open(k).is_ok())
                                    .cloned();
                                match old {
                                    Some(k) => shared.on_stale(env.from, &k, None),
                                    None => warn!("decrypt from {} failed: {e}", env.from),
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
