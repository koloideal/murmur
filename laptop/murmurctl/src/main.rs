//! murmurctl: thin CLI over the murmurd unix socket.
//! Sends one JSON line, prints human-readable output.

use anyhow::Context;
use clap::{CommandFactory, Parser, Subcommand};
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
    /// Dial a LAN peer directly by ip[:port] — for networks where
    /// broadcast discovery can't pass (guest Wi-Fi, client isolation).
    Dial { addr: String },
    /// Remove every murmur artifact from this machine: the daemon unit,
    /// island bits, pairing secrets, completions, PATH markers, binaries.
    /// Does not need a running daemon — that's when people uninstall.
    Uninstall {
        /// Don't ask for confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Print shell completions (bash|zsh|fish) on stdout.
    Completions { shell: clap_complete::Shell },
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

/// Exit-code contract for scripts:
///   0 ok · 1 cli usage/other · 2 murmurd unreachable · 3 daemon
///   reported failure · 4 the *target* is unreachable (kind=offline).
/// Kinds ride the daemon's `kind` field; absent kinds are `internal`.

/// murmurd answered `ok:false` — carries the machine-readable class.
#[derive(Debug)]
struct DaemonError {
    kind: String,
    msg: String,
}
impl std::fmt::Display for DaemonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.msg)
    }
}
impl std::error::Error for DaemonError {}

/// The control socket itself wouldn't connect — murmurd is down.
#[derive(Debug)]
struct DaemonDown(String);
impl std::fmt::Display for DaemonDown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for DaemonDown {}

fn request(req: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    let path = sock_path();
    let stream = UnixStream::connect(&path).map_err(|_| {
        DaemonDown(format!("connect {} — is murmurd running?", path.display()))
    })?;
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
        Err(DaemonError {
            kind: res["kind"].as_str().unwrap_or("internal").to_string(),
            msg: res["error"].as_str().unwrap_or("daemon error").to_string(),
        }
        .into())
    }
}

