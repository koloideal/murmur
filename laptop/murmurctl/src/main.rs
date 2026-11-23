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
    cmd: Option<Cmd>,
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
    /// Invite a new device: QR + code, then walk its request through
    /// accept → code check → confirm right here.
    Invite,
    /// Mint a fresh family key and hand it to every member (automatic
    /// after a revocation — this is the manual lever).
    Rotate,
    /// Pairing requests: interactive walkthrough (no args), or the
    /// scriptable accept / confirm steps.
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
    fn accent(&self, s: impl std::fmt::Display) -> String {
        self.wrap("38;5;141", s)
    }
}

const ART: &str = " _ __ ___  _   _ _ __ _ __ ___  _   _ _ __\n\
| \x27_ ` _ \\| | | | \x27__| \x27_ ` _ \\| | | | \x27__|\n\
| | | | | | |_| | |  | | | | | | |_| | |\n\
|_| |_| |_|\\__,_|_|  |_| |_| |_|\\__,_|_|";

/// Installer-style banner: the logo bold in accent, a dim subtitle.
fn banner(p: &Paint, sub: &str) {
    println!();
    for l in ART.lines() {
        println!("  {}", p.wrap("1;38;5;141", l));
    }
    if !sub.is_empty() {
        println!("  {}", p.dim(sub));
    }
    println!();
}

fn print_help(p: &Paint) {
    let d = |s: &str| p.dim(s);
    let c = |s: &str| p.accent(format!("{s:<12}"));
    let lines: Vec<String> = [
        vec![String::new()],
        vec![
            format!("  {} connection, presence, active node", c("status")),
            format!("  {} family devices (--all for revoked)", c("devices")),
            format!("  {} invite a device: QR + walkthrough", c("invite")),
            format!("  {} pending pairing requests", c("pair")),
            format!("  {} rotate the family key", c("rotate")),
            format!("  {} self-revoke a device", c("revoke")),
        ],
        vec![
            String::new(),
            d("kick other devices from the relay host:"),
            d("murmurs revoke --id <device>"),
        ],
    ]
    .concat();
    banner(p, "control the running murmurd");
    for l in &lines[1..] {
        println!("  {l}");
    }
    println!();
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
    let revoked = d["revoked"].as_bool() == Some(true);
    let state = if online {
        p.green("online")
    } else if revoked {
        p.red("revoked")
    } else {
        p.red("offline")
    };

    banner(&p, "");
    println!(
        "{} {} {}",
        p.bold(d["name"].as_str().unwrap_or("?")),
        p.dim(d["self"].as_str().unwrap_or("?")),
        state,
    );
    println!("  relay   {}", d["server"].as_str().unwrap_or("?"));
    println!(
        "  family  {}",
        p.dim(format!("as {}", d["family_kind"].as_str().unwrap_or("serverful"))),
    );
    if let Some(peers) = d["lan_peers"].as_array().filter(|p| !p.is_empty()) {
        let rtt = d["lan_rtt_ms"].as_object();
        let list = peers
            .iter()
            .filter_map(|p| p.as_str())
            .map(|p| {
                let ms = rtt.and_then(|m| m.get(p)).and_then(|v| v.as_u64()).unwrap_or(0);
                if ms > 0 { format!("{p} {ms}ms") } else { p.to_string() }
            })
            .collect::<Vec<_>>()
            .join("  ");
        println!("  lan     {}", p.green(format!("⇄ {list}")));
    }
    if revoked {
        println!(
            "  {}",
            p.red("this machine was removed from the family — re-pair: murmurd pair --code …"),
        );
    }
    println!(
        "  key     epoch {}  {}",
        d["key_epoch"].as_u64().unwrap_or(0),
        p.dim(format!("· {} pinned member(s)", d["members"].as_u64().unwrap_or(0))),
    );

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
        if server.starts_with("ws") {
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
        } else {
            // Serverless: `server` carries `host:port` of the one-shot
            // listener, and a LAN pair beacon makes the short code
            // resolvable without a relay. The full invite goes on the
            // terminal too — a laptop can't scan a QR.
            println!("  code      {}", p.bold(short));
            println!("  direct    {}", server);
            println!("  {}", p.dim("lan only — expires in 10 minutes"));
            println!(
                "\n  {}",
                p.dim("phone:  open murmur → scan the QR · or enter the code")
            );
            println!(
                "  {}",
                p.dim(format!("laptop: murmurd pair --code {short}"))
            );
            println!("\n  {}", p.dim("invite:"));
            println!("  {}", code);
        }
    }
    if std::io::stdout().is_terminal() {
        let session = d["session"].as_str().unwrap_or_default().to_owned();
        println!();
        wait_and_walk(&p, Some(&session))?;
    }
    Ok(())
}

