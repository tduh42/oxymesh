//! The room server (`mc-server`, `Role::Room`; `docs/spec/server.md` §3) against our own
//! companions: logins, posts pushed to the other clients until they acknowledge, keep-alive,
//! status and the room's console.

use mc_air::PayloadType;
use mc_core::Duration;
use mc_proto::codec::APP_VER;
use mc_proto::{Command, Response};
use mc_server::acl;
use mc_sim::World;

fn secs(s: u64) -> Duration {
    Duration::from_secs(s)
}

fn ask(w: &mut World, i: usize, c: &Command<'_>) -> Response<'static> {
    let mut b = [0u8; 256];
    let n = c.encode(&mut b).unwrap();
    let f = w.app(i, &b[..n]).into_iter().next().expect("a response");
    let f: &'static [u8] = Box::leak(f.to_vec().into_boxed_slice());
    Response::decode(f).unwrap()
}

fn wait<T>(
    w: &mut World,
    i: usize,
    max: Duration,
    mut pick: impl FnMut(&Response<'_>) -> Option<T>,
) -> Option<T> {
    let from = w.nodes[i].app.len();
    let mut found = None;
    w.run_until(max, |w| {
        found = w.nodes[i]
            .app
            .get(from..)
            .unwrap_or(&[])
            .iter()
            .find_map(|f| Response::decode(f).ok().and_then(|r| pick(&r)));
        found.is_some()
    });
    found
}

const ME: usize = 0;
const ROOM: usize = 1;
const BOB: usize = 2;

/// Us (0) and Bob (2) around a room server (1, admin "secret", room password "hello"); we
/// do not hear each other.
fn room(seed: u64, read_only: bool) -> World {
    let mut w = World::new(seed);
    let me = w.add("Morgan", |_| {});
    let room = w.add("Hut", |_| {});
    let bob = w.add("Bob", |_| {});
    w.link(me, room);
    w.link(bob, room);
    w.serve_room(room, "secret", "hello");
    w.nodes[room].server.as_mut().unwrap().prefs.allow_read_only = read_only;
    for i in [me, bob] {
        ask(&mut w, i, &Command::DeviceQuery { app_ver: APP_VER });
    }
    w.run(secs(20)); // the room's boot advert
    w
}

fn key(w: &World, i: usize) -> [u8; 32] {
    *w.nodes[i].dev.node.id.pub_key()
}

/// (admin byte, permissions, firmware level) of a login.
fn login(w: &mut World, who: usize, password: &[u8]) -> Option<(u8, u8, u8)> {
    let key = key(w, ROOM);
    ask(w, who, &Command::SendLogin { key, password });
    wait(w, who, secs(15), |r| match r {
        Response::LoginSuccess {
            permissions,
            server: Some(s),
            ..
        } => Some((*permissions, s.acl, s.fw_level)),
        _ => None,
    })
}

/// Posts `text` (plain text to the room); true if the room acknowledged it.
fn post(w: &mut World, who: usize, text: &str) -> bool {
    let now = w.now;
    let timestamp = w.node(who).time(now);
    let prefix: [u8; 6] = key(w, ROOM)[..6].try_into().unwrap();
    ask(
        w,
        who,
        &Command::SendTxtMsg {
            txt_type: 0,
            attempt: 0,
            timestamp,
            prefix,
            text: text.as_bytes(),
        },
    );
    wait(w, who, secs(15), |r| {
        matches!(r, Response::SendConfirmed { .. }).then_some(())
    })
    .is_some()
}

/// The pushed posts `who` received: (author prefix, text).
fn inbox(w: &mut World, who: usize) -> Vec<([u8; 4], String)> {
    let mut out = Vec::new();
    loop {
        match ask(w, who, &Command::SyncNextMessage) {
            Response::ContactMsg {
                txt_type: 2, text, ..
            } => {
                // The companion frame puts the author's prefix before the text.
                let (a, t) = text.split_at(4);
                out.push((
                    a.try_into().unwrap(),
                    String::from_utf8_lossy(t).into_owned(),
                ));
            }
            Response::NoMoreMessages => break,
            _ => {}
        }
    }
    out
}

fn room_txt_count(w: &World) -> usize {
    w.log
        .iter()
        .filter(|(_, from, raw)| {
            *from == ROOM
                && mc_air::Packet::decode(raw).is_ok_and(|p| p.header.ptype == PayloadType::TXT_MSG)
        })
        .count()
}

#[test]
fn logins_by_password() {
    let mut w = room(81, false);
    assert_eq!(login(&mut w, ME, b"secret"), Some((1, acl::ADMIN, 1)));
    assert_eq!(login(&mut w, BOB, b"hello"), Some((0, acl::READ_WRITE, 1)));
    let mut w = room(82, false);
    assert_eq!(login(&mut w, ME, b"nope"), None);
    let mut w = room(83, true);
    assert_eq!(
        login(&mut w, ME, b"nope"),
        Some((2, acl::GUEST, 1)),
        "read only: admin byte 2"
    );
}

#[test]
fn a_post_reaches_the_other_client_once() {
    let mut w = room(84, false);
    login(&mut w, ME, b"hello").unwrap();
    login(&mut w, BOB, b"hello").unwrap();
    assert!(
        post(&mut w, ME, "Snow on the pass"),
        "the room acknowledges"
    );
    w.run(secs(30));
    let got = inbox(&mut w, BOB);
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].0, key(&w, ME)[..4]);
    assert_eq!(got[0].1, "Snow on the pass");
    assert!(
        inbox(&mut w, ME).is_empty(),
        "not pushed back to its author"
    );
    // Bob acknowledged: no second push.
    let pushes = room_txt_count(&w);
    w.run(secs(60));
    assert_eq!(room_txt_count(&w), pushes);
    let srv = w.nodes[ROOM].server.as_ref().unwrap();
    let bob = srv
        .acl
        .clients
        .iter()
        .find(|c| c.key == key(&w, BOB))
        .unwrap();
    assert_eq!(bob.pending_ack, 0);
    assert!(bob.sync_since > 0);
}

