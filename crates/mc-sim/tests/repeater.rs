//! Forwarding as a repeater (`docs/spec/air.md` §4.6, `examples/simple_repeater/MyMesh.cpp`):
//! Alice — relay — Bob, Alice and Bob out of each other's range.

use mc_air::packet::PathLen;
use mc_air::{PacketBuf, PayloadType, Route};
use mc_core::Duration;
use mc_node::node::Routing;
use mc_node::region::DENY_FLOOD;
use mc_node::{Event, Sent};
use mc_sim::World;
use rand_chacha::ChaCha20Rng;
use rand_chacha::rand_core::SeedableRng;

fn secs(s: u64) -> Duration {
    Duration::from_secs(s)
}

/// Alice (0) — relay (1) — Bob (2).
fn line(seed: u64, tweak: impl FnOnce(&mut Routing)) -> World {
    let mut w = World::new(seed);
    let a = w.add("alice", |_| {});
    let r = w.add("relay", |_| {});
    let b = w.add("bob", |_| {});
    w.link(a, r);
    w.link(r, b);
    let mut routing = Routing::REPEATER;
    tweak(&mut routing);
    w.node(r).routing = Some(routing);
    w
}

fn heard_advert(w: &World, at: usize, from: &[u8; 32]) -> bool {
    w.nodes[at]
        .events
        .iter()
        .any(|e| matches!(e, Event::Advert { key, .. } if key == from))
}

fn flood_advert(w: &mut World, from: usize, to: usize) -> bool {
    let now = w.now;
    w.node(from).send_self_advert(now, true).unwrap();
    let k = *w.node(from).id.pub_key();
    w.run_until(secs(15), |w| heard_advert(w, to, &k))
}

fn sent_by(w: &World, n: usize, ptype: PayloadType) -> usize {
    w.log
        .iter()
        .filter(|(_, from, raw)| {
            *from == n && mc_air::Packet::decode(raw).is_ok_and(|p| p.header.ptype == ptype)
        })
        .count()
}

#[test]
fn floods_and_direct_messages_go_through() {
    let mut w = line(51, |_| {});
    assert!(flood_advert(&mut w, 0, 2));
    assert!(flood_advert(&mut w, 2, 0));
    // A DM by flood, then the path comes back and the next one goes direct.
    for round in 0..2 {
        let now = w.now;
        let ts = w.node(0).time(now);
        let to: [u8; 6] = w.node(2).id.pub_key()[..6].try_into().unwrap();
        let Sent { flood, .. } = w
            .node(0)
            .send_text(now, &to, 0, 0, ts, format!("hello {round}").as_bytes())
            .unwrap();
        assert_eq!(flood, round == 0);
        assert!(w.run_until(secs(20), |w| {
            w.nodes[0]
                .events
                .iter()
                .any(|e| matches!(e, Event::SendConfirmed { .. }))
        }));
        w.nodes[0].events.clear();
    }
}

#[test]
fn advert_hop_limit() {
    let mut w = line(52, |r| r.flood_max_advert = 0);
    assert!(!flood_advert(&mut w, 0, 2), "adverts stop at the relay");
}

#[test]
fn wildcard_denying_flood_stops_plain_floods() {
    let mut w = line(53, |r| r.regions.wildcard_flags = DENY_FLOOD);
    assert!(!flood_advert(&mut w, 0, 2));
}

#[test]
fn disabled_forwarding() {
    let mut w = line(54, |r| r.disable_fwd = true);
    assert!(!flood_advert(&mut w, 0, 2));
}

/// A flood whose path already holds the relay's hash `times` times, given to the relay.
fn queued_after_loop(times: usize) -> usize {
    let mut w = line(55, |_| {});
    let me = w.node(1).id.pub_key()[0];
    let path: Vec<u8> = (0..times).map(|_| me).collect();
    let mut p = PacketBuf::new(Route::Flood, PayloadType::GRP_TXT);
    p.set_payload(&[0x5A; 24]).unwrap();
    p.set_path(PathLen::new(1, times as u8).unwrap(), &path)
        .unwrap();
    let mut raw = [0u8; 256];
    let n = p.encode(&mut raw).unwrap();
    let now = w.now;
    let mut rng = ChaCha20Rng::seed_from_u64(1);
    w.node(1).on_rx(now, &raw[..n], 20, -60, &mut rng);
    w.node(1).disp.len()
}

#[test]
fn loop_detection_minimal() {
    // Minimal, 1-byte hashes: forwarded while the relay is in the path fewer than 4 times.
    assert_eq!(queued_after_loop(3), 1);
    assert_eq!(queued_after_loop(4), 0);
}

#[test]
fn relayed_direct_acks_are_sent_again_as_multi_acks() {
    let mut w = line(56, |_| {});
    w.node(1).prefs.multi_acks = 1;
    assert!(flood_advert(&mut w, 0, 2));
    assert!(flood_advert(&mut w, 2, 0));
    // The first DM by flood teaches Alice her path to Bob; her reciprocal PATH teaches Bob his,
    // after the second DM's ACK has left by flood. The third one's ACK goes direct, through
    // the relay.
    for _ in 0..3 {
        let now = w.now;
        let ts = w.node(0).time(now);
        let to: [u8; 6] = w.node(2).id.pub_key()[..6].try_into().unwrap();
        w.node(0).send_text(now, &to, 0, 0, ts, b"ping").unwrap();
        assert!(w.run_until(secs(20), |w| {
            w.nodes[0]
                .events
                .iter()
                .any(|e| matches!(e, Event::SendConfirmed { .. }))
        }));
        w.nodes[0].events.clear();
    }
    assert!(
        sent_by(&w, 1, PayloadType::MULTIPART) >= 1,
        "the relay sent a MULTIPART ACK"
    );
}

/// Alice floods her advert scoped to region "fr"; the relay knows `known` regions.
fn scoped_advert_crosses(region_allows: Option<bool>) -> bool {
    let mut w = line(57, |r| {
        if let Some(allow) = region_allows {
            let id = r.regions.put("fr", 0, 0).unwrap();
            r.regions.set_flags(id, if allow { 0 } else { DENY_FLOOD });
        }
    });
    let p = &mut w.node(0).prefs;
    p.default_scope_key = mc_air::crypto::region_key("fr");
    p.default_scope_name[..2].copy_from_slice(b"fr");
    flood_advert(&mut w, 0, 2)
}

#[test]
fn scoped_floods_need_a_known_region_that_allows_them() {
    assert!(scoped_advert_crosses(Some(true)));
    assert!(!scoped_advert_crosses(Some(false)));
    assert!(!scoped_advert_crosses(None), "unknown scope");
}
