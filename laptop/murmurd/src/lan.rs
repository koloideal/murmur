//! LAN fast-path: peer discovery over UDP broadcast, envelopes over a
//! direct TCP link — the same sealed `Envelope` JSON as the relay
//! transport, but commands never leave the subnet.
//!
//! Roles:
//!   - Every device *broadcasts* a sealed `LanBeacon{tcp_port}` — the
//!     envelope IS the authentication (only a family member can seal a
//!     frame that opens), so neither side needs a handshake.
//!   - On a valid beacon a *dialer* opens TCP to `beacon_ip:tcp_port`;
//!     the *listener* accepts. Laptops dial only `Phone` beacons and
//!     vice versa, so at most one direction connects per pair.
//!   - A live link registers itself via [`Shared::lan_adopt`]: `send()`
//!     routes `Route::Device(peer)` envelopes there; `Route::All` still
//!     goes through the relay (broadcast semantics are relay-side).
//!
//! Link liveness is detected by the reader hitting EOF / writer errors —
//! no heartbeat; the relay remains the long-lived control plane.

use crate::feature::Hub;
use crate::state::Shared;
use murmur_proto::{DeviceKind, Envelope, Message, Route};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

const BEACON_PORT: u16 = 17893;
const LINK_PORT: u16 = 17894;
const BEACON_EVERY: std::time::Duration = std::time::Duration::from_secs(4);
const DIAL_EVERY: std::time::Duration = std::time::Duration::from_secs(10);

/// The dial direction for this node: laptops dial phones.
const PEER_KIND: DeviceKind = DeviceKind::Phone;

pub async fn run(shared: Arc<Shared>, hub: Arc<Hub>) -> anyhow::Result<()> {
    // A taken port must not kill the daemon — the relay stays the
    // control plane; LAN is a bonus fast-path only.
    let listener = match TcpListener::bind(("0.0.0.0", LINK_PORT)).await {
        Ok(l) => l,
        Err(e) => {
            warn!("lan: :{LINK_PORT} busy ({e}); fast-path disabled");
            std::future::pending::<()>().await;
            unreachable!();
        }
    };
    match UdpSocket::bind(("0.0.0.0", BEACON_PORT)).await {
        Ok(udp) => {
            udp.set_broadcast(true)?;
            info!("lan: beacon :{BEACON_PORT}, link :{LINK_PORT}");
            let udp = Arc::new(udp);
            tokio::spawn(beacon_send(shared.clone(), udp.clone()));
            tokio::spawn(beacon_recv(shared.clone(), hub.clone(), udp));
        }
        Err(e) => warn!("lan: beacon off ({e}); inbound links still accepted"),
    }
    accept_loop(&shared, &hub, &listener).await
}

/// Periodic sealed-beacon broadcast — proves family membership to anyone
/// listening on the subnet.
async fn beacon_send(shared: Arc<Shared>, udp: Arc<UdpSocket>) -> anyhow::Result<()> {
    let dst = ("255.255.255.255", BEACON_PORT);
    let mut tick = tokio::time::interval(BEACON_EVERY);
    loop {
        tick.tick().await;
        let env = Envelope::seal(
            shared.id,
            Route::All,
            shared.next_seq(),
            &shared.keys.read().unwrap().current,
            &Message::LanBeacon {
                tcp_port: LINK_PORT,
                kind: DeviceKind::Laptop,
            },
        )?;
        let _ = udp.send_to(env.to_json().as_bytes(), dst).await;
    }
}

/// UDP → decode → open (auth) → dial. A beacon that can't be opened is
/// dropped — a forged broadcast just doesn't go anywhere.
async fn beacon_recv(shared: Arc<Shared>, hub: Arc<Hub>, udp: Arc<UdpSocket>) -> anyhow::Result<()> {
    let mut last_dial = std::collections::HashMap::new();
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        let (n, src) = udp.recv_from(&mut buf).await?;
        let Ok(env) = Envelope::from_json(std::str::from_utf8(&buf[..n]).unwrap_or("")) else {
            continue;
        };
        if env.from == shared.id {
            continue;
        }
        let Ok(Message::LanBeacon { tcp_port, kind }) =
            env.open(&shared.keys.read().unwrap().current)
        else {
            continue;
        };
        if kind != PEER_KIND {
            continue;
        }
        if shared.lan_peer() == Some(env.from) {
            continue; // already linked to this peer
        }
        // Rate-limit dials per peer — beacons repeat every few seconds.
        let now = std::time::Instant::now();
        if last_dial.get(&env.from).is_some_and(|t| now - *t < DIAL_EVERY) {
            continue;
        }
        last_dial.insert(env.from, now);
        let addr = SocketAddr::new(src.ip(), tcp_port);
        match TcpStream::connect(addr).await {
            Ok(s) => {
                let _ = s.set_nodelay(true);
                info!("lan: dialed {addr} ({})", env.from);
                spawn_link(&shared, &hub, s);
            }
            Err(e) => debug!("lan: dial {addr}: {e}"),
        }
    }
}

/// Accept inbound links (a phone dialing us).
async fn accept_loop(shared: &Arc<Shared>, hub: &Arc<Hub>, listener: &TcpListener) -> anyhow::Result<()> {
    loop {
        let (s, addr) = listener.accept().await?;
        let _ = s.set_nodelay(true);
        info!("lan: accepted {addr}");
        spawn_link(shared, hub, s);
    }
}

/// Split the socket: writer drains the per-link envelope queue; reader
/// opens + dispatches. Either ending tears the link down.
fn spawn_link(shared: &Arc<Shared>, hub: &Arc<Hub>, s: TcpStream) {
    let (tx, rx) = mpsc::unbounded_channel::<Envelope>();
    let (r, w) = s.into_split();
    tokio::spawn(link_writer(shared.clone(), w, rx, tx.clone()));
    tokio::spawn(link_reader(shared.clone(), hub.clone(), r, tx));
}

async fn link_writer(
    shared: Arc<Shared>,
    mut w: tokio::net::tcp::OwnedWriteHalf,
    mut rx: mpsc::UnboundedReceiver<Envelope>,
    tx: mpsc::UnboundedSender<Envelope>,
) {
    while let Some(env) = rx.recv().await {
        let mut line = env.to_json();
        line.push('\n');
        if w.write_all(line.as_bytes()).await.is_err() {
            break;
        }
    }
    shared.lan_clear(&tx);
}

async fn link_reader(
    shared: Arc<Shared>,
    hub: Arc<Hub>,
    r: tokio::net::tcp::OwnedReadHalf,
    tx: mpsc::UnboundedSender<Envelope>,
) {
    let mut br = BufReader::new(r);
    let mut line = String::new();
    loop {
        line.clear();
        match br.read_line(&mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let Ok(env) = Envelope::from_json(line.trim_end()) else {
            continue;
        };
        match env.open(&shared.keys.read().unwrap().current) {
            Ok(msg) => {
                // A frame that opens proves family membership → adopt
                // the link for outbound traffic (newest link wins — a
                // redialed conn supersedes a stale one).
                if shared.lan_peer() != Some(env.from) {
                    info!("lan: link up with {}", env.from);
                    shared.lan_adopt(env.from, tx.clone());
                }
                hub.dispatch(env.from, &msg);
            }
            Err(e) => debug!("lan: undecryptable from {}: {e}", env.from),
        }
    }
    shared.lan_clear(&tx);
}
