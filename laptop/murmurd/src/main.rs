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
mod links;
mod lan;
mod integ;
mod lockproc;
mod logind;
mod mpris;
mod notify;
mod pair;
mod presence;
mod state;
mod usbwatch;
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
        Some("run") => {}
        Some("init") => return config::init(&args[1..]),
        Some("pair") => return pair_join(&args[1..]).await,
        Some("install") => return install::install_user_unit(),
        Some("doctor") => return doctor().await,
        Some("completions") => return print_completions(args.get(1).map(String::as_str)),
        Some("-h") | Some("--help") | Some("help") => {
            print_help();
            return Ok(());
        }
        Some(other) => anyhow::bail!("unknown command {other:?} — try `murmurd --help`"),
        None => {
            print_help();
            return Ok(());
        }
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

    // Outbound envelopes: features seal and push here, conn drains to
    // WS — hi (commands/control) always wins over lo (replaceable state).
    let (out_tx, out_rx) = mpsc::unbounded_channel();
    let (lo_tx, lo_rx) = mpsc::unbounded_channel();
    let (dial_tx, dial_rx) = mpsc::unbounded_channel();
    let shared = Arc::new(Shared::new(cfg, secrets, out_tx, lo_tx, dial_tx)?);

    // Wire features: each gets its filtered inbox, hub gets the sinks.
    let mut hub = Hub::new();
    let mut tasks: Vec<feature::BoxRun> = registry()
        .into_iter()
        .map(|f| f.wire(&shared, &mut hub))
        .collect();

    let hub = Arc::new(hub);
    // A serverless family has no relay in its world — the conn task
    // simply doesn't exist; the LAN mesh is the whole transport.
    if shared.cfg.family_kind == murmur_proto::FamilyKind::Serverful {
        tasks.push(Box::pin(conn::run(shared.clone(), hub.clone(), out_rx, lo_rx)));
    }
    tasks.push(Box::pin(lan::run(shared.clone(), hub.clone(), dial_rx)));

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
            let sane = c.family_kind == murmur_proto::FamilyKind::Serverless
                || c.server_url.starts_with("ws://")
                || c.server_url.starts_with("wss://");
            ok(
                sane,
                format!(
                    "config.toml — {} · {:?} ({})",
                    if c.server_url.is_empty() { "no relay" } else { &c.server_url },
                    c.family_kind,
                    c.name,
                ),
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

    // Wayland capabilities — clipboard sync and idle detection degrade
    // silently without them; the fix is the compositor, not a package.
    match crate::wlclip::probe() {
        Ok(caps) => {
            let dc = caps.iter().find(|c| c.contains("data-control"));
            ok(
                dc.is_some(),
                format!("wayland data-control — {}", dc.copied().unwrap_or("?")),
                "no data-control protocol — clipboard sync won't work on this compositor (GNOME?); needs ext-data-control-v1 or wlr-data-control-v1",
            );
            ok(
                caps.contains(&"ext-idle-notify-v1"),
                "wayland idle-notify — presence tracking works".into(),
                "no ext-idle-notify-v1 — this node will never report idle (presence degrades)",
            );
        }
        Err(_) => ok(
            false,
            String::new(),
            "no wayland session — murmurd needs Wayland for clipboard/idle; check WAYLAND_DISPLAY",
        ),
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

    // relay reachability: bare WS handshake, no auth frame. A
    // serverless family has no relay — nothing to check.
    if let Some(c) = cfg
        .as_ref()
        .filter(|c| c.family_kind == murmur_proto::FamilyKind::Serverful)
    {
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
        family_kind: paired.kind,
        server_url: paired.server_url,
        name,
        ..config::Config::default()
    };
    let secrets = config::Secrets {
        device_id: paired.device_id.to_string(),
        auth_token_hex: paired.token_hex,
        family_key_hex: paired.family_key_hex,
        key_epoch: paired.key_epoch,
        family_name: paired.family_name,
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
    let tag = "      _\n   __| |\n  / _` |\n | (_| |\n  \\__,_|";
    let tg = |s: &str| if tty { format!("\x1b[38;5;215m{s}\x1b[0m") } else { s.into() };
    println!();
    let __aw = art.lines().map(|l| l.len()).max().unwrap_or(0);
    let __art_off: Vec<&str> = std::iter::once("").chain(art.lines()).collect();
    for (l, t) in __art_off.iter().zip(tag.lines()) {
        println!("  {}{}", ac(&format!("{l:<width$}", width = __aw)), tg(t));
    }
    println!("  {}", dm("murmur daemon - the murmur client daemon"));
    println!();
    let rows: &[(&str, &str)] = &[
        ("murmurd run", "run the daemon"),
        ("murmurd init", "name, server, credentials"),
        ("murmurd pair", "join via invite code"),
        ("murmurd install", "systemd user unit"),
        ("murmurd doctor", "check config + relay"),
        ("murmurd completions", "shell completions on stdout"),
    ];
    println!("  {}", bd("commands"));
    for (c, d) in rows {
        println!("  {} {}", cc(&format!("{c:<18}")), dm(d));
    }
    println!();
    println!("  {}", dm("control a running daemon with murmurctl"));
    println!();
}

/// `murmurd completions <shell>` — hand-written, the command surface is
/// five verbs; not worth pulling clap into the daemon for generation.
fn print_completions(shell: Option<&str>) -> anyhow::Result<()> {
    let shell = shell.unwrap_or("zsh");
    let out = match shell {
        "zsh" => r#"#compdef murmurd
_murmurd() {
  local -a cmds
  cmds=(
    'run:run the daemon'
    'init:name, server, credentials'
    'pair:join via invite code'
    'install:systemd user unit'
    'doctor:check config + relay'
    'completions:shell completions on stdout'
  )
  if (( CURRENT == 2 )); then
    _describe 'command' cmds
  else
    case $words[2] in
      init) _arguments '--server[relay url]:url:' '--name[device name]:name:' '--kind[family kind]:kind:(serverful serverless)' ;;
      pair) _arguments '--server[relay url]:url:' '--code[invite code]:code:' ;;
      completions) _arguments '1:shell:(bash zsh fish)' ;;
    esac
  fi
}
_murmurd "$@"
"#,
        "bash" => r#"_murmurd() {
  local cur prev cmds
  COMPREPLY=()
  cur="${COMP_WORDS[COMP_CWORD]}"
  cmds="run init pair install doctor completions"
  if [ "$COMP_CWORD" -eq 1 ]; then
    COMPREPLY=($(compgen -W "$cmds" -- "$cur"))
  elif [ "${COMP_WORDS[1]}" = "completions" ]; then
    COMPREPLY=($(compgen -W "bash zsh fish" -- "$cur"))
  elif [ "${COMP_WORDS[1]}" = "init" ] || [ "${COMP_WORDS[1]}" = "pair" ]; then
    case "$prev" in
      murmurd|init|pair) COMPREPLY=($(compgen -W "--server --name --kind --code" -- "$cur")) ;;
    esac
  fi
}
complete -F _murmurd murmurd
"#,
        "fish" => r#"complete -c murmurd -n '__fish_use_subcommand' -a run -d 'run the daemon'
complete -c murmurd -n '__fish_use_subcommand' -a init -d 'name, server, credentials'
complete -c murmurd -n '__fish_use_subcommand' -a pair -d 'join via invite code'
complete -c murmurd -n '__fish_use_subcommand' -a install -d 'systemd user unit'
complete -c murmurd -n '__fish_use_subcommand' -a doctor -d 'check config + relay'
complete -c murmurd -n '__fish_use_subcommand' -a completions -d 'shell completions on stdout'
complete -c murmurd -n '__fish_seen_subcommand_from completions' -a 'bash zsh fish'
"#,
        other => anyhow::bail!("unknown shell {other:?} — bash|zsh|fish"),
    };
    print!("{out}");
    Ok(())
}