#[test]
fn a_late_client_gets_what_it_missed() {
    let mut w = room(85, false);
    login(&mut w, ME, b"hello").unwrap();
    assert!(post(&mut w, ME, "first"));
    assert!(post(&mut w, ME, "second"));
    w.run(secs(10));
    login(&mut w, BOB, b"hello").unwrap();
    w.run(secs(40));
    let texts: Vec<String> = inbox(&mut w, BOB).into_iter().map(|(_, t)| t).collect();
    assert_eq!(texts, ["first", "second"]);
}

#[test]
fn guests_read_but_do_not_post() {
    let mut w = room(86, true);
    login(&mut w, ME, b"anything").unwrap();
    assert!(!post(&mut w, ME, "spam"), "no ACK for a guest");
    assert_eq!(w.nodes[ROOM].server.as_ref().unwrap().room.posted, 0);
}

#[test]
fn the_room_posts_from_its_console() {
    let mut w = room(87, false);
    login(&mut w, BOB, b"hello").unwrap();
    let now = w.now;
    let mut out = mc_node::cli::Reply::new();
    let mut serial = String::new();
    let sn = &mut w.nodes[ROOM];
    let mut rng = rand_chacha::ChaCha20Rng::seed_from_u64(3);
    let srv = sn.server.as_mut().unwrap();
    srv.command(
        &mut sn.dev.node,
        0,
        b"room.post Hut closed Monday",
        &mut out,
        &mut serial,
        now,
        &mut rng,
    );
    assert_eq!(out, "OK");
    for (cmd, want) in [
        ("room.post   ", "ERR empty message"),
        ("get role", "> room_server"),
        ("set repeat on", "ERR - repeat now deprecated"),
        ("get repeat", "> off"),
        ("discover.neighbors", "Unknown command"),
    ] {
        out.clear();
        srv.command(
            &mut sn.dev.node,
            0,
            cmd.as_bytes(),
            &mut out,
            &mut serial,
            now,
            &mut rng,
        );
        assert_eq!(out, want, "{cmd}");
    }
    w.run(secs(30));
    let got = inbox(&mut w, BOB);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0, key(&w, ROOM)[..4], "the room is the author");
}

#[test]
fn status_counts_posts_and_pushes() {
    let mut w = room(88, false);
    login(&mut w, ME, b"hello").unwrap();
    login(&mut w, BOB, b"hello").unwrap();
    assert!(post(&mut w, ME, "one"));
    w.run(secs(30));
    let key = key(&w, ROOM);
    ask(&mut w, ME, &Command::SendStatusReq { key });
    let data = wait(&mut w, ME, secs(15), |r| match r {
        Response::StatusResponse { data, .. } => Some(data.to_vec()),
        _ => None,
    })
    .expect("a status");
    // 52 bytes (padded), the last two u16 posts stored and pushes.
    assert!(data.len() >= 52);
    assert_eq!(u16::from_le_bytes([data[48], data[49]]), 1);
    assert_eq!(u16::from_le_bytes([data[50], data[51]]), 1);
}

#[test]
fn keep_alive_is_answered_with_an_ack_and_the_unsynced_count() {
    let mut w = room(89, false);
    login(&mut w, ME, b"hello").unwrap();
    login(&mut w, BOB, b"hello").unwrap();
    w.run(secs(5));
    // Our post waits for Bob; Bob is told how many by a keep-alive (direct, path known).
    let id = key(&w, ME);
    let now = w.now;
    let sn = &mut w.nodes[ROOM];
    let srv = sn.server.as_mut().unwrap();
    srv.add_post(&mut sn.dev.node, &id, "count me", now);
    let before = w.log.len();
    let key = key(&w, ROOM);
    ask(&mut w, BOB, &Command::SendBinaryReq { key, data: &[0x02] });
    w.run(secs(5));
    let ack = w.log[before..]
        .iter()
        .filter(|(_, from, _)| *from == ROOM)
        .filter_map(|(_, _, raw)| mc_air::Packet::decode(raw).ok())
        .find(|p| p.header.ptype == PayloadType::ACK)
        .expect("an ACK");
    assert_eq!(ack.payload.len(), 5);
    assert!(ack.payload[4] >= 1, "posts waiting: {}", ack.payload[4]);
}

use rand_chacha::rand_core::SeedableRng;

#[test]
fn a_restored_room_server_still_does_not_forward() {
    // Restored before `start` with nothing stored, as the firmware does at first boot.
    let now = mc_core::Instant::from_nanos(1_000_000_000);
    let id = mc_sim::identity("hut");
    let mut node = mc_node::Node::new(
        id.clone(),
        mc_node::Prefs::defaults(id.pub_key()),
        mc_sim::EPOCH,
        now,
    );
    let mut s = mc_server::Server::new(now, "v1.18.0-oxymesh", mc_server::Role::Room);
    s.restore(&mut node, None, std::iter::empty(), None);
    s.start(&mut node, now);
    assert!(node.routing.as_ref().unwrap().disable_fwd);
}
