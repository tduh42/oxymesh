//! The repeater role (`docs/spec/server.md`): what strangers send it once the node has
//! decrypted it (anonymous requests, requests and console text from a logged-in client,
//! control data), and console lines (from the serial port and over the air).
//!
//! Input: records `[selector][length][bytes…]`. Selector bits 0-1: 0 anonymous request,
//! 1 request, 2 text, 3 control data; bit 4: came by flood; bit 5: from a second client.
//! An admin login comes first, so that requests and the console are reachable.
//!
//! Oracle: never panic; the access list stays within its capacity; every packet the node
//! then transmits decodes and re-encodes to the same bytes.
#![no_main]

use libfuzzer_sys::fuzz_target;
use mc_air::{LocalIdentity, PayloadType};
use mc_core::{Duration, Instant};
use mc_node::cli::Reply;
use mc_node::{Event, Node, Prefs};
use mc_server::Server;
use rand_chacha::ChaCha20Rng;
use rand_chacha::rand_core::SeedableRng;

fn le32(b: &[u8]) -> u32 {
    let mut a = [0u8; 4];
    for (o, v) in a.iter_mut().zip(b) {
        *o = *v;
    }
    u32::from_le_bytes(a)
}

fuzz_target!(|data: &[u8]| {
    let id = LocalIdentity::from_seed(&[7; 32]);
    let prefs = Prefs::defaults(id.pub_key());
    let mut now = Instant::from_nanos(1_000_000_000);
    let mut node = Node::new(id, prefs, 1_793_900_000, now);
    let mut rng = ChaCha20Rng::seed_from_u64(1);
    let role = if data.first().is_some_and(|b| b & 0x40 != 0) {
        mc_server::Role::Room
    } else {
        mc_server::Role::Repeater
    };
    let mut r = Server::new(now, "v1.18.0-oxymesh", role);
    r.start(&mut node, now);
    let clients = [
        *LocalIdentity::from_seed(&[9; 32]).pub_key(),
        *LocalIdentity::from_seed(&[11; 32]).pub_key(),
    ];
    let login = Event::AnonRequest {
        key: clients[0],
        ts: 1,
        data: heapless::Vec::from_slice(b"password").unwrap(),
        flood: true,
        in_path_len: 0,
        in_path: heapless::Vec::new(),
        region: None,
    };
    r.on_event(&mut node, &login, now, &mut rng);

    let mut rest = data;
    while let [sel, len, tail @ ..] = rest {
        let n = usize::from(*len).min(tail.len());
        let (body, next) = tail.split_at(n);
        rest = next;
        now = now + Duration::from_millis(250);
        let key = clients[usize::from(sel & 0x20 != 0)];
        let flood = sel & 0x10 != 0;
        let ev = match sel & 3 {
            0 => Event::AnonRequest {
                key,
                ts: le32(body),
                data: heapless::Vec::from_slice(body.get(4..).unwrap_or(&[])).unwrap_or_default(),
                flood,
                in_path_len: 0,
                in_path: heapless::Vec::new(),
                region: None,
            },
            1 | 2 => Event::ServerPeer {
                key,
                ptype: if sel & 3 == 1 {
                    PayloadType::REQ
                } else {
                    PayloadType::TXT_MSG
                },
                data: heapless::Vec::from_slice(body.get(..184).unwrap_or(body))
                    .unwrap_or_default(),
                flood,
                in_path_len: 0,
                in_path: heapless::Vec::new(),
                region: None,
            },
            _ => Event::ControlData {
                snr4: 12,
                rssi: -90,
                path_len: 0,
                data: heapless::Vec::from_slice(body).unwrap_or_default(),
            },
        };
        r.on_event(&mut node, &ev, now, &mut rng);
        r.tick(&mut node, now, &mut rng);
        assert!(r.acl.len() <= 32);
    }
    // The console, line by line, from the serial port and over the air.
    if let Ok(s) = core::str::from_utf8(data) {
        for (i, line) in s.split('\n').take(16).enumerate() {
            let ts = if i % 2 == 0 {
                0
            } else {
                1_793_900_000 + i as u32
            };
            let mut out = Reply::new();
            let mut serial = heapless::String::<4096>::new();
            r.command(
                &mut node,
                ts,
                line.as_bytes(),
                &mut out,
                &mut serial,
                now,
                &mut rng,
            );
            assert!(out.len() <= mc_node::cli::REPLY_MAX);
            r.tick(&mut node, now, &mut rng);
            assert!(r.acl.len() <= 32);
        }
        while r.poll_action().is_some() {}
    }

    // Everything queued goes out as canonical packets.
    for _ in 0..64 {
        now = now + Duration::from_secs(60);
        let Some(f) = node.poll_tx(now, || false, &mut rng) else {
            continue;
        };
        let p = mc_air::Packet::decode(&f.raw).expect("a packet we made decodes");
        let mut b = [0u8; 256];
        let n = p.encode(&mut b).expect("and encodes");
        assert_eq!(&b[..n], &f.raw[..]);
        node.tx_done(now + Duration::from_secs(2));
    }
});
