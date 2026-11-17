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
        health: mc_ui::Health::default(),
        gps_searching: None,
        rx: None,
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

/// About says why the device last started, and what went wrong since the power came on
/// (ADR-0013): nothing more than the start when all is well.
#[test]
fn about_shows_the_devices_health() {
    let w = world();
    let to_about = [Double(A), Long(A), Double(A), Long(A), Long(A)];
    let well = Ctx {
        health: mc_ui::Health {
            restart: "power on",
            ..Default::default()
        },
        ..ctx(&w)
    };
    let mut app = App::new(COMPACT, false);
    press(&mut app, &well, &to_about);
    let screen = text(&mut app, COMPACT, &well);
    assert!(
        screen.contains("Started") && screen.contains("power on"),
        "{screen}"
    );
    for absent in ["Errors", "Radio", "Storage"] {
        assert!(!screen.contains(absent), "{absent} in {screen}");
    }

    let unwell = Ctx {
        health: mc_ui::Health {
            restart: "after an error",
            panics: 2,
            panic_at: Some(("firmware/esp32s3/src/run/radio.rs", 259)),
            radio_recoveries: 1,
            radio_down: false,
            store_errors: 3,
        },
        ..ctx(&w)
    };
    let screen = text(&mut app, COMPACT, &unwell);
    for line in [
        "after an error",
        "2 radio.rs:259",
        "1 reset|",
        "3 write errors",
    ] {
        assert!(screen.contains(line), "{line} not in {screen}");
    }
    let down = Ctx {
        health: mc_ui::Health {
            radio_down: true,
            radio_recoveries: 4,
            ..unwell.health
        },
        ..ctx(&w)
    };
    assert!(text(&mut app, COMPACT, &down).contains("not answering"));
}