/// Map a failure to its script-visible exit code.
fn exit_code(e: &anyhow::Error) -> i32 {
    if e.downcast_ref::<DaemonDown>().is_some() {
        return 2;
    }
    if let Some(de) = e.downcast_ref::<DaemonError>() {
        return if de.kind == "offline" { 4 } else { 3 };
    }
    1
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

/// Binary tag appended to the logo in a contrasting color.
const TAG: &str = "       _   _\n   ___| |_| |\n  / __| __| |\n | (__| |_| |\n  \\___|\\__|_|";

/// Installer-style banner: the logo bold in accent, a dim subtitle.
fn banner(p: &Paint, sub: &str) {
    println!();
    let __aw = ART.lines().map(|l| l.len()).max().unwrap_or(0);
    let __art_off: Vec<&str> = std::iter::once("").chain(ART.lines()).collect();
    for (a, t) in __art_off.iter().zip(TAG.lines()) {
        println!("  {}{}", p.wrap("1;38;5;141", format!("{a:<width$}", width = __aw)), p.wrap("38;5;215", t));
    }
    if !sub.is_empty() {
        println!("  {}", p.dim(sub));
    }
    println!();
}

fn print_help(p: &Paint) {
    let d = |s: &str| p.dim(s);
    let c = |s: &str| p.accent(format!("murmurctl {s:<22}"));
    let lines: Vec<String> = [
        vec![String::new()],
        vec![
            format!("  {} connection, presence, active node", c("status")),
            format!("  {} family devices (--all for revoked)", c("devices")),
            format!("  {} invite a device: QR + walkthrough", c("invite")),
            format!("  {} pending pairing requests", c("pair")),
            format!("  {} rotate the family key", c("rotate")),
            format!("  {} revoke a device (serverless: key rotation kicks it)", c("revoke")),
            format!("  {} dial a LAN peer by ip[:port]", c("dial")),
            format!("  {} remove every murmur artifact (--yes)", c("uninstall")),
        ],
        vec![
            String::new(),
            d("kick other devices from the relay host:"),
            d("murmurs revoke --id <device>"),
        ],
    ]
    .concat();
    banner(p, "murmur control - control the running murmurd");
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
    let fam_name = d["family_name"].as_str().filter(|s| !s.is_empty());
    println!(
        "  family  {} {}",
        fam_name.unwrap_or("—"),
        p.dim(format!(
            "· {}",
            d["family_kind"].as_str().unwrap_or("serverful")
        )),
    );
    if let Some(peers) = d["lan_peers"].as_array().filter(|p| !p.is_empty()) {
        let rtt = d["lan_rtt_ms"].as_object();
        let addrs = d["lan_addrs"].as_object();
        let usb: Vec<&str> = d["lan_usb"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        let list = peers
            .iter()
            .filter_map(|p| p.as_str())
            .map(|p| {
                let addr = addrs.and_then(|m| m.get(p)).and_then(|v| v.as_str());
                let ms = rtt
                    .and_then(|m| m.get(p))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let transport = if usb.contains(&p) { " usb" } else { "" };
                match (addr, ms) {
                    (Some(a), ms) if ms > 0 => format!("{p} {a}{transport} {ms}ms"),
                    (Some(a), _) => format!("{p} {a}{transport}"),
                    _ if ms > 0 => format!("{p} {ms}ms"),
                    _ => p.to_string(),
                }
            })
            .collect::<Vec<_>>()
            .join("  ");
        println!("  lan     {}", p.green(format!("⇄ {list}")));
    } else if let Some(addrs) = d["lan_addrs"].as_object().filter(|a| !a.is_empty()) {
        // No live link, but remembered addresses are being redialed —
        // say so instead of showing nothing.
        let list = addrs
            .iter()
            .map(|(id, a)| format!("{id} {}", a.as_str().unwrap_or("?")))
            .collect::<Vec<_>>()
            .join("  ");
        println!("  lan     {}", p.dim(format!("↻ redialing {list}")));
    } else if d["family_kind"].as_str() == Some("serverless") {
        println!(
            "  lan     {}",
            p.dim("no peers — broadcast-filtered network? murmurctl dial <peer-ip>"),
        );
    }
    if revoked {
        println!(
            "  {}",
            p.red("this machine was removed from the family — re-pair: murmurd join --code …"),
        );
    }
    println!(
        "  key     epoch {}  {}",
        d["key_epoch"].as_u64().unwrap_or(0),
        p.dim(format!(
            "· {} pinned member(s)",
            d["members"].as_u64().unwrap_or(0)
        )),
    );

    // Peers speaking another protocol generation — a family silently
    // half-deaf is the worst failure mode, so it goes loud right here.
    if let Some(mm) = d["proto_mismatch"].as_object().filter(|m| !m.is_empty()) {
        let list = mm
            .iter()
            .map(|(id, v)| format!("{id} (v{})", v.as_u64().unwrap_or(0)))
            .collect::<Vec<_>>()
            .join(", ");
        println!("  {}", p.yellow(format!("proto   mismatch: {list} — upgrade the peer")));
    }

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
    let mut devs: Vec<serde_json::Value> = d["devices"].as_array().cloned().unwrap_or_default();
    if !all {
        devs.retain(|x| x["revoked"].as_bool() != Some(true));
    }
    // Online first, then by name.
    devs.sort_by(|a, b| {
        let on = |x: &serde_json::Value| x["online"].as_bool() == Some(true);
        on(b).cmp(&on(a)).then_with(|| {
            a["name"]
                .as_str()
                .unwrap_or("")
                .cmp(b["name"].as_str().unwrap_or(""))
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
        let me = if id == selfid {
            p.dim(" (me)")
        } else {
            String::new()
        };
        println!("  {dot} {name:<w$}  {id}{me}  {tail}");
    }
    // A wiped or lost device never comes back — but nothing in this
    // list says what to do about it. The hint costs one line exactly
    // when it's relevant.
    let zombies: Vec<&str> = devs
        .iter()
        .filter(|x| {
            x["online"].as_bool() != Some(true)
                && x["revoked"].as_bool() != Some(true)
                && x["device_id"].as_str() != Some(selfid)
        })
        .filter_map(|x| x["device_id"].as_str())
        .collect();
    if !zombies.is_empty() {
        println!(
            "  {}",
            p.dim(format!(
                "offline for good? wiped or lost devices leave the family with:  murmurctl revoke {}",
                zombies[0]
            ))
        );
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
                    "laptop: murmurd join --server {server} --code {short}"
                ))
            );
        } else {
            // Serverless: `server` carries `host:port` of the one-shot
            // listener, and a LAN pair beacon makes the short code
            // resolvable without a relay — it's what a laptop feeds
            // to `murmurd join --code`.
            println!("  code      {}", p.bold(short));
            println!("  direct    {}", server);
            println!("  {}", p.dim("lan only — expires in 10 minutes"));
            println!(
                "\n  {}",
                p.dim("phone:  open murmur → scan the QR · or enter the code")
            );
            println!(
                "  {}",
                p.dim(format!("laptop: murmurd join --code {short}"))
            );
            // Beacon-free fallback for wi-fi that isolates clients or
            // subnets where broadcast can't pass.
            println!(
                "  {}",
                p.dim(format!("        or direct: murmurd join --code {short} --server {server}"))
            );
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
    println!("  {}", p.dim("ctrl-c finishes later with `murmurctl pair`"));
    let req = loop {
        // Spinner ticks every 120ms; the daemon is asked every 6th tick.
        if i % 6 == 0 {
            let d = ask(serde_json::json!({"cmd": "pair"}))?;
            let found = d["pending"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|x| only.is_none_or(|s| x["session"].as_str() == Some(s)));
            if let Some(x) = found {
                print!("\r\x1b[2K");
                break x.clone();
            }
        }
        if started.elapsed() > std::time::Duration::from_secs(600) {
            print!("\r\x1b[2K");
            println!(
                "  {}",
                p.dim("no request within 10 minutes — the invite expired")
            );
            return Ok(());
        }
        print!(
            "\r  {} {}",
            p.yellow(frames[i % frames.len()]),
            p.dim("waiting for the device to scan or type the code…"),
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
    println!(
        "  │  {}",
        p.dim(format!(
            "{name} shows six digits now — they must be the same"
        ))
    );
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
    // clap's own help has no banner — intercept it for visual consistency
    let raw: Vec<String> = std::env::args().collect();
    if raw.len() == 2 && (raw[1] == "--help" || raw[1] == "-h") {
        print_help(&Paint::new());
        return Ok(());
    }
    let cli = Cli::parse();
    let Some(cmd) = cli.cmd else {
        print_help(&Paint::new());
        return Ok(());
    };
    let r = dispatch(cmd);
    if let Err(e) = r {
        let code = exit_code(&e);
        eprintln!("{e:#}");
        std::process::exit(code);
    }
    Ok(())
}

fn dispatch(cmd: Cmd) -> anyhow::Result<()> {
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
            let d = ask(serde_json::json!({"cmd": "pair_confirm", "session": session}))?;
            println!(
                "paired — new device {}",
                d["device_id"].as_str().unwrap_or("?")
            );
            Ok(())
        }
        Cmd::Send => {
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf)?;
            let msg: serde_json::Value =
                serde_json::from_str(buf.trim()).context("send expects a Message JSON on stdin")?;
            ask(serde_json::json!({"cmd": "send", "msg_json": msg}))?;
            Ok(())
        }
        Cmd::Dial { addr } => {
            let p = Paint::new();
            let probe_target = if addr.contains(':') {
                addr.clone()
            } else {
                format!("{addr}:17894")
            };

            // Interactive probe: spin while TCP connects, verify the link
            // comes up through the daemon afterwards. A dial that cannot
            // connect is rejected before the daemon ever hears about it.
            use std::net::{TcpStream, ToSocketAddrs};
            let target = probe_target.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let resolved = target
                    .to_socket_addrs()
                    .ok()
                    .and_then(|mut it| it.next());
                let ok = resolved
                    .map(|sa| {
                        TcpStream::connect_timeout(&sa, std::time::Duration::from_millis(1500))
                            .is_ok()
                    })
                    .unwrap_or(false);
                let _ = tx.send(ok);
            });

            let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "ⴇ", "⦿", "⦗", "⧈", "⬗"];
            let mut i = 0usize;
            let probe = loop {
                if let Ok(ok) = rx.try_recv() {
                    break Some(ok);
                }
                if i * 90 > 2500 {
                    break None;
                }
                print!(
                    "\r  {} {}",
                    p.wrap("38;5;141", frames[i % frames.len()]),
                    p.dim(format!("connecting to {probe_target}…"))
                );
                let _ = std::io::Write::flush(&mut std::io::stdout());
                std::thread::sleep(std::time::Duration::from_millis(90));
                i += 1;
            };
            print!("\r{}\r", " ".repeat(60));

            match probe {
                Some(true) => {}
                _ => {
                    println!(
                        "  {} {} {}",
                        p.red("✗"),
                        p.bold("cannot reach"),
                        format!("{probe_target} — dial rejected, nothing was added")
                    );
                    println!(
                        "  {}",
                        p.dim("host unreachable or murmur is not listening there")
                    );
                    std::process::exit(4); // target offline — the contract's kind=offline
                }
            }
            println!(
                "  {} {} {}",
                p.green("✓"),
                p.dim("peer answers on"),
                p.bold(&probe_target)
            );

            ask(serde_json::json!({"cmd": "dial", "addr": addr}))?;

            // Now confirm the handshake actually completes: poll the daemon
            // for the peer showing up in the lan table.
            let ip = addr.split(':').next().unwrap_or(&addr).to_string();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let mut linked: Option<String> = None;
            while std::time::Instant::now() < deadline {
                if let Ok(d) = ask(serde_json::json!({"cmd": "status"})) {
                    if let (Some(peers), Some(addrs)) =
                        (d["lan_peers"].as_array(), d["lan_addrs"].as_object())
                    {
                        for peer in peers.iter().filter_map(|v| v.as_str()) {
                            if let Some(a) = addrs.get(peer).and_then(|v| v.as_str()) {
                                if a.starts_with(&ip) {
                                    linked = Some(peer.to_string());
                                    break;
                                }
                            }
                        }
                    }
                }
                if linked.is_some() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(300));
            }
            match linked {
                Some(name) => println!(
                    "  {} linked — {} joined the family table",
                    p.green("✓"),
                    p.bold(name)
                ),
                None => {
                    println!(
                        "  {} {}",
                        p.red("✗"),
                        "handshake never completed — wrong key family, firewall, or ghost host; the daemon will keep redialing in background"
                    );
                    // The honest verdict: no link exists. TCP-connect
                    // proves nothing on middleboxed networks (hotspots
                    // answer SYNs for dead addresses) — the handshake
                    // is the only real signal, and it failed.
                    std::process::exit(4);
                }
            }
            Ok(())
        }
        Cmd::Uninstall { yes } => cmd_uninstall(yes),
        Cmd::Completions { shell } => {
            let mut cli = Cli::command();
            let name = cli.get_name().to_string();
            clap_complete::generate(shell, &mut cli, name, &mut std::io::stdout());
            Ok(())
        }
    }
}

