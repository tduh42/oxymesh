//! The repeater role (`mc-server`, `docs/spec/server.md`) against our own companion, which
//! speaks to it like the MeshCore app does: logins, requests, the console over the air,
//! anonymous requests, neighbours, discovery and adverts.

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

/// The first app frame of `i` (from now on) that `pick` accepts, within `max`.
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
const RELAY: usize = 1;

/// Us (0) next to a repeater (1, admin "secret", guest "hello").
fn pair(seed: u64) -> World {
    let mut w = World::new(seed);
    let me = w.add("Morgan", |_| {});
    let relay = w.add("relay", |_| {});
    w.link(me, relay);
    w.serve_repeater(relay, "secret", "hello");
    ask(&mut w, me, &Command::DeviceQuery { app_ver: APP_VER });
    let clock = mc_sim::EPOCH + 60;
    ask(&mut w, me, &Command::SetDeviceTime { unix_s: clock });
    w
}

/// [`pair`], once we have heard the repeater's boot advert.
fn met(seed: u64) -> World {
    let mut w = pair(seed);
    w.run(secs(20));
    w
}

fn key(w: &World, i: usize) -> [u8; 32] {
    *w.nodes[i].dev.node.id.pub_key()
}

/// Logs in with `password`; the server's (permissions, firmware level) if it answered.
fn login(w: &mut World, to: usize, password: &[u8]) -> Option<(u8, u8)> {
    let key = key(w, to);
    ask(w, ME, &Command::SendLogin { key, password });
    wait(w, ME, secs(15), |r| match r {
        Response::LoginSuccess {
            server: Some(s), ..
        } => Some((s.acl, s.fw_level)),
        _ => None,
    })
}

#[test]
fn admin_and_guest_logins() {
    let mut w = met(61);
    assert_eq!(login(&mut w, RELAY, b"secret"), Some((acl::ADMIN, 2)));
    // A v1.18 guest has role 0 (the stand-in this replaces said 1).
    let mut w = met(62);
    assert_eq!(login(&mut w, RELAY, b"hello"), Some((acl::GUEST, 2)));
    let mut w = met(63);
    assert_eq!(
        login(&mut w, RELAY, b"nope"),
        None,
        "wrong password: no answer"
    );
}

#[test]
fn a_known_client_logs_in_again_without_its_password() {
    let mut w = met(64);
    assert_eq!(login(&mut w, RELAY, b"secret"), Some((acl::ADMIN, 2)));
    w.run(secs(2));
    assert_eq!(login(&mut w, RELAY, b""), Some((acl::ADMIN, 2)));
}

