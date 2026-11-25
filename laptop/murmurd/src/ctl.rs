//! murmurctl control socket: $XDG_RUNTIME_DIR/murmur.sock, newline-delimited
//! JSON. All daemon ops go through here — no other UI on laptops.

use crate::feature::{Feature, FeatureCtx};
use crate::state::Shared;
use murmur_proto::Control;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tracing::{info, warn};

#[derive(Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum Req {
    Status,
    Devices,
    Revoke {
        device_id: String,
    },
    Invite,
    Rotate,
    Pair,
    PairAccept {
        session: String,
    },
    PairConfirm {
        session: String,
    },
    /// Broadcast a raw `Message` (debug seam — `murmurctl send` reads the
    /// JSON from stdin).
    Send {
        msg_json: serde_json::Value,
    },
    /// Manual LAN dial: `<ip>` or `<ip:port>` — the rendezvous for
    /// networks where broadcast discovery can't pass (guest Wi-Fi
    /// client isolation, cross-subnet unicast).
    Dial {
        addr: String,
    },
}

#[derive(Serialize)]
struct Res {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl Res {
    fn ok(data: serde_json::Value) -> Self {
        Self {
            ok: true,
            data: Some(data),
            error: None,
        }
    }

    fn err(e: impl std::fmt::Display) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(e.to_string()),
        }
    }
}

fn sock_path() -> std::path::PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join("murmur.sock")
}

pub fn feature() -> Feature {
    Feature::new("ctl", None, |ctx| Box::pin(run(ctx)))
}

async fn run(ctx: FeatureCtx) -> anyhow::Result<()> {
    let shared = ctx.shared;
    let path = sock_path();
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    // The socket grants full daemon control — owner only. Matters when
    // XDG_RUNTIME_DIR is absent and we land in world-readable /tmp.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    info!("ctl socket: {}", path.display());

    loop {
        let (stream, _) = listener.accept().await?;
        let shared = shared.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(stream, shared).await {
                warn!("ctl: {e:#}");
            }
        });
    }
}

async fn handle(stream: tokio::net::UnixStream, shared: Arc<Shared>) -> anyhow::Result<()> {
    let (r, mut w) = stream.into_split();
    let mut lines = BufReader::new(r).lines();
    while let Some(line) = lines.next_line().await? {
        let res = match serde_json::from_str::<Req>(&line) {
            Ok(req) => dispatch(&shared, req).await,
            Err(e) => {
                warn!("ctl parse: {e}");
                Res::err("bad request")
            }
        };
        w.write_all(serde_json::to_string(&res)?.as_bytes()).await?;
        w.write_all(b"\n").await?;
    }
    Ok(())
}

