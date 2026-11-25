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

pub(crate) const BEACON_PORT: u16 = 17893;
pub(crate) const LINK_PORT: u16 = 17894;
/// Serverless pairing listener — fixed like the link port so a host
/// firewall has something stable to allow.
pub(crate) const PAIR_PORT: u16 = 17895;
const BEACON_EVERY: std::time::Duration = std::time::Duration::from_secs(4);
const DIAL_EVERY: std::time::Duration = std::time::Duration::from_secs(10);

/// A peer someone discovered for us — UDP beacon or a relay-delivered
/// `LanBeacon` both land here; `dial_loop` does the connecting.
/// `peer: None` is a manual dial (`murmurctl dial`) — the id is
/// learned from the first openable frame instead, and the guards that
/// keep *discovered* hints LAN-scoped don't apply: the user naming an
/// address is the scoping. `trusted` marks addresses proven reachable
/// before (a remembered link, a manual dial): they skip the
/// in-local-subnet guard that keeps relay gossip LAN-scoped.
#[derive(Clone, Copy, Debug)]
pub struct DialHint {
    pub peer: Option<murmur_proto::DeviceId>,
    pub addr: std::net::IpAddr,
    pub port: u16,
    pub kind: DeviceKind,
    pub trusted: bool,
}

pub async fn run(
    shared: Arc<Shared>,
    hub: Arc<Hub>,
    dial_rx: mpsc::UnboundedReceiver<DialHint>,
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
    tokio::spawn(redial_loop(shared.clone()));
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
pub(crate) fn broadcast_addrs() -> Vec<std::net::Ipv4Addr> {
    let Ok(ifaces) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };
    ifaces
        .iter()
        .filter(|i| physical(&i.name))
        .filter_map(|i| match &i.addr {
            if_addrs::IfAddr::V4(v4) if !v4.is_loopback() => v4.broadcast,
            _ => None,
        })
        .collect()
}

/// Tunnel/overlay/bridge interfaces are not "the LAN" — a peer's
/// advertised 10.x CGNAT address must not dial into tun0, and a dead
/// virbr0 is never what the LAN peer can reach us on.
fn physical(name: &str) -> bool {
    const VIRTUAL: &[&str] = &[
        "tun", "tap", "wg", "ppp", "ippp", "tailscale", "ts-", "zt", "nord", "proton",
        "virbr", "docker", "br-", "veth", "vmnet", "lxc", "podman", "cni", "flannel",
        "kube", "vboxnet",
    ];
    if VIRTUAL.iter().any(|p| name.starts_with(p)) {
        return false;
    }
    // A bridge that exists but is down still owns an address — and would
    // win find_map ordering over the real interface.
    match std::fs::read_to_string(format!("/sys/class/net/{name}/operstate")) {
        Ok(s) => matches!(s.trim(), "up" | "unknown"),
        Err(_) => true,
    }
}

/// First non-loopback, non-link-local IPv4 on a physical interface —
/// the address we advertise in beacons for peers to dial back to.
pub(crate) fn lan_ipv4() -> Option<std::net::Ipv4Addr> {
    if_addrs::get_if_addrs()
        .ok()?
        .iter()
        .filter(|i| physical(&i.name))
        .find_map(|i| match &i.addr {
            if_addrs::IfAddr::V4(v4)
                if !v4.is_loopback() && !v4.ip.is_link_local() =>
            {
                Some(v4.ip)
            }
            _ => None,
        })
}