// ---------- uninstall: every murmur artifact, nothing left behind ----------

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// $XDG_<var>/murmur with the standard ~/.<fallback> default.
fn murmur_xdg(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(fallback))
        .join("murmur")
}

struct Rm<'a> {
    p: &'a Paint,
}
impl Rm<'_> {
    /// rm a file, printing what happened; absent files are quiet.
    fn file(&self, path: &PathBuf) {
        if path.exists() || path.is_symlink() {
            match std::fs::remove_file(path) {
                Ok(()) => println!("  {} {}", self.p.green("✓"), self.p.dim(path.display())),
                Err(e) => println!("  {} {} ({e})", self.p.red("✗"), path.display()),
            }
        }
    }
    /// rm -rf a dir, same semantics.
    fn dir(&self, path: &PathBuf) {
        if path.is_dir() {
            match std::fs::remove_dir_all(path) {
                Ok(()) => println!("  {} {}", self.p.green("✓"), self.p.dim(path.display())),
                Err(e) => println!("  {} {} ({e})", self.p.red("✗"), path.display()),
            }
        }
    }
}

/// Lines the installer writes into shell rc files all carry `# murmur`;
/// the zsh completions block hides two payload lines under its marker.
/// Rewrites each rc file minus our lines; returns true if it changed.
fn strip_rc_markers(path: &PathBuf) -> bool {
    let Ok(content) = std::fs::read_to_string(path) else {
        return false;
    };
    let mut out = String::with_capacity(content.len());
    let mut skip_payload = 0u8;
    for line in content.split_inclusive('\n') {
        let bare = line.trim_end_matches('\n');
        if bare.contains("# murmur completions") {
            skip_payload = 2; // fpath+= + compinit ride under this marker
            continue;
        }
        if skip_payload > 0 {
            skip_payload -= 1;
            continue;
        }
        if bare.contains("# murmur") {
            continue;
        }
        out.push_str(line);
    }
    if out == content {
        return false;
    }
    std::fs::write(path, out).is_ok()
}