async fn dispatch(shared: &Arc<Shared>, req: Req) -> Res {
    match req {
        Req::Status => {
            let presence = {
                let m = shared.presence.lock().unwrap();
                m.iter()
                    .map(|(id, (s, ts))| {
                        serde_json::json!({
                            "device": id.to_string(), "state": s, "ts": ts
                        })
                    })
                    .collect::<Vec<_>>()
            };
            // "online" = the family can reach us: the relay for a
            // serverful family, a live LAN link for a serverless one.
            let online = shared.online.load(std::sync::atomic::Ordering::Relaxed)
                || (shared.cfg.family_kind == murmur_proto::FamilyKind::Serverless
                    && !shared.lan_peers().is_empty());
            Res::ok(serde_json::json!({
                "self": shared.id.to_string(),
                "name": shared.cfg.name,
                "server": shared.cfg.server_url,
                "family_kind": shared.cfg.family_kind,
                "family_name": shared.family_name.lock().unwrap().clone(),
                "online": online,
                "revoked": shared.revoked.load(std::sync::atomic::Ordering::Relaxed),
                "lan_peers": shared.lan_peers().iter().map(|i| i.to_string()).collect::<Vec<_>>(),
                "lan_rtt_ms": shared.lan_peers().iter()
                    .map(|i| (i.to_string(), shared.lan_rtt(i)))
                    .collect::<std::collections::HashMap<_, _>>(),
                "active_node": shared.active_node().map(|i| i.to_string()),
                "key_epoch": shared.keys.read().unwrap().epoch,
                "members": shared.roster.lock().unwrap().len(),
                "presence": presence,
            }))
        }
        Req::Devices => {
            // Serverless: there is no registry to ask — the roster we
            // pinned from Identity gossip IS the family list.
            if shared.cfg.family_kind == murmur_proto::FamilyKind::Serverless {
                let peers: std::collections::HashSet<murmur_proto::DeviceId> =
                    shared.lan_peers().into_iter().collect();
                let revoked = &shared.keys.read().unwrap().revoked;
                let roster = shared.roster.lock().unwrap();
                let devices: Vec<_> = roster
                    .iter()
                    .map(|(id, m)| {
                        serde_json::json!({
                            "device_id": id.to_string(),
                            "name": m.name.clone().unwrap_or_else(|| id.to_string()),
                            "online": peers.contains(id),
                            "revoked": revoked.contains(id),
                            "rtt_ms": peers.contains(id).then(|| shared.lan_rtt(id)),
                            "kind": m.kind,
                        })
                    })
                    .collect();
                let family = shared.family_name.lock().unwrap().clone();
                return Res::ok(serde_json::json!({
                    "self": shared.id.to_string(),
                    "family": family,
                    "devices": devices,
                }));
            }
            let mut rx = shared.ctl_events.subscribe();
            match shared.send_control(&Control::ListDevices) {
                Ok(()) => {
                    let devs = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                        loop {
                            match rx.recv().await {
                                Ok((_, Control::Devices {
                                    devices,
                                    family_name,
                                })) => return Some((devices, family_name)),
                                Ok(_) => continue,
                                // Skip lagged gaps; Closed means nobody
                                // is sending anymore — give up.
                                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                    return None;
                                }
                                Err(_) => continue,
                            }
                        }
                    })
                    .await;
                    match devs {
                        Ok(Some((d, fam))) => Res::ok(serde_json::json!({
                            "self": shared.id.to_string(),
                            "family": fam,
                            "devices": d,
                        })),
                        _ => Res::err("no reply from server (offline?)"),
                    }
                }
                Err(e) => Res::err(e),
            }
        }
        Req::Revoke { device_id } => match device_id.parse::<murmur_proto::DeviceId>() {
            Ok(device_id) => {
                let mut rx = shared.ctl_events.subscribe();
                match shared.send_control(&Control::RevokeDevice { device_id }) {
                    Ok(()) => {
                        // The server only honours self-revoke; anything else
                        // draws a `Control::Error` — surface it instead of
                        // pretending the kick worked.
                        let err = tokio::time::timeout(
                            std::time::Duration::from_secs(3),
                            async {
                                loop {
                                    match rx.recv().await {
                                        Ok((_, Control::Error { reason })) => {
                                            return Some(reason);
                                        }
                                        Ok(_) => continue,
                                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                            return None;
                                        }
                                        Err(_) => continue,
                                    }
                                }
                            },
                        )
                        .await;
                        match err {
                            Ok(Some(reason)) => Res::err(reason),
                            _ => Res::ok(serde_json::json!(null)),
                        }
                    }
                    Err(e) => Res::err(e),
                }
            }
            Err(e) => Res::err(e),
        },
        Req::Invite => match shared.invite() {
            Ok(invite) => Res::ok(serde_json::json!({
                "code": invite.encode(),
                "short": invite.code,
                "session": invite.session.to_string(),
                // Serverful: the relay URL. Serverless: `host:port` of
                // the ephemeral direct listener — the joiner dials it.
                "server": invite.server_url,
            })),
            Err(e) => Res::err(e),
        },
        Req::Rotate => match shared.rotate(vec![]) {
            Ok(epoch) => Res::ok(serde_json::json!({"epoch": epoch})),
            Err(e) => Res::err(e),
        },
        Req::Pair => Res::ok(serde_json::json!({"pending": shared.pair_pending()})),
        Req::PairAccept { session } => match session.parse::<murmur_proto::DeviceId>() {
            Ok(session) => match shared.pair_accept(session) {
                Ok(()) => Res::ok(serde_json::json!(null)),
                Err(e) => Res::err(e),
            },
            Err(e) => Res::err(e),
        },
        Req::PairConfirm { session } => match session.parse::<murmur_proto::DeviceId>() {
            Ok(session) => match shared.pair_confirm(session) {
                Ok(device_id) => Res::ok(serde_json::json!({"device_id": device_id.to_string()})),
                Err(e) => Res::err(e),
            },
            Err(e) => Res::err(e),
        },
        Req::Send { msg_json } => match serde_json::from_value::<murmur_proto::Message>(msg_json) {
            Ok(msg) => match shared.send(murmur_proto::Route::All, &msg) {
                Ok(()) => Res::ok(serde_json::json!(null)),
                Err(e) => Res::err(e),
            },
            Err(e) => Res::err(format!("not a Message: {e}")),
        },
        Req::Dial { addr } => {
            let target = addr
                .parse::<std::net::SocketAddr>()
                .map(|s| (s.ip(), s.port()))
                .or_else(|_| {
                    addr.parse::<std::net::IpAddr>()
                        .map(|ip| (ip, crate::lan::LINK_PORT))
                });
            match target {
                Ok((ip, port)) => {
                    let _ = shared.lan_dial.send(crate::lan::DialHint {
                        peer: None,
                        addr: ip,
                        port,
                        kind: murmur_proto::DeviceKind::Phone,
                    });
                    Res::ok(serde_json::json!({"dialing": format!("{ip}:{port}")}))
                }
                Err(_) => Res::err("dial expects <ip> or <ip:port>"),
            }
        }
    }
}
