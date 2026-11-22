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
use murmur_proto::{
    Control, DeviceId, Envelope, FamilyKey, Route,
    control::{DeviceInfo, HelloRole, PairBlob},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, IsTerminal, Write};
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

/// WS ping bookkeeping shared between this conn's writer (sets `sent`
/// before each ping) and reader (stamps `ms` on the matching pong).
#[derive(Default)]
struct RttSlot {
    sent: Option<std::time::Instant>,
    ms: Option<u64>,
}

struct Conn {
    tx: mpsc::UnboundedSender<String>,
    /// Fire to make the reader loop exit (revocation, shutdown).
    close: mpsc::Sender<()>,
    pairing: bool,
    rtt: std::sync::Arc<Mutex<RttSlot>>,
    /// Unique registration mark — a dying conn must only remove the
    /// registry entry if it still owns it (see drop_conn_if).
    epoch: u64,
}

static EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

struct Shared {
    /// When this relay process came up — status shows uptime.
    started: std::time::Instant,
    /// Bind address actually listening, for `status`.
    addr: Mutex<String>,
    devices_path: PathBuf,
    /// Persistent registry, mirrored to devices.json on mutation.
    devices: Mutex<HashMap<DeviceId, DeviceRec>>,
    /// Family name — set via --family-name, stored in devices.json.
    family_name: Mutex<Option<String>>,
    /// The URL devices actually dial (wss://…), shown in `status`.
    /// Set once via --public-url / MURMUR_PUBLIC_URL, persisted.
    public_url: Mutex<Option<String>>,
    /// Live connections.
    conns: Mutex<HashMap<DeviceId, Conn>>,
    /// Open pair sessions: session → inviter. Lets a joiner reach the
    /// inviter through @server without knowing its device id.
    open_pairs: Mutex<HashMap<DeviceId, DeviceId>>,
    /// One-shot bootstrap invite, armed while the family is empty
    /// (no non-revoked devices). The relay itself plays the inviter.
    bootstrap: Mutex<Option<Bootstrap>>,
}

/// Server-side state of the bootstrap invite: an ephemeral DH key and
/// the joiner's half once `PairHello` arrives.
struct Bootstrap {
    code: String,
    session: DeviceId,
    key: murmur_proto::PairKey,
    joiner: Option<(murmur_proto::PublicKey, String)>,
    /// SAS shown to the operator while a joiner is mid-handshake.
    sas: Option<String>,
}

impl Shared {
    fn load(path: PathBuf) -> anyhow::Result<Self> {
        // devices.json is the device map + an optional "family_name" key.
        let mut family_name = None;
        let mut public_url = None;
        let devices: HashMap<DeviceId, DeviceRec> = match std::fs::read_to_string(&path) {
            Ok(s) => {
                let mut v: serde_json::Value =
                    serde_json::from_str(&s).context("parse devices.json")?;
                if let Some(o) = v.as_object_mut() {
                    family_name = o
                        .remove("family_name")
                        .and_then(|x| x.as_str().map(String::from));
                    public_url = o
                        .remove("public_url")
                        .and_then(|x| x.as_str().map(String::from));
                }
                serde_json::from_value(v).context("parse devices.json")?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
            Err(e) => return Err(e).context("read devices.json"),
        };
        let bootstrap = if devices.values().all(|d| d.revoked) {
            let code = murmur_proto::pair_code();
            Some(Bootstrap {
                session: DeviceId::from_code(&code),
                code,
                key: murmur_proto::PairKey::generate(),
                joiner: None,
                sas: None,
            })
        } else {
            None
        };
        Ok(Self {
            started: std::time::Instant::now(),
            addr: Mutex::new(String::new()),
            devices_path: path,
            devices: Mutex::new(devices),
            family_name: Mutex::new(family_name),
            public_url: Mutex::new(public_url),
            conns: Mutex::new(HashMap::new()),
            open_pairs: Mutex::new(HashMap::new()),
            bootstrap: Mutex::new(bootstrap),
        })
    }

    /// Bootstrap is armed only while no active (non-revoked) device
    /// exists — the moment a founder registers, the window is closed.
    fn bootstrap_session(&self) -> Option<DeviceId> {
        let mut bs = self.bootstrap.lock().unwrap();
        if bs.is_some() && !self.devices.lock().unwrap().values().all(|d| d.revoked) {
            *bs = None;
        }
        bs.as_ref().map(|b| b.session)
    }

    fn persist(&self) {
        let devices = self.devices.lock().unwrap();
        let tmp = self.devices_path.with_extension("tmp");
        let json = serde_json::to_value(&*devices).ok().and_then(|v| {
            let mut m = v.as_object()?.clone();
            if let Some(n) = self.family_name.lock().unwrap().as_ref() {
                m.insert("family_name".into(), n.clone().into());
            }
            if let Some(u) = self.public_url.lock().unwrap().as_ref() {
                m.insert("public_url".into(), u.clone().into());
            }
            serde_json::to_string_pretty(&m).ok()
        });
        match json {
            Some(json) => {
                let res = self
                    .devices_path
                    .parent()
                    .filter(|d| !d.as_os_str().is_empty())
                    .map(std::fs::create_dir_all)
                    .unwrap_or(Ok(()))
                    .and_then(|_| std::fs::write(&tmp, json))
                    .and_then(|_| std::fs::rename(&tmp, &self.devices_path));
                if let Err(e) = res {
                    warn!("persist devices.json: {e}");
                }
            }
            None => warn!("serialize devices.json: devices map is not a json object"),
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
                self.push_devices(); // members rotate the family key on this
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
                rtt_ms: conns.get(id).and_then(|c| c.rtt.lock().unwrap().ms),
            })
            .collect()
    }