/// Is `ip` inside any *physical* interface's IPv4 subnet? Relay
/// beacons are global — this keeps the dial strictly LAN-scoped and
/// keeps tunnel subnets from matching an unreachable address.
fn in_local_subnet(ip: std::net::IpAddr) -> bool {
    let std::net::IpAddr::V4(ip) = ip else { return false };
    let Ok(ifaces) = if_addrs::get_if_addrs() else {
        return false;
    };
    ifaces.iter().filter(|i| physical(&i.name)).any(|i| match &i.addr {
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
        let _ = shared.lan_dial.send(DialHint {
            peer: Some(env.from),
            addr: src.ip(),
            port: tcp_port,
            kind,
            trusted: false,
        });
    }
}

/// One dialer for both discovery paths — UDP beacons and relay-delivered
/// `LanBeacon`s end up here. A request is skipped when the peer is
/// already linked or its address isn't in any local subnet (a relay
/// beacon can legitimately arrive from another network — that's not
/// a LAN peer for us). Per-peer rate-limit covers beacon bursts.
///
/// Dial direction is deterministic so a pair never holds two
/// connections: phones only listen, so a laptop always calls a phone;
/// between two dialing-capable nodes the lower device id calls.
async fn dial_loop(
    shared: Arc<Shared>,
    hub: Arc<Hub>,
    mut rx: mpsc::UnboundedReceiver<DialHint>,
) {
    let mut last_dial = std::collections::HashMap::new();
    while let Some(hint) = rx.recv().await {
        if let Some(peer) = hint.peer {
            if hint.kind != DeviceKind::Phone && shared.id > peer {
                continue; // the lower-id side initiates
            }
            if shared.lan_peers().contains(&peer)
                || (!hint.trusted && !in_local_subnet(hint.addr))
            {
                continue;
            }
            let now = std::time::Instant::now();
            // A link that just died gets one immediate redial — the fast
            // path should come back on the next beacon, not a rate-limit
            // window later.
            if !shared.lan_redial_pending(&peer)
                && last_dial.get(&peer).is_some_and(|t| now - *t < DIAL_EVERY)
            {
                continue;
            }
            last_dial.insert(peer, now);
        }
        let addr = SocketAddr::new(hint.addr, hint.port);
        match TcpStream::connect(addr).await {
            Ok(s) => {
                let _ = s.set_nodelay(true);
                match hint.peer {
                    Some(peer) => {
                        info!("lan: dialed {addr} ({peer})");
                        // Reachable once → worth remembering for the
                        // redial loop, beacon or no beacon.
                        shared.lan_addr_set(peer, addr);
                    }
                    None => info!("lan: dialed {addr} (manual)"),
                }
                spawn_link(&shared, &hub, s, hint.peer);
            }
            Err(e) => debug!("lan: dial {addr}: {e}"),
        }
    }
}

/// Redial remembered peers without waiting for a beacon — the only
/// way a link comes back on networks where broadcast can't pass
/// (guest Wi-Fi, isolated VLANs). One gentle retry per peer per
/// interval; `dial_loop`'s per-peer rate limit absorbs duplicates
/// when beacons do arrive.
async fn redial_loop(shared: Arc<Shared>) {
    let mut tick = tokio::time::interval(DIAL_EVERY);
    loop {
        tick.tick().await;
        let linked: std::collections::HashSet<_> =
            shared.lan_peers().into_iter().collect();
        let roster = shared.roster.lock().unwrap().clone();
        for (peer, addr) in shared.lan_addrs() {
            if linked.contains(&peer) {
                continue;
            }
            let kind = roster
                .get(&peer)
                .map(|m| m.kind)
                .unwrap_or(DeviceKind::Phone);
            let _ = shared.lan_dial.send(DialHint {
                peer: Some(peer),
                addr: addr.ip(),
                port: addr.port(),
                kind,
                trusted: true,
            });
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

/// How long a fresh link may stay unvalidated — producing no frame
/// that opens with the family key — before it is dropped. A genuine
/// peer answers within seconds; a dead end (stale DHCP address, a
/// subnet collision into a VPN) never does.
const VERIFY_AFTER: std::time::Duration = std::time::Duration::from_secs(25);

/// Split the socket: writer drains the per-link envelope queue; reader
/// opens + dispatches. Either ending tears the link down.
///
/// A dialed link is adopted immediately — the beacon that led us to
/// the address was already authenticated, so waiting for the peer's
/// first frame would deadlock (each side waits for the other to send).
/// An accepted link waits for an openable frame like before.
///
/// Every link starts by sending a sealed beacon — the peer gets proof
/// of family instantly instead of waiting for the next broadcast —
/// and gets a verify timer: no openable frame within [VERIFY_AFTER]
/// means the far end is not family, and the socket is a dead end.
fn spawn_link(
    shared: &Arc<Shared>,
    hub: &Arc<Hub>,
    s: TcpStream,
    adopt_as: Option<murmur_proto::DeviceId>,
) {
    let (tx, rx) = mpsc::unbounded_channel::<Envelope>();
    // tokio::TcpStream has no try_clone — bounce through std to grab a
    // shutdown handle for the verify timer.
    let (probe, s) = match (|| -> std::io::Result<(std::net::TcpStream, TcpStream)> {
        let std = s.into_std()?;
        let probe = std.try_clone()?;
        Ok((probe, TcpStream::from_std(std)?))
    })() {
        Ok(v) => v,
        Err(_) => return,
    };
    let remote_ip = s.peer_addr().ok().map(|a| a.ip());
    let (r, w) = s.into_split();
    if let Some(peer) = adopt_as {
        if !shared.lan_peers().contains(&peer) {
            info!("lan: link up with {peer}");
        }
        shared.lan_adopt(peer, tx.clone());
    }
    if let Ok(env) = Envelope::seal(
        shared.id,
        Route::All,
        shared.next_seq(),
        &shared.keys.read().unwrap().current,
        &Message::LanBeacon {
            tcp_port: LINK_PORT,
            kind: DeviceKind::Laptop,
            lan_addr: lan_ipv4().map(|a| a.to_string()),
        },
    ) {
        let _ = tx.send(env);
    }
    let validated = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let probe = probe;
        let shared = shared.clone();
        let tx2 = tx.clone();
        let validated = validated.clone();
        tokio::spawn(async move {
            tokio::time::sleep(VERIFY_AFTER).await;
            if !validated.load(std::sync::atomic::Ordering::Relaxed) {
                info!("lan: link down (unvalidated)");
                let _ = probe.shutdown(std::net::Shutdown::Both);
                shared.lan_clear(&tx2);
            }
        });
    }
    // Keepalives jump the data queue — a pong that waits behind a
    // media burst measures queue depth, not link latency.
    let (tx_hi, rx_hi) = mpsc::unbounded_channel::<Envelope>();
    tokio::spawn(link_writer(shared.clone(), w, rx, rx_hi, tx.clone()));
    tokio::spawn(link_reader(
        shared.clone(),
        hub.clone(),
        r,
        tx.clone(),
        tx_hi.clone(),
        validated,
        remote_ip,
    ));
    tokio::spawn(ping_loop(shared.clone(), tx_hi));
}

/// Local epoch milliseconds — timestamps for `LanPing` echoes; RTT is
/// computed on the pinger's clock only.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Every link gets its own RTT probe — the pong replies land back in
/// `link_reader` and refresh `Shared::lan_rtt`. Dies with the channel.
const PING_EVERY: std::time::Duration = std::time::Duration::from_secs(5);

async fn ping_loop(shared: Arc<Shared>, tx: mpsc::UnboundedSender<Envelope>) {
    let mut tick = tokio::time::interval(PING_EVERY);
    loop {
        tick.tick().await;
        let Ok(env) = Envelope::seal(
            shared.id,
            Route::All,
            shared.next_seq(),
            &shared.keys.read().unwrap().current,
            &Message::LanPing { ts: now_ms() },
        ) else {
            continue;
        };
        if tx.send(env).is_err() {
            return; // writer is gone — the reader/writer tears the link down
        }
    }
}

async fn link_writer(
    shared: Arc<Shared>,
    mut w: tokio::net::tcp::OwnedWriteHalf,
    mut rx: mpsc::UnboundedReceiver<Envelope>,
    mut rx_hi: mpsc::UnboundedReceiver<Envelope>,
    tx: mpsc::UnboundedSender<Envelope>,
) {
    loop {
        // biased: keepalives are always drained before queued data.
        let env = tokio::select! {
            biased;
            e = rx_hi.recv() => e,
            e = rx.recv() => e,
        };
        let Some(env) = env else { break };
        let mut line = env.to_json();
        line.push('\n');
        if let Err(e) = w.write_all(line.as_bytes()).await {
            info!("lan: link down (writer: {e})");
            break;
        }
    }
    shared.lan_clear(&tx);
}

/// Max inbound silence before the link is presumed dead. The peer's
/// own broadcasts (beacon cadence) reach the link every few seconds,
/// so a longer gap means the transport died without RST — Wi-Fi off,
/// subnet change — and TCP alone would sit quiet for minutes.
const LINK_IDLE: std::time::Duration = std::time::Duration::from_secs(20);

async fn link_reader(
    shared: Arc<Shared>,
    hub: Arc<Hub>,
    r: tokio::net::tcp::OwnedReadHalf,
    tx: mpsc::UnboundedSender<Envelope>,
    tx_hi: mpsc::UnboundedSender<Envelope>,
    validated: std::sync::Arc<std::sync::atomic::AtomicBool>,
    remote_ip: Option<std::net::IpAddr>,
) {
    let mut br = BufReader::new(r);
    let mut line = String::new();
    loop {
        line.clear();
        match tokio::time::timeout(LINK_IDLE, br.read_line(&mut line)).await {
            Ok(Ok(n)) if n > 0 => {}
            Ok(Ok(_)) => {
                info!("lan: link down (peer closed)");
                break;
            }
            Ok(Err(e)) => {
                info!("lan: link down (reader: {e})");
                break;
            }
            Err(_) => {
                info!("lan: link down (idle timeout)");
                break;
            }
        }
        let Ok(env) = Envelope::from_json(line.trim_end()) else {
            continue;
        };
        match env.open(&shared.keys.read().unwrap().current) {
            Ok(msg) => {
                validated.store(true, std::sync::atomic::Ordering::Relaxed);
                // Link-local keepalives are transport chatter — pong
                // replies and RTT records never reach dedupe/features.
                match &msg {
                    Message::LanPing { ts } => {
                        if let Ok(env) = Envelope::seal(
                            shared.id,
                            Route::All,
                            shared.next_seq(),
                            &shared.keys.read().unwrap().current,
                            &Message::LanPong { ts: *ts },
                        ) {
                            let _ = tx_hi.send(env);
                        }
                        continue;
                    }
                    Message::LanPong { ts } => {
                        // The peer is already adopted at this point —
                        // the pong arrives after the beacon exchange.
                        shared.lan_rtt_set(env.from, now_ms().saturating_sub(*ts));
                        continue;
                    }
                    _ => {}
                }
                // A frame that opens proves family membership → adopt.
                // Each peer owns its own map slot — a second device
                // links alongside, never evicts.
                if !shared.lan_peers().contains(&env.from) {
                    info!("lan: link up with {}", env.from);
                    // Reachable once → worth remembering for the
                    // beacon-free redial path. Dialed links were
                    // recorded by dial_loop already.
                    if let Some(ip) = remote_ip {
                        shared.lan_addr_set(env.from, SocketAddr::new(ip, LINK_PORT));
                    }
                }
                shared.lan_adopt(env.from, tx.clone());
                // The peer's advertised lan_addr:tcp_port is the
                // better truth — it's where IT accepts dials.
                if let Message::LanBeacon {
                    lan_addr: Some(ip),
                    tcp_port,
                    ..
                } = &msg
                {
                    if let Ok(ip) = ip.parse::<std::net::IpAddr>() {
                        shared.lan_addr_set(env.from, SocketAddr::new(ip, *tcp_port));
                    }
                }
                if shared.dedupe(env.from, env.seq) {
                    hub.dispatch(env.from, &msg);
                }
            }
            Err(e) => {
                // Same stale-member recovery as the relay path: an old
                // key opening the frame means the sender missed a
                // rotation. In a serverless family this link is the
                // only way the re-key can ever reach it.
                let old = shared
                    .keys
                    .read()
                    .unwrap()
                    .old()
                    .iter()
                    .find(|k| env.open(k).is_ok())
                    .cloned();
                match old {
                    Some(k) => shared.on_stale(env.from, &k, Some(&tx_hi)),
                    None => debug!("lan: undecryptable from {}: {e}", env.from),
                }
            }
        }
    }
    shared.lan_clear(&tx);
}
