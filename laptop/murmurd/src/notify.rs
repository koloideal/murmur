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
use crate::state::Shared;
use futures_util::StreamExt;
use murmur_proto::{Message, Route};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, warn};
use zbus::zvariant;

const BUS: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
const APP: &str = "murmur";

/// Dismissal was initiated by the user (not expiry, not our own close).
const REASON_DISMISSED: u32 = 2;

/// Everything the daemon can tell us about a posted notification.
enum BusEvent {
    /// (id, close reason) — reason 2 means the user dismissed it.
    Closed(u32, u32),
    /// (id, action key) — body click arrives as the "default" key;
    /// anything else is one of our `murmur-action-N` buttons.
    Invoked(u32, String),
    /// (id, reply text) — the non-standard KDE/swaync inline-reply
    /// contract: the daemon drew a text field inside the notification
    /// because we advertised an "inline-reply" action.
    Replied(u32, String),
}

/// Thin seam over the notification daemon.
struct NotifyBus {
    proxy: zbus::Proxy<'static>,
    events_rx: mpsc::UnboundedReceiver<BusEvent>,
}

impl NotifyBus {
    async fn connect() -> anyhow::Result<Self> {
        let conn = zbus::Connection::session().await?;
        let proxy = zbus::Proxy::new(&conn, BUS, PATH, BUS).await?;
        let (tx, events_rx) = mpsc::unbounded_channel();

        let mut closed = proxy.receive_signal("NotificationClosed").await?;
        let mut invoked = proxy.receive_signal("ActionInvoked").await?;
        // NotificationReplied is non-standard (KDE pioneered, swaync
        // implements) — subscribing is harmless on daemons that never
        // emit it; the stream just stays quiet.
        let mut replied = proxy.receive_signal("NotificationReplied").await?;

        tokio::spawn({
            let tx = tx.clone();
            async move {
                let _conn = conn; // keep the bus alive
                while let Some(sig) = closed.next().await {
                    if let Ok((id, reason)) = sig.body().deserialize::<(u32, u32)>() {
                        if tx.send(BusEvent::Closed(id, reason)).is_err() {
                            return;
                        }
                    }
                }
            }
        });
        tokio::spawn({
            let tx = tx.clone();
            async move {
                while let Some(sig) = invoked.next().await {
                    if let Ok((id, key)) = sig.body().deserialize::<(u32, String)>() {
                        if tx.send(BusEvent::Invoked(id, key)).is_err() {
                            return;
                        }
                    }
                }
            }
        });
        tokio::spawn(async move {
            while let Some(sig) = replied.next().await {
                if let Ok((id, text)) = sig.body().deserialize::<(u32, String)>() {
                    if tx.send(BusEvent::Replied(id, text)).is_err() {
                        return;
                    }
                }
            }
        });
        Ok(Self { proxy, events_rx })
    }

