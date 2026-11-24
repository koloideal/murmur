//! Wayland clipboard adapter: ext-data-control-v1.
//!
//! One tokio task owns the connection; `AsyncFd` on the wayland socket
//! lets it multiplex compositor events with apply commands — no extra
//! threads for the event loop (offer reads and paste writes do block,
//! so they go to `spawn_blocking`).
//!
//! Surface: [`spawn`] → `(Receiver<ClipEvent>, Sender<ClipCmd>)`.
//! - `ClipEvent::Data` — local selection changed, data already read.
//! - `ClipEvent::Lost` — selection cleared.
//! - `ClipCmd::Set` — own the selection and serve it on paste.
//!
//! Only the regular selection is handled; primary (middle-click) is not
//! synced. Best-effort like every adapter: fails cleanly without the
//! protocol.

use anyhow::Context;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use tokio::io::unix::AsyncFd;
use tokio::sync::mpsc;
use wayland_client::globals::GlobalListContents;
use wayland_client::protocol::wl_registry::{Event as RegEvent, WlRegistry};
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Connection, Dispatch, QueueHandle, globals::registry_queue_init};
use wayland_protocols::ext::data_control::v1::client::ext_data_control_device_v1::{
    self, ExtDataControlDeviceV1,
};
use wayland_protocols::ext::data_control::v1::client::ext_data_control_manager_v1::ExtDataControlManagerV1;
use wayland_protocols::ext::data_control::v1::client::ext_data_control_offer_v1::{
    self, ExtDataControlOfferV1,
};
use wayland_protocols::ext::data_control::v1::client::ext_data_control_source_v1::{
    self, ExtDataControlSourceV1,
};

#[derive(Debug)]
pub enum ClipEvent {
    /// Local selection changed; preferred mime + full data. `offered` is
    /// every mime the source advertised — policies (e.g. password hints)
    /// must inspect the whole offer, not just the mime we read.
    Data {
        mime: String,
        offered: Vec<String>,
        data: Vec<u8>,
    },
    /// Selection was cleared.
    Lost,
}

#[derive(Debug)]
pub enum ClipCmd {
    /// Take the selection and serve `data` to pasters.
    Set { mime: String, data: Vec<u8> },
}

/// Mimes we sync, most preferred first.
const PREFERRED: &[&str] = &[
    "image/png",
    "text/plain;charset=utf-8",
    "text/plain",
    "UTF8_STRING",
];

fn pick_mime(mimes: &[String]) -> Option<String> {
    PREFERRED
        .iter()
        .find(|want| mimes.iter().any(|m| m.as_str() == **want))
        .map(|s| s.to_string())
        .or_else(|| mimes.first().cloned())
}

struct State {
    events: mpsc::UnboundedSender<ClipEvent>,
    /// Offer object + collected mimes for the pending selection.
    offer: Option<(ExtDataControlOfferV1, Vec<String>)>,
    /// Source we set ourselves — kept alive to serve `send` events.
    source: Option<(ExtDataControlSourceV1, String, Vec<u8>)>,
}

// ----- dispatch -----

impl Dispatch<ExtDataControlDeviceV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtDataControlDeviceV1,
        event: ext_data_control_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_data_control_device_v1::Event::DataOffer { id } => {
                state.offer = Some((id, Vec::new()));
            }
            ext_data_control_device_v1::Event::Selection { id } => match id {
                Some(offer) => {
                    // The mimes for this offer were collected under
                    // `offer` events; read it if it's our tracked one.
                    let mimes = match &state.offer {
                        Some((o, mimes)) if *o == offer => mimes.clone(),
                        _ => Vec::new(),
                    };
                    state.offer = None;
                    let Some(mime) = pick_mime(&mimes) else {
                        tracing::debug!("clip: selection with no usable mime");
                        return;
                    };
                    tracing::debug!("clip: selection → reading offer as {mime}");
                    read_offer(offer, mime, mimes, state.events.clone());
                }
                None => {
                    let _ = state.events.send(ClipEvent::Lost);
                }
            },
            _ => {} // finished, primary_selection — not synced
        }
    }

    // `data_offer` events create the offer object — hand it our udata.
    fn event_created_child(
        _opcode: u16,
        qhandle: &QueueHandle<Self>,
    ) -> std::sync::Arc<dyn wayland_client::backend::ObjectData> {
        qhandle.make_data::<ExtDataControlOfferV1, _>(())
    }
}

impl Dispatch<ExtDataControlOfferV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtDataControlOfferV1,
        event: ext_data_control_offer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_data_control_offer_v1::Event::Offer { mime_type } = event {
            if let Some((_, mimes)) = &mut state.offer {
                mimes.push(mime_type);
            }
        }
    }
}

impl Dispatch<ExtDataControlSourceV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtDataControlSourceV1,
        event: ext_data_control_source_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_data_control_source_v1::Event::Send { mime_type: _, fd } => {
                let Some((_, _, data)) = &state.source else {
                    return;
                };
                let mut data = data.clone();
                tokio::task::spawn_blocking(move || {
                    let mut f = std::fs::File::from(fd);
                    let _ = f.write_all(&mut data);
                });
            }
            ext_data_control_source_v1::Event::Cancelled => {
                state.source = None;
            }
            _ => {}
        }
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: RegEvent,
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