/// Every path murmur can leave behind on a laptop-class install.
/// murmurt's own `uninstall` handles waybar/hyprland marker blocks —
/// we shell out to it when it's around, then sweep the files ourselves.
fn cmd_uninstall(yes: bool) -> anyhow::Result<()> {
    let p = Paint::new();
    let home = home_dir();
    let bin = home.join(".local/bin");
    let xdg_cfg = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    let xdg_data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"));

    let unit_dir = xdg_cfg.join("systemd/user");
    let unit = unit_dir.join("murmurd.service");
    let wants = unit_dir.join("graphical-session.target.wants/murmurd.service");

    let bins = ["murmurd", "murmurctl", "murmurt"]
        .map(|b| bin.join(b));
    let island = [
        "murmur-island.py",
        "murmur-island-hover.py",
        "murmur-player.py",
        "murmur-player-toggle.sh",
    ]
    .map(|b| bin.join(b));
    let dbus = xdg_data.join("dbus-1/services/murmur.player.service");
    let fonts = xdg_data.join("fonts/murmur");
    let dirs = [
        murmur_xdg("XDG_CONFIG_HOME", ".config"),
        murmur_xdg("XDG_DATA_HOME", ".local/share"),
        murmur_xdg("XDG_CACHE_HOME", ".cache"),
    ];
    let comps: Vec<PathBuf> = ["murmurd", "murmurctl", "murmurt"]
        .iter()
        .flat_map(|t| {
            [
                home.join(format!(".zfunc/_{t}")),
                xdg_data.join(format!("bash-completion/completions/{t}")),
                xdg_cfg.join(format!("fish/completions/{t}.fish")),
            ]
        })
        .collect();
    let rcs = [
        home.join(".bash_profile"),
        home.join(".profile"),
        home.join(".bashrc"),
        home.join(".zshrc"),
        xdg_cfg.join("fish/config.fish"),
    ];

    if !yes {
        println!("  {}", p.bold("this removes every murmur artifact:"));
        let mut any = false;
        let mut listing: Vec<PathBuf> = vec![
            unit.clone(), wants.clone(), dbus.clone(), fonts.clone(), sock_path(),
        ];
        listing.extend(dirs.iter().cloned());
        listing.extend(bins.iter().cloned());
        listing.extend(island.iter().cloned());
        listing.extend(comps.iter().cloned());
        for c in &listing {
            if c.exists() || c.is_symlink() {
                println!("    {}", p.dim(c.display()));
                any = true;
            }
        }
        for f in &rcs {
            if std::fs::read_to_string(f)
                .map(|s| s.contains("# murmur"))
                .unwrap_or(false)
            {
                println!("    {} {}", p.dim(f.display()), p.dim("(murmur lines)"));
                any = true;
            }
        }
        if !any {
            println!("    {}", p.dim("nothing found — already clean"));
            return Ok(());
        }
        if !std::io::stdin().is_terminal() {
            anyhow::bail!("not a tty — re-run with --yes to confirm removal");
        }
        print!("  {} ", p.accent("remove all of the above? (secrets included — re-pair required) [y/N]"));
        std::io::stdout().flush()?;
        let mut a = String::new();
        std::io::stdin().read_line(&mut a)?;
        if !matches!(a.trim().to_lowercase().as_str(), "y" | "yes") {
            anyhow::bail!("aborted — nothing removed");
        }
    }

    let rm = Rm { p: &p };
    println!("\n  {}", p.bold("stopping + removing the daemon unit"));
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "disable", "--now", "murmurd.service"])
        .status();
    // systemctl can quietly no-op — no user bus in this shell, or the
    // daemon was spawned bare rather than by the unit. Sweep survivors
    // by exact process name; a living daemon is the artifact people
    // actually mean by "uninstall didn't remove murmurd".
    let uid = std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_owned());
    let mut pkill = std::process::Command::new("pkill");
    pkill.arg("-x");
    if let Some(u) = &uid {
        pkill.args(["-U", u]);
    }
    let _ = pkill.arg("murmurd").status();
    // Verify: a survivor means an odd runtime (no procps, foreign pid
    // ns) — say so instead of leaving a ghost behind the deleted unit.
    let alive = std::process::Command::new("pgrep")
        .args(["-x", "murmurd"])
        .output()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);
    if alive {
        println!(
            "  {} a murmurd process is still alive — kill it by hand: pkill -x murmurd",
            p.yellow("!")
        );
    }
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status();
    rm.file(&wants);
    rm.file(&unit);
    rm.file(&sock_path());

    println!("  {}", p.bold("island (waybar/hyprland)"));
    // murmurt uninstall restores .murmurt-bak configs — run it when present;
    // spawn failure == absent: ~/.local/bin first, then a bare PATH lookup
    let murt_ok = [bin.join("murmurt"), PathBuf::from("murmurt")].iter().any(|m| {
        std::process::Command::new(m)
            .arg("uninstall")
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    });
    if !murt_ok {
        println!(
            "  {} no murmurt (or it failed) — sweeping its files; strip any murmur marker blocks in waybar/hyprland configs by hand",
            p.yellow("!")
        );
    }
    for f in &island {
        rm.file(f);
    }
    rm.file(&dbus);
    rm.dir(&fonts);

    println!("  {}", p.bold("state, config, cache"));
    for d in &dirs {
        rm.dir(d);
    }

    println!("  {}", p.bold("shell completions + PATH markers"));
    for f in &comps {
        rm.file(f);
    }
    for f in &rcs {
        if strip_rc_markers(f) {
            println!("  {} {} {}", p.green("✓"), p.dim(f.display()), p.dim("(murmur lines removed)"));
        }
    }

    println!("  {}", p.bold("binaries"));
    for f in &bins {
        rm.file(&PathBuf::from(format!("{}.etag", f.display())));
        rm.file(f);
    }

    println!(
        "\n  {} murmur is gone — open a new shell (PATH/completions drop)",
        p.green("✓")
    );
    Ok(())
}
