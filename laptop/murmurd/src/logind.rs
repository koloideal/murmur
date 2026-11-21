//! logind adapter: session Lock/Unlock → `Lock` events.
//!
//! The user's graphical session is found by seat (its object path, not
//! PID — murmurd may run under the systemd manager session). Initial
//! `LockedHint` is read once so we don't guess on startup.

use anyhow::Context;
use futures_util::StreamExt;
use tokio::sync::mpsc;

const BUS: &str = "org.freedesktop.login1";
const MANAGER_PATH: &str = "/org/freedesktop/login1";
const MANAGER_IFACE: &str = "org.freedesktop.login1.Manager";
const SESSION_IFACE: &str = "org.freedesktop.login1.Session";

#[derive(Clone, Copy, Debug)]
pub enum Lock {
    Locked,
    Unlocked,
}

/// Initial lock state plus a stream of transitions.
pub struct Watch {
    pub initially_locked: bool,
    pub rx: mpsc::UnboundedReceiver<Lock>,
}

async fn session_path(conn: &zbus::Connection) -> anyhow::Result<zbus::zvariant::OwnedObjectPath> {
    let mgr = zbus::Proxy::new(conn, BUS, MANAGER_PATH, MANAGER_IFACE).await?;
    // ListSessions → (id, uid, user_name, seat, object_path).
    let sessions: Vec<(String, u32, String, String, zbus::zvariant::OwnedObjectPath)> =
        mgr.call("ListSessions", &()).await?;
    // Prefer a wayland graphical session on a seat; fall back to any
    // session that has a seat (x11, tty greeter-launched wayland, ...).
    for (id, _, _, seat, path) in &sessions {
        if seat.is_empty() {
            continue;
        }
        let ses = zbus::Proxy::new(conn, BUS, path.as_str(), SESSION_IFACE).await?;
        if ses.get_property::<String>("Type").await.ok().as_deref() == Some("wayland") {
            return Ok(path.clone());
        }
        let _ = id;
    }
    sessions
        .iter()
        .find(|(_, _, _, seat, _)| !seat.is_empty())
        .map(|(_, _, _, _, path)| path.clone())
        .context("no graphical session with a seat")
}

/// Subscribe to Lock/Unlock signals on the graphical session.
/// Fails cleanly when logind or a suitable session is unavailable.
pub async fn watch() -> anyhow::Result<Watch> {
    let conn = zbus::Connection::system().await?;
    let path = session_path(&conn).await?;
    let ses = zbus::Proxy::new(&conn, BUS, path.as_str(), SESSION_IFACE).await?;

    let initially_locked = ses
        .get_property::<bool>("LockedHint")
        .await
        .unwrap_or(false);
    let mut lock_stream = ses.receive_signal("Lock").await?;
    let mut unlock_stream = ses.receive_signal("Unlock").await?;

    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        // Keeps the bus connection alive while the streams live.
        let _conn = conn;
        loop {
            let ev = tokio::select! {
                s = lock_stream.next() => s.map(|_| Lock::Locked),
                s = unlock_stream.next() => s.map(|_| Lock::Unlocked),
            };
            match ev {
                Some(ev) if tx.send(ev).is_ok() => {}
                _ => break, // stream ended or nobody listens — bail out
            }
        }
    });
    Ok(Watch {
        initially_locked,
        rx,
    })
}