impl Dispatch<ExtDataControlManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ExtDataControlManagerV1,
        _: wayland_protocols::ext::data_control::v1::client::ext_data_control_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/// `offer.receive(mime, fd)` hands the writer end to the source client;
/// we read the other end to EOF off-thread, then emit `ClipEvent::Data`.
fn read_offer(
    offer: ExtDataControlOfferV1,
    mime: String,
    offered: Vec<String>,
    events: mpsc::UnboundedSender<ClipEvent>,
) {
    let Ok((mut rd, wr)) = UnixStream::pair() else {
        return;
    };
    offer.receive(mime.clone(), wr.as_fd());
    drop(wr); // our copy must close so reads see EOF
    offer.destroy();
    tokio::task::spawn_blocking(move || {
        let mut data = Vec::new();
        match rd.read_to_end(&mut data) {
            Ok(n) => {
                tracing::debug!("clip: read {n}B as {mime}");
                if !data.is_empty() {
                    let _ = events.send(ClipEvent::Data {
                        mime,
                        offered,
                        data,
                    });
                }
            }
            Err(e) => tracing::debug!("clip: offer read failed: {e}"),
        }
    });
}

use std::os::fd::AsFd;

/// Start the clipboard adapter: watch selection changes, accept `Set`
/// commands. Fails cleanly when the compositor lacks ext-data-control.
pub fn spawn() -> anyhow::Result<(
    mpsc::UnboundedReceiver<ClipEvent>,
    mpsc::UnboundedSender<ClipCmd>,
)> {
    let conn = Connection::connect_to_env().context("wayland connect")?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn)?;
    let qh = queue.handle();

    let seat: WlSeat = globals.bind(&qh, 1..=7, ())?;
    let mgr: ExtDataControlManagerV1 = globals
        .bind(&qh, 1..=1, ())
        .map_err(|e| anyhow::anyhow!("ext-data-control-v1 unsupported: {e}"))?;
    let device = mgr.get_data_device(&seat, &qh, ());

    let (events_tx, events_rx) = mpsc::unbounded_channel();
    let (cmds_tx, mut cmds_rx) = mpsc::unbounded_channel::<ClipCmd>();

    let mut state = State {
        events: events_tx,
        offer: None,
        source: None,
    };
    // Flushes get_data_device (otherwise the request sits in the outgoing
    // buffer — nothing ever arrives to wake the loop) and consumes the
    // initial selection if one is already set.
    queue
        .roundtrip(&mut state)
        .context("initial clipboard roundtrip")?;

    tokio::spawn(async move {
        // The borrowed fd's lifetime is the backend's — it outlives the
        // registration because `backend` drops after `wlfd` (reverse
        // declaration order).
        let backend = conn.backend();
        // SAFETY: `backend` owns the fd and outlives `wlfd`.
        let Ok(wlfd) = (unsafe { AsyncFd::register(backend.poll_fd()) }) else {
            tracing::warn!("wlclip: AsyncFd registration failed");
            return;
        };
        loop {
            tokio::select! {
                ready = wlfd.readable() => {
                    let mut ready = match ready {
                        Ok(r) => r,
                        Err(e) => {
                            tracing::warn!("wlclip: poll error: {e}");
                            return;
                        }
                    };
                    if let Some(guard) = queue.prepare_read() {
                        match guard.read() {
                            Ok(_) => {}
                            Err(e) => {
                                tracing::debug!("wlclip: read: {e}");
                            }
                        }
                    }
                    if let Err(e) = queue.dispatch_pending(&mut state) {
                        tracing::warn!("wlclip: dispatch failed: {e} — clipboard dead");
                        return;
                    }
                    // Requests queued during dispatch (offer.receive etc.)
                    // must go out now — otherwise offer reads deadlock.
                    if let Err(e) = queue.flush() {
                        tracing::warn!("wlclip: flush: {e}");
                        return;
                    }
                    ready.clear_ready();
                }
                cmd = cmds_rx.recv() => {
                    let Some(ClipCmd::Set { mime, data }) = cmd else {
                        return;
                    };
                    let source = mgr.create_data_source(&qh, ());
                    // Offer the received mime; for text also offer the
                    // charset-annotated twin — pasters ask for either.
                    let mut mimes = vec![mime.clone()];
                    if mime.starts_with("text/plain") && mime != "text/plain;charset=utf-8" {
                        mimes.push("text/plain;charset=utf-8".into());
                    } else if mime == "text/plain;charset=utf-8" {
                        mimes.push("text/plain".into());
                    }
                    for m in mimes {
                        source.offer(m);
                    }
                    device.set_selection(Some(&source));
                    if let Err(e) = queue.flush() {
                        tracing::warn!("wlclip: flush: {e}");
                        return;
                    }
                    state.source = Some((source, mime, data));
                }
            }
        }
    });
    Ok((events_rx, cmds_tx))
}
