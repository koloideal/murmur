//! Wayland idle adapter: ext-idle-notify-v1 → `Idle` events.
//!
//! wayland-client's event queue is blocking, so it lives on its own
//! thread; the adapter's surface is a plain `Receiver<Idle>` that the
//! presence feature selects over. Best-effort: [`spawn`] fails cleanly
//! if the compositor lacks the protocol — presence then just misses
//! the idle source, everything else still works.

use std::time::Duration;
use tokio::sync::mpsc;
use wayland_client::globals::GlobalListContents;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Connection, Dispatch, QueueHandle, globals::registry_queue_init};
use wayland_protocols::ext::idle_notify::v1::client::ext_idle_notification_v1::{
    self, ExtIdleNotificationV1,
};
use wayland_protocols::ext::idle_notify::v1::client::ext_idle_notifier_v1::ExtIdleNotifierV1;

#[derive(Clone, Copy, Debug)]
pub enum Idle {
    Idled,
    Resumed,
}

struct State {
    tx: mpsc::UnboundedSender<Idle>,
}

impl Dispatch<ExtIdleNotificationV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let ev = match event {
            ext_idle_notification_v1::Event::Idled => Idle::Idled,
            ext_idle_notification_v1::Event::Resumed => Idle::Resumed,
            _ => return,
        };
        let _ = state.tx.send(ev);
    }
}

// No-op dispatch: registry globals are resolved at init, the seat is
// only a handle for the notifier (its capabilities event is ignored),
// the notifier itself has no events.
impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: wayland_client::protocol::wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlSeat,
        _: wayland_client::protocol::wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtIdleNotifierV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ExtIdleNotifierV1,
        _: wayland_protocols::ext::idle_notify::v1::client::ext_idle_notifier_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/// Bind the idle notifier and start dispatching on a background thread.
/// Returns the event receiver, or an error if Wayland/the protocol is
/// unavailable — callers treat that as "no idle source".
pub fn spawn(timeout: Duration) -> anyhow::Result<mpsc::UnboundedReceiver<Idle>> {
    let (tx, rx) = mpsc::unbounded_channel();
    let conn = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn)?;
    let qh = queue.handle();

    let seat: WlSeat = globals.bind(&qh, 1..=7, ())?;
    let notifier: ExtIdleNotifierV1 = globals
        .bind(&qh, 1..=2, ())
        .map_err(|e| anyhow::anyhow!("ext-idle-notify-v1 unsupported: {e}"))?;
    let _notification = notifier.get_idle_notification(
        u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX),
        &seat,
        &qh,
        (),
    );

    std::thread::Builder::new()
        .name("wlidle".into())
        .spawn(move || {
            let mut state = State { tx };
            loop {
                if queue.blocking_dispatch(&mut state).is_err() {
                    tracing::warn!("wlidle: wayland dispatch failed, idle source dead");
                    return;
                }
            }
        })?;
    Ok(rx)
}
