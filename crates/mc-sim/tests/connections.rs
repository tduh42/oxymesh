//! Keep-alive connections (`HAS_CONNECTION`, `LOGOUT`; `docs/spec/companion.md` §6.2): a server
//! asking for keep-alive in its login reply gets a slot; a v1.18 server never does.

use mc_core::Duration;
use mc_proto::codec::{APP_VER, err};
use mc_proto::{Command, Response};
use mc_sim::World;

fn ask(w: &mut World, i: usize, c: &Command<'_>) -> Response<'static> {
    let mut b = [0u8; 256];
    let n = c.encode(&mut b).unwrap();
    let f = w.app(i, &b[..n]).into_iter().next().expect("a response");
    let f: &'static [u8] = Box::leak(f.to_vec().into_boxed_slice());
    Response::decode(f).unwrap()
}

/// Logs in to the relay and says whether the device now holds a connection to it.
fn login_then_has(keep_alive: u8) -> (bool, bool) {
    let mut w = World::new(41);
    let me = w.add("me", |_| {});
    let relay = w.add("relay", |_| {});
    w.link(me, relay);
    w.serve_repeater(relay, "secret", "");
    w.nodes[relay].server.as_mut().unwrap().legacy_keep_alive = keep_alive;
    ask(&mut w, me, &Command::DeviceQuery { app_ver: APP_VER });
    let now = w.now;
    w.node(relay).send_self_advert(now, false).unwrap();
    w.run(Duration::from_secs(3));
    let key = *w.node(relay).id.pub_key();
    ask(
        &mut w,
        me,
        &Command::SendLogin {
            key,
            password: b"secret",
        },
    );
    assert!(w.run_until(Duration::from_secs(15), |w| {
        w.nodes[me]
            .app
            .iter()
            .any(|f| matches!(Response::decode(f), Ok(Response::LoginSuccess { .. })))
    }));
    let has = ask(&mut w, me, &Command::HasConnection { key });
    let connected = has == Response::Ok;
    if !connected {
        assert_eq!(has, Response::Err(err::NOT_FOUND));
    }
    assert_eq!(ask(&mut w, me, &Command::Logout { key }), Response::Ok);
    let after = ask(&mut w, me, &Command::HasConnection { key }) == Response::Ok;
    (connected, after)
}

#[test]
fn v1_18_servers_never_open_one() {
    assert_eq!(login_then_has(0), (false, false));
}

#[test]
fn a_server_asking_for_keep_alive_gets_a_slot_until_logout() {
    assert_eq!(login_then_has(4), (true, false));
}
