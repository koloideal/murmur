//! murmur-server: dumb authenticated relay.
//!
//! - Every device connects outbound with WS and says `Control::Hello` first.
//! - `role=auth`: token hash checked against the registry, conn bound to id.
//! - `role=pair`: pending pairing conn; may only exchange Pair* frames until
//!   an authed device registers it.
//! - Frames are routed by `to`: `"*"` fans out, `"<id>"` unicast,
//!   `"@server"` is the device control plane (registry ops from devices).
//! - `from` is always overwritten with the authenticated id.
//! - No queues, no storage of payloads. Live-only.
//!
//! Admin ops go through a unix socket (newline-JSON, same protocol shape as
//! murmurctl → murmurd). devices.json is write-only output: loaded once at
//! startup, persisted on every mutation. The socket is the only seam.
//!
//! Commands (same binary):
//!   murmur-server                    — run the relay
//!   murmur-server add-device --name X
//!   murmur-server list-devices
//!   murmur-server revoke --id ID

use anyhow::{Context, bail};
use futures_util::{SinkExt, StreamExt};
use murmur_proto::{Control, DeviceId, Envelope, Route, control::DeviceInfo, control::HelloRole};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UnixListener, UnixStream};
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
    /// Fire to make the reader loop exit (revocation, shutdown).
    close: mpsc::Sender<()>,
    pairing: bool,
    /// Unique registration mark — a dying conn must only remove the
    /// registry entry if it still owns it (see drop_conn_if).
    epoch: u64,
}

static EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

struct Shared {
    devices_path: PathBuf,
    /// Persistent registry, mirrored to devices.json on mutation.
    devices: Mutex<HashMap<DeviceId, DeviceRec>>,
    /// Live connections.
    conns: Mutex<HashMap<DeviceId, Conn>>,
    /// Open pair sessions: session → inviter. Lets a joiner reach the
    /// inviter through @server without knowing its device id.
    open_pairs: Mutex<HashMap<DeviceId, DeviceId>>,
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
            open_pairs: Mutex::new(HashMap::new()),
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

    fn register(&self, id: DeviceId, name: String, token_hash_hex: String) {
        self.devices.lock().unwrap().insert(
            id,
            DeviceRec {
                name,
                token_hash_hex,
                revoked: false,
            },
        );
        self.persist();
    }

    fn revoke(&self, id: &DeviceId) -> bool {
        let mut devices = self.devices.lock().unwrap();
        match devices.get_mut(id) {
            Some(rec) => {
                rec.revoked = true;
                drop(devices);
                self.persist();
                self.drop_conn(id); // live conn dies immediately
                true
            }
            None => false,
        }
    }

    fn device_list(&self) -> Vec<DeviceInfo> {
        let devs = self.devices.lock().unwrap();
        let conns = self.conns.lock().unwrap();
        devs.iter()
            .map(|(id, r)| DeviceInfo {
                device_id: *id,
                name: r.name.clone(),
                revoked: r.revoked,
                online: conns.contains_key(id),
            })
            .collect()
    }

