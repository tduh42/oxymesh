//! The interface end to end on a simulated device: navigation, popups, overlays on the panel,
//! and a text snapshot of every screen at both grid sizes.

use mc_core::Duration;
use mc_ui::Intent;
use mc_ui::app::{App, CONFIRM_MS, Ctx};
use mc_ui::input::{Button, Gesture};
use mc_ui::panel::{self, COMPACT, Frame, Grid, LARGE};
use mc_ui::{overlay, screens};
use ratatui_core::backend::TestBackend;
use ratatui_core::terminal::Terminal;

use Button::{A, B};
use Gesture::*;

/// "Morgan" with two contacts heard (Alice, Bob), 2026-11-05 around 18:3x in Paris; Alice
/// wrote on the Public channel, then Bob wrote a DM.
fn world() -> mc_sim::World {
    let mut w = mc_sim::World::new(5);
    let me = w.add("Morgan", |_| {});
    for name in ["Alice", "Bob"] {
        let i = w.add(name, |_| {});
        w.link(me, i);
        let now = w.now;
        w.node(i).send_self_advert(now, false).unwrap();
        w.run(Duration::from_secs(3));
    }
    // Morgan's advert, so Bob can write to them.
    let now = w.now;
    w.node(me).send_self_advert(now, false).unwrap();
    w.run(Duration::from_secs(3));
    let now = w.now;
    let ts = w.node(1).time(now);
    w.node(1)
        .send_channel_text(now, 0, ts, "Snow on the pass, take chains".as_bytes())
        .unwrap();
    w.run(Duration::from_secs(5));
    let now = w.now;
    let ts = w.node(2).time(now);
    let to: [u8; 6] = w.node(me).id.pub_key()[..6].try_into().unwrap();
    w.node(2)
        .send_text(
            now,
            &to,
            0,
            0,
            ts,
            "Coffee at the refuge around 4?".as_bytes(),
        )
        .unwrap();
    w.run(Duration::from_secs(5));
    assert_eq!(
        w.nodes[me].dev.history().unread(),
        2,
        "Bob's DM and Alice's channel message"
    );
    w
}

fn ctx(w: &mc_sim::World) -> Ctx<'_> {
    Ctx {
        dev: &w.nodes[0].dev,
        now_ms: 3_725_000,
        unix: mc_sim::EPOCH + 30,
        tz_offset_min: 60,
        battery_percent: Some(87),
        charging: false,
        light: false,
        ble: mc_ui::Ble::default(),
        gps: None,
    }
}

