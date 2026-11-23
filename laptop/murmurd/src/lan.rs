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
//!     gives `Route::Device(peer)` envelopes an early LAN copy while
//!     the relay copy follows — LAN wins on latency, the relay copy is
//!     deduped on receipt, and a dead link can never eat a command.
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

pub async fn run(
    shared: Arc<Shared>,
    hub: Arc<Hub>,
    dial_rx: mpsc::UnboundedReceiver<(murmur_proto::DeviceId, std::net::IpAddr, u16)>,
) -> anyhow::Result<()> {
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
            tokio::spawn(beacon_recv(shared.clone(), udp));
        }
        Err(e) => warn!("lan: beacon off ({e}); inbound links still accepted"),
    }
    tokio::spawn(dial_loop(shared.clone(), hub.clone(), dial_rx));
    accept_loop(&shared, &hub, &listener).await
}

/// Periodic sealed-beacon broadcast — proves family membership to anyone
/// listening on the subnet.
///
/// Targets every interface's *directed* broadcast (192.168.31.255-style)
/// plus the limited one: under a full-tunnel VPN `255.255.255.255`
/// routes into the tunnel and never reaches the LAN, while directed
/// broadcasts follow each interface's own subnet route.
async fn beacon_send(shared: Arc<Shared>, udp: Arc<UdpSocket>) -> anyhow::Result<()> {
    let mut tick = tokio::time::interval(BEACON_EVERY);
    loop {
        tick.tick().await;
        let beacon = Message::LanBeacon {
            tcp_port: LINK_PORT,
            kind: DeviceKind::Laptop,
            lan_addr: lan_ipv4().map(|a| a.to_string()),
        };
        let env = Envelope::seal(
            shared.id,
            Route::All,
            shared.next_seq(),
            &shared.keys.read().unwrap().current,
            &beacon,
        )?;
        let bytes = env.to_json().into_bytes();
        let _ = udp.send_to(&bytes, ("255.255.255.255", BEACON_PORT)).await;
        for bcast in broadcast_addrs() {
            let _ = udp.send_to(&bytes, (bcast, BEACON_PORT)).await;
        }
        // The same beacon rides the relay — APs frequently suppress
        // client broadcast entirely, and a relay copy makes discovery
        // independent of L2 quirks.
        let _ = shared.send(Route::All, &beacon);
    }
}

/// Directed broadcast addresses of all non-loopback IPv4 interfaces —
/// empty on any error (the limited broadcast still goes out).
fn broadcast_addrs() -> Vec<std::net::Ipv4Addr> {
    let Ok(ifaces) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };
    ifaces
        .iter()
        .filter_map(|i| match &i.addr {
            if_addrs::IfAddr::V4(v4) if !v4.is_loopback() => v4.broadcast,
            _ => None,
        })
        .collect()
}

/// First non-loopback, non-link-local IPv4 — the address we advertise
/// in beacons for peers to dial back to.
fn lan_ipv4() -> Option<std::net::Ipv4Addr> {
    if_addrs::get_if_addrs()
        .ok()?
        .iter()
        .find_map(|i| match &i.addr {
            if_addrs::IfAddr::V4(v4)
                if !v4.is_loopback() && !v4.ip.is_link_local() =>
            {
                Some(v4.ip)
            }
            _ => None,
        })
}

/// Is `ip` inside any local interface's IPv4 subnet? Relay beacons are
/// global — this keeps the dial strictly LAN-scoped.
fn in_local_subnet(ip: std::net::IpAddr) -> bool {
    let std::net::IpAddr::V4(ip) = ip else { return false };
    let Ok(ifaces) = if_addrs::get_if_addrs() else {
        return false;
    };
    ifaces.iter().any(|i| match &i.addr {
        if_addrs::IfAddr::V4(v4) if !v4.is_loopback() => {
            let mask = u32::from(v4.netmask);
            (u32::from(v4.ip) & mask) == (u32::from(ip) & mask)
        }
        _ => false,
    })
}

