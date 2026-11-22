//! murmurctl: thin CLI over the murmurd unix socket.
//! Sends one JSON line, prints human-readable output.

use anyhow::Context;
use clap::{Parser, Subcommand};
use std::io::{BufRead, BufReader, IsTerminal, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "murmurctl", about = "control murmurd", version)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show connection state, presence table, active node.
    Status,
    /// List family devices known to the server.
    Devices {
        /// Include revoked devices.
        #[arg(long)]
        all: bool,
    },
    /// Revoke a device by id (kills it instantly on the server).
    Revoke { device_id: String },
    /// Issue an invite code for a new device (QR + pasteable string).
    Invite,
    /// Pairing requests: list pending / accept / confirm after SAS check.
    Pair {
        #[command(subcommand)]
        op: Option<PairOp>,
    },
    /// Broadcast a Message (JSON from stdin) to all devices (debug).
    Send,
}

#[derive(Subcommand)]
enum PairOp {
    /// Send our pubkey to a pending joiner (it can then show the SAS too).
    Accept { session: String },
    /// After the codes match: register the device and ship credentials.
    Confirm { session: String },
}

fn sock_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("murmur.sock")
}

fn request(req: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    let path = sock_path();
    let stream = UnixStream::connect(&path)
        .with_context(|| format!("connect {} — is murmurd running?", path.display()))?;
    let mut w = stream.try_clone()?;
    w.write_all(serde_json::to_string(&req)?.as_bytes())?;
    w.write_all(b"\n")?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    Ok(serde_json::from_str(line.trim())?)
}