    /// Post/update a notification; returns the daemon-assigned id.
    async fn notify(
        &self,
        replaces_id: u32,
        app: &str,
        pkg: &str,
        title: &str,
        body: &str,
        icon_b64: Option<&str>,
        actions: &[murmur_proto::NotifAction],
    ) -> anyhow::Result<u32> {
        let icon_path = icon_b64.and_then(|b64| write_icon(pkg, b64));
        let mut hints: HashMap<String, zvariant::Value> = HashMap::new();
        // "dev.murmur/<pkg>": murmurd is the real sender, the prefix is
        // a unique marker daemons can match on (swaync visibility rules
        // pin urgency=low on it → .low class → the tag) — while the pkg
        // tail keeps per-app rules (e.g. ignored apps) working.
        hints.insert("desktop-entry".into(), format!("dev.murmur/{pkg}").into());
        // freedesktop action pairs: key/label alternating.
        // "default" — body click opens the notification on the phone.
        // "inline-reply" — the KDE/swaync contract: daemons that know it
        // draw a text field inside the notification and answer with a
        // NotificationReplied signal; daemons that don't just render an
        // inert button, so merge it into one Reply entry only when the
        // phone says this notification actually takes text.
        let mut pairs: Vec<String> = vec!["default".into(), "Open".into()];
        if actions.iter().any(|a| a.kind == "reply") {
            pairs.push("inline-reply".into());
            pairs.push("Reply".into());
            hints.insert(
                "x-kde-reply-placeholder-text".into(),
                "reply — sent to the phone".into(),
            );
        }
        for a in actions.iter().filter(|a| a.kind != "reply") {
            pairs.push(format!("murmur-action-{}", a.index));
            pairs.push(a.title.clone());
        }
        let id: u32 = self
            .proxy
            .call(
                "Notify",
                &(
                    app,
                    replaces_id,
                    icon_path.unwrap_or_default(),
                    title,
                    body,
                    pairs,
                    hints,
                    // 0 = never expire (-1 is the daemon's ~3s default,
                    // not "forever"). Dismissal is synced, not timed.
                    0i32,
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
    // pkg comes from the network — strip anything that isn't filename-safe.
    let safe_pkg: String = pkg
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let name = format!("{safe_pkg}-{:x}.img", fnv(&bytes));
    let path = dir.join(name);
    std::fs::write(&path, bytes).ok()?;
    Some(path.to_string_lossy().into_owned())
}

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x100000001b3)
    })
}

/// Is any of `names` a live process? argv0's basename must match —
/// comm is truncated to 15 chars, cmdline keeps the full name.
fn proc_running(names: &[String]) -> bool {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return false;
    };
    for e in dir.flatten() {
        let fname = e.file_name();
        if fname.to_str().is_none_or(|s| s.parse::<u32>().is_err()) {
            continue;
        }
        let Ok(cmd) = std::fs::read(e.path().join("cmdline")) else {
            continue;
        };
        let arg0 = cmd.split(|b| *b == 0).next().unwrap_or_default();
        let base = arg0.rsplit(|b| *b == b'/').next().unwrap_or_default();
        let base = String::from_utf8_lossy(base);
        if names.iter().any(|n| base == n.as_str()) {
            return true;
        }
    }
    false
}