    fn check_auth(&self, id: &DeviceId, token_hash_hex: &str) -> bool {
        let devices = self.devices.lock().unwrap();
        matches!(devices.get(id), Some(r) if !r.revoked && r.token_hash_hex == token_hash_hex)
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

    /// Remove the conn from the registry, close it, and drop any pair
    /// sessions this conn had opened.
    fn drop_conn(&self, id: &DeviceId) {
        if let Some(c) = self.conns.lock().unwrap().remove(id) {
            let _ = c.close.try_send(());
        }
        self.open_pairs.lock().unwrap().retain(|_, inv| inv != id);
    }

    /// Like drop_conn, but only if the entry still belongs to the conn
    /// that is cleaning itself up. Without this, a dying conn that was
    /// already displaced would kill the fresh conn that took its id.
    fn drop_conn_if(&self, id: &DeviceId, epoch: u64) {
        let mut conns = self.conns.lock().unwrap();
        if conns.get(id).is_some_and(|c| c.epoch == epoch) {
            if let Some(c) = conns.remove(id) {
                let _ = c.close.try_send(());
            }
            drop(conns);
            self.open_pairs.lock().unwrap().retain(|_, inv| inv != id);
        }
    }
}

// ---------- admin socket (CLI ↔ running server) ----------

fn devices_path() -> PathBuf {
    PathBuf::from(std::env::var("MURMUR_DEVICES").unwrap_or_else(|_| "devices.json".into()))
}

fn admin_sock_path() -> PathBuf {
    std::env::var_os("MURMUR_ADMIN_SOCK")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("XDG_RUNTIME_DIR")
                .map(PathBuf::from)
                .map(|p| p.join("murmur-server.sock"))
        })
        .unwrap_or_else(|| PathBuf::from("/tmp/murmur-server.sock"))
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum AdminReq {
    AddDevice { name: String },
    ListDevices,
    Revoke { device_id: DeviceId },
}

/// One roundtrip against the running server's admin socket.
fn admin_req(req: &AdminReq) -> anyhow::Result<serde_json::Value> {
    let path = admin_sock_path();
    let mut stream = std::os::unix::net::UnixStream::connect(&path)
        .with_context(|| format!("connect {} — is murmur-server running?", path.display()))?;
    stream.write_all(serde_json::to_string(req)?.as_bytes())?;
    stream.write_all(b"\n")?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    Ok(serde_json::from_str(line.trim())?)
}

/// Admin task: same ops as the CLI, performed against live state.
async fn handle_admin(stream: UnixStream, shared: Arc<Shared>) -> anyhow::Result<()> {
    let (r, mut w) = stream.into_split();
    let mut lines = tokio::io::BufReader::new(r).lines();
    while let Some(line) = lines.next_line().await? {
        let res = match serde_json::from_str::<AdminReq>(&line) {
            Ok(req) => admin_dispatch(&shared, req),
            Err(e) => serde_json::json!({"ok": false, "error": e.to_string()}),
        };
        w.write_all(serde_json::to_string(&res)?.as_bytes()).await?;
        w.write_all(b"\n").await?;
    }
    Ok(())
}

fn admin_dispatch(shared: &Shared, req: AdminReq) -> serde_json::Value {
    match req {
        AdminReq::AddDevice { name } => {
            let id = DeviceId::random();
            let token = murmur_proto::AuthToken::random();
            shared.register(id, name, token.hash_hex());
            serde_json::json!({
                "ok": true,
                "data": {"device_id": id.to_string(), "token": token.to_hex()}
            })
        }
        AdminReq::ListDevices => {
            serde_json::json!({"ok": true, "data": shared.device_list()})
        }
        AdminReq::Revoke { device_id } => {
            if shared.revoke(&device_id) {
                serde_json::json!({"ok": true})
            } else {
                serde_json::json!({"ok": false, "error": format!("no such device: {device_id}")})
            }
        }
    }
}

