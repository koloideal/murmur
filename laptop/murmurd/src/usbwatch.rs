//! USB-tethering fast path: an Android phone in usb-modem mode *is* the
//! default gateway of the usb interface it creates — no beacon luck
//! needed, the gateway address names the peer directly. We poll for
//! usb-backed interfaces and dial their gateways as trusted hints,
//! so plugging the cable establishes the link within seconds.
//!
//! Detection is sysfs + /proc only — no netlink subscription, no udev
//! dependency: a cheap poll loop is more robust than either.

use crate::lan::{DialHint, LINK_PORT};
use crate::state::Shared;
use murmur_proto::DeviceKind;
use std::collections::HashSet;
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex, OnceLock};
use tracing::{debug, info};

/// Gateways seen on usb interfaces recently — ctl status uses this to
/// tag the transport of a peer address. Process-global and append-only
/// within an interface's lifetime; stale entries are harmless (a few
/// bytes, and worst case the tag lingers on a re-used address).
fn usb_gateways() -> &'static Mutex<HashSet<std::net::IpAddr>> {
    static G: OnceLock<Mutex<HashSet<std::net::IpAddr>>> = OnceLock::new();
    G.get_or_init(|| Mutex::new(HashSet::new()))
}

pub fn is_usb_gateway(ip: &std::net::IpAddr) -> bool {
    usb_gateways().lock().unwrap().contains(ip)
}

/// Interface names whose backing device lives on the usb bus — rndis,
/// cdc_ether, cdc_ncm all surface there.
fn usb_ifaces_up() -> Vec<String> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir("/sys/class/net") else { return out };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let base = e.path();
        // operstate must be up — a cable present but tethering off is
        // exactly the state we must NOT dial into.
        let up = std::fs::read_to_string(base.join("operstate"))
            .map(|s| s.trim() == "up")
            .unwrap_or(false);
        if !up {
            continue;
        }
        let devpath = base.join("device");
        let on_usb = std::fs::canonicalize(&devpath)
            .map(|p| p.to_string_lossy().contains("/usb"))
            .unwrap_or(false);
        if on_usb {
            out.push(name);
        }
    }
    out
}

/// Default gateways per interface, parsed from /proc/net/route.
/// Fields are hex; the gateway word is little-endian. Only default
/// routes (destination 00000000) point at the tethering host.
fn default_gateways() -> Vec<(String, Ipv4Addr)> {
    let mut out = Vec::new();
    let Ok(table) = std::fs::read_to_string("/proc/net/route") else { return out };
    for line in table.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 3 || f[1] != "00000000" {
            continue;
        }
        if let Ok(gw) = u32::from_str_radix(f[2], 16) {
            if gw != 0 {
                out.push((f[0].to_string(), Ipv4Addr::from(gw.to_le_bytes())));
            }
        }
    }
    out
}

pub async fn run(shared: Arc<Shared>) {
    let mut dialed: HashSet<Ipv4Addr> = HashSet::new();
    loop {
        let usb: HashSet<String> = usb_ifaces_up().into_iter().collect();
        let mut live: HashSet<Ipv4Addr> = HashSet::new();
        for (iface, gw) in default_gateways() {
            if !usb.contains(&iface) {
                continue;
            }
            live.insert(gw);
            if dialed.insert(gw) {
                info!("usbwatch: tethering gateway {gw} on {iface} — dialing");
                let _ = shared.lan_dial.send(DialHint {
                    peer: None,
                    addr: std::net::IpAddr::V4(gw),
                    port: LINK_PORT,
                    kind: DeviceKind::Phone,
                    trusted: true,
                transport: crate::lan::Transport::Usb,
                });
            }
        }
        // Interfaces vanish -> the gateway set resets, so the next plug
        // re-dials even the same address.
        dialed.retain(|g| live.contains(g));
        if live.is_empty() {
            debug!("usbwatch: no usb tethering interfaces");
        }
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
}
