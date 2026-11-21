//! Notification feature: phone notifications land in the desktop
//! notification daemon (swaync & friends) over freedesktop Notifications;
//! dismissals sync both ways.
//!
//! Inbound `Notification` → `org.freedesktop.Notifications.Notify`
//! (repost with the same `key` replaces in place). `NotificationDismiss`
//! → `CloseNotification`. A user-initiated close (`NotificationClosed`
//! reason=2) is broadcast back as `NotificationDismiss` — the phone and
//! every other laptop drop it. Reasons 1/3 (expired / we closed it
//! ourselves) don't echo.
//!
//! State (`ids`/`keys`) is feature-local; the D-Bus seam is the private
//! [`NotifyBus`] — call surface: `notify`, `close`, one event channel.

use crate::feature::{Feature, FeatureCtx};
use futures_util::StreamExt;
use murmur_proto::{Message, Route};
use std::collections::HashMap;
use tokio::sync::mpsc;
use tracing::{debug, warn};
use zbus::zvariant;

const BUS: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
const APP: &str = "murmur";

/// Dismissal was initiated by the user (not expiry, not our own close).
const REASON_DISMISSED: u32 = 2;

/// Thin seam over the notification daemon.
struct NotifyBus {
    proxy: zbus::Proxy<'static>,
    /// (notification id, close reason)
    closed_rx: mpsc::UnboundedReceiver<(u32, u32)>,
}

impl NotifyBus {
    async fn connect() -> anyhow::Result<Self> {
        let conn = zbus::Connection::session().await?;
        let proxy = zbus::Proxy::new(&conn, BUS, PATH, BUS).await?;
        let mut stream = proxy.receive_signal("NotificationClosed").await?;
        let (tx, closed_rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let _conn = conn; // keep the bus alive
            while let Some(sig) = stream.next().await {
                if let Ok(args) = sig.body().deserialize::<(u32, u32)>() {
                    if tx.send(args).is_err() {
                        return;
                    }
                }
            }
        });
        Ok(Self { proxy, closed_rx })
    }

    /// Post/update a notification; returns the daemon-assigned id.
    async fn notify(
        &self,
        replaces_id: u32,
        pkg: &str,
        title: &str,
        body: &str,
        icon_b64: Option<&str>,
    ) -> anyhow::Result<u32> {
        let icon_path = icon_b64.and_then(|b64| write_icon(pkg, b64));
        let mut hints: HashMap<String, zvariant::Value> = HashMap::new();
        hints.insert("desktop-entry".into(), pkg.into());
        if let Some(p) = &icon_path {
            hints.insert("image-path".into(), p.clone().into());
        }
        let id: u32 = self
            .proxy
            .call(
                "Notify",
                &(
                    APP,
                    replaces_id,
                    icon_path.unwrap_or_default(),
                    title,
                    body,
                    Vec::<String>::new(),
                    hints,
                    -1i32, // never expire — dismissal is synced, not timed
                ),
            )
            .await?;
        Ok(id)
    }

    async fn close(&self, id: u32) -> anyhow::Result<()> {
        let _: () = self.proxy.call("CloseNotification", &(id,)).await?;
        Ok(())
    }
}

/// Phone icons arrive as opaque bytes; the daemon wants a path.
fn write_icon(pkg: &str, b64: &str) -> Option<String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "/tmp".into())
        .join("murmur/icons");
    std::fs::create_dir_all(&dir).ok()?;
    // Stable name per package+bytes so reposts overwrite, not accumulate.
    let name = format!("{pkg}-{:x}.img", fnv(&bytes));
    let path = dir.join(name);
    std::fs::write(&path, bytes).ok()?;
    Some(path.to_string_lossy().into_owned())
}

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x100000001b3)
    })
}

pub fn feature() -> Feature {
    Feature::new(
        "notify",
        Some(|m| {
            matches!(
                m,
                Message::Notification { .. } | Message::NotificationDismiss { .. }
            )
        }),
        |ctx| Box::pin(run(ctx)),
    )
}

async fn run(ctx: FeatureCtx) -> anyhow::Result<()> {
    let shared = ctx.shared;
    let mut inbox = ctx.inbox.expect("feature declares wants");
    // No notification daemon on this box (headless, ssh) — the feature
    // idles instead of killing the daemon.
    let mut bus = match NotifyBus::connect().await {
        Ok(b) => b,
        Err(e) => {
            warn!("notify feature disabled: {e:#}");
            while inbox.recv().await.is_some() {}
            return Ok(());
        }
    };

    // Phone key ↔ daemon id, both directions.
    let mut ids: HashMap<String, u32> = HashMap::new();
    let mut keys: HashMap<u32, String> = HashMap::new();

    loop {
        tokio::select! {
            m = inbox.recv() => {
                let Some((_from, msg)) = m else { return Ok(()) };
                match msg {
                    Message::Notification { key, pkg, title, body, icon_b64, .. } => {
                        let replaces = ids.get(&key).copied().unwrap_or(0);
                        match bus.notify(replaces, &pkg, &title, &body, icon_b64.as_deref()).await {
                            Ok(id) => {
                                ids.insert(key.clone(), id);
                                keys.insert(id, key);
                            }
                            Err(e) => warn!("notify: {e:#}"),
                        }
                    }
                    Message::NotificationDismiss { key } => {
                        if let Some(id) = ids.remove(&key) {
                            keys.remove(&id);
                            if let Err(e) = bus.close(id).await {
                                debug!("close {id}: {e:#}");
                            }
                        }
                    }
                    _ => {}
                }
            }
            ev = bus.closed_rx.recv() => {
                let Some((id, reason)) = ev else { return Ok(()) };
                // Only echo user-initiated dismissals; expiry and our own
                // CloseNotification calls stay local.
                if reason != REASON_DISMISSED {
                    continue;
                }
                if let Some(key) = keys.remove(&id) {
                    ids.remove(&key);
                    let _ = shared.send(Route::All, &Message::NotificationDismiss { key });
                }
            }
        }
    }
}
