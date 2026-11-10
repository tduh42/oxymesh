//! Status pages of a repeater (`docs/spec/ui.md` §13): the same one-button pages as the
//! companion's on the Heltec V4's OLED (§12), and on the M5's e-paper in a larger font. A
//! click shows the next page, a double click (or the M5's other button) the previous one, a
//! long press acts on the page.
//! Everything else is done from the console (USB, or an admin over the air).
//!
//! Pages are built as text ([`Screen`]), drawn by [`crate::status::render`] on the OLED and by
//! [`render_epd`] on the e-paper.

use alloc::format;
use alloc::string::String;

use embedded_graphics::Drawable;
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::mono_font::ascii::{FONT_9X15, FONT_9X15_BOLD};
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::{DrawTarget, Point, Primitive};
use embedded_graphics::primitives::{Line, PrimitiveStyle};
use embedded_graphics::text::Text;
use mc_core::Instant;
use mc_node::Node;
use mc_server::{Role, Server};

use crate::clock;
use crate::input::{Button, Gesture};
use crate::status::{COLS, ROWS, Screen};

/// How long an acknowledgement stays, ms.
const NOTE_MS: u64 = 2500;

/// The pages, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Home,
    Radio,
    Traffic,
    Neighbours,
    Advert,
    Off,
}

const PAGES: [Page; 6] = [
    Page::Home,
    Page::Radio,
    Page::Traffic,
    Page::Neighbours,
    Page::Advert,
    Page::Off,
];

/// What a long press asks the runner for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    /// A flood advert, now.
    Advert,
    PowerOff,
}

/// What the pages show, gathered by the runner.
#[derive(Debug)]
pub struct View<'a> {
    pub node: &'a Node,
    pub server: &'a Server,
    pub now: Instant,
    pub battery_percent: Option<u8>,
    pub charging: bool,
    pub tz_offset_min: i16,
}

#[derive(Debug, Default)]
pub struct Pages {
    page: usize,
    note: Option<(String, u64)>,
}

impl Pages {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn page(&self) -> Page {
        PAGES.get(self.page).copied().unwrap_or(Page::Home)
    }

    /// One gesture of the board's button (reported as button A).
    pub fn gesture(&mut self, g: Gesture) -> Option<Act> {
        match g {
            Gesture::Click(Button::A) => {
                self.page = (self.page + 1) % PAGES.len();
                self.note = None;
                None
            }
            Gesture::Double(Button::A) | Gesture::Click(Button::B) => {
                self.page = (self.page + PAGES.len() - 1) % PAGES.len();
                self.note = None;
                None
            }
            Gesture::Long(Button::A) | Gesture::HeldEnd(Button::A) => match self.page() {
                Page::Advert => Some(Act::Advert),
                Page::Off => Some(Act::PowerOff),
                _ => None,
            },
            _ => None,
        }
    }

    /// The runner did it.
    pub fn done(&mut self, a: Act, now_ms: u64) {
        if a == Act::Advert {
            self.note = Some((String::from("Advert sent"), now_ms + NOTE_MS));
        }
    }

    /// Is something on screen going to change by itself (a note expiring)?
    pub fn tick(&mut self, now_ms: u64) -> bool {
        if self.note.as_ref().is_some_and(|(_, t)| now_ms >= *t) {
            self.note = None;
            return true;
        }
        false
    }

    /// The page as text.
    pub fn screen(&self, v: &View<'_>) -> Screen {
        let mut lines: [String; ROWS] = Default::default();
        lines[0] = header(v, self.page);
        let body = match self.page() {
            Page::Home => home(v),
            Page::Radio => radio(v),
            Page::Traffic => traffic(v),
            Page::Neighbours if v.server.role == Role::Room => posts(v),
            Page::Neighbours => neighbours(v),
            Page::Advert => [
                String::from("Advert"),
                String::from("Long press: send"),
                String::from("one, flooded."),
                String::new(),
                String::new(),
            ],
            Page::Off => [
                String::from("Power off"),
                String::from("Long press: off."),
                String::from("Press to wake."),
                String::new(),
                String::new(),
            ],
        };
        for (l, b) in lines.iter_mut().skip(1).zip(body) {
            *l = fit(&b);
        }
        if let Some((n, _)) = &self.note {
            lines[ROWS - 1] = fit(&format!("> {n}"));
        }
        Screen { lines, big: None }
    }
}

fn fit(s: &str) -> String {
    s.chars().take(COLS).collect()
}

fn name_of(b: &[u8; 32]) -> String {
    String::from_utf8_lossy(mc_node::contacts::name_str(b)).into_owned()
}

/// `Relay Col     1/6 87%`.
fn header(v: &View<'_>, page: usize) -> String {
    let battery = match v.battery_percent {
        Some(p) if v.charging => format!("+{p}%"),
        Some(p) => format!("{p}%"),
        None if v.charging => String::from("USB"),
        None => String::new(),
    };
    let right = format!("{}/{} {battery}", page + 1, PAGES.len());
    let room = COLS.saturating_sub(right.chars().count() + 1);
    let name: String = name_of(&v.node.prefs.name).chars().take(room).collect();
    let pad = COLS.saturating_sub(name.chars().count() + right.chars().count());
    format!("{name}{}{right}", " ".repeat(pad))
}