pub fn feature() -> Feature {
    Feature::new(
        "notify",
        &["notification", "notification_dismiss"],
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

    // While a peer is reachable, desktop twins' own popups are closed —
    // the phone mirror covers them. Fail-open: if the watcher dies the
    // native popups simply reappear, nothing stays silenced.
    let (twin_tx, mut twin_rx) = mpsc::unbounded_channel::<u32>();
    let twins = shared.cfg.notify_hide_twins_if_online.clone();
    if !twins.is_empty() {
        let (shared2, tx) = (shared.clone(), twin_tx.clone());
        tokio::spawn(async move {
            if let Err(e) = monitor_twins(shared2, twins, tx).await {
                warn!("twin monitor died: {e:#}");
            }
        });
    }
    // Sender stays alive either way → twin_rx.recv() parks forever
    // when no monitor is running or it has died (fail-open).
    let _twin_tx = twin_tx;

    loop {
        tokio::select! {
            m = inbox.recv() => {
                let Some((_from, msg)) = m else { return Ok(()) };
                match (*msg).clone() {
                    Message::Notification { key, pkg, app, title, body, icon_b64, actions, .. } => {
                        // A live desktop twin already shows its own
                        // notifications — the phone copy is a duplicate.
                        let suppressed = shared
                            .cfg
                            .notify_suppress_if_running
                            .get(&pkg)
                            .is_some_and(|procs| !procs.is_empty() && proc_running(procs));
                        if suppressed {
                            debug!("notif from {pkg} suppressed — local client running");
                            continue;
                        }
                        let replaces = ids.get(&key).copied().unwrap_or(0);
                        let app = app.as_deref().unwrap_or(APP);
                        match bus.notify(replaces, app, &pkg, &title, &body, icon_b64.as_deref(), &actions).await {
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
            id = twin_rx.recv() => {
                let Some(id) = id else { continue };
                if let Err(e) = bus.close(id).await {
                    debug!("twin close {id}: {e:#}");
                }
            }
            ev = bus.events_rx.recv() => {
                let Some(ev) = ev else { return Ok(()) };
                match ev {
                    BusEvent::Closed(id, reason) => {
                        // Only echo user-initiated dismissals; expiry and
                        // our own CloseNotification calls stay local.
                        if reason != REASON_DISMISSED {
                            continue;
                        }
                        if let Some(key) = keys.remove(&id) {
                            ids.remove(&key);
                            let _ = shared.send(Route::All, &Message::NotificationDismiss { key });
                        }
                    }
                    BusEvent::Invoked(id, action) => {
                        let Some(key) = keys.get(&id).cloned() else { continue };
                        match action.as_str() {
                            // body click — the notification opens on the
                            // phone exactly as a tap there would do it
                            "default" => {
                                let _ = shared.send(Route::All, &Message::NotificationInvoke { key });
                            }
                            // the inline-reply action itself carries no
                            // text — NotificationReplied delivers that
                            "inline-reply" => {}
                            other => {
                                if let Some(idx) = other
                                    .strip_prefix("murmur-action-")
                                    .and_then(|s| s.parse::<u32>().ok())
                                {
                                    let _ = shared.send(
                                        Route::All,
                                        &Message::NotificationAction { key, index: idx },
                                    );
                                }
                            }
                        }
                    }
                    BusEvent::Replied(id, text) => {
                        let Some(key) = keys.get(&id).cloned() else { continue };
                        if !text.trim().is_empty() {
                            let _ = shared.send(Route::All, &Message::NotificationReply { key, text });
                        }
                    }
                }
            }
        }
    }
}

/// org.freedesktop.Notifications.Notify args — only app_name is read.
type NotifyArgs = (
    String,
    u32,
    String,
    String,
    String,
    Vec<String>,
    HashMap<String, zvariant::OwnedValue>,
    i32,
);

/// Eavesdrops the session bus for `Notify` calls from configured twin
/// app names; while any peer is reachable, forwards the daemon-assigned
/// notification id for closing so only the phone mirror shows.
async fn monitor_twins(
    shared: Arc<Shared>,
    apps: Vec<String>,
    close: mpsc::UnboundedSender<u32>,
) -> anyhow::Result<()> {
    use zbus::message::Type;
    let conn = zbus::Connection::session().await?;
    conn.call_method(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus.Monitoring"),
        "BecomeMonitor",
        &(
            &[
                "type='method_call',interface='org.freedesktop.Notifications',member='Notify'",
                "type='method_return',sender='org.freedesktop.Notifications'",
                "type='error',sender='org.freedesktop.Notifications'",
            ] as &[&str],
            0u32,
        ),
    )
    .await?;
    debug!("twin monitor up: {:?}", apps);
    let mut stream = zbus::MessageStream::from(conn);
    // Pending Notify calls: (caller, serial) → close the reply id?
    // Serials are per-connection — the caller name disambiguates.
    let mut pending: HashMap<(String, u32), bool> = HashMap::new();
    while let Some(msg) = stream.next().await {
        let Ok(msg) = msg else { break };
        let hdr = msg.header();
        match hdr.message_type() {
            Type::MethodCall => {
                let app = msg.body().deserialize::<NotifyArgs>().ok().map(|a| a.0);
                let hide = app
                    .as_ref()
                    .is_some_and(|a| apps.iter().any(|t| t == a))
                    && shared.peer_online();
                debug!("twin monitor Notify from {app:?} → hide={hide}");
                if let Some(from) = hdr.sender().map(|s| s.as_str().to_string()) {
                    pending.insert((from, hdr.primary().serial_num().get()), hide);
                }
            }
            Type::MethodReturn | Type::Error => {
                let (Some(rs), Some(to)) = (hdr.reply_serial(), hdr.destination()) else {
                    continue;
                };
                let Some(hide) = pending.remove(&(to.as_str().to_string(), rs.get())) else {
                    continue;
                };
                if hide && hdr.message_type() == Type::MethodReturn {
                    if let Ok(id) = msg.body().deserialize::<u32>() {
                        let _ = close.send(id);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}
