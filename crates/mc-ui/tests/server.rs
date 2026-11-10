//! A repeater's status pages (`docs/spec/ui.md` §13), on a simulated relay with a neighbour.

use mc_core::Duration;
use mc_ui::input::{Button, Gesture};
use mc_ui::server::{Act, Page, Pages, View};
use mc_ui::status::Oled;

use Gesture::*;

/// A relay (1) next to us (0) and another repeater (2).
fn world() -> mc_sim::World {
    let mut w = mc_sim::World::new(8);
    let me = w.add("Morgan", |_| {});
    let relay = w.add("Relay Col", |_| {});
    let other = w.add("Relay Aiguille", |_| {});
    w.link(me, relay);
    w.link(relay, other);
    w.serve_repeater(relay, "secret", "");
    w.serve_repeater(other, "secret", "");
    w.run(Duration::from_secs(20)); // boot adverts
    let now = w.now;
    w.node(me).send_self_advert(now, true).unwrap();
    w.run(Duration::from_secs(3_600));
    w
}

fn draw(w: &mc_sim::World, pages: &Pages) -> String {
    let sn = &w.nodes[1];
    let v = View {
        node: &sn.dev.node,
        repeater: sn.server.as_ref().unwrap(),
        now: w.now,
        battery_percent: Some(87),
        charging: false,
        tz_offset_min: 60,
    };
    let s = pages.screen(&v);
    // Both renderers take it.
    let mut oled = Oled::new();
    mc_ui::status::render(&s, &mut oled);
    s.lines
        .iter()
        .map(|l| format!("|{l:<21}|"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn every_page() {
    let w = world();
    let mut pages = Pages::new();
    let mut all = Vec::new();
    for _ in 0..6 {
        all.push(draw(&w, &pages));
        assert_eq!(pages.gesture(Click(Button::A)), None);
    }
    assert_eq!(pages.page(), Page::Home, "round");
    insta::assert_snapshot!("server_pages", all.join("\n\n"));
}

#[test]
fn long_press_acts_on_advert_and_off_only() {
    let mut pages = Pages::new();
    assert_eq!(pages.gesture(Long(Button::A)), None);
    for _ in 0..4 {
        pages.gesture(Click(Button::A));
    }
    assert_eq!(pages.page(), Page::Advert);
    assert_eq!(pages.gesture(Long(Button::A)), Some(Act::Advert));
    pages.done(Act::Advert, 1000);
    let w = world();
    assert!(draw(&w, &pages).contains("> Advert sent"));
    assert!(pages.tick(4000));
    assert_eq!(pages.gesture(Double(Button::A)), None);
    assert_eq!(pages.page(), Page::Neighbours);
    pages.gesture(Click(Button::A));
    pages.gesture(Click(Button::A));
    assert_eq!(pages.gesture(Long(Button::A)), Some(Act::PowerOff));
}
