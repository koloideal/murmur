//! Reconnecting WSS client. Sends Hello, drains the outbound envelope queue,
//! decrypts inbound envelopes and dispatches them through the feature hub.

use crate::feature::Hub;
use crate::state::Shared;
use futures_util::{SinkExt, StreamExt};
use murmur_proto::{Control, control::HelloRole};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as Ws;
use tracing::{debug, info, warn};

const BACKOFF_MAX: Duration = Duration::from_secs(60);

pub async fn run(
    shared: Arc<Shared>,
    hub: Arc<Hub>,
    mut out_rx: mpsc::UnboundedReceiver<murmur_proto::Envelope>,
) -> anyhow::Result<()> {
    let mut backoff = Duration::from_secs(1);
    loop {
        match connect_once(&shared, &hub, &mut out_rx).await {
            Ok(()) => info!("connection closed cleanly"),
            Err(e) => warn!("connection lost: {e:#}"),
        }
        shared.note_presence_offline();
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

async fn connect_once(
    shared: &Arc<Shared>,
    hub: &Hub,
    out_rx: &mut mpsc::UnboundedReceiver<murmur_proto::Envelope>,
) -> anyhow::Result<()> {
    let (ws, _) = tokio_tungstenite::connect_async(&shared.cfg.server_url).await?;
    let (mut sink, mut source) = ws.split();

    let hello = serde_json::to_string(&Control::Hello {
        device_id: shared.id,
        name: shared.cfg.name.clone(),
        role: HelloRole::Auth,
        token_hex: Some(shared.token_hex()),
    })?;
    sink.send(Ws::Text(hello.into())).await?;

    match source.next().await {
        Some(Ok(Ws::Text(t))) => match serde_json::from_str::<Control>(&t) {
            Ok(Control::HelloOk) => info!("authed to {}", shared.cfg.server_url),
            Ok(Control::HelloErr { reason }) => anyhow::bail!("auth rejected: {reason}"),
            other => anyhow::bail!("unexpected hello reply: {other:?}"),
        },
        _ => anyhow::bail!("no hello reply"),
    }

    // Reannounce our presence after every (re)connect.
    shared.broadcast_presence();

    loop {
        tokio::select! {
            // Outbound: sealed envelopes from features → WS.
            env = out_rx.recv() => {
                let Some(env) = env else { return Ok(()) };
                sink.send(Ws::Text(env.to_json().into())).await?;
            }
            // Inbound: WS → decrypt → feature hub.
            msg = source.next() => {
                let Some(msg) = msg else { return Ok(()) };
                let Ws::Text(t) = msg? else { continue };
                let env = match murmur_proto::Envelope::from_json(&t) {
                    Ok(e) => e,
                    Err(e) => {
                        debug!("bad envelope: {e}");
                        continue;
                    }
                };
                match env.to {
                    murmur_proto::Route::Server => {
                        // Control replies (Devices, Error) — ctl consumes these.
                        if let Ok(ctl) = env.control_payload() {
                            let _ = shared.ctl_events.send(ctl);
                        }
                    }
                    _ => match env.open(&shared.key) {
                        Ok(msg) => hub.dispatch(env.from, &msg),
                        Err(e) => warn!("decrypt from {} failed: {e}", env.from),
                    },
                }
            }
        }
    }
}
