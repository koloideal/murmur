//! In-memory integration: two full `Shared` nodes wired by channel
//! pumps — no sockets, no relay, no clock dependence beyond what the
//! production code itself uses. The pump *is* the transport under test
//! in miniature: whatever `send()` emits is what the peer ingests,
//! through the same `ingest` invariant the real links use.
//!
//! Env safety: tests here mutate XDG_DATA_HOME so persistence writes
//! land in tempdirs, never in the real ~/.local/share/murmur. A static
//! lock serializes them because env is process-global.

#[cfg(test)]
mod tests {
    use crate::config::{Config, Secrets};
    use crate::feature::Hub;
    use crate::state::Shared;
    use murmur_proto::{DeviceId, Envelope, FamilyKey, Message, PairKey, PresenceState, Route};
    use std::sync::{Arc, Mutex};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn hex_id(b: u8) -> String {
        hex::encode([b; 8])
    }

    /// A fully in-memory node: real keyring derivation, real dedupe,
    /// real presence/election — only the disk and the wire are fake.
    fn node(
        dev: u8,
        family: &FamilyKey,
    ) -> (
        Arc<Shared>,
        tokio::sync::mpsc::UnboundedReceiver<Envelope>,
        tokio::sync::mpsc::UnboundedReceiver<(u64, Envelope)>,
        tokio::sync::mpsc::UnboundedReceiver<crate::lan::DialHint>,
    ) {
        let identity = PairKey::generate();
        let secrets = Secrets {
            device_id: hex_id(dev),
            auth_token_hex: hex::encode([dev; 16]),
            family_key_hex: hex::encode(family.0),
            key_epoch: 0,
            key_rotator: None,
            old_keys_hex: vec![],
            revoked: vec![],
            identity_hex: Some(hex::encode(identity.secret_bytes())),
            family_name: Some("testfam".into()),
        };
        let cfg = Config {
            family_kind: murmur_proto::FamilyKind::Serverful,
            server_url: "wss://unused".into(),
            name: format!("node{dev}"),
            idle_timeout_secs: 180,
            clipboard_mime_deny: vec![],
            lock_procs: vec![],
            notify_suppress_if_running: Default::default(),
            notify_hide_twins_if_online: vec![],
            lan_seeds: vec![],
        };
        let (out_tx, out_rx) = tokio::sync::mpsc::unbounded_channel();
        let (lo_tx, lo_rx) = tokio::sync::mpsc::unbounded_channel();
        let (lan_tx, lan_rx) = tokio::sync::mpsc::unbounded_channel();
        let shared = Shared::new(cfg, secrets, out_tx, lo_tx, lan_tx).unwrap();
        (Arc::new(shared), out_rx, lo_rx, lan_rx)
    }

