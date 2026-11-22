//! murmurd: laptop daemon. A conn task plus one task per feature.
//!
//! Features live in their own modules and register here — adding a feature
//! is a new file + one line in `registry()`.

mod clipboard;
mod config;
mod conn;
mod ctl;
mod feature;
mod install;
mod keyring;
mod lockproc;
mod logind;
mod mpris;
mod notify;
mod pair;
mod presence;
mod state;
mod wlclip;
mod wlidle;

use anyhow::Context;
use feature::{Feature, Hub};
use state::Shared;
use std::sync::Arc;
use tokio::sync::mpsc;

/// The feature registry. Order is irrelevant — each runs independently.
fn registry() -> Vec<Feature> {
    vec![
        presence::feature(),
        notify::feature(),
        mpris::feature(),
        clipboard::feature(),
        pair::feature(),
        keyring::feature(),
        ctl::feature(),
    ]
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    // rustls needs an explicit crypto provider — tokio-tungstenite pulls
    // it featureless, so ring is wired in murmurd's own Cargo.toml. Must
    // run before `pair` too — it opens a wss connection of its own.
    let _ = rustls::crypto::ring::default_provider().install_default();

    // `murmurd init` / `murmurd pair` — one-shot provisioning paths,
    // neither starts the daemon.
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("init") => return config::init(&args[1..]),
        Some("pair") => return pair_join(&args[1..]).await,
        Some("install") => return install::install_user_unit(),
        Some("doctor") => return doctor().await,
        Some("-h") | Some("--help") | Some("help") => {
            print_help();
            return Ok(());
        }
        Some(other) => anyhow::bail!("unknown command {other:?} — try `murmurd --help`"),
        None => {}
    }

    // Single instance: the lock socket is bound for the process lifetime.
    // A second murmurd would hijack the ctl socket and the server conn.
    let _lock = instance_lock()?;

    let cfg = config::Config::load().context("load config")?;
    let secrets = config::Secrets::load()
        .context("load secrets — run `murmurd init` (bootstrap) or pair this device first")?;
    tracing::info!(
        "murmurd {} ({}) → {}",
        secrets.device_id,
        cfg.name,
        cfg.server_url
    );

    // Outbound envelopes: features seal and push here, conn drains to WS.
    let (out_tx, out_rx) = mpsc::unbounded_channel();
    let shared = Arc::new(Shared::new(cfg, secrets, out_tx)?);

    // Wire features: each gets its filtered inbox, hub gets the sinks.
    let mut hub = Hub::new();
    let mut tasks: Vec<feature::BoxRun> = registry()
        .into_iter()
        .map(|f| f.wire(&shared, &mut hub))
        .collect();

    let hub = Arc::new(hub);
    tasks.push(Box::pin(conn::run(shared.clone(), hub, out_rx)));

    // All tasks are daemons; first error shuts everything down.
    futures_util::future::try_join_all(tasks).await?;
    Ok(())
}

/// Bind `$XDG_RUNTIME_DIR/murmur.lock` — a unix socket nobody accepts.
/// EADDRINUSE means either a live murmurd or a stale file; probing a
/// connect tells them apart (stale files refuse connections).
fn instance_lock() -> anyhow::Result<std::os::unix::net::UnixListener> {
    use std::os::unix::net::UnixListener;
    let path = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join("murmur.lock");
    match UnixListener::bind(&path) {
        Ok(l) => Ok(l),
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            if std::os::unix::net::UnixStream::connect(&path).is_ok() {
                anyhow::bail!("murmurd is already running (lock: {})", path.display());
            }
            std::fs::remove_file(&path)?; // stale lock from a dead instance
            Ok(UnixListener::bind(&path)?)
        }
        Err(e) => Err(e.into()),
    }
}

