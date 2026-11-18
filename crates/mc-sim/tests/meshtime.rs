//! Mesh time (`docs/spec/companion.md` §9, 14): a node whose clock was not set (an M5 switched
//! off at its hardware switch: the clock chip lost power) takes the time from the adverts it
//! hears, once 3 senders agree; a wrong clock among them does not count.

use mc_core::Duration;
use mc_node::clock::{Rtc, Source};
use mc_node::device::Action;
use mc_sim::{EPOCH, World};

/// The right time: a day after the build floor.
const TRUE: u32 = EPOCH + 86_400;

/// Our node with its clock unset, three neighbours with right clocks (20 s apart) and one
/// stuck an hour after the floor. Returns (world, us, right ones, wrong one).
fn mesh() -> (World, usize, Vec<usize>, usize) {
    let mut w = World::new(9);
    let me = w.add("morgan", |_| {});
    let now = w.now;
    w.nodes[me].dev.node.rtc = Rtc::new(EPOCH, now);
    let mut right = Vec::new();
    for (i, name) in ["ash", "birch", "cedar"].iter().enumerate() {
        let n = w.add(name, |_| {});
        w.nodes[n].dev.node.rtc.set(now, TRUE + i as u32 * 20);
        w.link(me, n);
        right.push(n);
    }
    let wrong = w.add("stuck", |_| {});
    w.nodes[wrong].dev.node.rtc.set(now, EPOCH + 3_600);
    w.link(me, wrong);
    (w, me, right, wrong)
}

fn advert(w: &mut World, n: usize) {
    let t = w.now;
    w.node(n).send_self_advert(t, false).unwrap();
    w.run(Duration::from_secs(5));
}

#[test]
fn an_unset_clock_takes_the_time_from_three_agreeing_adverts() {
    let (mut w, me, right, wrong) = mesh();
    for n in [wrong, right[0], right[1]] {
        advert(&mut w, n);
    }
    assert!(
        !w.nodes[me].dev.node.rtc.is_set(),
        "two right clocks and a wrong one: not yet"
    );
    assert_eq!(w.nodes[me].dev.mesh_time(w.now), (3, 2));
    advert(&mut w, right[2]);
    let rtc = &w.nodes[me].dev.node.rtc;
    assert_eq!(rtc.source(), Some(Source::Mesh));
    let mine = rtc.now(w.now);
    let median = w.nodes[right[1]].dev.node.rtc.now(w.now);
    assert!(mine.abs_diff(median) <= 5, "{mine} vs {median}");
    // The board's clock chip follows.
    let actions: Vec<Action> = std::iter::from_fn(|| w.nodes[me].dev.poll_action()).collect();
    assert!(actions.contains(&Action::ClockSet), "{actions:?}");
    assert_eq!(w.nodes[me].dev.mesh_time(w.now), (0, 0), "samples dropped");
}

#[test]
fn a_clock_already_set_ignores_the_mesh() {
    let (mut w, me, right, _) = mesh();
    let now = w.now;
    w.nodes[me].dev.set_time(now, TRUE + 600);
    for n in right {
        advert(&mut w, n);
    }
    let rtc = &w.nodes[me].dev.node.rtc;
    assert_eq!(rtc.source(), Some(Source::App));
    assert!(rtc.now(w.now) >= TRUE + 600);
    assert_eq!(w.nodes[me].dev.mesh_time(w.now), (0, 0));
}
