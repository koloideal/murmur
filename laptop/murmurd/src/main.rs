//! murmurd: laptop daemon. A conn task plus one task per feature.
//!
//! Features live in their own modules and register here — adding a feature
//! is a new file + one line in `registry()`.

mod clipboard;
mod config;
mod conn;
mod ctl;
mod feature;
mod mpris;
mod notify;
mod presence;
mod state;

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
        ctl::feature(),
    ]
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cfg = config::Config::load().context("load config")?;
    let secrets = config::Secrets::load().context(
        "load secrets — run `murmurd pair` (TODO: pairing mode) to provision this device",
    )?;
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