#[test]
fn status_is_56_bytes_of_counters() {
    let mut w = met(65);
    login(&mut w, RELAY, b"hello").unwrap();
    w.run(secs(30));
    let key = key(&w, RELAY);
    ask(&mut w, ME, &Command::SendStatusReq { key });
    let data = wait(&mut w, ME, secs(15), |r| match r {
        Response::StatusResponse { data, .. } => Some(data.to_vec()),
        _ => None,
    })
    .expect("a status");
    assert_eq!(data.len(), 60, "56 bytes, padded");
    let u16_at = |i: usize| u16::from_le_bytes([data[i], data[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes(data[i..i + 4].try_into().unwrap());
    assert_eq!(u16_at(0), 4_050, "battery");
    assert_eq!(u16_at(4) as i16, -112, "noise floor");
    let uptime = u32_at(20);
    assert!((30..60).contains(&uptime), "uptime {uptime}");
    assert!(u32_at(8) >= 2, "received: the login and this request");
}

#[test]
fn guests_get_base_telemetry() {
    let mut w = met(66);
    login(&mut w, RELAY, b"hello").unwrap();
    let key = key(&w, RELAY);
    ask(
        &mut w,
        ME,
        &Command::SendTelemetryReq {
            reserved: [0; 3],
            key: Some(key),
        },
    );
    let lpp = wait(&mut w, ME, secs(15), |r| match r {
        Response::TelemetryResponse { lpp, .. } => Some(lpp.to_vec()),
        _ => None,
    })
    .expect("telemetry");
    // Voltage 4.05 V (channel 1, LPP 116, 0.01 V), then the MCU temperature 23.1 °C (103).
    assert_eq!(lpp[..8], [1, 116, 0x01, 0x95, 1, 103, 0x00, 0xE7]);
    assert!(
        lpp[8..].iter().all(|&b| b == 0),
        "then padding only: {lpp:?}"
    );
}

/// A binary request (`SEND_BINARY_REQ`): the reply after its tag.
fn binary(w: &mut World, to: usize, req: &[u8]) -> Option<Vec<u8>> {
    let key = key(w, to);
    ask(w, ME, &Command::SendBinaryReq { key, data: req });
    wait(w, ME, secs(15), |r| match r {
        Response::BinaryResponse { data, .. } => Some(data.to_vec()),
        _ => None,
    })
}

#[test]
fn the_access_list_is_for_admins() {
    let mut w = met(67);
    login(&mut w, RELAY, b"hello").unwrap();
    assert_eq!(
        binary(&mut w, RELAY, &[0x05, 0, 0]),
        None,
        "guests are refused"
    );
    w.run(secs(2));
    login(&mut w, RELAY, b"secret").unwrap();
    let list = binary(&mut w, RELAY, &[0x05, 0, 0]).expect("the list");
    let me = key(&w, ME);
    assert_eq!(list[..6], me[..6]);
    assert_eq!(list[6], acl::ADMIN);
    assert!(
        list[7..].iter().all(|&b| b == 0),
        "the guest (role 0) is not listed"
    );
}

#[test]
fn owner_info() {
    let mut w = met(68);
    w.nodes[RELAY].server.as_mut().unwrap().prefs.owner_info =
        "Call sign F0XYZ".try_into().unwrap();
    login(&mut w, RELAY, b"hello").unwrap();
    let info = binary(&mut w, RELAY, &[0x07]).expect("owner info");
    let text = info.split(|&b| b == 0).next().unwrap();
    assert_eq!(text, b"v1.18.0-oxymesh\nrelay\nCall sign F0XYZ");
}

/// Sends a console command as a CLI text (or plain text, `txt_type` 0).
fn console(w: &mut World, to: usize, txt_type: u8, text: &str) -> Option<String> {
    let now = w.now;
    let timestamp = w.node(ME).time(now);
    let prefix: [u8; 6] = key(w, to)[..6].try_into().unwrap();
    ask(
        w,
        ME,
        &Command::SendTxtMsg {
            txt_type,
            attempt: 0,
            timestamp,
            prefix,
            text: text.as_bytes(),
        },
    );
    let _ = ask(w, ME, &Command::SyncNextMessage);
    let mut got = None;
    w.run_until(secs(15), |w| {
        got = w.nodes[ME]
            .app
            .iter()
            .any(|f| f.first() == Some(&0x83))
            .then_some(());
        got.is_some()
    });
    got?;
    match ask(w, ME, &Command::SyncNextMessage) {
        Response::ContactMsg {
            txt_type: 1, text, ..
        } => Some(String::from_utf8_lossy(text).into_owned()),
        other => panic!("{other:?}"),
    }
}

#[test]
fn admins_use_the_console_over_the_air() {
    let mut w = met(69);
    login(&mut w, RELAY, b"secret").unwrap();
    let r = console(&mut w, RELAY, 1, "get radio").expect("a reply");
    assert!(r.starts_with("> 869."), "{r}");
    w.nodes[ME].app.clear();
    let r = console(&mut w, RELAY, 1, "frobnicate").expect("a reply");
    assert_eq!(r, "Unknown command");
}

#[test]
fn plain_text_commands_are_acknowledged() {
    let mut w = met(70);
    login(&mut w, RELAY, b"secret").unwrap();
    console(&mut w, RELAY, 0, "get radio").expect("a reply");
    assert!(
        w.nodes[ME]
            .app
            .iter()
            .any(|f| matches!(Response::decode(f), Ok(Response::SendConfirmed { .. }))),
        "the legacy console's ACK"
    );
}

#[test]
fn guests_have_no_console() {
    let mut w = met(71);
    login(&mut w, RELAY, b"hello").unwrap();
    assert_eq!(console(&mut w, RELAY, 1, "get radio"), None);
}

#[test]
fn anonymous_clock_request_needs_a_direct_route() {
    // Before any advert: the companion makes a temporary zero-hop contact, so it goes direct.
    let mut w = pair(72);
    let key = key(&w, RELAY);
    ask(&mut w, ME, &Command::SendAnonReq { key, data: &[3, 0] });
    let data = wait(&mut w, ME, secs(15), |r| match r {
        Response::BinaryResponse { data, .. } => Some(data.to_vec()),
        _ => None,
    })
    .expect("the clock");
    // Their clock (4 bytes), then the features: forwarding on; padded like any datagram.
    assert!(data.len() >= 5);
    assert_eq!(data[4], 0);
}

/// Us — r1 — r2 — r3, repeaters; we only hear r1.
fn chain(seed: u64) -> World {
    let mut w = World::new(seed);
    let me = w.add("Morgan", |_| {});
    let mut prev = me;
    for name in ["r1", "r2", "r3"] {
        let r = w.add(name, |_| {});
        w.link(prev, r);
        w.serve_repeater(r, "secret", "");
        prev = r;
    }
    ask(&mut w, me, &Command::DeviceQuery { app_ver: APP_VER });
    // The boot adverts (zero hop), then flood adverts so that we know all three.
    w.run(secs(20));
    for r in 1..=3 {
        let now = w.now;
        w.node(r).send_self_advert(now, true).unwrap();
        w.run(secs(8));
    }
    w
}

#[test]
fn neighbours_are_repeaters_heard_directly() {
    let mut w = chain(73);
    login(&mut w, 2, b"").expect("a flood login through r1");
    // [0x06] version 0, count 10, offset 0, newest first, 6-byte prefixes.
    let r = binary(&mut w, 2, &[0x06, 0, 10, 0, 0, 0, 6, 1, 2, 3, 4]).expect("neighbours");
    let total = u16::from_le_bytes([r[0], r[1]]);
    let n = u16::from_le_bytes([r[2], r[3]]);
    assert_eq!((total, n), (2, 2));
    let mut got: Vec<[u8; 6]> = r[4..]
        .chunks(11)
        .take(usize::from(n))
        .map(|e| e[..6].try_into().unwrap())
        .collect();
    got.sort();
    let mut want: Vec<[u8; 6]> = [1, 3]
        .iter()
        .map(|&i| key(&w, i)[..6].try_into().unwrap())
        .collect();
    want.sort();
    assert_eq!(got, want, "r1 and r3, not us (a companion)");
}

#[test]
fn a_request_after_a_flood_login_goes_direct() {
    let mut w = chain(74);
    login(&mut w, 3, b"").expect("a flood login, two hops");
    let before = w.log.len();
    let key = key(&w, 3);
    ask(&mut w, ME, &Command::SendStatusReq { key });
    assert!(
        wait(&mut w, ME, secs(20), |r| matches!(
            r,
            Response::StatusResponse { .. }
        )
        .then_some(()))
        .is_some()
    );
    let first = w.log[before..]
        .iter()
        .find(|(_, from, _)| *from == ME)
        .and_then(|(_, _, raw)| mc_air::Packet::decode(raw).ok())
        .unwrap();
    assert_eq!(first.header.ptype, PayloadType::REQ);
    assert!(
        !first.header.route.is_flood(),
        "the path came back with the login reply"
    );
}

#[test]
fn discovery_finds_the_repeaters_around() {
    let mut w = chain(75);
    let now = w.now;
    let sn = &mut w.nodes[2];
    let mut rng = rand_chacha::ChaCha20Rng::seed_from_u64(1);
    sn.server
        .as_mut()
        .unwrap()
        .discover(&mut sn.dev.node, now, &mut rng);
    w.run(secs(10));
    let r2 = w.nodes[2].server.as_ref().unwrap();
    let mut found: Vec<[u8; 32]> = r2.neighbours.iter().map(|n| n.key).collect();
    found.sort();
    let mut want = vec![key(&w, 1), key(&w, 3)];
    want.sort();
    assert_eq!(found, want);
}

#[test]
fn companions_get_discovery_answers() {
    let mut w = met(76);
    // A discovery request for repeaters, as the app sends it (prefix only).
    let data = [0x81, 1 << 2, 9, 8, 7, 6];
    assert_eq!(
        ask(&mut w, ME, &Command::SendControlData { data: &data }),
        Response::Ok
    );
    let got = wait(&mut w, ME, secs(10), |r| match r {
        Response::ControlData { payload, .. } => Some(payload.to_vec()),
        _ => None,
    })
    .expect("an answer");
    assert_eq!(got[0], 0x92);
    assert_eq!(got[2..6], [9, 8, 7, 6]);
    assert_eq!(got[6..], key(&w, RELAY)[..8]);
}

#[test]
fn local_adverts_on_their_timer() {
    let mut w = met(77);
    w.run(secs(5 * 60));
    let adverts: Vec<u64> = w
        .log
        .iter()
        .filter(|(_, from, raw)| {
            *from == RELAY
                && mc_air::Packet::decode(raw).is_ok_and(|p| p.header.ptype == PayloadType::ADVERT)
        })
        .map(|(t, _, _)| {
            t.saturating_duration_since(mc_core::Instant::ZERO)
                .as_millis()
                / 1000
        })
        .collect();
    // At boot + 16 s, then every 2 minutes.
    assert_eq!(adverts, [16, 120, 240]);
}

use rand_chacha::rand_core::SeedableRng;