/// Until something sets the clock, no screen shows a time of day: the node counts from its
/// floor, which is not the time.
#[test]
fn an_unset_clock_shows_no_time() {
    let mut w = world();
    let now = w.now;
    w.nodes[0].dev.node.rtc = mc_node::clock::Rtc::new(mc_sim::EPOCH, now);
    {
        let c = ctx(&w);
        let mut app = App::new(LARGE, false);
        let home = text(&mut app, LARGE, &c);
        assert!(home.contains("--:--"), "{home}");
        assert!(home.contains("Clock not set"), "{home}");
        let mut panel = App::new(LARGE, true);
        draw_panel(&mut panel, LARGE, &c);
        let texts = body_texts(&panel);
        assert!(texts.iter().any(|t| t == "--:--"), "{texts:?}");
        assert!(texts.iter().any(|t| t == "Clock not set"), "{texts:?}");
    }
    // An app or the GPS sets it: the time shows.
    w.nodes[0].dev.node.rtc.set(now, mc_sim::EPOCH + 30);
    let c = ctx(&w);
    let mut app = App::new(LARGE, false);
    let home = text(&mut app, LARGE, &c);
    assert!(
        !home.contains("--:--") && !home.contains("Clock not set"),
        "{home}"
    );
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
            assert_eq!(body_overlays(&app).len(), 1, "the menu's scene");
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
        // The PIN hides the screen: presses do not act on it, the light still answers.
        press(&mut app, &c, &[Long(A), Click(A), Double(A), Double(B)]);
        assert_eq!(app.depth(), 1, "nothing opened behind the PIN");
        assert_eq!(press(&mut app, &c, &[Long(B)]), vec![Intent::ToggleLight]);
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

/// Home, the lists, a conversation, a contact, a channel, the menus and the pickers draw
/// their body in pixels; the tool pages stay text.
#[test]
fn main_screens_draw_their_body_in_pixels() {
    let w = world();
    let c = ctx(&w);
    let cases: [(&[Gesture], &[&str]); 20] = [
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
        // Tools and Settings, with each item's value on the right.
        (
            &[Click(A), Click(A), Click(A), Long(A)],
            &["Heard nodes", "Discover", "Signal", "Repeater admin"],
        ),
        (
            &[Double(A), Long(A)],
            &["Radio", "SF", "Identity", "Morgan", "Bluetooth", "Display"],
        ),
        // About, as labelled values.
        (
            &[Double(A), Long(A), Double(A), Long(A), Long(A)],
            &[
                "Name",
                "Firmware",
                "Frequency",
                "869.618 MHz",
                "Contacts",
                "Uptime",
            ],
        ),
        // Tools: heard nodes, Signal, the packet log, Airtime.
        (
            &[Click(A), Click(A), Click(A), Long(A), Long(A)],
            &["Bob", "Alice", "direct"],
        ),
        (
            &[
                Click(A),
                Click(A),
                Click(A),
                Long(A),
                Click(A),
                Click(A),
                Click(A),
                Click(A),
                Long(A),
            ],
            &[
                "Last",
                "Noise floor",
                "Packets",
                "Noise floor, last hour",
                "Packets a minute",
            ],
        ),
        (
            &[
                Click(A),
                Click(A),
                Click(A),
                Long(A),
                Click(A),
                Click(A),
                Click(A),
                Click(A),
                Click(A),
                Click(A),
                Long(A),
            ],
            &[
                "TX total",
                "Sent / received",
                "2/5",
                "Duty 0.00% of 10.0% (g3)",
            ],
        ),
        // GPS (9th tool), Discover (2nd), and first-boot setup again (System, 2nd).
        (
            &[
                Click(A),
                Click(A),
                Click(A),
                Long(A),
                Click(A),
                Click(A),
                Click(A),
                Click(A),
                Click(A),
                Click(A),
                Click(A),
                Click(A),
                Long(A),
            ],
            &["GPS off.", "Shared position", "none"],
        ),
        (
            &[Click(A), Click(A), Click(A), Long(A), Click(A), Long(A)],
            &["Hold A to find"],
        ),
        (
            &[Double(A), Long(A), Double(A), Long(A), Click(A), Long(A)],
            &["Radio settings shared", "Keep current", "SF8"],
        ),
        // Trace (3rd), ping & range (4th), repeater admin (8th): each on the first contact the
        // picker offers.
        (
            &[
                Click(A),
                Click(A),
                Click(A),
                Long(A),
                Click(A),
                Click(A),
                Long(A),
                Long(A),
            ],
            &["trace"],
        ),
        (
            &[
                Click(A),
                Click(A),
                Click(A),
                Long(A),
                Click(A),
                Click(A),
                Click(A),
                Long(A),
                Long(A),
            ],
            &["Answered", "Test", "SNR of the answers"],
        ),
        (
            &[
                Click(A),
                Click(A),
                Click(A),
                Long(A),
                Click(A),
                Click(A),
                Click(A),
                Click(A),
                Click(A),
                Click(A),
                Click(A),
                Long(A),
                Long(A),
            ],
            &["Not logged in.", "Login as guest", "Status", "Neighbours"],
        ),
        // A settings page (no icons), and a picker.
        (
            &[
                Double(A),
                Long(A),
                Click(A),
                Click(A),
                Click(A),
                Click(A),
                Long(A),
            ],
            &["Text size", "large", "Light off", "Invert"],
        ),
        (
            &[
                Double(A),
                Long(A),
                Click(A),
                Click(A),
                Click(A),
                Click(A),
                Long(A),
                Long(A),
            ],
            &["Large (25×15)", "Compact (33×20)"],
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
        // The map is still grid text (with a pixel overlay once there is a position): Tools →
        // Map (last), here with no position.
        let mut app = App::new(grid, true);
        press(
            &mut app,
            &c,
            &[Click(A), Click(A), Click(A), Long(A), Double(A), Long(A)],
        );
        draw_panel(&mut app, grid, &c);
        assert!(body_overlays(&app).is_empty(), "{:?}", body_overlays(&app));
    }
}

/// A channel that has nothing unread opens on its own page: a card like a contact's, with its
/// four actions as tiles.
#[test]
fn a_channel_page_is_a_card_with_its_actions() {
    let mut w = world();
    {
        let sn = &mut w.nodes[0];
        sn.dev
            .mark_read(mc_node::history::Peer::Channel(0), &mut sn.st);
    }
    let c = ctx(&w);
    for grid in [LARGE, COMPACT] {
        let mut app = App::new(grid, true);
        // Channels, past "Add #channel", open.
        press(
            &mut app,
            &c,
            &[Click(A), Click(A), Long(A), Click(A), Long(A)],
        );
        draw_panel(&mut app, grid, &c);
        let texts = body_texts(&app);
        for want in [
            "Public",
            "Channel · slot 0",
            "0 unread",
            "Notifies like a direct message",
            "Messages",
            "Write",
            "Mute",
            "Remove",
        ] {
            assert!(
                texts.iter().any(|t| t.contains(want)),
                "{want:?} missing from {texts:?}"
            );
        }
    }
    // Muted: the page says so, and the tile offers the way back.
    let muted = w.nodes[0].dev.node.prefs.ui.with_muted(0, true);
    {
        let sn = &mut w.nodes[0];
        sn.dev.set_ui(muted, &mut sn.st);
    }
    let c = ctx(&w);
    let mut app = App::new(LARGE, true);
    press(
        &mut app,
        &c,
        &[Click(A), Click(A), Long(A), Click(A), Long(A)],
    );
    draw_panel(&mut app, LARGE, &c);
    let texts = body_texts(&app);
    assert!(texts.iter().any(|t| t == "Unmute"), "{texts:?}");
    assert!(texts.iter().any(|t| t.starts_with("Muted")), "{texts:?}");
}

/// A menu's top items carry an icon, a settings page starts at the margin, and a picker marks
/// the current value with a check.
#[test]
fn menus_and_pickers_are_pixel_lists() {
    use mc_ui::scene::{Icon, Item};
    let w = world();
    let c = ctx(&w);
    let scene_of = |gestures: &[Gesture]| {
        let mut app = App::new(LARGE, true);
        press(&mut app, &c, gestures);
        draw_panel(&mut app, LARGE, &c);
        match body_overlays(&app).as_slice() {
            [overlay::Overlay::Scene { scene, .. }] => scene.clone(),
            other => panic!("{gestures:?}: {other:?}"),
        }
    };
    let icons = |sc: &mc_ui::scene::Scene| -> Vec<Icon> {
        sc.items
            .iter()
            .filter_map(|i| match i {
                Item::Icon { icon, .. } => Some(*icon),
                _ => None,
            })
            .collect()
    };
    let x_of = |sc: &mc_ui::scene::Scene, text: &str| {
        sc.items.iter().find_map(|i| match i {
            Item::Text { x, text: t, .. } if t == text => Some(*x),
            _ => None,
        })
    };
    let settings = scene_of(&[Double(A), Long(A)]);
    assert_eq!(
        icons(&settings).first().copied(),
        Some(Icon::Broadcast),
        "Radio's icon"
    );
    assert!(icons(&settings).len() >= 6, "{:?}", icons(&settings));
    assert_eq!(x_of(&settings, "Radio"), Some(32), "past the icon");
    let display = scene_of(&[
        Double(A),
        Long(A),
        Click(A),
        Click(A),
        Click(A),
        Click(A),
        Long(A),
    ]);
    assert!(icons(&display).is_empty());
    assert_eq!(x_of(&display, "Text size"), Some(8), "at the margin");
    let picker = scene_of(&[
        Double(A),
        Long(A),
        Click(A),
        Click(A),
        Click(A),
        Click(A),
        Long(A),
        Long(A),
    ]);
    assert_eq!(icons(&picker), vec![Icon::Check], "the current value only");
    assert_eq!(
        x_of(&picker, "Large (25×15)"),
        x_of(&picker, "Compact (33×20)"),
        "aligned past the check"
    );
}

/// On the panel the hold-to-confirm popup is a card over the faded screen, which stays drawn
/// in pixels under it; its bar fills while A is held.
#[test]
fn the_confirm_popup_is_a_card_over_the_faded_screen() {
    use mc_ui::scene::Item;
    let w = world();
    let c = ctx(&w);
    for grid in [LARGE, COMPACT] {
        let mut app = App::new(grid, true);
        // Settings → System (last) → Reboot (third).
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
        draw_panel(&mut app, grid, &c);
        let body = body_overlays(&app);
        let [
            overlay::Overlay::Scene { scene: under, .. },
            overlay::Overlay::Over { scene, .. },
        ] = body.as_slice()
        else {
            panic!("the menu, then the card: {body:?}");
        };
        assert!(under.texts().contains(&"Reboot"), "the menu is still there");
        assert!(scene.items.iter().any(|i| matches!(i, Item::Dither { .. })));
        let texts = scene.texts();
        for want in [
            "Reboot",
            "Restart the device.",
            "Hold A to confirm",
            "2.0 s",
        ] {
            assert!(texts.contains(&want), "{want:?} not in {texts:?}");
        }
        let filled = |sc: &mc_ui::scene::Scene| {
            sc.items
                .iter()
                .filter_map(|i| match i {
                    Item::Rect {
                        w,
                        h: 9,
                        fill: Some(true),
                        ..
                    } => Some(*w),
                    _ => None,
                })
                .next()
        };
        assert_eq!(filled(scene), None, "an empty bar");
        // Half the hold: half the bar, and the time left.
        press(&mut app, &c, &[Held(A, 1000)]);
        draw_panel(&mut app, grid, &c);
        let body = body_overlays(&app);
        let Some(overlay::Overlay::Over { scene, .. }) = body.last() else {
            panic!("no card: {body:?}");
        };
        assert!(scene.texts().contains(&"1.0 s"), "{:?}", scene.texts());
        let bar = filled(scene).expect("a bar");
        assert!((80..=95).contains(&bar), "about half of 169: {bar}");
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
        // Over Home (pixels), and over a page that stays text: Tools → Map (last).
        for (gestures, under) in [
            (&[][..], 1),
            (
                &[Click(A), Click(A), Click(A), Long(A), Double(A), Long(A)][..],
                0,
            ),
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

/// While the GPS searches, Home and the GPS tool say how many satellites it uses (an open sky
/// reads differently from a room), and the noise floor shows no figure before its first round.
#[test]
fn a_searching_gps_counts_its_satellites() {
    let mut w = world();
    w.nodes[0].dev.info.has_gps = true;
    w.nodes[0].dev.node.prefs.gps_enabled = true;
    w.nodes[0].dev.noise_floor = 0;
    let searching = Ctx {
        gps: None,
        gps_searching: Some(4),
        ..ctx(&w)
    };
    let mut home = App::new(LARGE, true);
    draw_panel(&mut home, LARGE, &searching);
    let texts = body_texts(&home);
    assert!(texts.iter().any(|t| t == "No fix, 4 sats"), "{texts:?}");
    assert!(texts.iter().any(|t| t == "-- dBm"), "{texts:?}");
    // A runner that cannot count says only that there is no fix.
    let unknown = Ctx {
        gps_searching: None,
        ..searching
    };
    draw_panel(&mut home, LARGE, &unknown);
    assert!(body_texts(&home).iter().any(|t| t == "GPS, no fix"));

    // Tools (4th on Home's launcher) → GPS (9th tool).
    let mut app = App::new(LARGE, false);
    press(
        &mut app,
        &searching,
        &[Click(A), Click(A), Click(A), Long(A)],
    );
    press(&mut app, &searching, &[Click(A); 8]);
    press(&mut app, &searching, &[Long(A)]);
    let tool = text(&mut app, LARGE, &searching);
    assert!(
        tool.contains("Searching") && tool.contains("4 satellites"),
        "{tool}"
    );
    assert!(
        !tool.contains("hold"),
        "nothing to hold without a fix: {tool}"
    );
    assert!(text(&mut app, LARGE, &unknown).contains("Waiting for a fix"));
    w.nodes[0].dev.node.prefs.gps_enabled = false;
    let off = Ctx {
        gps: None,
        ..ctx(&w)
    };
    assert!(text(&mut app, LARGE, &off).contains("GPS off."));
}

/// The Signal tool shows what the radio chip itself received: readable packets, and ones too
/// weak or too noisy to read (something was on the air).
#[test]
fn the_signal_tool_counts_what_the_chip_received() {
    let w = world();
    let c = Ctx {
        rx: Some(mc_ui::RxSeen {
            ok: 412,
            damaged: 15,
        }),
        ..ctx(&w)
    };
    for grid in [LARGE, COMPACT] {
        let mut app = App::new(grid, false);
        // Tools → Signal (5th tool).
        press(&mut app, &c, &[Click(A), Click(A), Click(A), Long(A)]);
        press(&mut app, &c, &[Click(A); 4]);
        press(&mut app, &c, &[Long(A)]);
        let s = text(&mut app, grid, &c);
        assert!(s.contains("412 ok, 15 bad"), "{s}");
        // Both charts are still there under the extra line.
        assert!(
            s.contains("Noise floor, 1 h") && s.contains("Packets per minute"),
            "{s}"
        );
        // A runner that does not count shows no such line.
        let none = Ctx { rx: None, ..c };
        assert!(!text(&mut app, grid, &none).contains("Radio"));
    }
}

/// While typing, B deletes one character per press, however fast: presses the recognizer
/// counted as a double click or more never go Home with the text, and the press that cancels
/// the entry is the last one taken.
#[test]
fn quick_deletes_never_throw_the_text_away() {
    let w = world();
    let c = ctx(&w);
    let mut app = App::new(LARGE, false);
    // Messages, Bob, write; insert "OK" (the first quick reply) twice over.
    press(&mut app, &c, &[Long(A), Click(A), Long(A), Long(A)]);
    text(&mut app, LARGE, &c);
    assert!(app.typing());
    let typing_depth = app.depth();
    press(&mut app, &c, &[Long(A)]);
    assert_eq!(app.keyboard().unwrap().text, "OK");
    // A double click of B: two deletes, still typing.
    press(&mut app, &c, &[Double(B)]);
    assert_eq!(app.keyboard().unwrap().text, "");
    assert_eq!(app.depth(), typing_depth);
    // Four presses on an empty text: the first cancels the entry, the rest are dropped.
    press(&mut app, &c, &[Quad(B)]);
    assert!(!app.typing());
    assert_eq!(
        app.depth(),
        typing_depth - 1,
        "back on the conversation only"
    );
    // Outside text entry a double click of B still goes Home.
    press(&mut app, &c, &[Double(B)]);
    assert_eq!(app.depth(), 1);
}

/// Message previews off: the banner, Home and the lists show no text ("Sender only"), and
/// with "Hidden" the banner and Home's card name nobody; an opened conversation shows all.
#[test]
fn message_previews_hide_the_text_and_the_sender() {
    use mc_node::Previews;
    use mc_node::device::UiEvent;
    use mc_node::history::Peer;
    for mode in [Previews::Sender, Previews::Hidden] {
        let mut w = world();
        w.nodes[0].dev.node.prefs.ui.previews = mode;
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
        let c = ctx(&w);
        for grid in [LARGE, COMPACT] {
            // On the panel: the banner over Home, then the Messages list.
            let mut app = App::new(grid, true);
            app.event(&e, &c);
            draw_panel(&mut app, grid, &c);
            let shown = body_texts(&app).join(" | ");
            assert!(!shown.contains("Coffee"), "{mode:?}: {shown}");
            assert!(shown.contains("Message hidden"), "{mode:?}: {shown}");
            if mode == Previews::Hidden {
                assert!(!shown.contains("Bob"), "{mode:?}: {shown}");
                assert!(shown.contains("New message"), "{mode:?}: {shown}");
            }
            press(&mut app, &c, &[Click(B)]);
            press(&mut app, &c, &[Long(A)]);
            draw_panel(&mut app, grid, &c);
            let list = body_texts(&app).join(" | ");
            assert!(
                !list.contains("Coffee") && !list.contains("Snow"),
                "{mode:?}: {list}"
            );
            // Opened, the conversation shows the message.
            press(&mut app, &c, &[Click(A), Long(A)]);
            draw_panel(&mut app, grid, &c);
            assert!(body_texts(&app).iter().any(|t| t.contains("Coffee")));
            // The text version (terminal) too.
            let mut app = App::new(grid, false);
            app.event(&e, &c);
            let screen = text(&mut app, grid, &c);
            assert!(!screen.contains("Coffee"), "{mode:?}:\n{screen}");
            if mode == Previews::Hidden {
                assert!(!screen.contains("New message: Bob"), "{mode:?}:\n{screen}");
            }
        }
    }
}

/// Home's shortcuts: B ×3 cycles the buzzer, B ×4 the message previews, each with a toast;
/// elsewhere they go Home like a double click.
#[test]
fn home_shortcuts_cycle_the_buzzer_and_the_previews() {
    use mc_node::Previews;
    for (from, to, toast) in [
        (2u8, 0u8, "Buzzer: off"),
        (0, 1, "Buzzer: quiet"),
        (1, 2, "Buzzer: on"),
    ] {
        let mut w = world();
        w.nodes[0].dev.node.prefs.ui.buzzer = from;
        let c = ctx(&w);
        let mut app = App::new(LARGE, true);
        let mut u = w.nodes[0].dev.node.prefs.ui;
        u.buzzer = to;
        assert_eq!(press(&mut app, &c, &[Triple(B)]), vec![Intent::SetUi(u)]);
        assert_eq!(app.toast_text(), Some(toast));
    }
    for (from, to, toast) in [
        (Previews::Shown, Previews::Sender, "Previews: sender only"),
        (Previews::Sender, Previews::Hidden, "Previews: hidden"),
        (Previews::Hidden, Previews::Shown, "Previews: shown"),
    ] {
        let mut w = world();
        w.nodes[0].dev.node.prefs.ui.previews = from;
        let c = ctx(&w);
        let mut app = App::new(LARGE, true);
        let mut u = w.nodes[0].dev.node.prefs.ui;
        u.previews = to;
        assert_eq!(press(&mut app, &c, &[Quad(B)]), vec![Intent::SetUi(u)]);
        assert_eq!(app.toast_text(), Some(toast));
    }
    // Elsewhere B ×3 and B ×4 go Home, changing nothing.
    let w = world();
    let c = ctx(&w);
    let mut app = App::new(LARGE, true);
    for g in [Triple(B), Quad(B)] {
        press(&mut app, &c, &[Click(A), Long(A)]);
        text(&mut app, LARGE, &c);
        assert!(app.depth() > 1);
        assert!(press(&mut app, &c, &[g]).is_empty());
        assert_eq!(app.depth(), 1);
    }
}

/// While previews are not shown, a lock shows in the header.
#[test]
fn the_header_shows_a_lock_while_previews_are_off() {
    use mc_node::Previews;
    use mc_ui::scene::{Icon, Item};
    for (mode, lock) in [
        (Previews::Shown, false),
        (Previews::Sender, true),
        (Previews::Hidden, true),
    ] {
        let mut w = world();
        w.nodes[0].dev.node.prefs.ui.previews = mode;
        let c = ctx(&w);
        let mut app = App::new(LARGE, true);
        draw_panel(&mut app, LARGE, &c);
        let header = app.overlays().iter().rev().nth(1).cloned();
        let Some(overlay::Overlay::Scene { scene, .. }) = header else {
            panic!("no header scene");
        };
        let has = scene.items.iter().any(|i| {
            matches!(
                i,
                Item::Icon {
                    icon: Icon::Lock,
                    ..
                }
            )
        });
        assert_eq!(has, lock, "{mode:?}");
    }
}