    /// Unsolicited `Devices` to every authed conn — how members learn
    /// about a revocation the moment it happens.
    fn push_devices(&self) {
        let ctl = Control::Devices {
            devices: self.device_list(),
            family_name: self.family_name.lock().unwrap().clone(),
        };
        if let Ok(env) = Envelope::control(DeviceId::ZERO, Route::All, 0, &ctl) {
            self.broadcast(&DeviceId::ZERO, &env, true);
        }
    }

    /// Why `check_auth` would fail — told to the device so it can stop
    /// retrying ("revoked") instead of hammering the relay forever.
    fn auth_failure(&self, id: &DeviceId) -> &'static str {
        match self.devices.lock().unwrap().get(id) {
            Some(r) if r.revoked => "revoked",
            Some(_) => "bad token",
            None => "unknown device",
        }
    }

    fn rename(&self, id: &DeviceId, name: String) {
        let mut devices = self.devices.lock().unwrap();
        if let Some(rec) = devices.get_mut(id) {
            rec.name = name;
            drop(devices);
            self.persist();
        }
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


// ---------- tiny ansi kit (colors only on a tty) ----------

struct Paint(bool);
impl Paint {
    fn new() -> Self {
        Self(std::io::stdout().is_terminal())
    }
    fn wrap(&self, code: &str, s: impl std::fmt::Display) -> String {
        if self.0 {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
    fn accent(&self, s: impl std::fmt::Display) -> String {
        self.wrap("38;5;214", s) // the murmur amber
    }
    fn green(&self, s: impl std::fmt::Display) -> String {
        self.wrap("32", s)
    }
    fn red(&self, s: impl std::fmt::Display) -> String {
        self.wrap("31", s)
    }
    fn dim(&self, s: impl std::fmt::Display) -> String {
        self.wrap("2", s)
    }
    fn bold(&self, s: impl std::fmt::Display) -> String {
        self.wrap("1", s)
    }
}

const ART: &str = "   _ __ ___  _   _ _ __ _ __ ___  _   _ _ __\n\
  | \x27_ ` _ \\| | | | \x27__| \x27_ ` _ \\| | | | \x27__|\n\
  | | | | | | |_| | |  | | | | | | |_| | |\n\
  |_| |_| |_|\\__,_|_|  |_| |_| |_|\\__,_|_|";

/// One framed block: `╭─ murmur ────╮` … `│ lines` … `╰────╯`,
/// matching the installer boxes. Widths count chars, not bytes.
fn frame(p: &Paint, title: &str, lines: &[String]) {
    let w = lines
        .iter()
        .map(|l| l.chars().count() + 2)
        .max()
        .unwrap_or(0)
        .max(title.chars().count() + 4)
        + 2;
    let head = format!("─ {title} ");
    println!(
        "  {}╭{}{}╮",
        "",
        p.accent(&head),
        "─".repeat(w.saturating_sub(head.chars().count())),
    );
    for l in lines {
        println!("  {}  {l}", p.accent("│"));
    }
    println!("  {}╰{}╯", "", "─".repeat(w));
}

// ---------- admin socket (CLI ↔ running server) ----------

/// CLI flag beats env beats nothing. All paths are overridable three ways.
fn arg_or_env(args: &[String], flag: &str, env: &str) -> Option<PathBuf> {
    flag_value(args, flag)
        .map(PathBuf::from)
        .or_else(|| std::env::var_os(env).map(PathBuf::from))
}

fn devices_path(args: &[String]) -> PathBuf {
    if let Some(p) = arg_or_env(args, "--devices", "MURMUR_DEVICES") {
        return p;
    }
    let sys = PathBuf::from("/var/lib/murmur/devices.json");
    if sys.parent().is_some_and(|d| d.is_dir()) {
        return sys;
    }
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".local/share/murmur-server/devices.json"))
        .unwrap_or_else(|| PathBuf::from("devices.json"))
}

fn admin_sock_path(args: &[String]) -> PathBuf {
    if let Some(p) = arg_or_env(args, "--admin-sock", "MURMUR_ADMIN_SOCK") {
        return p;
    }
    let sys = PathBuf::from("/run/murmur/admin.sock");
    if sys.parent().is_some_and(|d| d.is_dir()) {
        return sys;
    }
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(|p| PathBuf::from(p).join("murmur-server.sock"))
        .unwrap_or_else(|| PathBuf::from("/tmp/murmur-server.sock"))
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum AdminReq {
    AddDevice { name: String },
    ListDevices,
    Revoke { device_id: DeviceId },
    BootstrapCode,
    Status,
}

/// One roundtrip against the running server's admin socket.
fn admin_req(req: &AdminReq, args: &[String]) -> anyhow::Result<serde_json::Value> {
    let path = admin_sock_path(args);
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
            let name_exists = shared
                .devices
                .lock()
                .unwrap()
                .values()
                .any(|r| r.name == name && !r.revoked);
            let id = DeviceId::random();
            let token = murmur_proto::AuthToken::random();
            shared.register(id, name, token.hash_hex());
            serde_json::json!({
                "ok": true,
                "data": {"device_id": id.to_string(), "token": token.to_hex(),
                         "name_exists": name_exists}
            })
        }
        AdminReq::ListDevices => {
            serde_json::json!({
                "ok": true,
                "family": *shared.family_name.lock().unwrap(),
                "data": shared.device_list(),
            })
        }
        AdminReq::BootstrapCode => {
            // Same lazy disarm as bootstrap_session: a live device means
            // the window is closed even if it was opened via add-device.
            let mut bs = shared.bootstrap.lock().unwrap();
            if bs.is_some() && !shared.devices.lock().unwrap().values().all(|d| d.revoked) {
                *bs = None;
            }
            let (code, sas) = bs
                .as_ref()
                .map(|b| (Some(b.code.clone()), b.sas.clone()))
                .unwrap_or((None, None));
            serde_json::json!({"ok": true, "code": code, "sas": sas})
        }
        AdminReq::Status => {
            let devices = shared.devices.lock().unwrap();
            let conns = shared.conns.lock().unwrap();
            let online: Vec<String> = devices
                .iter()
                .filter(|(id, r)| !r.revoked && conns.contains_key(*id))
                .map(|(_, r)| r.name.clone())
                .collect();
            let active = devices.values().filter(|r| !r.revoked).count();
            let mut bs = shared.bootstrap.lock().unwrap();
            if bs.is_some() && !devices.values().all(|d| d.revoked) {
                *bs = None;
            }
            serde_json::json!({
                "ok": true,
                "addr": *shared.addr.lock().unwrap(),
                "uptime_secs": shared.started.elapsed().as_secs(),
                "family": *shared.family_name.lock().unwrap(),
                "public_url": *shared.public_url.lock().unwrap(),
                "registered": devices.len(),
                "active": active,
                "online": online,
                "open_pairs": shared.open_pairs.lock().unwrap().len(),
                "bootstrap": bs.as_ref().map(|b| {
                    serde_json::json!({"code": b.code, "sas": b.sas})
                }),
            })
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


fn print_help(p: &Paint) {
    let d = |s: &str| p.dim(s);
    let lines: Vec<String> = [
        ART.lines().map(|l| p.accent(l)).collect::<Vec<_>>(),
        vec![String::new(), d("the murmur relay — it routes ciphertext, nothing else")],
        vec![String::new()],
        vec![
            p.bold("service").to_string(),
            format!("  {}  run the relay", p.accent(format!("{:<24}", "serve --addr A"))),
            format!("  {}  systemd unit + autostart", p.accent(format!("{:<24}", "install [--addr A]"))),
        ],
        vec![String::new(), p.bold("admin").to_string()],
        vec![
            format!("  {}  what the relay is doing right now", p.accent(format!("{:<24}", "status"))),
            format!("  {}  device table · state · online · rtt", p.accent(format!("{:<24}", "list-devices"))),
            format!("  {}  register a device by hand", p.accent(format!("{:<24}", "add-device --name N"))),
            format!("  {}  kick a device — members rotate the key", p.accent(format!("{:<24}", "revoke --id ID"))),
            format!("  {}  one-shot code while the family is empty", p.accent(format!("{:<24}", "bootstrap-code"))),
        ],
        vec![
            String::new(),
            d("admin commands talk to the live relay over the admin socket —"),
            d("no sudo needed once you're in the `murmur` group."),
        ],
    ]
    .concat();
    println!();
    frame(p, "murmur-server", &lines);
    println!();
}


    let args: Vec<String> = std::env::args().skip(1).collect();
    // A flag in first position means "run the relay" (the systemd
    // ExecStart spells it that way); `serve` works too. Bare or a
    // subcommand word → the CLI.
    match args.first().filter(|a| !a.starts_with('-')).map(String::as_str) {
        None if args.is_empty() => {
            print_help(&Paint::new());
            Ok(())
        }
        None => serve(&args).await,
        Some("serve") => serve(&args[1..]).await,
        Some("status") => status(&args),
        Some("-h") | Some("--help") | Some("help") => {
            print_help(&Paint::new());
            Ok(())
        }
        Some("add-device") => {
            let name = flag_value(&args[1..], "--name")
                .or_else(|| args.get(1).cloned())
                .or_else(|| prompt_line("device name"))
                .context("usage: murmur-server add-device --name NAME")?;
            let res = admin_req(&AdminReq::AddDevice { name }, &args)?;
            if res["ok"].as_bool() != Some(true) {
                bail!("{}", res["error"].as_str().unwrap_or("add-device failed"));
            }
            let d = &res["data"];
            let p = Paint::new();
            println!();
            let mut lines = vec![
                format!("id      {}", p.bold(d["device_id"].as_str().unwrap())),
                format!("token   {}", p.dim(d["token"].as_str().unwrap())),
                p.dim("token shown once — store it in secrets.toml on the device"),
            ];
            if d["name_exists"].as_bool() == Some(true) {
                lines.push(p.dim("heads-up: an active device already has this name"));
            }
            frame(&p, "device added", &lines);
            println!();
            Ok(())
        }
        Some("list-devices") => {
            let res = admin_req(&AdminReq::ListDevices, &args)?;
            if res["ok"].as_bool() != Some(true) {
                bail!("{}", res["error"].as_str().unwrap_or("list-devices failed"));
            }
            let p = Paint::new();
            println!();
            println!("  {}", p.dim(format!("family · {}", res["family"].as_str().unwrap_or("(unnamed)"))));
            for d in res["data"].as_array().into_iter().flatten() {
                let (mark, state) = if d["revoked"].as_bool() == Some(true) {
                    (p.red("✕"), p.dim("revoked"))
                } else if d["online"].as_bool() == Some(true) {
                    (p.green("●"), p.green("online"))
                } else {
                    (p.dim("○"), p.dim("offline"))
                };
                println!(
                    "  {mark}  {}  {} {state}  {}",
                    p.dim(d["device_id"].as_str().unwrap_or("?")),
                    p.bold(format!("{:<20}", d["name"].as_str().unwrap_or("?"))),
                    d["rtt_ms"]
                        .as_u64()
                        .map(|ms| p.dim(format!("{ms}ms")))
                        .unwrap_or_default(),
                );
            }
            println!();
            Ok(())
        }
        Some("revoke") => {
            let id: DeviceId = flag_value(&args[1..], "--id")
                .or_else(|| args.get(1).cloned())
                .context("usage: murmur-server revoke --id DEVICE_ID")?
                .parse()?;
            let res = admin_req(&AdminReq::Revoke { device_id: id }, &args)?;
            match res["ok"].as_bool() {
                Some(true) => {
                    let p = Paint::new();
                    println!();
                    println!("  {} {id} revoked", p.green("✓"));
                    println!("  {}", p.dim("live connection dropped · members rotate the family key"));
                    println!();
                }
                _ => bail!("{}", res["error"].as_str().unwrap_or("failed")),
            }
            Ok(())
        }
        Some("bootstrap-code") => {
            let res = admin_req(&AdminReq::BootstrapCode, &args)?;
            if res["ok"].as_bool() != Some(true) {
                bail!("{}", res["error"].as_str().unwrap_or("bootstrap-code failed"));
            }
            let p = Paint::new();
            match res["code"].as_str() {
                Some(c) => {
                    println!("{}", p.bold(c));
                    if let Some(s) = res["sas"].as_str() {
                        println!("{}", p.dim(format!("sas: {s}  (a joiner is pairing right now)")));
                    }
                }
                None => println!("{}", p.dim("no bootstrap code — the family already has devices")),
            }
            Ok(())
        }
        Some("install") => install(&args[1..]),
        Some(other) => {
            print_help(&Paint::new());
            bail!("unknown command {other:?}")
        }
    }
}

/// `murmur-server status` — one framed screen of what the relay is doing.
fn status(args: &[String]) -> anyhow::Result<()> {
    let p = Paint::new();
    let res = admin_req(&AdminReq::Status, args)?;
    if res["ok"].as_bool() != Some(true) {
        bail!(
            "{}",
            res["error"].as_str().unwrap_or("status failed — is the relay on an older binary?")
        );
    }
    let up = res["uptime_secs"].as_u64().unwrap_or(0);
    let uptime = match up {
        s if s < 90 => format!("{s}s"),
        s if s < 5400 => format!("{}m", s / 60),
        s => format!("{}h {:02}m", s / 3600, (s % 3600) / 60),
    };
    let online: Vec<&str> = res["online"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|x| x.as_str())
        .collect();
    let pairs = res["open_pairs"].as_u64().unwrap_or(0);
    let public = res["public_url"].as_str().unwrap_or("");

    let mut lines: Vec<String> = ART.lines().map(|l| p.accent(l)).collect();
    lines.push(String::new());
    lines.push(format!("listen    {}", p.bold(res["addr"].as_str().unwrap_or("?"))));
    lines.push(format!("public    {}", p.bold(if public.is_empty() { "—" } else { public })));
    lines.push(format!("family    {}", p.bold(res["family"].as_str().unwrap_or("(unnamed)"))));
    lines.push(format!("up        {uptime}"));
    lines.push(String::new());
    let online_str = if online.is_empty() {
        p.dim("nobody")
    } else {
        online.join(", ")
    };
    lines.push(format!(
        "devices   {} registered · {} active · {}",
        res["registered"].as_u64().unwrap_or(0),
        res["active"].as_u64().unwrap_or(0),
        if online.is_empty() { p.dim("0 online") } else { p.green(format!("{} online · {online_str}", online.len())) },
    ));
    lines.push(format!(
        "invites   {}",
        if pairs == 0 { p.dim("none") } else { p.bold(format!("{pairs} open pair session{}", if pairs == 1 { "" } else { "s" })) },
    ));
    match &res["bootstrap"] {
        b if b.is_object() => {
            let code = b["code"].as_str().unwrap_or("?");
            match b["sas"].as_str() {
                Some(sas) => lines.push(format!(
                    "bootstrap {} {code} · sas {sas} {}",
                    p.accent("armed ·"), p.dim("(a joiner is pairing)")
                )),
                None => lines.push(format!("bootstrap {} {code}", p.accent("armed ·"))),
            }
        }
        _ => lines.push(format!("bootstrap {}", p.dim("closed — the family has devices"))),
    }
    println!();
    frame(&p, "murmur · relay", &lines);
    println!();
    Ok(())
}

/// `murmur-server install [--addr A] [--user]` — write a systemd unit and
/// enable the relay. System unit when /etc/systemd is writable (DynamicUser,
/// state under /var/lib/murmur, admin socket under /run/murmur); otherwise a
/// user unit + linger. Always prints the Caddy reverse-proxy snippet —
/// WS upgrade needs no special handling there.
fn install(args: &[String]) -> anyhow::Result<()> {
    let addr = flag_value(args, "--addr").unwrap_or_else(|| "0.0.0.0:8770".into());
    // Family name goes into ExecStart quoted — systemd strips the quotes.
    let family_arg = flag_value(args, "--family-name")
        .map(|f| format!(" --family-name \"{}\"", f.replace('"', "")))
        .unwrap_or_default();
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
        install_user(&exe, &addr, &family_arg)
    } else {
        install_system(&exe, &addr, &family_arg)
    }?;
    print_caddy_hint(&addr);
    Ok(())
}

fn install_system(exe: &str, addr: &str, family_arg: &str) -> anyhow::Result<()> {
    let unit = format!(
        "[Unit]\n\
         Description=murmur relay server\n\
         After=network-online.target\n\
         Wants=network-online.target\n\n\
         [Service]\n\
         DynamicUser=yes\n\
         StateDirectory=murmur\n\
         RuntimeDirectory=murmur\n\
         SupplementaryGroups=murmur\n\
         ExecStart={exe} --addr {addr} --devices /var/lib/murmur/devices.json --admin-sock /run/murmur/admin.sock --admin-group murmur{family_arg}\n\
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
    // The admin socket is group-writable: anyone in `murmur` can run
    // `murmur-server status|bootstrap-code|...` without sudo.
    if run("groupadd", &["-f", "murmur"]).is_err() {
        eprintln!("hint: create a `murmur` group to use admin commands without sudo");
    }
    let caller = std::env::var("SUDO_USER").ok().filter(|u| u != "root");
    if let Some(u) = caller {
        if run("usermod", &["-aG", "murmur", &u]).is_ok() {
            eprintln!("added {u} to the `murmur` group — log in again for it to take effect");
        }
    }
    run("systemctl", &["daemon-reload"])?;
    // `enable --now` skips a start when the unit is already active —
    // on upgrades the old binary would keep serving forever. Restart
    // unconditionally: it starts a stopped service and swaps a stale one.
    run("systemctl", &["enable", "murmur-server.service"])?;
    run("systemctl", &["restart", "murmur-server.service"])?;
    eprintln!("installed + started: {path:?}");
    eprintln!("admin commands: murmur-server status · list-devices · bootstrap-code · revoke --id …");
    eprintln!("  (no sudo — the admin socket is writable by the `murmur` group)");
    Ok(())
}

fn install_user(exe: &str, addr: &str, family_arg: &str) -> anyhow::Result<()> {
    let unit = format!(
        "[Unit]\n\
         Description=murmur relay server\n\
         After=network-online.target\n\n\
         [Service]\n\
         ExecStart={exe} --addr {addr} --devices %h/.local/share/murmur-server/devices.json{family_arg}\n\
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

fn prompt_line(label: &str) -> Option<String> {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        return None;
    }
    eprint!("{label}: ");
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).ok()?;
    let line = line.trim().to_string();
    (!line.is_empty()).then_some(line)
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

// ---------- relay ----------

async fn serve(args: &[String]) -> anyhow::Result<()> {
    let addr: SocketAddr = flag_value(args, "--addr")
        .or_else(|| std::env::var("MURMUR_ADDR").ok())
        .unwrap_or_else(|| "0.0.0.0:8770".into())
        .parse()
        .context("--addr")?;
    let shared = Arc::new(Shared::load(devices_path(args))?);
    if let Some(fam) = flag_value(args, "--family-name")
        .or_else(|| std::env::var("MURMUR_FAMILY_NAME").ok())
    {
        *shared.family_name.lock().unwrap() = Some(fam);
        shared.persist();
    }
    if let Some(url) = flag_value(args, "--public-url")
        .or_else(|| std::env::var("MURMUR_PUBLIC_URL").ok())
    {
        *shared.public_url.lock().unwrap() = Some(url);
        shared.persist();
    }

    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("bind {addr} — is a relay already running there? `murmur-server status` will tell you"))?;
    *shared.addr.lock().unwrap() = addr.to_string();
    let admin_path = admin_sock_path(args);
    let _ = std::fs::remove_file(&admin_path);
    let admin_listener = UnixListener::bind(&admin_path)?;
    // The socket grants full registry control. With --admin-group the
    // admin group gets rw — `murmur-server status` et al. without sudo.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let group = flag_value(args, "--admin-group")
            .or_else(|| std::env::var("MURMUR_ADMIN_GROUP").ok());
        match group {
            Some(g) => {
                let st = std::process::Command::new("chgrp")
                    .args([&g, admin_path.to_str().unwrap_or_default()])
                    .status();
                if st.is_ok_and(|s| s.success()) {
                    std::fs::set_permissions(&admin_path, std::fs::Permissions::from_mode(0o660))?;
                } else {
                    warn!("chgrp {g} {admin_path:?} failed — admin socket stays owner-only");
                    std::fs::set_permissions(&admin_path, std::fs::Permissions::from_mode(0o600))?;
                }
            }
            None => {
                std::fs::set_permissions(&admin_path, std::fs::Permissions::from_mode(0o600))?;
            }
        }
    }
    info!(
        "murmur-server on {addr}, registry {}, admin {}",
        shared.devices_path.display(),
        admin_path.display()
    );
    if let Some(code) = shared.bootstrap.lock().unwrap().as_ref().map(|b| b.code.clone()) {
        info!(
            "family is empty — bootstrap code: {code}\n\
             first device to `murmurd pair --server <this-url> --code {code}` becomes the founder"
        );
    }

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
                            reason: shared.auth_failure(&device_id).into(),
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
    let rtt = std::sync::Arc::new(Mutex::new(RttSlot::default()));
    {
        let mut conns = shared.conns.lock().unwrap();
        if let Some(old) = conns.remove(&device_id) {
            // Same conn id claimed twice (duplicate login or reused pair
            // session): newest wins, the stale conn is terminated.
            let _ = old.close.try_send(());
            eprintln!("device {device_id} reconnected — dropped the previous conn");
        }
        conns.insert(
            device_id,
            Conn {
                tx,
                close: close_tx,
                pairing,
                epoch,
                rtt: rtt.clone(),
            },
        );
    }
    send_ctl(&mut sink, &Control::HelloOk).await?;

    // Writer: queued frames + periodic WS pings (NATs drop silent conns;
    // phones on mobile networks depend on this).
    let rtt_w = rtt.clone();
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
                    rtt_w.lock().unwrap().sent = Some(std::time::Instant::now());
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
            if let Ok(Ws::Pong(_)) = &msg {
                let mut slot = rtt.lock().unwrap();
                if let Some(sent) = slot.sent.take() {
                    slot.ms = Some(sent.elapsed().as_millis() as u64);
                }
            }
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
                    && matches!(
                        env.control_payload(),
                        Ok(Control::PairHello { .. }) | Ok(Control::PairConfirm)
                    );
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
        // A pairing conn's only @server ops: the inviter registered this
        // session via PairOpen — forward the hello to it (env.from is
        // already the session id). If nobody opened it and the family is
        // still empty, the session may be the bootstrap code — then the
        // relay plays the inviter itself.
        Control::PairHello { pubkey_b64, name } if pairing => {
            let inviter = shared.open_pairs.lock().unwrap().get(&from).copied();
            match inviter {
                Some(inv) => shared.send_to(&inv, env),
                None if shared.bootstrap_session() == Some(from) => {
                    bootstrap_hello(shared, from, &pubkey_b64, &name).await?
                }
                None => {
                    warn!("{from}: PairHello for an unknown/expired session");
                    // Tell the joiner — a silent drop leaves it waiting
                    // for an inviter that will never come.
                    if let Ok(reply) = Envelope::control(
                        DeviceId::ZERO,
                        Route::Device(from),
                        0,
                        &Control::PairReject {
                            reason: "invite expired or mistyped — get a fresh `murmurctl invite`"
                                .into(),
                        },
                    ) {
                        shared.send_to(&from, &reply);
                    }
                }
            }
        }
        Control::PairConfirm if pairing => {
            bootstrap_confirm(shared, from).await?;
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
            // Self-revoke only — a compromised device must not be able to
            // kick others out of the family. Admin revokes go through the
            // unix socket (`murmur-server revoke`).
            if device_id == from {
                info!("{from} revoked itself");
                shared.revoke(&device_id);
            } else {
                warn!("{from} tried to revoke {device_id} — self-revoke only");
                let reply = Envelope::control(
                    DeviceId::ZERO,
                    Route::device(from),
                    0,
                    &Control::Error {
                        reason: "can only revoke self".into(),
                    },
                )?;
                shared.send_to(&from, &reply);
            }
        }
        Control::RenameDevice { device_id, name } => {
            if device_id == from {
                info!("{from} renamed to {name}");
                shared.rename(&from, name);
            } else {
                warn!("{from} tried to rename {device_id} — self-rename only");
            }
        }
        Control::ListDevices => {
            let reply = Envelope::control(
                DeviceId::ZERO,
                Route::device(from),
                0,
                &Control::Devices {
                    devices: shared.device_list(),
                    family_name: shared.family_name.lock().unwrap().clone(),
                },
            )?;
            shared.send_to(&from, &reply);
        }
        other => debug!("{from}: unhandled control {other:?}"),
    }
    Ok(())
}

