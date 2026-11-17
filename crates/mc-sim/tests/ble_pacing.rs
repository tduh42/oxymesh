//! The contact list over Bluetooth (`docs/spec/companion.md` §1.1): a model of the firmware's
//! link loop (`run::link::AppLink::poll`) between a device and the 4-frame queue of a connected
//! phone (`mc_proto::ble::BleLink`), the queue emptied by one notification per step as the
//! Bluetooth task does every 60 ms. Found on an M5 with 112 contacts: without pacing, the app
//! got a few contacts and no END_OF_CONTACTS.

use mc_proto::ble::BleLink;
use mc_proto::codec::to_fixed;
use mc_proto::{Command, FRAME_MAX, Response};
use mc_sim::World;
use rand_core::SeedableRng;

const CONTACTS: usize = 112;

fn enc(c: &Command<'_>) -> Vec<u8> {
    let mut b = [0u8; 256];
    let n = c.encode(&mut b).unwrap();
    b[..n].to_vec()
}

/// A device with `CONTACTS` contacts, added through the app protocol.
fn device() -> World {
    let mut w = World::new(5);
    w.add("morgan", |_| {});
    for i in 0..CONTACTS {
        let mut key = [0x5A; 32];
        key[0] = 1 + i as u8;
        let name = format!("node {i}");
        let r = w.app(
            0,
            &enc(&Command::AddUpdateContact {
                key,
                kind: 1,
                flags: 0,
                path_len: 0,
                path: [0; 64],
                name: to_fixed::<32>(name.as_bytes()),
                last_advert: mc_sim::EPOCH,
                lat_lon: None,
                lastmod: None,
            }),
        );
        assert!(matches!(Response::decode(&r[0]).unwrap(), Response::Ok));
    }
    assert_eq!(w.nodes[0].dev.node.contacts.list.len(), CONTACTS);
    w
}

/// Runs GET_CONTACTS through the model; at step 20 the app also asks for the time, whose reply
/// must get through between the contacts. Returns the frames the phone received, in order, and
/// how many the link refused.
fn sync(paced: bool) -> (Vec<Vec<u8>>, usize) {
    let mut w = device();
    let now = w.now;
    let mut rng = rand_chacha::ChaCha20Rng::seed_from_u64(1);
    let mut link = BleLink::new();
    link.on_authenticated();
    let mut received = Vec::new();
    let mut dropped = 0;
    let sn = &mut w.nodes[0];
    sn.dev.on_app_frame(
        now,
        &enc(&Command::GetContacts { since: None }),
        &mut rng,
        &mut sn.st,
    );
    let mut buf = [0u8; FRAME_MAX];
    for step in 0..2_000u64 {
        if step == 20 {
            sn.dev
                .on_app_frame(now, &enc(&Command::GetDeviceTime), &mut rng, &mut sn.st);
        }
        // The runner's pass.
        loop {
            let (any, contact) = if paced {
                (link.ready(false, step * 60), link.ready(true, step * 60))
            } else {
                (true, true)
            };
            if !any {
                break;
            }
            let Some(f) = sn.dev.poll_app(contact) else {
                break;
            };
            if !link.send(&f) {
                dropped += 1;
            }
        }
        // The Bluetooth task: one notification, 60 ms after the last.
        if let Some(n) = link.next_notify(step * 60, &mut buf) {
            received.push(buf[..n].to_vec());
        }
    }
    (received, dropped)
}

#[test]
fn a_full_contact_list_reaches_the_phone_in_order() {
    let (frames, dropped) = sync(true);
    assert_eq!(dropped, 0, "nothing refused by the queue");
    let decoded: Vec<Response<'_>> = frames
        .iter()
        .map(|f| Response::decode(f).unwrap())
        .collect();
    assert!(matches!(
        decoded.first(),
        Some(Response::ContactsStart { total }) if *total as usize == CONTACTS
    ));
    let keys: Vec<u8> = decoded
        .iter()
        .filter_map(|r| match r {
            Response::Contact(c) => Some(c.key[0]),
            _ => None,
        })
        .collect();
    assert_eq!(
        keys,
        (1..=CONTACTS as u8).collect::<Vec<_>>(),
        "all, in order"
    );
    assert!(matches!(
        decoded.last(),
        Some(Response::EndOfContacts { .. })
    ));
    assert_eq!(
        decoded
            .iter()
            .filter(|r| matches!(r, Response::CurrTime { .. }))
            .count(),
        1,
        "the reply asked in the middle got through"
    );
}

/// The bug, kept as a check that the model sees it: drained in one go, the queue refuses most
/// of the list and the end.
#[test]
fn unpaced_the_list_is_cut_short() {
    let (frames, dropped) = sync(false);
    assert!(dropped > 100, "{dropped}");
    assert!(
        !frames
            .iter()
            .any(|f| matches!(Response::decode(f), Ok(Response::EndOfContacts { .. }))),
        "no END_OF_CONTACTS"
    );
}

/// Without a phone, frames only go to USB: the device is never held back.
#[test]
fn without_a_phone_the_link_never_holds_frames_back() {
    let link = BleLink::new();
    assert!(link.ready(true, 0) && link.ready(false, 0));
}

/// A phone that stops taking notifications holds frames back for 2 s at most; then they flow
/// again (to USB), Bluetooth dropping what does not fit, as the C++.
#[test]
fn a_stalled_phone_holds_frames_back_for_two_seconds_only() {
    let mut link = BleLink::new();
    link.on_authenticated();
    let mut buf = [0u8; FRAME_MAX];
    for i in 0..mc_proto::ble::QUEUE {
        assert!(link.send(&[1, i as u8]));
    }
    // One notification at 100 ms, then nothing.
    assert!(link.next_notify(100, &mut buf).is_some());
    assert!(link.send(&[2]));
    assert!(!link.ready(false, 100), "full");
    assert!(!link.ready(false, 2_099));
    assert!(link.ready(false, 2_100), "stalled: as the C++");
    assert!(link.ready(true, 2_100));
    // A notification gets it moving: paced again.
    assert!(link.next_notify(2_200, &mut buf).is_some());
    assert!(link.ready(false, 2_200));
    assert!(!link.ready(true, 2_200), "contacts wait for an empty queue");
}
