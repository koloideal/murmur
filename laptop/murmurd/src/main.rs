//! murmurd: laptop daemon. A conn task plus one task per feature.
//!
//! Features live in their own modules and register here — adding a feature
//! is a new file + one line in `registry()`.

mod clipboard;
mod config;
mod conn;
mod ctl;
mod feature;
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

    // `murmurd init` / `murmurd pair` — one-shot provisioning paths,
    // neither starts the daemon.
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("init") => return config::init(&args[1..]),
        Some("pair") => return pair_join(&args[1..]).await,
        _ => {}
    }

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

/// `murmurd pair --code <invite> [--name X]` — join the family through an
/// invite issued by `murmurctl invite` on an existing device.
async fn pair_join(args: &[String]) -> anyhow::Result<()> {
    fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .map(String::as_str)
    }
    let code = flag(args, "--code").context("usage: murmurd pair --code CODE [--name NAME]")?;
    let name = flag(args, "--name")
        .map(str::to_owned)
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "murmur-laptop".into());

    let paired = pair::join(code, &name).await?;
    let cfg = config::Config {
        server_url: paired.server_url,
        name,
        ..config::Config::default()
    };
    let secrets = config::Secrets {
        device_id: paired.device_id.to_string(),
        auth_token_hex: paired.token_hex,
        family_key_hex: paired.family_key_hex,
    };
    config::provision(&cfg, &secrets)?;
    eprintln!("paired as {} — start murmurd", paired.device_id);
    Ok(())
}