fn ask(req: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    let res = request(req)?;
    if res["ok"].as_bool() == Some(true) {
        Ok(res["data"].clone())
    } else {
        anyhow::bail!("{}", res["error"].as_str().unwrap_or("daemon error"))
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
    fn green(&self, s: impl std::fmt::Display) -> String {
        self.wrap("32", s)
    }
    fn red(&self, s: impl std::fmt::Display) -> String {
        self.wrap("31", s)
    }
    fn yellow(&self, s: impl std::fmt::Display) -> String {
        self.wrap("33", s)
    }
    fn dim(&self, s: impl std::fmt::Display) -> String {
        self.wrap("2", s)
    }
    fn bold(&self, s: impl std::fmt::Display) -> String {
        self.wrap("1", s)
    }
}

fn ago(ts: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let d = now.saturating_sub(ts);
    match d {
        0..=15 => "now".into(),
        16..=59 => format!("{d}s ago"),
        60..=3599 => format!("{}m ago", d / 60),
        3600..=86399 => format!("{}h ago", d / 3600),
        _ => format!("{}d ago", d / 86400),
    }
}

fn print_qr(data: &str) {
    // EcLevel::L packs the same payload into the fewest modules —
    // the invite string is long, every module is screen space.
    match qrcode::QrCode::with_error_correction_level(data, qrcode::EcLevel::L) {
        Ok(code) => {
            let img = code
                .render::<qrcode::render::unicode::Dense1x2>()
                .quiet_zone(false)
                .build();
            println!("\n{img}\n");
        }
        Err(e) => eprintln!("(qr render failed: {e})"),
    }
}

// ---------- commands ----------

fn cmd_status() -> anyhow::Result<()> {
    let p = Paint::new();
    let d = ask(serde_json::json!({"cmd": "status"}))?;
    let online = d["online"].as_bool() == Some(true);
    let state = if online {
        p.green("online")
    } else {
        p.red("offline")
    };

    println!(
        "{} {} {}",
        p.bold(d["name"].as_str().unwrap_or("?")),
        p.dim(d["self"].as_str().unwrap_or("?")),
        state,
    );
    println!("  relay   {}", d["server"].as_str().unwrap_or("?"));

    let selfid = d["self"].as_str().unwrap_or_default();
    match d["active_node"].as_str() {
        Some(id) => println!(
            "  active  {}{}",
            id,
            if id == selfid {
                p.green("  (me)")
            } else {
                String::new()
            },
        ),
        None => println!("  active  {}", p.dim("none")),
    }

    let mut presence: Vec<&serde_json::Value> =
        d["presence"].as_array().into_iter().flatten().collect();
    presence.sort_by_key(|p| p["device"].as_str() == Some(selfid));
    presence.reverse();
    if !presence.is_empty() {
        println!("\n{}", p.bold("presence"));
        for dev in presence {
            let id = dev["device"].as_str().unwrap_or("?");
            let state = dev["state"].as_str().unwrap_or("?");
            let dot = if state == "active" {
                p.green("●")
            } else {
                p.dim("○")
            };
            let me = if id == selfid {
                p.dim(" (me)")
            } else {
                String::new()
            };
            println!(
                "  {dot} {id}{me}  {:<8} {}",
                state,
                p.dim(ago(dev["ts"].as_u64().unwrap_or(0))),
            );
        }
    }
    Ok(())
}

fn cmd_devices(all: bool) -> anyhow::Result<()> {
    let p = Paint::new();
    let d = ask(serde_json::json!({"cmd": "devices"}))?;
    let selfid = d["self"].as_str().unwrap_or_default();
    let mut devs: Vec<serde_json::Value> =
        d["devices"].as_array().cloned().unwrap_or_default();
    if !all {
        devs.retain(|x| x["revoked"].as_bool() != Some(true));
    }
    // Online first, then by name.
    devs.sort_by(|a, b| {
        let on = |x: &serde_json::Value| x["online"].as_bool() == Some(true);
        on(b).cmp(&on(a)).then_with(|| {
            a["name"].as_str().unwrap_or("").cmp(b["name"].as_str().unwrap_or(""))
        })
    });
    if devs.is_empty() {
        println!("{}", p.dim("no devices in the registry"));
        return Ok(());
    }
    if let Some(fam) = d["family"].as_str() {
        println!("{}", p.dim(format!("family · {fam}")));
    }
    let w = devs
        .iter()
        .map(|x| x["name"].as_str().unwrap_or("?").len())
        .max()
        .unwrap_or(4)
        .max(4);
    for x in &devs {
        let id = x["device_id"].as_str().unwrap_or("?");
        let name = x["name"].as_str().unwrap_or("?");
        let (dot, tail) = if x["revoked"].as_bool() == Some(true) {
            (p.red("✕"), p.dim("revoked"))
        } else if x["online"].as_bool() == Some(true) {
            let rtt = x["rtt_ms"]
                .as_u64()
                .map(|ms| format!("{ms} ms"))
                .unwrap_or_else(|| "online".into());
            (p.green("●"), p.green(rtt))
        } else {
            (p.dim("○"), p.dim("offline"))
        };
        let me = if id == selfid { p.dim(" (me)") } else { String::new() };
        println!("  {dot} {name:<w$}  {id}{me}  {tail}");
    }
    Ok(())
}

fn cmd_invite() -> anyhow::Result<()> {
    let p = Paint::new();
    let d = ask(serde_json::json!({"cmd": "invite"}))?;
    let code = d["code"].as_str().unwrap_or_default();
    let short = d["short"].as_str().unwrap_or_default();
    let server = d["server"].as_str().unwrap_or_default();

    print_qr(code);
    if !short.is_empty() {
        println!("  code      {}", p.bold(short));
        println!("  server    {}", server);
        println!("  {}", p.dim("expires in 10 minutes"));
        println!(
            "\n  {}",
            p.dim("phone:  open murmur → enter the code · or scan the QR")
        );
        println!(
            "  {}",
            p.dim(format!(
                "laptop: murmurd pair --server {server} --code {short}"
            ))
        );
    }
    Ok(())
}

fn cmd_pair() -> anyhow::Result<()> {
    let p = Paint::new();
    let d = ask(serde_json::json!({"cmd": "pair"}))?;
    let pending = d["pending"].as_array().cloned().unwrap_or_default();
    if pending.is_empty() {
        println!("{}", p.dim("no pending pairings"));
        return Ok(());
    }
    for x in &pending {
        println!(
            "  {}  {}  code {}",
            p.bold(x["session"].as_str().unwrap_or("?")),
            x["name"].as_str().unwrap_or("?"),
            p.yellow(x["sas"].as_str().unwrap_or("?")),
        );
    }
    println!(
        "\n  {}\n  {}",
        p.dim("murmurctl pair accept  <session>  — shares our pubkey, shows the code on both ends"),
        p.dim("murmurctl pair confirm <session>  — after the codes match on screen"),
    );
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Status => cmd_status(),
        Cmd::Devices { all } => cmd_devices(all),
        Cmd::Revoke { device_id } => {
            match ask(serde_json::json!({"cmd": "revoke", "device_id": device_id})) {
                Ok(_) => println!("{device_id} revoked — live connection dropped"),
                Err(e) => anyhow::bail!(
                    "{e}\n  devices can only self-revoke over the wire — \
                     kick others from the relay host: `murmur-server revoke --id {device_id}`"
                ),
            }
            Ok(())
        }
        Cmd::Invite => cmd_invite(),
        Cmd::Pair { op: None } => cmd_pair(),
        Cmd::Pair {
            op: Some(PairOp::Accept { session }),
        } => {
            ask(serde_json::json!({"cmd": "pair_accept", "session": session}))?;
            println!("accepted — compare the codes, then `murmurctl pair confirm {session}`");
            Ok(())
        }
        Cmd::Pair {
            op: Some(PairOp::Confirm { session }),
        } => {
            let d =
                ask(serde_json::json!({"cmd": "pair_confirm", "session": session}))?;
            println!("paired — new device {}", d["device_id"].as_str().unwrap_or("?"));
            Ok(())
        }
        Cmd::Send => {
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf)?;
            let msg: serde_json::Value = serde_json::from_str(buf.trim())
                .context("send expects a Message JSON on stdin")?;
            ask(serde_json::json!({"cmd": "send", "msg_json": msg}))?;
            Ok(())
        }
    }
}