/// Read one answer from the terminal itself — works under pipes too.
fn tty_yes(p: &Paint, q: &str, default_yes: bool) -> anyhow::Result<bool> {
    use std::io::Write;
    let hint = if default_yes { "[Y/n]" } else { "[y/N]" };
    print!("  {} {q} {}  ", p.yellow("?"), p.dim(hint));
    std::io::stdout().flush()?;
    let mut line = String::new();
    let tty = std::fs::File::open("/dev/tty")?;
    BufReader::new(tty).read_line(&mut line)?;
    Ok(match line.trim().to_ascii_lowercase().as_str() {
        "" => default_yes,
        a => a == "y" || a == "yes",
    })
}

/// Block until a pairing request shows up (for `only` session, or any),
/// then walk it: accept → show the code → confirm.
fn wait_and_walk(p: &Paint, only: Option<&str>) -> anyhow::Result<()> {
    use std::io::Write;
    let started = std::time::Instant::now();
    let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let mut i = 0;
    let req = loop {
        // Spinner ticks every 120ms; the daemon is asked every 6th tick.
        if i % 6 == 0 {
            let d = ask(serde_json::json!({"cmd": "pair"}))?;
            let found = d["pending"].as_array().into_iter().flatten().find(|x| {
                only.is_none_or(|s| x["session"].as_str() == Some(s))
            });
            if let Some(x) = found {
                print!("\r\x1b[2K");
                break x.clone();
            }
        }
        if started.elapsed() > std::time::Duration::from_secs(600) {
            print!("\r\x1b[2K");
            println!("  {}", p.dim("no request within 10 minutes — the invite expired"));
            return Ok(());
        }
        print!(
            "\r  {} {}",
            p.yellow(frames[i % frames.len()]),
            p.dim("waiting for the device to scan or type the code…  Ctrl-C = finish later with `murmurctl pair`"),
        );
        std::io::stdout().flush()?;
        i += 1;
        std::thread::sleep(std::time::Duration::from_millis(120));
    };
    let session = req["session"].as_str().unwrap_or_default();
    let name = req["name"].as_str().unwrap_or("a device");
    let sas = req["sas"].as_str().unwrap_or("??????");

    println!("  {} {} wants to join", p.green("›"), p.bold(name));
    println!();
    if !tty_yes(p, "accept it?", true)? {
        println!("  {}", p.dim("left pending — nothing was shared"));
        return Ok(());
    }
    ask(serde_json::json!({"cmd": "pair_accept", "session": session}))?;
    let pretty = format!("{} {}", &sas[..3.min(sas.len())], &sas[3.min(sas.len())..]);
    println!();
    println!("  │");
    println!("  │  verification code   {}", p.bold(p.yellow(pretty)));
    println!("  │");
    println!("  │  {}", p.dim(format!("{name} shows six digits now — they must be the same")));
    println!("  │");
    if !tty_yes(p, "codes match?", false)? {
        println!(
            "  {} {}",
            p.red("✗"),
            p.dim("not confirmed — tap “don't match” on the device, then start over"),
        );
        return Ok(());
    }
    let d = ask(serde_json::json!({"cmd": "pair_confirm", "session": session}))?;
    println!();
    println!(
        "  {} {} joined the family  {}",
        p.green("✓"),
        p.bold(name),
        p.dim(d["device_id"].as_str().unwrap_or("?")),
    );
    Ok(())
}

fn cmd_pair() -> anyhow::Result<()> {
    let p = Paint::new();
    if std::io::stdout().is_terminal() {
        let d = ask(serde_json::json!({"cmd": "pair"}))?;
        if d["pending"].as_array().is_none_or(|a| a.is_empty()) {
            println!(
                "  {}",
                p.dim("no pending pairings — the device must scan or type the code first"),
            );
            println!("  {}", p.dim("need an invite?  murmurctl invite"));
            return Ok(());
        }
        return wait_and_walk(&p, None);
    }
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
    let Some(cmd) = cli.cmd else {
        print_help(&Paint::new());
        return Ok(());
    };
    match cmd {
        Cmd::Status => cmd_status(),
        Cmd::Devices { all } => cmd_devices(all),
        Cmd::Revoke { device_id } => {
            match ask(serde_json::json!({"cmd": "revoke", "device_id": device_id})) {
                Ok(_) => println!("{device_id} revoked — live connection dropped"),
                Err(e) => anyhow::bail!(
                    "{e}\n  devices can only self-revoke over the wire — \
                     kick others from the relay host: `murmurs revoke --id {device_id}`"
                ),
            }
            Ok(())
        }
        Cmd::Invite => cmd_invite(),
        Cmd::Rotate => {
            let d = ask(serde_json::json!({"cmd": "rotate"}))?;
            let p = Paint::new();
            println!(
                "  {} family key rotated → epoch {}",
                p.green("✓"),
                d["epoch"].as_u64().unwrap_or(0),
            );
            Ok(())
        }
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