/// UDP → decode → open (auth) → dial. A beacon that can't be opened is
/// dropped — a forged broadcast just doesn't go anywhere. A transient
/// socket error must not kill discovery — keep listening.
async fn beacon_recv(shared: Arc<Shared>, udp: Arc<UdpSocket>) -> anyhow::Result<()> {
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        let (n, src) = match udp.recv_from(&mut buf).await {
            Ok(v) => v,
            Err(e) => {
                warn!("lan: beacon recv: {e}");
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                continue;
            }
        };
        let Ok(env) = Envelope::from_json(std::str::from_utf8(&buf[..n]).unwrap_or("")) else {
            continue;
        };
        if env.from == shared.id {
            continue;
        }
        let Ok(Message::LanBeacon { tcp_port, kind, .. }) =
            env.open(&shared.keys.read().unwrap().current)
        else {
            continue;
        };
        if kind != PEER_KIND {
            continue;
        }
        let _ = shared.lan_dial.send((env.from, src.ip(), tcp_port));
    }
}

/// One dialer for both discovery paths — UDP beacons and relay-delivered
/// `LanBeacon`s end up here. A request is skipped when the peer is
/// already linked or its address isn't in any local subnet (a relay
/// beacon can legitimately arrive from another network — that's not
/// a LAN peer for us). Per-peer rate-limit covers beacon bursts.
async fn dial_loop(
    shared: Arc<Shared>,
    hub: Arc<Hub>,
    mut rx: mpsc::UnboundedReceiver<(murmur_proto::DeviceId, std::net::IpAddr, u16)>,
) {
    let mut last_dial = std::collections::HashMap::new();
    while let Some((peer, ip, port)) = rx.recv().await {
        if shared.lan_peer() == Some(peer) || !in_local_subnet(ip) {
            continue;
        }
        let now = std::time::Instant::now();
        if last_dial.get(&peer).is_some_and(|t| now - *t < DIAL_EVERY) {
            continue;
        }
        last_dial.insert(peer, now);
        let addr = SocketAddr::new(ip, port);
        match TcpStream::connect(addr).await {
            Ok(s) => {
                let _ = s.set_nodelay(true);
                info!("lan: dialed {addr} ({peer})");
                spawn_link(&shared, &hub, s, Some(peer));
            }
            Err(e) => debug!("lan: dial {addr}: {e}"),
        }
    }
}

/// Accept inbound links (a phone dialing us). Transient accept errors
/// (fd pressure, …) must not take the daemon down — retry briefly.
async fn accept_loop(shared: &Arc<Shared>, hub: &Arc<Hub>, listener: &TcpListener) -> anyhow::Result<()> {
    loop {
        match listener.accept().await {
            Ok((s, addr)) => {
                let _ = s.set_nodelay(true);
                info!("lan: accepted {addr}");
                spawn_link(shared, hub, s, None);
            }
            Err(e) => {
                warn!("lan: accept: {e}");
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        }
    }
}

/// Split the socket: writer drains the per-link envelope queue; reader
/// opens + dispatches. Either ending tears the link down.
///
/// A dialed link is adopted immediately — the beacon that led us to
/// the address was already authenticated, so waiting for the peer's
/// first frame would deadlock (each side waits for the other to send).
/// An accepted link waits for an openable frame like before.
fn spawn_link(
    shared: &Arc<Shared>,
    hub: &Arc<Hub>,
    s: TcpStream,
    adopt_as: Option<murmur_proto::DeviceId>,
) {
    let (tx, rx) = mpsc::unbounded_channel::<Envelope>();
    let (r, w) = s.into_split();
    if let Some(peer) = adopt_as {
        let cur = shared.lan_peer();
        if cur.is_none() || cur == Some(peer) {
            if cur.is_none() {
                info!("lan: link up with {peer}");
            }
            shared.lan_adopt(peer, tx.clone());
        }
    }
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
                // the link for outbound traffic. Only an empty slot or
                // the same peer re-adopts — a *different* peer must not
                // evict a live link (multi-phone flap).
                let cur = shared.lan_peer();
                if cur.is_none() || cur == Some(env.from) {
                    if cur.is_none() {
                        info!("lan: link up with {}", env.from);
                    }
                    shared.lan_adopt(env.from, tx.clone());
                }
                if shared.dedupe(env.from, env.seq) {
                    hub.dispatch(env.from, &msg);
                }
            }
            Err(e) => debug!("lan: undecryptable from {}: {e}", env.from),
        }
    }
    shared.lan_clear(&tx);
}