fn duration(s: u32) -> String {
    match s {
        0..3600 => format!("{} min", s / 60),
        3600..86_400 => format!("{} h {:02} min", s / 3600, s / 60 % 60),
        _ => format!("{} d {} h", s / 86_400, s / 3600 % 24),
    }
}

fn home(v: &View<'_>) -> [String; 5] {
    let t = clock::local(v.node.rtc.now(v.now), v.tz_offset_min);
    let fwd = v.node.routing.as_ref().is_some_and(|r| !r.disable_fwd);
    let acl = v
        .server
        .acl
        .clients
        .iter()
        .filter(|c| c.permissions != 0)
        .count();
    match v.server.role {
        Role::Repeater => [
            format!("Repeater   {}", clock::hhmm(t)),
            String::from(if fwd { "Forwarding" } else { "Forwarding off" }),
            format!("Up {}", duration(v.server.uptime_secs(v.now))),
            format!("Logins {acl}  Nbrs {}", v.server.neighbours.iter().count()),
            String::new(),
        ],
        Role::Room => [
            format!("Room server {}", clock::hhmm(t)),
            format!(
                "Posts {}  pushed {}",
                v.server.room.posted, v.server.room.pushes
            ),
            format!("Up {}", duration(v.server.uptime_secs(v.now))),
            format!("Logins {acl}"),
            String::new(),
        ],
    }
}

/// The room's newest posts, newest first: `ABCD1234 text`.
fn posts(v: &View<'_>) -> [String; 5] {
    let mut out: [String; 5] = Default::default();
    let all: alloc::vec::Vec<_> = v.server.room.iter().collect();
    out[0] = format!("Posts {}", all.len());
    for (slot, p) in out.iter_mut().skip(1).zip(all.iter().rev()) {
        *slot = format!("{:02X}{:02X} {}", p.author[0], p.author[1], p.text);
    }
    if all.is_empty() {
        out[1] = String::from("No posts yet");
    }
    out
}

fn radio(v: &View<'_>) -> [String; 5] {
    let r = &v.node.prefs.radio;
    let s = &v.node.disp.stats;
    [
        format!("{}.{:03} MHz", r.freq_khz / 1000, r.freq_khz % 1000),
        format!(
            "BW {}.{} SF{} CR{}",
            r.bw_hz / 1000,
            r.bw_hz % 1000 / 100,
            r.sf,
            r.cr
        ),
        format!("TX {} dBm", r.tx_power_dbm),
        format!("Noise {} dBm", v.server.board.noise_floor),
        format!(
            "Air TX {} s RX {} s",
            s.tx_airtime_ms / 1000,
            s.rx_airtime_ms / 1000
        ),
    ]
}

fn traffic(v: &View<'_>) -> [String; 5] {
    let s = &v.node.disp.stats;
    [
        String::from("Packets  flood direct"),
        format!("Heard  {:>6} {:>6}", s.recv_flood, s.recv_direct),
        format!("Sent   {:>6} {:>6}", s.sent_flood, s.sent_direct),
        format!(
            "Dups   {:>6} {:>6}",
            v.node.seen.flood_dups, v.node.seen.direct_dups
        ),
        format!("Last {} dBm", v.node.last_rssi),
    ]
}

/// The repeaters heard directly, newest first: `ABCD1234 -6.5 3m`.
fn neighbours(v: &View<'_>) -> [String; 5] {
    let mut out: [String; 5] = Default::default();
    let sorted = v.server.neighbours.sorted(0);
    out[0] = format!("Neighbours {}", sorted.len());
    let clock = v.node.rtc.now(v.now);
    for (slot, n) in out.iter_mut().skip(1).zip(sorted.iter()) {
        let ago = clock.wrapping_sub(n.heard);
        let when = if ago < 3600 {
            format!("{}m", ago / 60)
        } else {
            format!("{}h", ago / 3600)
        };
        let snr = i32::from(n.snr4) * 10 / 4;
        let sign = if snr < 0 { "-" } else { "" };
        *slot = format!(
            "{:02X}{:02X}{:02X}{:02X} {sign}{}.{} {when}",
            n.key[0],
            n.key[1],
            n.key[2],
            n.key[3],
            snr.unsigned_abs() / 10,
            snr.unsigned_abs() % 10
        );
    }
    if sorted.is_empty() {
        out[1] = String::from("None heard yet");
    }
    out
}

/// Draws a page on the 200×200 e-paper: 9×15 font, the header in bold over a rule.
pub fn render_epd<D: DrawTarget<Color = BinaryColor>>(s: &Screen, f: &mut D) {
    let _ = f.clear(BinaryColor::Off);
    let bold = MonoTextStyle::new(&FONT_9X15_BOLD, BinaryColor::On);
    let normal = MonoTextStyle::new(&FONT_9X15, BinaryColor::On);
    let _ = Text::new(&s.lines[0], Point::new(4, 18), bold).draw(f);
    let _ = Line::new(Point::new(0, 26), Point::new(199, 26))
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 2))
        .draw(f);
    for (i, l) in s.lines.iter().enumerate().skip(1) {
        let y = 50 + 30 * (i as i32 - 1);
        let _ = Text::new(l, Point::new(4, y), normal).draw(f);
    }
}