/// `murmurd doctor` — first-run and break-glass diagnostics: checks each
/// piece a healthy laptop needs and prints the fix when one is missing.
/// Non-destructive: the relay check is a bare WS handshake, no auth —
/// it never disturbs a running daemon's connection.
async fn doctor() -> anyhow::Result<()> {
    let mut bad = 0u32;
    let mut ok = |cond: bool, good: String, fix: &str| {
        if cond {
            println!("  ✓ {good}");
        } else {
            bad += 1;
            println!("  ✗ {fix}");
        }
    };

    // config.toml
    let cfg = config::Config::load().ok();
    match &cfg {
        Some(c) => {
            let sane = c.server_url.starts_with("ws://") || c.server_url.starts_with("wss://");
            ok(
                sane,
                format!("config.toml — {} ({})", c.server_url, c.name),
                "config.toml missing or server_url is not ws(s):// — run `murmurd pair` or `murmurd init`",
            );
        }
        None => ok(
            false,
            String::new(),
            "no config.toml — provision first: `murmurd pair --code …` or `murmurd init`",
        ),
    }

    // secrets.toml
    match config::Secrets::load() {
        Ok(s) => ok(
            s.device_id().is_ok() && s.family_key().is_ok(),
            format!("secrets.toml — device {}", s.device_id),
            "secrets.toml is corrupt — re-pair the device",
        ),
        Err(_) => ok(
            false,
            String::new(),
            "no secrets.toml — this laptop isn't in a family yet",
        ),
    }

    // systemd user unit
    let unit = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .map(|h| h.join(".config/systemd/user/murmurd.service"));
    let unit_exists = unit.as_ref().is_some_and(|p| p.exists());
    let active = std::process::Command::new("systemctl")
        .args(["--user", "is-active", "murmurd"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "active")
        .unwrap_or(false);
    ok(
        unit_exists,
        "systemd unit installed".into(),
        "no user unit — `murmurd install`",
    );
    if unit_exists {
        ok(
            active,
            "daemon running under systemd".into(),
            "unit installed but not running — `systemctl --user start murmurd`",
        );
    }

    // ctl socket: daemon alive outside systemd too
    if !active {
        let sock = std::env::var_os("XDG_RUNTIME_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
            .join("murmur.sock");
        ok(
            std::os::unix::net::UnixStream::connect(&sock).is_ok(),
            "daemon alive (ctl socket)".into(),
            "daemon not running — `systemctl --user start murmurd` or run murmurd",
        );
    }

    // relay reachability: bare WS handshake, no auth frame.
    if let Some(c) = &cfg {
        let t0 = std::time::Instant::now();
        match tokio::time::timeout(
            std::time::Duration::from_secs(8),
            tokio_tungstenite::connect_async(&c.server_url),
        )
        .await
        {
            Ok(Ok(_)) => ok(
                true,
                format!("relay reachable — handshake in {}ms", t0.elapsed().as_millis()),
                "",
            ),
            _ => ok(
                false,
                String::new(),
                "relay unreachable — check server_url, DNS, TLS and that murmurs is up",
            ),
        }
    }

    if bad == 0 {
        println!("all good");
        Ok(())
    } else {
        anyhow::bail!("{bad} problem(s) found")
    }
}

/// `murmurd pair --code <invite> [--name X]` — join the family through an
/// invite issued by `murmurctl invite` on an existing device.
async fn pair_join(args: &[String]) -> anyhow::Result<()> {
    fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .map(String::as_str)
    }
    let code = flag(args, "--code")
        .context("usage: murmurd pair --code CODE [--server URL] [--name NAME]")?;
    let server = flag(args, "--server").map(str::to_owned);
    let name = flag(args, "--name")
        .map(str::to_owned)
        .or_else(|| std::env::var("HOSTNAME").ok())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "murmur-laptop".into());

    let paired = pair::join(code, &name, server.as_deref()).await?;
    let cfg = config::Config {
        server_url: paired.server_url,
        name,
        ..config::Config::default()
    };
    let secrets = config::Secrets {
        device_id: paired.device_id.to_string(),
        auth_token_hex: paired.token_hex,
        family_key_hex: paired.family_key_hex,
        key_epoch: paired.key_epoch,
        ..config::Secrets::default()
    };
    config::provision(&cfg, &secrets)?;
    eprintln!("  │");
    eprintln!("  ✓ paired as {}", paired.device_id);
    if let Err(e) = install::install_user_unit() {
        eprintln!("  ! unit install skipped: {e:#} — start murmurd manually");
    }
    Ok(())
}

// ---------- pretty help (same banner as the installers) ----------

fn print_help() {
    let tty = std::io::IsTerminal::is_terminal(&std::io::stdout());
    let ac = |s: &str| if tty { format!("\x1b[1;38;5;141m{s}\x1b[0m") } else { s.into() };
    let dm = |s: &str| if tty { format!("\x1b[2m{s}\x1b[0m") } else { s.into() };
    let bd = |s: &str| if tty { format!("\x1b[1m{s}\x1b[0m") } else { s.into() };
    let cc = |s: &str| if tty { format!("\x1b[38;5;141m{s}\x1b[0m") } else { s.into() };
    let art = " _ __ ___  _   _ _ __ _ __ ___  _   _ _ __\n| \x27_ ` _ \\| | | | \x27__| \x27_ ` _ \\| | | | \x27__|\n| | | | | | |_| | |  | | | | | | |_| | |\n|_| |_| |_|\\__,_|_|  |_| |_| |_|\\__,_|_|";
    println!();
    for l in art.lines() {
        println!("  {}", ac(l));
    }
    println!("  {}", dm("the murmur client daemon"));
    println!();
    let rows: &[(&str, &str)] = &[
        ("murmurd", "run the daemon"),
        ("murmurd init", "name, server, credentials"),
        ("murmurd pair", "join via invite code"),
        ("murmurd install", "systemd user unit"),
        ("murmurd doctor", "check config + relay"),
    ];
    println!("  {}", bd("commands"));
    for (c, d) in rows {
        println!("  {} {}", cc(&format!("{c:<18}")), dm(d));
    }
    println!();
    println!("  {}", dm("control a running daemon with murmurctl"));
    println!();
}