// ---------- CLI ----------

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => serve().await,
        Some("add-device") => {
            let name = flag_value(&args[1..], "--name")
                .or_else(|| args.get(1).cloned())
                .context("usage: murmur-server add-device --name NAME")?;
            let res = admin_req(&AdminReq::AddDevice { name })?;
            let d = &res["data"];
            println!("device_id: {}", d["device_id"].as_str().unwrap());
            println!("token: {}", d["token"].as_str().unwrap());
            eprintln!("(token is shown once — store it in secrets.toml on the device)");
            Ok(())
        }
        Some("list-devices") => {
            let res = admin_req(&AdminReq::ListDevices)?;
            for d in res["data"].as_array().into_iter().flatten() {
                println!(
                    "{}  {}  {}  {}",
                    d["device_id"].as_str().unwrap_or("?"),
                    d["name"].as_str().unwrap_or("?"),
                    if d["revoked"].as_bool() == Some(true) {
                        "revoked"
                    } else {
                        "active"
                    },
                    if d["online"].as_bool() == Some(true) {
                        "online"
                    } else {
                        "offline"
                    },
                );
            }
            Ok(())
        }
        Some("revoke") => {
            let id: DeviceId = flag_value(&args[1..], "--id")
                .or_else(|| args.get(1).cloned())
                .context("usage: murmur-server revoke --id DEVICE_ID")?
                .parse()?;
            let res = admin_req(&AdminReq::Revoke { device_id: id })?;
            match res["ok"].as_bool() {
                Some(true) => println!("{id} revoked — live connection dropped"),
                _ => bail!("{}", res["error"].as_str().unwrap_or("failed")),
            }
            Ok(())
        }
        Some("install") => install(&args[1..]),
        Some(other) => bail!(
            "unknown command {other:?} — usage: murmur-server [add-device|list-devices|revoke|install]"
        ),
    }
}

/// `murmur-server install [--addr A] [--user]` — write a systemd unit and
/// enable the relay. System unit when /etc/systemd is writable (DynamicUser,
/// state under /var/lib/murmur, admin socket under /run/murmur); otherwise a
/// user unit + linger. Always prints the Caddy reverse-proxy snippet —
/// WS upgrade needs no special handling there.
fn install(args: &[String]) -> anyhow::Result<()> {
    let addr = flag_value(args, "--addr").unwrap_or_else(|| "127.0.0.1:8770".into());
    let exe = std::env::current_exe()?.display().to_string();
    let unit_dir = PathBuf::from("/etc/systemd/system");
    let user_mode =
        args.iter().any(|a| a == "--user") || std::fs::metadata(&unit_dir.join(".")).is_err() || {
            // Cheap writability probe: can we create a file in the unit dir?
            let probe = unit_dir.join(".murmur-probe");
            match std::fs::write(&probe, "") {
                Ok(()) => {
                    let _ = std::fs::remove_file(&probe);
                    false
                }
                Err(_) => true,
            }
        };

    if user_mode {
        install_user(&exe, &addr)
    } else {
        install_system(&exe, &addr)
    }?;
    print_caddy_hint(&addr);
    Ok(())
}

fn install_system(exe: &str, addr: &str) -> anyhow::Result<()> {
    let unit = format!(
        "[Unit]\n\
         Description=murmur relay server\n\
         After=network-online.target\n\
         Wants=network-online.target\n\n\
         [Service]\n\
         DynamicUser=yes\n\
         StateDirectory=murmur\n\
         RuntimeDirectory=murmur\n\
         Environment=MURMUR_ADDR={addr}\n\
         Environment=MURMUR_DEVICES=/var/lib/murmur/devices.json\n\
         Environment=MURMUR_ADMIN_SOCK=/run/murmur/admin.sock\n\
         ExecStart={exe}\n\
         Restart=on-failure\n\
         RestartSec=3\n\
         NoNewPrivileges=yes\n\
         PrivateTmp=yes\n\
         ProtectSystem=strict\n\n\
         [Install]\n\
         WantedBy=multi-user.target\n"
    );
    let path = PathBuf::from("/etc/systemd/system/murmur-server.service");
    std::fs::write(&path, &unit).context("write unit file")?;
    run("systemctl", &["daemon-reload"])?;
    run("systemctl", &["enable", "--now", "murmur-server.service"])?;
    eprintln!("installed + started: {path:?}");
    eprintln!(
        "admin commands need: MURMUR_ADMIN_SOCK=/run/murmur/admin.sock sudo -E {exe} <cmd>\n\
         (the socket is owned by the dynamic service user)"
    );
    Ok(())
}