fn text(app: &mut App, grid: Grid, c: &Ctx<'_>) -> String {
    let mut t = Terminal::new(TestBackend::new(grid.cols, grid.rows)).unwrap();
    t.draw(|f| app.draw(f, c)).unwrap();
    let buf = t.backend().buffer();
    (0..grid.rows)
        .map(|y| {
            let line: String = (0..grid.cols).map(|x| buf[(x, y)].symbol()).collect();
            format!("|{line}|")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn press(app: &mut App, c: &Ctx<'_>, gs: &[Gesture]) -> Vec<Intent> {
    for g in gs {
        app.gesture(*g, c);
    }
    app.take_intents()
}

#[test]
fn back_pops_and_double_back_goes_home() {
    let w = world();
    let c = ctx(&w);
    let mut app = App::new(LARGE, false);
    // Home → Settings (5th) → System (last) → About (first).
    press(
        &mut app,
        &c,
        &[Double(A), Long(A), Double(A), Long(A), Long(A)],
    );
    assert_eq!(app.depth(), 4);
    press(&mut app, &c, &[Click(B)]);
    assert_eq!(app.depth(), 3);
    press(&mut app, &c, &[Double(B)]);
    assert_eq!(app.depth(), 1);
    press(&mut app, &c, &[Click(B)]);
    assert_eq!(app.depth(), 1, "B on Home does nothing");
    assert_eq!(press(&mut app, &c, &[Long(B)]), vec![Intent::ToggleLight]);
}

#[test]
fn hold_to_confirm() {
    let w = world();
    let c = ctx(&w);
    let mut app = App::new(LARGE, false);
    // Settings → System → Reboot.
    press(
        &mut app,
        &c,
        &[
            Double(A),
            Long(A),
            Double(A),
            Long(A),
            Click(A),
            Click(A),
            Long(A),
        ],
    );
    assert!(app.popup().is_some());
    // Released early: nothing, and the progress starts again.
    assert!(press(&mut app, &c, &[Held(A, 1000), Held(A, 1500), HeldEnd(A)]).is_empty());
    assert!(app.popup().is_some());
    // A click does not confirm either.
    assert!(press(&mut app, &c, &[Click(A), Long(A)]).is_empty());
    let got = press(&mut app, &c, &[Held(A, 1000), Held(A, CONFIRM_MS)]);
    assert_eq!(got, vec![Intent::Reboot]);
    assert!(app.popup().is_none());
    // B cancels another one.
    press(&mut app, &c, &[Click(A), Long(A)]);
    assert!(app.popup().is_some());
    press(&mut app, &c, &[Click(B)]);
    assert!(app.popup().is_none());
    assert_eq!(app.depth(), 3, "cancelling the popup stays on System");
}

#[test]
fn advert_from_identity_shows_a_toast() {
    let w = world();
    let c = ctx(&w);
    let mut app = App::new(LARGE, false);
    // Settings → Identity (2nd) → Advert, zero hop (4th).
    let got = press(
        &mut app,
        &c,
        &[
            Double(A),
            Long(A),
            Click(A),
            Long(A),
            Click(A),
            Click(A),
            Click(A),
            Long(A),
        ],
    );
    assert_eq!(got, vec![Intent::SendAdvert { flood: false }]);
    let screen = text(&mut app, LARGE, &c);
    assert!(screen.contains("|Advert sent"), "{screen}");
    let later = Ctx {
        now_ms: c.now_ms + 3_000,
        ..c
    };
    app.tick(&later);
    assert!(!text(&mut app, LARGE, &c).contains("Advert sent"));
}

#[test]
fn lists_wrap_and_a_hold_opens_once() {
    let w = world();
    let c = ctx(&w);
    let mut app = App::new(LARGE, false);
    // Home launcher: previous from the first row is the last row (Settings).
    press(&mut app, &c, &[Double(A)]);
    // Held: Settings opens once; the later ticks open nothing inside it.
    press(
        &mut app,
        &c,
        &[
            Held(A, 1000),
            Held(A, 1250),
            Held(A, 2000),
            Held(A, 4500),
            HeldEnd(A),
        ],
    );
    let screen = text(&mut app, LARGE, &c);
    assert!(screen.starts_with("|Settings"), "{screen}");
    // Back once: Home.
    press(&mut app, &c, &[Click(B)]);
    assert!(!text(&mut app, LARGE, &c).starts_with("|Settings"));
}

/// The hold that opens a hold-to-confirm popup never confirms it, however long it lasts; a
/// new hold does, and what is left of it does nothing more.
#[test]
fn a_hold_never_confirms_the_popup_it_opened() {
    let w = world();
    let c = ctx(&w);
    let mut app = App::new(LARGE, false);
    // Settings → System → Reboot, opened by holding.
    press(
        &mut app,
        &c,
        &[Double(A), Long(A), Double(A), Long(A), Click(A), Click(A)],
    );
    let got = press(
        &mut app,
        &c,
        &[
            Held(A, 1000),
            Held(A, CONFIRM_MS),
            Held(A, 5000),
            HeldEnd(A),
        ],
    );
    assert!(got.is_empty(), "{got:?}");
    assert!(app.popup().is_some());
    let got = press(
        &mut app,
        &c,
        &[
            Held(A, 1000),
            Held(A, CONFIRM_MS),
            Held(A, 3000),
            HeldEnd(A),
        ],
    );
    assert_eq!(got, vec![Intent::Reboot]);
    assert!(app.popup().is_none());
}

/// On the panel, leaving Home must not leave clock pixels behind: the frame after Home → Tools
/// is exactly a fresh Tools frame.
#[test]
fn overlays_never_go_stale_on_the_panel() {
    let w = world();
    let c = ctx(&w);
    for grid in [LARGE, COMPACT] {
        let mut app = App::new(grid, true);
        let mut frame = Frame::new();
        {
            let mut t = Terminal::new(panel::backend(&mut frame, grid)).unwrap();
            t.draw(|f| app.draw(f, &c)).unwrap();
            assert_eq!(body_overlays(&app).len(), 1, "the clock");
            for o in app.overlays() {
                overlay::draw(o, t.backend_mut().display_mut()).unwrap();
            }
            press(&mut app, &c, &[Click(A), Click(A), Click(A), Long(A)]);
            t.draw(|f| app.draw(f, &c)).unwrap();
            assert!(body_overlays(&app).is_empty());
            for o in app.overlays() {
                overlay::draw(o, t.backend_mut().display_mut()).unwrap();
            }
        }
        let mut fresh = Frame::new();
        {
            let mut app2 = App::new(grid, true);
            press(&mut app2, &c, &[Click(A), Click(A), Click(A), Long(A)]);
            let mut t = Terminal::new(panel::backend(&mut fresh, grid)).unwrap();
            t.draw(|f| app2.draw(f, &c)).unwrap();
            for o in app2.overlays() {
                overlay::draw(o, t.backend_mut().display_mut()).unwrap();
            }
        }
        assert!(frame == fresh, "{grid:?}: stale pixels after leaving Home");
    }
}

#[test]
fn snapshots() {
    let w = world();
    let c = ctx(&w);
    for grid in [LARGE, COMPACT] {
        for (name, gestures) in mc_ui::tour::TOUR {
            let mut app = App::new(grid, false);
            press(&mut app, &c, gestures);
            let size = if grid == LARGE { "large" } else { "compact" };
            insta::assert_snapshot!(format!("{name}_{size}"), text(&mut app, grid, &c));
        }
    }
}

#[test]
fn contact_link_matches_the_app_format() {
    let k = [0xABu8; 32];
    assert_eq!(
        screens::info_link("Zoé B", &k),
        format!(
            "meshcore://contact/add?name=Zo%C3%A9%20B&public_key={}&type=1",
            "ab".repeat(32)
        )
    );
}

fn ble(state: mc_ui::BleState) -> mc_ui::Ble {
    mc_ui::Ble {
        state,
        pin: 48_213,
        paired: 2,
    }
}

#[test]
fn pairing_covers_the_screen_with_the_pin() {
    let w = world();
    let mut c = ctx(&w);
    c.ble = ble(mc_ui::BleState::Pairing);
    for grid in [LARGE, COMPACT] {
        let size = if grid == LARGE { "large" } else { "compact" };
        let mut app = App::new(grid, false);
        let t = text(&mut app, grid, &c);
        assert!(t.contains("048 213"), "{t}");
        insta::assert_snapshot!(format!("ble_pairing_{size}"), t);
        // On the panel, big digits over those cells and nothing else painted.
        let mut app = App::new(grid, true);
        let mut frame = Frame::new();
        let mut term = Terminal::new(panel::backend(&mut frame, grid)).unwrap();
        term.draw(|f| app.draw(f, &c)).unwrap();
        assert!(
            matches!(
                body_overlays(&app).as_slice(),
                [overlay::Overlay::Pin { pin: 48_213, .. }]
            ),
            "{:?}",
            app.overlays()
        );
    }
}

#[test]
fn header_says_bt_while_a_phone_is_connected() {
    let w = world();
    let mut c = ctx(&w);
    let mut app = App::new(LARGE, false);
    press(&mut app, &c, &[Click(A), Long(A)]); // Contacts: a header with room
    c.ble = ble(mc_ui::BleState::Advertising);
    let first =
        |app: &mut App, c: &Ctx<'_>| text(app, LARGE, c).lines().next().unwrap().to_string();
    assert!(!first(&mut app, &c).contains("BT"));
    c.ble = ble(mc_ui::BleState::Connected);
    assert!(
        first(&mut app, &c).contains("BT"),
        "{}",
        first(&mut app, &c)
    );
}

#[test]
fn forgetting_paired_phones_needs_a_hold() {
    let w = world();
    let mut c = ctx(&w);
    c.ble = ble(mc_ui::BleState::Connected);
    let mut app = App::new(LARGE, false);
    // Home → Settings → Bluetooth (third) → Paired phones (fourth).
    let intents = press(
        &mut app,
        &c,
        &[
            Double(A),
            Long(A),
            Click(A),
            Click(A),
            Long(A),
            Click(A),
            Click(A),
            Click(A),
            Long(A),
        ],
    );
    assert!(intents.is_empty());
    assert!(app.popup().is_some(), "{}", text(&mut app, LARGE, &c));
    let intents = press(&mut app, &c, &[Held(A, 1000), Held(A, CONFIRM_MS)]);
    assert_eq!(intents, vec![Intent::ForgetPaired]);
}

#[test]
fn a_muted_channel_counts_quietly() {
    let mut w = world();
    let public = mc_node::history::Peer::Channel(0);
    // Public read once, so it opens on its own screen rather than its messages.
    {
        let sn = &mut w.nodes[0];
        sn.dev.mark_read(public, &mut sn.st);
    }
    let c = ctx(&w);
    let mut app = App::new(LARGE, false);
    // Home → Channels → Public → Mute (third).
    let intents = press(
        &mut app,
        &c,
        &[
            Click(A),
            Click(A),
            Long(A),
            Click(A),
            Long(A),
            Click(A),
            Click(A),
            Long(A),
        ],
    );
    let muted = w.nodes[0].dev.node.prefs.ui.with_muted(0, true);
    assert_eq!(intents, vec![Intent::SetUi(muted)]);
    {
        let sn = &mut w.nodes[0];
        sn.dev.set_ui(muted, &mut sn.st);
    }
    let c = ctx(&w);
    insta::assert_snapshot!("channel_large", text(&mut app, LARGE, &c));
    // Alice writes on Public again: its own count shows it, nothing else calls.
    let now = w.now;
    let ts = w.node(1).time(now);
    w.node(1)
        .send_channel_text(now, 0, ts, "Chains on, it's icy".as_bytes())
        .unwrap();
    w.run(Duration::from_secs(5));
    assert_eq!(w.nodes[0].dev.history().unread_with(public), 1);
    assert_eq!(w.nodes[0].dev.unread_alerting(), 1, "Bob's DM only");
    let c = ctx(&w);
    let mut app = App::new(LARGE, false);
    let _ = press(&mut app, &c, &[Click(A), Click(A), Long(A)]);
    insta::assert_snapshot!("channels_muted_large", text(&mut app, LARGE, &c));
    let mut app = App::new(LARGE, false);
    let _ = press(&mut app, &c, &[Long(A)]);
    insta::assert_snapshot!("messages_muted_large", text(&mut app, LARGE, &c));
}

#[test]
fn import_from_cpp_shows_what_it_brings_then_needs_a_hold() {
    const IMAGE: &[u8] = include_bytes!("../../../testdata/spiffs/cpp.bin");
    let w = world();
    let c = ctx(&w);
    let mut app = App::new(LARGE, false);
    // Home → Settings → System (last) → Import from C++ (last).
    let intents = press(
        &mut app,
        &c,
        &[Double(A), Long(A), Double(A), Long(A), Double(A), Long(A)],
    );
    assert_eq!(intents, vec![Intent::CppFind]);
    insta::assert_snapshot!("import_looking_large", text(&mut app, LARGE, &c));
    let mut fs = mc_spiffs::Spiffs::new(IMAGE, IMAGE.len() as u32).unwrap();
    let found = mc_spiffs::cpp::summary(&mut fs);
    app.done(&Intent::CppFind, &mc_ui::Outcome::Cpp(found), &c);
    insta::assert_snapshot!("import_found_large", text(&mut app, LARGE, &c));
    assert!(press(&mut app, &c, &[Long(A)]).is_empty());
    assert!(app.popup().is_some());
    let intents = press(&mut app, &c, &[Held(A, 1000), Held(A, CONFIRM_MS)]);
    assert_eq!(intents, vec![Intent::CppImport]);

    let mut app = App::new(LARGE, false);
    press(
        &mut app,
        &c,
        &[Double(A), Long(A), Double(A), Long(A), Double(A), Long(A)],
    );
    app.done(
        &Intent::CppFind,
        &mc_ui::Outcome::Cpp(Err(mc_spiffs::cpp::ImportError::NoIdentity)),
        &c,
    );
    insta::assert_snapshot!("import_nothing_large", text(&mut app, LARGE, &c));
    assert!(press(&mut app, &c, &[Long(A)]).is_empty());
    assert!(app.popup().is_none());
}

/// The overlays other than the header and footer (drawn on every panel frame).
fn body_overlays(app: &App) -> Vec<overlay::Overlay> {
    let n = app.overlays().len();
    app.overlays()
        .iter()
        .take(n.saturating_sub(2))
        .cloned()
        .collect()
}

/// The texts of the pixel scenes over the body (ADR-0011), banner card included.
fn body_texts(app: &App) -> Vec<String> {
    body_overlays(app)
        .iter()
        .flat_map(|o| match o {
            overlay::Overlay::Scene { scene, .. } | overlay::Overlay::Over { scene, .. } => {
                scene.texts().into_iter().map(String::from).collect()
            }
            _ => Vec::new(),
        })
        .collect()
}

fn draw_panel(app: &mut App, grid: Grid, c: &Ctx<'_>) {
    let mut t = Terminal::new(TestBackend::new(grid.cols, grid.rows)).unwrap();
    t.draw(|f| app.draw(f, c)).unwrap();
}

/// Home, the lists, a conversation and a contact draw their body in pixels; the rest stays
/// text.
#[test]
fn main_screens_draw_their_body_in_pixels() {
    let w = world();
    let c = ctx(&w);
    let cases: [(&[Gesture], &[&str]); 6] = [
        (
            &[],
            &["Messages", "Contacts", "Map", "Bob", "Coffee at the refuge"],
        ),
        (&[Long(A)], &["New message", "Bob", "Public", "now"]),
        (&[Long(A), Click(A), Long(A)], &["Coffee at the refuge"]),
        (&[Click(A), Long(A)], &["Alice", "Bob"]),
        (&[Click(A), Click(A), Long(A)], &["Add #channel", "Public"]),
        (
            &[Click(A), Long(A), Long(A)],
            &["Write", "Messages", "Ping", "Trace route", "Remove"],
        ),
    ];
    for grid in [LARGE, COMPACT] {
        for (gestures, want) in cases {
            let mut app = App::new(grid, true);
            press(&mut app, &c, gestures);
            draw_panel(&mut app, grid, &c);
            let body = body_overlays(&app);
            assert!(
                matches!(body.as_slice(), [overlay::Overlay::Scene { .. }]),
                "{gestures:?}: one scene for the body, got {body:?}"
            );
            let texts = body_texts(&app);
            for w in want {
                assert!(
                    texts.iter().any(|t| t.contains(w)),
                    "{gestures:?}: {w:?} missing from {texts:?}"
                );
            }
        }
        // Tools stays a text list.
        let mut app = App::new(grid, true);
        press(&mut app, &c, &[Click(A), Click(A), Click(A), Long(A)]);
        draw_panel(&mut app, grid, &c);
        assert!(body_overlays(&app).is_empty());
    }
}

/// A new direct message fades the screen and shows a card over it, pixel screen or text one.
#[test]
fn the_banner_is_a_card_over_the_faded_screen() {
    use mc_node::device::UiEvent;
    use mc_node::history::Peer;
    use mc_ui::scene::Item;
    let w = world();
    let c = ctx(&w);
    let bob = w.nodes[0]
        .dev
        .history()
        .iter()
        .filter(|m| matches!(m.peer, Peer::Contact(_)))
        .last()
        .unwrap();
    let e = UiEvent::Message {
        seq: bob.seq,
        peer: bob.peer,
        outgoing: false,
    };
    for grid in [LARGE, COMPACT] {
        for (gestures, under) in [
            (&[][..], 1),
            (&[Click(A), Click(A), Click(A), Long(A)][..], 0),
        ] {
            let mut app = App::new(grid, true);
            press(&mut app, &c, gestures);
            app.event(&e, &c);
            draw_panel(&mut app, grid, &c);
            let body = body_overlays(&app);
            assert_eq!(body.len(), under + 1, "{gestures:?}: {body:?}");
            let Some(overlay::Overlay::Over { scene, .. }) = body.last() else {
                panic!("{gestures:?}: no card: {body:?}");
            };
            assert!(scene.items.iter().any(|i| matches!(i, Item::Dither { .. })));
            let texts = scene.texts();
            assert!(texts.contains(&"Bob"), "{texts:?}");
            assert!(texts.iter().any(|t| t.contains("Coffee")), "{texts:?}");
            // Over a text screen the text banner's cells are blanked first.
            let blanks = matches!(
                scene.items.first(),
                Some(Item::Rect {
                    fill: Some(false),
                    ..
                })
            );
            assert_eq!(blanks, under == 0, "{gestures:?}");
        }
    }
}

/// A long conversation opens on the newest bubble and scrolls up by half a body.
#[test]
fn conversations_scroll_in_pixels() {
    let mut w = world();
    let to: [u8; 6] = w.node(0).id.pub_key()[..6].try_into().unwrap();
    for i in 0..8 {
        let now = w.now;
        let ts = w.node(2).time(now);
        let text = format!("Message number {i} with enough words to wrap on two lines");
        w.node(2)
            .send_text(now, &to, 0, 0, ts, text.as_bytes())
            .unwrap();
        w.run(Duration::from_secs(5));
    }
    let c = ctx(&w);
    for grid in [LARGE, COMPACT] {
        let mut app = App::new(grid, true);
        press(&mut app, &c, &[Long(A), Click(A), Long(A)]);
        draw_panel(&mut app, grid, &c);
        let newest = body_texts(&app);
        assert!(newest.iter().any(|t| t.contains("number 7")), "{newest:?}");
        assert!(!newest.iter().any(|t| t.contains("Coffee")), "{newest:?}");
        press(&mut app, &c, &[Double(A), Double(A), Double(A), Double(A)]);
        draw_panel(&mut app, grid, &c);
        let older = body_texts(&app);
        assert!(!older.iter().any(|t| t.contains("number 7")), "{older:?}");
        // Far enough up, the top: Bob's first message.
        for _ in 0..20 {
            press(&mut app, &c, &[Double(A)]);
        }
        draw_panel(&mut app, grid, &c);
        let top = body_texts(&app);
        assert!(top.iter().any(|t| t.contains("Coffee")), "{top:?}");
        // And back down to the newest.
        for _ in 0..30 {
            press(&mut app, &c, &[Click(A)]);
        }
        draw_panel(&mut app, grid, &c);
        assert_eq!(body_texts(&app), newest);
    }
}

/// In text entry a hold past the long press acts once as a long press; its later ticks do
/// nothing, even after the press changed the screen.
#[test]
fn holding_a_in_the_keyboard_inserts_once() {
    let w = world();
    let c = ctx(&w);
    let mut app = App::new(LARGE, false);
    // Messages, Bob, write: the first stop is the first quick reply, "OK".
    press(&mut app, &c, &[Long(A), Click(A), Long(A), Long(A)]);
    text(&mut app, LARGE, &c);
    assert_eq!(app.keyboard().unwrap().text, "");
    press(
        &mut app,
        &c,
        &[Held(A, 1000), Held(A, 1250), Held(A, 1500), HeldEnd(A)],
    );
    let kb = app.keyboard().unwrap();
    assert_eq!(kb.text, "OK", "one insertion");
    assert_eq!(kb.selected(), Some(&mc_ui::keyboard::Cand::Done));
    // Held again on Send: sent once, and the conversation underneath does not scroll.
    let intents = press(
        &mut app,
        &c,
        &[Held(A, 1000), Held(A, 1250), Held(A, 2500), HeldEnd(A)],
    );
    assert!(app.keyboard().is_none(), "back on the conversation");
    assert_eq!(
        intents
            .iter()
            .filter(|i| matches!(i, Intent::SendText { .. }))
            .count(),
        1,
        "{intents:?}"
    );
}