/// The relay answers a bootstrap PairHello itself: stores the joiner's
/// pubkey and name, replies PairAccept, logs the SAS for the operator
/// to eyeball against the joiner's screen.
async fn bootstrap_hello(
    shared: &Arc<Shared>,
    session: DeviceId,
    pubkey_b64: &str,
    name: &str,
) -> anyhow::Result<()> {
    let joiner_pub = murmur_proto::PairKey::public_from_b64(pubkey_b64)?;
    let reply = {
        let mut guard = shared.bootstrap.lock().unwrap();
        let Some(b) = guard.as_mut() else {
            return Ok(());
        };
        b.joiner = Some((joiner_pub, name.to_owned()));
        let sas =
            murmur_proto::sas_code(&b.key.public, &joiner_pub, &session, &DeviceId::ZERO);
        b.sas = Some(sas.clone());
        info!("bootstrap: '{name}' pairing — SAS {sas} (must match the joiner's screen)");
        Envelope::control(
            DeviceId::ZERO,
            Route::device(session),
            0,
            &Control::PairAccept {
                pubkey_b64: b.key.public_b64(),
            },
        )?
    };
    shared.send_to(&session, &reply);
    Ok(())
}

/// Joiner confirmed the SAS — mint credentials and seal them to the
/// joiner's DH half. `family_key_b64`/`server_url` are left empty: the
/// founder generates the key locally and keeps the URL it used —
/// the relay never learns either.
async fn bootstrap_confirm(shared: &Arc<Shared>, session: DeviceId) -> anyhow::Result<()> {
    let taken = shared.bootstrap.lock().unwrap().take();
    let Some(b) = taken else {
        warn!("{session}: PairConfirm with no pending bootstrap");
        return Ok(());
    };
    let Some((joiner_pub, name)) = b.joiner else {
        warn!("{session}: PairConfirm before PairHello");
        *shared.bootstrap.lock().unwrap() = Some(b);
        return Ok(());
    };
    let device_id = DeviceId::random();
    let token = murmur_proto::AuthToken::random();
    shared.register(device_id, name, token.hash_hex());

    let blob = PairBlob {
        device_id,
        token_hex: token.to_hex(),
        family_key_b64: None,
        server_url: None,
        key_epoch: 0,
    };
    let sealed = FamilyKey::from_raw(b.key.dh(&joiner_pub)).seal(&serde_json::to_vec(&blob)?)?;
    use base64::Engine;
    let reply = Envelope::control(
        DeviceId::ZERO,
        Route::device(session),
        0,
        &Control::PairData {
            blob_b64: base64::engine::general_purpose::STANDARD.encode(sealed),
        },
    )?;
    shared.send_to(&session, &reply);
    info!("bootstrap complete — {device_id} is the founder; window closed");
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