fn install_user(exe: &str, addr: &str) -> anyhow::Result<()> {
    let unit = format!(
        "[Unit]\n\
         Description=murmur relay server\n\
         After=network-online.target\n\n\
         [Service]\n\
         Environment=MURMUR_ADDR={addr}\n\
         Environment=MURMUR_DEVICES=%h/.local/share/murmur-server/devices.json\n\
         ExecStart={exe}\n\
         Restart=on-failure\n\
         RestartSec=3\n\n\
         [Install]\n\
         WantedBy=default.target\n"
    );
    let dir =
        PathBuf::from(std::env::var_os("HOME").context("no HOME")?).join(".config/systemd/user");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("murmur-server.service");
    std::fs::write(&path, &unit)?;
    std::fs::create_dir_all(
        PathBuf::from(std::env::var_os("HOME").unwrap()).join(".local/share/murmur-server"),
    )?;
    run("systemctl", &["--user", "daemon-reload"])?;
    run(
        "systemctl",
        &["--user", "enable", "--now", "murmur-server.service"],
    )?;
    // Survives logout; failure is non-fatal — needs polkit/root on some systems.
    if run("loginctl", &["enable-linger"]).is_err() {
        eprintln!("hint: run `loginctl enable-linger $USER` so it survives logout");
    }
    eprintln!("installed + started (user unit): {path:?}");
    Ok(())
}

fn run(cmd: &str, args: &[&str]) -> anyhow::Result<()> {
    let status = std::process::Command::new(cmd)
        .args(args)
        .status()
        .with_context(|| format!("spawn {cmd}"))?;
    status
        .success()
        .then_some(())
        .context(format!("{cmd} {} exited {status}", args.join(" ")))
}

fn print_caddy_hint(addr: &str) {
    eprintln!(
        "\nTLS + public endpoint — one Caddyfile block (WebSocket upgrade is proxied as-is):\n\
         \n  murmur.example.org {{\n      reverse_proxy {addr}\n  }}\n"
    );
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

// ---------- relay ----------

async fn serve() -> anyhow::Result<()> {
    let addr: SocketAddr = std::env::var("MURMUR_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8770".into())
        .parse()
        .context("MURMUR_ADDR")?;
    let shared = Arc::new(Shared::load(devices_path())?);

    let listener = TcpListener::bind(addr).await?;
    let admin_path = admin_sock_path();
    let _ = std::fs::remove_file(&admin_path);
    let admin_listener = UnixListener::bind(&admin_path)?;
    // The socket grants full registry control — owner only.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&admin_path, std::fs::Permissions::from_mode(0o600))?;
    }
    info!(
        "murmur-server on {addr}, registry {}, admin {}",
        shared.devices_path.display(),
        admin_path.display()
    );

    loop {
        tokio::select! {
            acc = listener.accept() => {
                let (stream, peer) = acc?;
                let shared = shared.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_conn(stream, shared).await {
                        debug!("conn {peer}: {e:#}");
                    }
                });
            }
            acc = admin_listener.accept() => {
                let (stream, _) = acc?;
                let shared = shared.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_admin(stream, shared).await {
                        warn!("admin conn: {e:#}");
                    }
                });
            }
        }
    }
}