    fn with_env<R>(f: impl FnOnce() -> R) -> R {
        let _g = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("murmur-integ-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: serialized by ENV_LOCK; the test process owns its env
        // and no other thread reads xdg paths concurrently in tests.
        unsafe { std::env::set_var("XDG_DATA_HOME", &dir) };
        f()
    }

    /// Drain whatever `send()` emitted — hi channel first (commands),
    /// then lo (replaceable state). One pump step = one envelope.
    fn pump_one(
        out: &mut tokio::sync::mpsc::UnboundedReceiver<Envelope>,
        lo: &mut tokio::sync::mpsc::UnboundedReceiver<(u64, Envelope)>,
    ) -> Option<Envelope> {
        out.try_recv().ok().or_else(|| lo.try_recv().ok().map(|(_, e)| e))
    }

    #[test]
    fn duplex_delivery_and_dual_path_dedup() {
        with_env(|| {
            let family = FamilyKey::random();
            let (a, mut a_out, mut a_lo, _a_lan) = node(1, &family);
            let (b, mut _b_out, mut _b_lo, _b_lan) = node(2, &family);

            // B's feature tap: a hub sink capturing everything
            let hub = Hub::new();
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let mut hub = hub;
            hub.sinks
                .entry("presence")
                .or_default()
                .push(tx);

            // A announces itself; the envelope must exist on the wire
            a.send(Route::All, &Message::Presence { state: PresenceState::Active, ts: 1000 })
                .unwrap();
            let env = pump_one(&mut a_out, &mut a_lo).expect("an envelope leaves A");

            // B opens and ingests — exactly like a link reader would
            let key = b.keys.read().unwrap().current.clone();
            let msg = env.open(&key).unwrap();
            assert!(b.ingest(&hub, env.from, env.seq, &msg), "first copy delivers");

            // the SAME frame rides the second path (LAN+relay twins)
            assert!(!b.ingest(&hub, env.from, env.seq, &msg), "twin copy dies in dedupe");

            // exactly one copy reached features
            let got = rx.try_recv().expect("feature inbox got the presence");
            assert_eq!(got.0, DeviceId([1; 8]));
            assert!(rx.try_recv().is_err(), "no duplicate reaches features");

            // and B's presence table learned the peer — election inputs
            assert!(b.lan_peers().is_empty()); // no links in-memory — sanity
        });
    }

    #[test]
    fn key_rotation_flows_end_to_end() {
        with_env(|| {
            use murmur_proto::DeviceKind;
            let family = FamilyKey::random();
            let (a, mut a_out, mut a_lo, _a_lan) = node(1, &family);
            let (b, _b_out, _b_lo, _b_lan) = node(2, &family);

            // both nodes pin each other — in production Identity frames
            // do this; here we seed the same roster state directly
            for (node, peer_id, peer_pub) in [
                (&a, DeviceId([2; 8]), b.identity.public_b64()),
                (&b, DeviceId([1; 8]), a.identity.public_b64()),
            ] {
                node.pin(peer_id, &peer_pub, DeviceKind::Laptop, None, &[]);
            }

            // A rotates: the KeyRotate frame must leave under the OLD
            // key so stragglers can read it
            let old_key = a.keys.read().unwrap().current.clone();
            a.rotate(vec![]).expect("rotate");
            assert_eq!(a.keys.read().unwrap().epoch, 1);

            // the wire carries whatever pin() announced too — drain
            // until the rotation frame, exactly like a real link would
            let mut found = None;
            for _ in 0..8 {
                let Some(env) = pump_one(&mut a_out, &mut a_lo) else { break };
                let msg = env.open(&old_key).expect("rotation rides the pre-rotation key");
                if let murmur_proto::Message::KeyRotate { epoch, revoked, keys } = msg {
                    found = Some((env, epoch, revoked, keys));
                    break;
                }
            }
            let (env, epoch, revoked, keys) = found.expect("rotation frame emitted");

            // B applies it exactly as its keyring feature would
            b.apply_rotation(env.from, epoch, revoked, keys);
            assert_eq!(b.keys.read().unwrap().epoch, 1, "B followed the rotation");
            assert_eq!(
                b.keys.read().unwrap().current.0,
                a.keys.read().unwrap().current.0,
                "both sides now hold the same family key"
            );

            // and post-rotation traffic really flows: A's new-key frame
            // opens at B
            a.send(Route::All, &Message::Presence { state: PresenceState::Active, ts: 7 })
                .unwrap();
            let mut env2 = None;
            while let Some(e) = pump_one(&mut a_out, &mut a_lo) {
                env2 = Some(e); // last frame on the wire is ours
            }
            let env2 = env2.unwrap();
            assert!(env2.open(&old_key).is_err(), "post-rotation frames reject the old key");
            let key_b = b.keys.read().unwrap().current.clone();
            assert!(env2.open(&key_b).is_ok(), "B opens A's post-rotation traffic");
        });
    }

    #[test]
    fn wrong_family_frames_never_open() {
        with_env(|| {
            let fam_a = FamilyKey::random();
            let fam_b = FamilyKey::random();
            let (a, mut a_out, mut a_lo, _a_lan) = node(1, &fam_a);
            let (b, _b_out, _b_lo, _b_lan) = node(2, &fam_b);
            a.send(Route::All, &Message::Presence { state: PresenceState::Active, ts: 1 })
                .unwrap();
            let env = pump_one(&mut a_out, &mut a_lo).unwrap();
            let key = b.keys.read().unwrap().current.clone();
            assert!(env.open(&key).is_err(), "foreign family frames die at the seal");
        });
    }

    /// Simultaneous-dial tie-break: the socket dialed by the smaller
    /// device id must win on both sides independently, or each end
    /// closes the socket the other one keeps — the flap loop seen on
    /// real wifi. The matrix pins the decision table.
    #[test]
    fn lan_adopt_tiebreak_matrix() {
        use crate::lan::{LinkDir, Transport, winner_dir};
        use crate::links::AdoptOutcome;
        with_env(|| {
            let family = FamilyKey::random();
            // me = 0x10; peer_big = 0x20 (I am smaller), peer_small = 0x01.
            let (shared, _o, _l, _lan_rx) = node(0x10, &family);
            let peer_big = DeviceId([0x20; 8]);
            let peer_small = DeviceId([0x01; 8]);

            assert_eq!(winner_dir(shared.id, peer_big), LinkDir::Outbound);
            assert_eq!(winner_dir(shared.id, peer_small), LinkDir::Inbound);

            let mk = || tokio::sync::mpsc::unbounded_channel().0;

            // Fresh adopt always succeeds.
            let a = mk();
            assert_eq!(
                shared.lan_adopt(peer_big, a.clone(), Transport::Lan, LinkDir::Outbound),
                AdoptOutcome::Adopted
            );
            // Same channel is a no-op.
            assert_eq!(
                shared.lan_adopt(peer_big, a, Transport::Lan, LinkDir::Outbound),
                AdoptOutcome::Kept
            );
            // Winner direction (Outbound for the bigger peer) holds: an
            // inbound challenger is rejected.
            assert_eq!(
                shared.lan_adopt(peer_big, mk(), Transport::Lan, LinkDir::Inbound),
                AdoptOutcome::Rejected
            );
            // A winner-direction challenger replaces even a winner.
            assert_eq!(
                shared.lan_adopt(peer_big, mk(), Transport::Lan, LinkDir::Outbound),
                AdoptOutcome::Adopted
            );

            // Mirror image: peer_small is the dialer of the surviving
            // socket, so inbound wins for me. A loser-direction incumbent
            // must yield to the winner-direction challenger.
            let loser = mk();
            assert_eq!(
                shared.lan_adopt(peer_small, loser, Transport::Lan, LinkDir::Outbound),
                AdoptOutcome::Adopted // no incumbent yet — adopted provisionally
            );
            assert_eq!(
                shared.lan_adopt(peer_small, mk(), Transport::Lan, LinkDir::Inbound),
                AdoptOutcome::Adopted // inbound is winner here — evicts
            );
            // And once the winner holds, the loser is refused.
            assert_eq!(
                shared.lan_adopt(peer_small, mk(), Transport::Lan, LinkDir::Outbound),
                AdoptOutcome::Rejected
            );
        });
    }

    #[test]
    fn repaired_device_replaces_its_old_id() {
        // The inviter mints a fresh device id per pairing, but the
        // physical device's identity key survives. When the same key
        // reappears under a new id, the stale id must leave the roster
        // — keeping both ghosts it forever.
        use murmur_proto::DeviceKind;
        let family = FamilyKey::random();
        let (a, _a_out, _a_lo, _a_lan) = node(1, &family);
        let phone_key = murmur_proto::PairKey::generate();
        let pub_b64 = phone_key.public_b64();

        a.pin(DeviceId([7; 8]), &pub_b64, DeviceKind::Phone, None, &[]);
        assert!(a.roster.lock().unwrap().contains_key(&DeviceId([7; 8])));

        // same physical phone re-pairs: new id, same identity key
        a.pin(DeviceId([9; 8]), &pub_b64, DeviceKind::Phone, None, &[]);
        let r = a.roster.lock().unwrap();
        assert!(!r.contains_key(&DeviceId([7; 8])), "stale id must be retired");
        assert!(r.contains_key(&DeviceId([9; 8])), "new id must be pinned");

        // a genuinely different device with another key coexists fine
        drop(r);
        let other = murmur_proto::PairKey::generate();
        a.pin(DeviceId([11; 8]), &other.public_b64(), DeviceKind::Phone, None, &[]);
        let r = a.roster.lock().unwrap();
        assert!(r.contains_key(&DeviceId([9; 8])) && r.contains_key(&DeviceId([11; 8])));
    }

    // ---- transport failover (F-10/F-11 in FLOWS.md) ------------------

    #[test]
    fn dial_guards_revoked_direction_rate_limit_and_winner_link() {
        use crate::lan::{LinkDir, Transport, peer_dial_permitted};
        use murmur_proto::DeviceKind;
        use std::net::{IpAddr, Ipv4Addr, SocketAddr};
        use std::time::{Duration, Instant};
        with_env(|| {
            let family = FamilyKey::random();
            // me = 0x10; peer_big = 0x20 (I am lower — I dial laptops),
            // peer_small = 0x01 (it dials me — I must not).
            let (shared, _o, _l, _lan_rx) = node(0x10, &family);
            let big = DeviceId([0x20; 8]);
            let small = DeviceId([0x01; 8]);
            // TEST-NET-3 — never inside a real interface's subnet, so
            // the untrusted-subnet guard is deterministic on any host.
            let nonlocal = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7)), 17894);
            let now = Instant::now();

            // Untrusted hint from outside every local subnet: refused.
            assert!(!peer_dial_permitted(&shared, big, DeviceKind::Laptop, nonlocal, false, None, now));
            // A trusted hint (usbwatch gateway, relay-verified gossip)
            // bypasses the subnet guard — that's its entire purpose.
            assert!(peer_dial_permitted(&shared, big, DeviceKind::Laptop, nonlocal, true, None, now));

            // Direction: laptops only dial bigger laptop ids.
            assert!(peer_dial_permitted(&shared, big, DeviceKind::Laptop, nonlocal, true, None, now));
            assert!(!peer_dial_permitted(&shared, small, DeviceKind::Laptop, nonlocal, true, None, now));
            // Phones never dial — every laptop dials every phone
            // regardless of id ordering.
            assert!(peer_dial_permitted(&shared, small, DeviceKind::Phone, nonlocal, true, None, now));

            // Revoked members are never dialed, trusted or not.
            shared.keys.write().unwrap().revoked.push(big);
            assert!(!peer_dial_permitted(&shared, big, DeviceKind::Laptop, nonlocal, true, None, now));
            shared.keys.write().unwrap().revoked.clear();

            // A live winner-direction link makes any dial pure churn.
            let mk = || tokio::sync::mpsc::unbounded_channel().0;
            shared.lan_adopt(big, mk(), Transport::Lan, LinkDir::Outbound);
            assert!(!peer_dial_permitted(&shared, big, DeviceKind::Laptop, nonlocal, true, None, now));
            // But a loser-direction registration is transient — the flap
            // window must not starve the surviving socket's redial.
            shared.lan.lock().unwrap().clear();
            shared.lan_adopt(big, mk(), Transport::Lan, LinkDir::Inbound);
            assert!(peer_dial_permitted(&shared, big, DeviceKind::Laptop, nonlocal, true, None, now));
            shared.lan.lock().unwrap().clear();

            // Rate limit: a dial inside the window is refused, outside
            // the window is fine. The window is 10s in production.
            let recent = now - Duration::from_secs(3);
            assert!(!peer_dial_permitted(&shared, big, DeviceKind::Laptop, nonlocal, true, Some(recent), now));
            let old = now - Duration::from_secs(11);
            assert!(peer_dial_permitted(&shared, big, DeviceKind::Laptop, nonlocal, true, Some(old), now));

            // The just-dropped marker overrides the rate limit exactly
            // once — consuming it is part of the contract.
            shared.lan_dropped.lock().unwrap().insert(big, now);
            assert!(peer_dial_permitted(&shared, big, DeviceKind::Laptop, nonlocal, true, Some(recent), now));
            assert!(shared.lan_dropped.lock().unwrap().is_empty(), "marker is consumed");
            assert!(!peer_dial_permitted(&shared, big, DeviceKind::Laptop, nonlocal, true, Some(recent), now));
        });
    }

    #[test]
    fn ipv4_subnet_match_is_bitwise_and_honest() {
        use crate::lan::ipv4_in_subnet;
        use std::net::Ipv4Addr;
        let net = Ipv4Addr::new(192, 168, 31, 214);
        let mask24 = Ipv4Addr::new(255, 255, 255, 0);
        assert!(ipv4_in_subnet(Ipv4Addr::new(192, 168, 31, 166), net, mask24));
        assert!(!ipv4_in_subnet(Ipv4Addr::new(192, 168, 32, 166), net, mask24));
        // Non-contiguous edge: /32 matches only the interface itself.
        let mask32 = Ipv4Addr::new(255, 255, 255, 255);
        assert!(ipv4_in_subnet(net, net, mask32));
        assert!(!ipv4_in_subnet(Ipv4Addr::new(192, 168, 31, 215), net, mask32));
        // /0 matches everything — the degenerate DHCP case.
        assert!(ipv4_in_subnet(Ipv4Addr::new(203, 0, 113, 7), net, Ipv4Addr::new(0, 0, 0, 0)));
    }

    #[test]
    fn hint_appends_alternate_set_promotes_to_front() {
        with_env(|| {
            let family = FamilyKey::random();
            let (n, _o, _l, _d) = node(1, &family);
            let peer = DeviceId([9; 8]);
            let sa = |s: &str| s.parse::<std::net::SocketAddr>().unwrap();
            let (advert, usb) = (sa("192.168.31.46:17894"), sa("127.0.0.1:27894"));

            // a beacon advert learned before any link lands as a candidate…
            n.lan_addr_hint(peer, advert);
            // …then the validated usb link's addr becomes the proven front,
            // advert demoted to alternate — failover material, not noise.
            n.lan_addr_set(peer, usb);
            assert_eq!(n.lan_addrs()[&peer], vec![usb, advert]);

            // later adverts can't displace the proven front — the dead
            // .46 wifi addr the phone kept advertising on the real mesh.
            n.lan_addr_hint(peer, sa("192.168.31.46:17895"));
            assert_eq!(n.lan_addrs()[&peer][0], usb);

            // re-proving the same addr is a no-op, not a dup…
            n.lan_addr_set(peer, usb);
            assert_eq!(n.lan_addrs()[&peer].len(), 3);
            // …and proving an alternate promotes it to front.
            n.lan_addr_set(peer, advert);
            assert_eq!(n.lan_addrs()[&peer][0], advert);
            assert_eq!(n.lan_addrs()[&peer][1], usb);
        });
    }

    #[test]
    fn candidate_set_capped_and_proven_never_evicts() {
        with_env(|| {
            let family = FamilyKey::random();
            let (n, _o, _l, _d) = node(1, &family);
            let peer = DeviceId([9; 8]);
            let proven: std::net::SocketAddr = "10.0.0.1:17894".parse().unwrap();
            n.lan_addr_set(peer, proven);
            for i in 0..6u8 {
                n.lan_addr_hint(peer, format!("10.0.1.{i}:17894").parse().unwrap());
            }
            let v = &n.lan_addrs()[&peer];
            assert_eq!(v.len(), 4, "candidate set stays bounded (LAN_ADDR_CAP)");
            assert_eq!(v[0], proven, "eviction never touches the proven front");
        });
    }

    #[test]
    fn link_drop_dials_every_candidate_front_first() {
        use crate::lan::{LinkDir, Transport};
        use murmur_proto::DeviceKind;
        with_env(|| {
            let family = FamilyKey::random();
            let (n, _o, _l, mut dial) = node(0x10, &family);
            let peer = DeviceId([0x20; 8]);
            n.pin(peer, &PairKey::generate().public_b64(), DeviceKind::Laptop, None, &[]);
            let sa = |s: &str| s.parse::<std::net::SocketAddr>().unwrap();
            let (proven, alt1, alt2) =
                (sa("10.0.0.9:17894"), sa("10.0.0.9:27894"), sa("192.168.31.46:17894"));
            n.lan_addr_set(peer, proven);
            n.lan_addr_hint(peer, alt1);
            n.lan_addr_hint(peer, alt2);

            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            n.lan_adopt(peer, tx.clone(), Transport::Lan, LinkDir::Outbound);
            assert_eq!(n.lan_peers(), vec![peer]);
            n.lan_clear(&tx);
            assert!(n.lan_peers().is_empty());

            // A drop must not wait for the next redial tick — every
            // candidate goes out immediately, proven first so the
            // rate-limit exemption lands on the most likely transport.
            let mut got = Vec::new();
            while let Ok(h) = dial.try_recv() {
                got.push(h);
            }
            let addrs: Vec<_> = got
                .iter()
                .map(|h| std::net::SocketAddr::new(h.addr, h.port))
                .collect();
            assert_eq!(addrs, vec![proven, alt1, alt2]);
            assert!(got.iter().all(|h| h.trusted && h.peer == Some(peer)));

            // the dropped marker is one-shot: the first dial skips the
            // rate limit, later ones pace per (peer, addr)
            assert!(n.lan_redial_pending(&peer));
            assert!(!n.lan_redial_pending(&peer));
        });
    }

    #[test]
    fn lan_addrs_file_loads_flat_and_list_forms() {
        with_env(|| {
            // The pre-candidates on-disk form was {"id": "addr"} — it
            // must still load or every rendezvous dies on upgrade.
            let p = crate::config::data_path("lan_addrs.json");
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            let (flat, list) = (DeviceId([3; 8]), DeviceId([4; 8]));
            std::fs::write(
                &p,
                format!(
                    r#"{{"{flat}": "127.0.0.1:27894", "{list}": ["10.1.1.1:17894", "10.1.1.2:17894"]}}"#
                ),
            )
            .unwrap();
            let m = crate::links::load_lan_addrs();
            assert_eq!(m[&flat], vec!["127.0.0.1:27894".parse::<std::net::SocketAddr>().unwrap()]);
            assert_eq!(m[&list].len(), 2);
            let _ = std::fs::remove_file(&p);
        });
    }

    #[test]
    fn addressed_frames_transit_self_and_server_do_not() {
        // The pong-routing invariant the ICMP keepalive depends on:
        // a frame addressed to another member floods onward, one
        // addressed to us or @server stops here.
        use crate::lan::{LinkDir, Transport};
        with_env(|| {
            let family = FamilyKey::random();
            let (n, _o, _l, _d) = node(0x10, &family);
            let (peer_a, peer_b) = (DeviceId([0x20; 8]), DeviceId([0x30; 8]));
            let (tx_a, mut rx_a) = tokio::sync::mpsc::unbounded_channel();
            let (tx_b, mut rx_b) = tokio::sync::mpsc::unbounded_channel();
            {
                let mut lan = n.lan.lock().unwrap();
                lan.insert(peer_a, (tx_a.clone(), Transport::Lan, LinkDir::Outbound));
                lan.insert(peer_b, (tx_b.clone(), Transport::Lan, LinkDir::Outbound));
            }
            let key = n.keys.read().unwrap().current.clone();
            let seal = |to: Route, seq: u64| {
                Envelope::seal(
                    peer_a,
                    to,
                    seq,
                    &key,
                    &Message::LanPong { ts: 1 },
                )
                .unwrap()
            };

            // addressed to peer_b — transits onward to B, never back to A
            let onward = seal(Route::Device(peer_b), 1);
            n.lan_forward(&onward, &tx_a);
            assert!(rx_b.try_recv().is_ok(), "addressed frame must transit");
            assert!(rx_a.try_recv().is_err(), "never back down the arrival link");

            // addressed to us — stops
            let mine = seal(Route::Device(n.id), 2);
            n.lan_forward(&mine, &tx_a);
            assert!(rx_b.try_recv().is_err(), "self-addressed must not forward");

            // @server is relay business — a LAN flood can't complete it
            let srv = seal(Route::Server, 3);
            n.lan_forward(&srv, &tx_a);
            assert!(rx_b.try_recv().is_err(), "@server must not flood LAN");

            // broadcast — floods to every other link
            let all = seal(Route::All, 4);
            n.lan_forward(&all, &tx_a);
            assert!(rx_b.try_recv().is_ok(), "broadcast floods onward");
        });
    }

    #[test]
    fn transit_frames_mark_member_heard() {
        with_env(|| {
            // F-8/F-18: a member reachable only through another node's
            // link stays "online" by any valid frame — phones emit no
            // Presence, their flooded pings are the liveness signal.
            let family = FamilyKey::random();
            let (a, mut a_out, mut a_lo, _d1) = node(1, &family);
            let (b, _o, _l, _d2) = node(2, &family);
            let hub = Hub::new();
            a.send(Route::All, &Message::Presence { state: PresenceState::Active, ts: 1 })
                .unwrap();
            let env = pump_one(&mut a_out, &mut a_lo).unwrap();
            let key = b.keys.read().unwrap().current.clone();
            let msg = env.open(&key).unwrap();
            assert!(!b.heard_recently(&env.from, std::time::Duration::from_secs(30)));
            assert!(b.ingest(&hub, env.from, env.seq, &msg));
            assert!(b.heard_recently(&env.from, std::time::Duration::from_secs(30)));
            // a frame never seen (another family member entirely) is not heard
            assert!(!b.heard_recently(&DeviceId([77; 8]), std::time::Duration::from_secs(30)));
        });
    }

    #[test]
    fn leaving_unhears_instantly_and_still_counts_as_seen() {
        with_env(|| {
            // Pause/exit: the goodbye flips liveness off NOW — not after
            // the 30s silence window — and its (from, seq) is consumed
            // like any first frame so it still floods exactly once.
            let family = FamilyKey::random();
            let (a, mut a_out, mut a_lo, _d1) = node(1, &family);
            let (b, _o, _l, _d2) = node(2, &family);
            let hub = Hub::new();
            a.send(Route::All, &Message::Presence { state: PresenceState::Active, ts: 1 })
                .unwrap();
            let env = pump_one(&mut a_out, &mut a_lo).unwrap();
            let key = b.keys.read().unwrap().current.clone();
            let msg = env.open(&key).unwrap();
            assert!(b.ingest(&hub, env.from, env.seq, &msg));
            assert!(b.heard_recently(&env.from, std::time::Duration::from_secs(30)));

            a.send(Route::All, &Message::Leaving { ts: 2 }).unwrap();
            let env = pump_one(&mut a_out, &mut a_lo).unwrap();
            let msg = env.open(&key).unwrap();
            assert!(matches!(msg, Message::Leaving { .. }));
            assert!(b.ingest(&hub, env.from, env.seq, &msg));
            assert!(!b.heard_recently(&env.from, std::time::Duration::from_secs(30)));
            // the same goodbye seen again is a dupe — never re-floods
            assert!(!b.ingest(&hub, env.from, env.seq, &msg));
        });
    }

    #[test]
    fn pair_hello_consumes_invite_exactly_once() {
        use crate::pair::{InviteEntry, handle_ctl};
        use murmur_proto::Control;
        with_env(|| {
            let family = FamilyKey::random();
            let (inviter, _o, _l, _lan) = node(0x10, &family);
            let session = DeviceId([0x77; 8]);
            inviter.pair.lock().unwrap().invites.insert(
                session,
                InviteEntry { key: PairKey::generate(), created: std::time::Instant::now() },
            );
            let joiner_key = PairKey::generate();
            let hello = Control::PairHello {
                pubkey_b64: joiner_key.public_b64(),
                name: "newphone".into(),
            };

            // First hello: invite moves to pending.
            handle_ctl(&inviter, session, hello.clone());
            assert!(inviter.pair.lock().unwrap().invites.is_empty());
            assert_eq!(inviter.pair.lock().unwrap().pending.len(), 1);

            // The F-4 gap, pinned: a retry with the same code arrives
            // after the invite was consumed — it must NOT silently
            // resurrect or duplicate the session. Current honest
            // behaviour: unknown session, ignored.
            handle_ctl(&inviter, session, hello);
            assert_eq!(inviter.pair.lock().unwrap().pending.len(), 1, "no duplicate pending");

            // A hello for a session nobody invited is ignored entirely.
            let stranger = DeviceId([0x55; 8]);
            handle_ctl(&inviter, stranger, Control::PairHello {
                pubkey_b64: PairKey::generate().public_b64(),
                name: "stranger".into(),
            });
            assert_eq!(inviter.pair.lock().unwrap().pending.len(), 1);
        });
    }

    #[test]
    fn pair_confirm_consumes_pending_and_mints_fresh_id() {
        use crate::pair::{InviteEntry, handle_ctl};
        use murmur_proto::Control;
        with_env(|| {
            let family = FamilyKey::random();
            let (inviter, mut out, mut lo, _lan) = node(0x10, &family);
            let session = DeviceId([0x77; 8]);
            inviter.pair.lock().unwrap().invites.insert(
                session,
                InviteEntry { key: PairKey::generate(), created: std::time::Instant::now() },
            );
            let joiner_key = PairKey::generate();
            handle_ctl(&inviter, session, Control::PairHello {
                pubkey_b64: joiner_key.public_b64(),
                name: "newphone".into(),
            });

            let new_id = inviter.pair_confirm(session).expect("confirm succeeds");
            assert_ne!(new_id, session, "the joiner gets a fresh random device id");
            assert_ne!(new_id, inviter.id);
            assert!(inviter.pair.lock().unwrap().pending.is_empty(), "pending consumed");

            // A second confirm of the same session fails loudly instead
            // of minting a second device for one physical phone.
            assert!(inviter.pair_confirm(session).is_err());

            // The wire got a RegisterDevice (serverful node) and a
            // PairData addressed to the session.
            let mut saw_register = false;
            let mut saw_pairdata = false;
            while let Some(env) = pump_one(&mut out, &mut lo) {
                if let Ok(ctl) = serde_json::from_slice::<murmur_proto::Control>(&env.payload) {
                    match ctl {
                        Control::RegisterDevice { device_id, .. } => {
                            assert_eq!(device_id, new_id);
                            saw_register = true;
                        }
                        Control::PairData { .. } => {
                            assert_eq!(env.to, Route::Device(session));
                            saw_pairdata = true;
                        }
                        _ => {}
                    }
                }
            }
            assert!(saw_register && saw_pairdata);
        });
    }

    #[test]
    fn simultaneous_dial_flap_settles_on_winner_direction() {
        use crate::lan::{LinkDir, Transport, winner_dir};
        use crate::links::AdoptOutcome;
        with_env(|| {
            let family = FamilyKey::random();
            // Both nodes adopt both directions in the worst possible
            // interleave — the flap observed live. Deterministic
            // tie-break must converge both sides to the SAME surviving
            // direction: the socket dialed by the lower id.
            let family2 = family.clone();
            let run_side = |me: u8, peer: DeviceId, first: LinkDir, second: LinkDir| {
                let (n, _o, _l, _lan) = node(me, &family2);
                let mk = || tokio::sync::mpsc::unbounded_channel().0;
                // loser's socket arrives first (worst case), winner's second
                let o1 = n.lan_adopt(peer, mk(), Transport::Lan, first);
                let o2 = n.lan_adopt(peer, mk(), Transport::Lan, second);
                let dir = n.lan.lock().unwrap().get(&peer).map(|(_, _, d)| *d);
                (o1, o2, dir)
            };
            let a = DeviceId([0x01; 8]);
            let b = DeviceId([0x20; 8]);
            // Node A (lower id): its own outbound socket should win.
            let (_o1, _o2, dir_a) = run_side(0x01, b, LinkDir::Inbound, LinkDir::Outbound);
            // Node B: the inbound socket (dialed by A) should win.
            let (_p1, _p2, dir_b) = run_side(0x20, a, LinkDir::Outbound, LinkDir::Inbound);
            assert_eq!(dir_a, Some(winner_dir(a, b)));
            assert_eq!(dir_b, Some(winner_dir(b, a)));
            // Mirror-symmetric: both sides keep the same physical socket —
            // A's outbound IS B's inbound.
            assert_eq!(winner_dir(a, b), LinkDir::Outbound);
            assert_eq!(winner_dir(b, a), LinkDir::Inbound);

            // And once settled, further loser-direction probes are
            // rejected without evicting the winner — no oscillation.
            let (n, _o, _l, _lan) = node(0x01, &family);
            let mk = || tokio::sync::mpsc::unbounded_channel().0;
            n.lan_adopt(b, mk(), Transport::Lan, LinkDir::Outbound);
            for _ in 0..8 {
                assert_eq!(
                    n.lan_adopt(b, mk(), Transport::Lan, LinkDir::Inbound),
                    AdoptOutcome::Rejected
                );
                assert_eq!(
                    n.lan.lock().unwrap().get(&b).map(|(_, _, d)| *d),
                    Some(LinkDir::Outbound),
                    "winner never evicted by loser churn"
                );
            }
        });
    }

    #[test]
    fn clipboard_echo_dies_at_every_hop_of_a_reverse_loop() {
        // laptop→phone→laptop: A broadcasts clip, phone applies and its
        // watcher rebroadcasts, A must not re-apply its own content when
        // it comes back around. The pure decision is pinned in
        // clipboard.rs unit tests — this pins the wire-level invariant:
        // the echo frame IS a normal family frame (transits, dedupes),
        // so suppression is the ONLY thing standing between a loop and
        // silence, and it must key on content hash, never on sender.
        use crate::clipboard::should_apply;
        with_env(|| {
            let family = FamilyKey::random();
            let (a, mut a_out, mut a_lo, _la) = node(1, &family);
            let (b, mut b_out, mut b_lo, _lb) = node(2, &family);
            let hub = Hub::new();
            let data = b"loop-me-not";
            let h = crate::clipboard::hash_clip(data);
            use base64::Engine;
            let clip = |origin| Message::Clipboard {
                origin,
                hash: h,
                mime: "text/plain".into(),
                data_b64: base64::engine::general_purpose::STANDARD.encode(data),
                explicit: false,
                ts: 1,
            };

            // A broadcasts; B ingests: foreign content applies.
            a.send(Route::All, &clip(a.id)).unwrap();
            let env = pump_one(&mut a_out, &mut a_lo).unwrap();
            let key = b.keys.read().unwrap().current.clone();
            let msg = env.open(&key).unwrap();
            assert!(b.ingest(&hub, env.from, env.seq, &msg));
            assert!(should_apply(h, false, None, true), "foreign clip applies at B");

            // B's watcher rebroadcasts the same content; the frame
            // rides back to A. A's last_sent is h — the echo must die
            // EVEN THOUGH it wears B's origin and B's hash. Keying on
            // sender would let the loop live.
            b.send(Route::All, &clip(b.id)).unwrap();
            let env2 = pump_one(&mut b_out, &mut b_lo).unwrap();
            let key_a = a.keys.read().unwrap().current.clone();
            let msg2 = env2.open(&key_a).unwrap();
            assert!(a.ingest(&hub, env2.from, env2.seq, &msg2), "the echo frame itself is a legit delivery");
            assert!(!should_apply(h, false, Some(h), true), "but applying it would ping-pong — refused");
        });
    }
}