async fn handle_conn(stream: TcpStream, shared: Arc<Shared>) -> anyhow::Result<()> {
    let ws = tokio_tungstenite::accept_async(stream).await?;
    let (mut sink, mut source) = ws.split();

    // First message must be a Hello.
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
                if !shared.check_auth(&device_id, &token.hash_hex()) {
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
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let (close_tx, mut close_rx) = mpsc::channel::<()>(1);

    // A pairing conn may only displace another pairing conn — a session
    // id must never kick a live authed device offline.
    let occupied = shared
        .conns
        .lock()
        .unwrap()
        .get(&device_id)
        .is_some_and(|c| pairing && !c.pairing);
    if occupied {
        send_ctl(
            &mut sink,
            &Control::HelloErr {
                reason: "device id already connected".into(),
            },
        )
        .await
        .ok();
        bail!("pairing conn claimed live device id {device_id}");
    }

    let epoch = EPOCH.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    {
        let mut conns = shared.conns.lock().unwrap();
        if let Some(old) = conns.remove(&device_id) {
            // Same conn id claimed twice (duplicate login or reused pair
            // session): newest wins, the stale conn is terminated.
            let _ = old.close.try_send(());
        }
        conns.insert(
            device_id,
            Conn {
                tx,
                close: close_tx,
                pairing,
                epoch,
            },
        );
    }
    send_ctl(&mut sink, &Control::HelloOk).await?;

    // Writer: queued frames + periodic WS pings (NATs drop silent conns;
    // phones on mobile networks depend on this).
    let writer = tokio::spawn(async move {
        let mut ping = tokio::time::interval(std::time::Duration::from_secs(25));
        ping.tick().await; // first tick fires immediately — skip it
        loop {
            tokio::select! {
                json = rx.recv() => {
                    let Some(json) = json else { break };
                    if sink.send(Ws::Text(json.into())).await.is_err() {
                        break;
                    }
                }
                _ = ping.tick() => {
                    if sink.send(Ws::Ping(Vec::new().into())).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    // Reader: route by `to`; exits on close signal (revocation) or EOF.
    let read_res = async {
        loop {
            let msg = tokio::select! {
                _ = close_rx.recv() => {
                    info!("{device_id}: conn closed by server");
                    break;
                }
                msg = source.next() => {
                    let Some(msg) = msg else { break };
                    msg
                }
            };
            let Ws::Text(t) = msg? else { continue };
            let mut env = match Envelope::from_json(&t) {
                Ok(e) => e,
                Err(e) => {
                    warn!("{device_id}: bad envelope: {e}");
                    continue;
                }
            };
            env.from = device_id; // never trust the client's claim

            if pairing {
                // A pairing conn may send exactly one thing: a cleartext
                // PairHello to @server, which forwards it to the inviter
                // that opened the session. No broadcast, no unicast,
                // no other server ops.
                let ok = matches!(env.to, Route::Server)
                    && matches!(env.control_payload(), Ok(Control::PairHello { .. }));
                if !ok {
                    warn!("{device_id}: pairing conn sent a non-pair frame");
                    continue;
                }
            }

            match &env.to {
                Route::Server => handle_control(&shared, device_id, pairing, &env).await?,
                Route::Device(dst) => shared.send_to(dst, &env),
                Route::All => {
                    if pairing {
                        warn!("{device_id}: pairing conn tried broadcast");
                    } else {
                        shared.broadcast(&device_id, &env, true);
                    }
                }
            }
        }
        anyhow::Ok(())
    }
    .await;

    shared.drop_conn_if(&device_id, epoch);
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
    match ctl {
        // A pairing conn's only @server op: the inviter registered this
        // session via PairOpen — forward the hello to it (env.from is
        // already the session id).
        Control::PairHello { .. } if pairing => {
            let inviter = shared.open_pairs.lock().unwrap().get(&from).copied();
            match inviter {
                Some(inv) => shared.send_to(&inv, env),
                None => warn!("{from}: PairHello for an unknown/expired session"),
            }
        }
        _ if pairing => {
            warn!("{from}: pairing conn sent server op");
        }
        Control::PairOpen { session } => {
            shared.open_pairs.lock().unwrap().insert(session, from);
            debug!("{from} opened pair session {session}");
        }
        Control::RegisterDevice {
            device_id,
            name,
            token_hash_hex,
        } => {
            info!("{from} registered device {device_id} ({name})");
            shared.register(device_id, name, token_hash_hex);
        }
        Control::RevokeDevice { device_id } => {
            info!("{from} revoked device {device_id}");
            shared.revoke(&device_id);
        }
        Control::ListDevices => {
            let reply = Envelope::control(
                DeviceId([0; 8]),
                Route::device(from),
                0,
                &Control::Devices {
                    devices: shared.device_list(),
                },
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
