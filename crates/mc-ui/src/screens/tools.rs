//! Tools (`docs/spec/ui.md` §6): what the radio hears, how well links work, where everyone is.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use mc_node::contacts::{Contact, PATH_UNKNOWN};
use mc_node::heard::HeardNode;
use ratatui_core::layout::{Alignment, Rect};
use ratatui_core::style::Color;
use ratatui_core::symbols::Marker;
use ratatui_core::terminal::Frame;
use ratatui_core::text::Line as TextLine;
use ratatui_widgets::canvas::{Canvas, Circle, Line};
use ratatui_widgets::list::ListState;
use ratatui_widgets::paragraph::{Paragraph, Wrap};
use ratatui_widgets::sparkline::Sparkline;

use super::contact::kind_name;
use super::home::heard_name;
use super::pixel::{self, Badge, Dot, Row, rich_list};
use super::{draw_list, name_of, rows, short_key, step};
use crate::act::{Intent, Outcome};
use crate::app::{Input, Nav, Screen, Ui};
use crate::chrome::{self, Footer};
use crate::scene::{Align, Font, Icon, Scene};
use crate::{clock, geo, regions, theme};

/// Signal strength as bars, from SNR × 4.
pub fn bars(snr4: i8) -> &'static str {
    match snr4 / 4 {
        10.. => "▂▄▆█",
        5..=9 => "▂▄▆ ",
        0..=4 => "▂▄  ",
        -7..=-1 => "▂   ",
        _ => "    ",
    }
}

fn snr_text(snr4: i8) -> String {
    let s = i16::from(snr4);
    let sign = if s < 0 { "-" } else { "" };
    let a = s.unsigned_abs();
    format!("{sign}{}.{} dB", a / 4, a % 4 * 25 / 10)
}

fn hops_text(h: u8) -> String {
    match h {
        0 => String::from("direct"),
        1 => String::from("1 hop"),
        n => format!("{n} hops"),
    }
}

/// A node's badge by its type: an icon for a repeater, a room or a sensor, initials otherwise
/// (as in Contacts).
fn kind_badge(kind: u8, name: &str) -> Badge {
    match kind {
        2 => Badge::Icon(Icon::Broadcast),
        3 => Badge::Icon(Icon::People),
        4 => Badge::Icon(Icon::Signal),
        _ => Badge::Initials(pixel::initials(name)),
    }
}

/// Our position: the GPS fix, else the one set in the settings.
pub(crate) fn my_position(ui: &Ui<'_>) -> Option<(i32, i32)> {
    if let Some(g) = ui.ctx.gps {
        return Some((g.lat, g.lon));
    }
    let p = &ui.ctx.dev.node.prefs;
    (p.lat != 0 || p.lon != 0).then_some((p.lat, p.lon))
}

pub(crate) fn position_of(c: &Contact) -> Option<(i32, i32)> {
    (c.lat != 0 || c.lon != 0).then_some((c.lat, c.lon))
}

fn lines_to(f: &mut Frame<'_>, area: Rect, lines: Vec<String>) {
    let l: Vec<TextLine<'_>> = lines.into_iter().map(TextLine::from).collect();
    f.render_widget(Paragraph::new(l), area);
}

// ------------------------------------------------------------------ heard nodes

#[derive(Debug, Default)]
pub struct HeardNodes {
    state: ListState,
}

impl Screen for HeardNodes {
    fn title(&self, ui: &Ui<'_>) -> String {
        format!("Heard {}", ui.ctx.dev.heard().len())
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        let heard = ui.ctx.dev.heard().sorted();
        if heard.is_empty() {
            f.render_widget(
                Paragraph::new("Nothing heard yet. Nodes appear when their adverts come in.")
                    .wrap(Wrap { trim: true }),
                rows(body, 1, body.height.saturating_sub(1)),
            );
            return;
        }
        let rows_: Vec<(String, String)> = heard
            .iter()
            .map(|n| {
                let tag = if n.kind == 2 { "R " } else { "" };
                let hops = if n.hops == 0 {
                    String::from("d")
                } else {
                    format!("{}h", n.hops)
                };
                (
                    format!("{tag}{}", heard_name(ui, n)),
                    format!("{} {hops}", bars(n.snr4)),
                )
            })
            .collect();
        draw_list(f, body, &rows_, &mut self.state, ui);
    }

    /// On the panel: a row per node, repeaters first, with its route, signal and age.
    fn scene(&mut self, w: u16, h: u16, ui: &Ui<'_>) -> Option<Scene> {
        let c = ui.ctx;
        let heard = c.dev.heard().sorted();
        let mut sc = Scene::new();
        if heard.is_empty() {
            sc.text(
                pixel::MARGIN,
                24,
                Font::B10,
                Align::Left,
                true,
                "Nothing heard yet.",
            );
            pixel::note(&mut sc, w, 28, "Nodes appear when their adverts come in.");
            return Some(sc);
        }
        let rows_: Vec<Row> = heard
            .iter()
            .map(|n| {
                let name = heard_name(ui, n);
                Row {
                    badge: Some(kind_badge(n.kind, &name)),
                    title: name,
                    bold: false,
                    right: clock::ago(n.at, c.unix),
                    sub: Some(format!("{} · {}", hops_text(n.hops), snr_text(n.snr4))),
                    dot: Dot::None,
                }
            })
            .collect();
        let sel = self.state.selected().unwrap_or(0).min(rows_.len() - 1);
        Some(rich_list(ui, w, h, &rows_, sel))
    }

    fn input(&mut self, input: Input, ui: &Ui<'_>) -> Nav {
        let heard = ui.ctx.dev.heard().sorted();
        if step(&mut self.state, heard.len(), input)
            && let Some(n) = heard.get(self.state.selected().unwrap_or(0))
        {
            return Nav::Push(Box::new(HeardDetail { key: n.key }));
        }
        Nav::Stay
    }
}

#[derive(Debug)]
pub struct HeardDetail {
    key: [u8; 32],
}

impl HeardDetail {
    fn node<'a>(&self, ui: &Ui<'a>) -> Option<&'a HeardNode> {
        ui.ctx
            .dev
            .heard()
            .sorted()
            .into_iter()
            .find(|n| n.key == self.key)
    }
}

impl Screen for HeardDetail {
    fn title(&self, ui: &Ui<'_>) -> String {
        self.node(ui)
            .map(|n| heard_name(ui, n))
            .unwrap_or_else(|| short_key(&self.key))
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        let Some(n) = self.node(ui) else {
            return;
        };
        let w = usize::from(body.width);
        let (rows_, note) = heard_detail(ui, n);
        let mut l: Vec<String> = rows_.iter().map(|(k, v)| chrome::spread(k, v, w)).collect();
        l.push(String::new());
        l.push(String::from(note));
        lines_to(f, body, l);
    }

    /// On the panel: the badge and name, then the same values.
    fn scene(&mut self, w: u16, _h: u16, ui: &Ui<'_>) -> Option<Scene> {
        let mut sc = Scene::new();
        let Some(n) = self.node(ui) else {
            sc.text(
                pixel::MARGIN,
                24,
                Font::B10,
                Align::Left,
                true,
                "No longer heard.",
            );
            return Some(sc);
        };
        let (rows_, note) = heard_detail(ui, n);
        let y = pixel::fields(&mut sc, w, 0, &rows_);
        pixel::note(&mut sc, w, y + 2, note);
        Some(sc)
    }

    fn footer(&self, _ui: &Ui<'_>) -> Footer {
        Footer::BACK
    }
}

/// What is known of a heard node, and whether it is a contact.
fn heard_detail(ui: &Ui<'_>, n: &HeardNode) -> (Vec<(&'static str, String)>, &'static str) {
    let c = ui.ctx;
    let mut l: Vec<(&'static str, String)> = alloc::vec![
        ("Type", String::from(kind_name(n.kind))),
        ("Key", short_key(&n.key)),
        ("Route", hops_text(n.hops)),
        (
            "Signal",
            format!("{} {}", bars(n.snr4).trim_end(), snr_text(n.snr4))
        ),
        (
            "Heard",
            format!(
                "{} {}",
                n.count,
                if n.count == 1 { "time" } else { "times" }
            ),
        ),
        ("Last", clock::stamp(n.at, c.unix, c.tz_offset_min)),
    ];
    if let Some(p) = n.location {
        l.push(("Lat", geo::coord(p.0, true)));
        l.push(("Lon", geo::coord(p.1, false)));
        if let Some(me) = my_position(ui) {
            let d = geo::distance_m(me, p);
            let b = geo::bearing_deg(me, p);
            l.push((
                "From here",
                format!("{} {}", geo::distance_text(d), geo::compass(b)),
            ));
        }
    }
    let note = if c.dev.node.contacts.find(&n.key).is_some() {
        "In your contacts."
    } else {
        "Not in your contacts."
    };
    (l, note)
}

// ------------------------------------------------------------------ ping & range test

/// Pings a contact every 5 s (telemetry requests), and counts.
#[derive(Debug)]
pub struct RangeTest {
    key: [u8; 32],
    running: bool,
    pending: Option<u32>,
    asked: bool,
    last_ms: u64,
    /// (answered, rtt ms, snr4), oldest first.
    results: Vec<(bool, u32, i8)>,
}

const RANGE_EVERY_MS: u64 = 5_000;

impl RangeTest {
    pub fn new(key: [u8; 32]) -> Self {
        Self {
            key,
            running: true,
            pending: None,
            asked: false,
            last_ms: 0,
            results: Vec::new(),
        }
    }

    fn collect(&mut self, ui: &Ui<'_>) {
        if let Some(tag) = self.pending
            && let Some(r) = ui.ctx.dev.request_reply(tag)
        {
            self.results.push((r.ok, r.rtt_ms, r.snr4));
            if self.results.len() > 200 {
                self.results.remove(0);
            }
            self.pending = None;
        }
    }
}

impl Screen for RangeTest {
    fn title(&self, ui: &Ui<'_>) -> String {
        let c = &ui.ctx.dev.node.contacts;
        let name = c
            .find(&self.key)
            .and_then(|s| c.get(s))
            .map(|k| name_of(&k.name))
            .unwrap_or_default();
        format!("Range {name}")
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        self.collect(ui);
        let w = usize::from(body.width);
        let sent = self.results.len();
        let ok: Vec<&(bool, u32, i8)> = self.results.iter().filter(|r| r.0).collect();
        let pct = (ok.len() * 100).checked_div(sent).unwrap_or(0);
        let avg = if ok.is_empty() {
            0
        } else {
            ok.iter().map(|r| r.1).sum::<u32>() / ok.len() as u32
        };
        let last = self.results.last();
        let state = if !self.running {
            "paused"
        } else if self.pending.is_some() {
            "waiting…"
        } else {
            "running"
        };
        let l = alloc::vec![
            chrome::spread("Answered", &format!("{}/{sent} {pct}%", ok.len()), w),
            chrome::spread(
                "RTT",
                &match last {
                    Some((true, rtt, _)) => format!(
                        "{}.{:02} s avg {}.{:02}",
                        rtt / 1000,
                        rtt % 1000 / 10,
                        avg / 1000,
                        avg % 1000 / 10
                    ),
                    Some((false, _, _)) => String::from("lost"),
                    None => String::from("-"),
                },
                w,
            ),
            chrome::spread(
                "SNR",
                &ok.last()
                    .map(|r| snr_text(r.2))
                    .unwrap_or_else(|| String::from("-")),
                w,
            ),
            chrome::spread("Test", state, w),
        ];
        lines_to(f, rows(body, 0, 4), l);
        // Hits and misses, the most recent on the right.
        let strip: String = self
            .results
            .iter()
            .rev()
            .take(w)
            .rev()
            .map(|r| if r.0 { '█' } else { '·' })
            .collect();
        f.render_widget(Paragraph::new(strip), rows(body, 5, 1));
        f.render_widget(Paragraph::new("SNR of the answers"), rows(body, 7, 1));
        let snr: Vec<u64> = ok
            .iter()
            .rev()
            .take(w)
            .rev()
            .map(|r| (i64::from(r.2) / 4 + 20).max(0) as u64)
            .collect();
        f.render_widget(
            Sparkline::default().data(snr),
            rows(body, 8, body.height.saturating_sub(8)),
        );
    }

    fn input(&mut self, input: Input, _ui: &Ui<'_>) -> Nav {
        if input == Input::Open {
            self.running = !self.running;
        }
        Nav::Stay
    }

    fn tick(&mut self, ui: &Ui<'_>) -> Nav {
        self.collect(ui);
        let now = ui.ctx.now_ms;
        if self.running
            && self.pending.is_none()
            && !self.asked
            && (self.last_ms == 0 || now >= self.last_ms + RANGE_EVERY_MS)
        {
            self.last_ms = now;
            self.asked = true;
            return Nav::Intent(Intent::Ping(self.key));
        }
        Nav::Stay
    }

    fn result(&mut self, intent: &Intent, outcome: &Outcome, _ui: &Ui<'_>) -> Nav {
        if let Intent::Ping(_) = intent {
            self.asked = false;
            match outcome {
                Outcome::Request(tag) => self.pending = Some(*tag),
                _ => self.results.push((false, 0, 0)),
            }
        }
        Nav::Stay
    }

    fn footer(&self, _ui: &Ui<'_>) -> Footer {
        Footer {
            a_click: None,
            a_hold: Some(if self.running { "pause" } else { "start" }),
            a_double: None,
            b_click: Some("back"),
            b_hold: None,
        }
    }
}

// ------------------------------------------------------------------ trace route

/// Traces the route to a contact and back, with the SNR at each hop.
#[derive(Debug)]
pub struct Trace {
    key: [u8; 32],
    tag: Option<u32>,
    asked: bool,
}

impl Trace {
    pub fn new(key: [u8; 32]) -> Self {
        Self {
            key,
            tag: None,
            asked: false,
        }
    }

    /// The hashes there and back, and their size code; `None` if nothing can be traced.
    fn path(&self, ui: &Ui<'_>) -> Option<(u8, Vec<u8>)> {
        let cs = &ui.ctx.dev.node.contacts;
        let c = cs.find(&self.key).and_then(|s| cs.get(s))?;
        let code = if c.out_path_len == PATH_UNKNOWN {
            0
        } else {
            c.out_path_len >> 6
        };
        let size = 1usize << code;
        let hops = if c.out_path_len == PATH_UNKNOWN {
            0
        } else {
            usize::from(c.out_path_len & 63)
        };
        let route: Vec<&[u8]> = c.out_path.get(..hops * size)?.chunks(size).collect();
        let mut out: Vec<u8> = Vec::new();
        for h in &route {
            out.extend_from_slice(h);
        }
        // A repeater answers the trace itself; otherwise turn at the last repeater.
        if c.kind == 2 {
            out.extend_from_slice(c.pub_key.get(..size)?);
            for h in route.iter().rev() {
                out.extend_from_slice(h);
            }
        } else {
            if route.is_empty() {
                return None;
            }
            for h in route.iter().rev().skip(1) {
                out.extend_from_slice(h);
            }
        }
        Some((code, out))
    }
}

impl Screen for Trace {
    fn title(&self, _ui: &Ui<'_>) -> String {
        String::from("Trace route")
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        let w = usize::from(body.width);
        let Some((code, path)) = self.path(ui) else {
            f.render_widget(
                Paragraph::new(
                    "Nothing to trace: this contact is reached directly and does not repeat.",
                )
                .wrap(Wrap { trim: true }),
                rows(body, 1, body.height.saturating_sub(1)),
            );
            return;
        };
        let size = 1usize << code;
        let reply = self.tag.and_then(|t| ui.ctx.dev.request_reply(t));
        let hops = path.len() / size;
        let mut l: Vec<String> = alloc::vec![format!(
            "{hops} {} there and back",
            if hops == 1 { "hop" } else { "hops" }
        )];
        match reply {
            None if self.tag.is_some() => l.push(String::from("Tracing…")),
            None => l.push(String::from("Hold A to trace.")),
            Some(r) if !r.ok => l.push(String::from("No answer.")),
            Some(r) => {
                // Data: hash count, hashes, SNR per hop passed, final SNR.
                let d = &r.data;
                let n = usize::from(d.first().copied().unwrap_or(0));
                let hashes = d.get(1..1 + n).unwrap_or(&[]);
                let snrs = d.get(1 + n..d.len().saturating_sub(1)).unwrap_or(&[]);
                for (i, (h, s)) in hashes.chunks(size).zip(snrs.iter()).enumerate() {
                    let s4 = *s as i8;
                    let who = name_by_hash(ui, h).unwrap_or_else(|| format!("{}", mc_core::Hex(h)));
                    l.push(chrome::spread(
                        &format!("{} {}", i + 1, who),
                        &format!("{} {}", bars(s4), snr_text(s4)),
                        w,
                    ));
                }
                let fin = d.last().copied().unwrap_or(0) as i8;
                l.push(chrome::spread(
                    "Back here",
                    &format!("{} {}", bars(fin), snr_text(fin)),
                    w,
                ));
                l.push(format!(
                    "Round trip {}.{:02} s",
                    r.rtt_ms / 1000,
                    r.rtt_ms % 1000 / 10
                ));
            }
        }
        lines_to(f, body, l);
    }

    fn input(&mut self, input: Input, ui: &Ui<'_>) -> Nav {
        if input == Input::Open
            && !self.asked
            && let Some((code, path)) = self.path(ui)
        {
            self.asked = true;
            return Nav::Intent(Intent::Trace { flags: code, path });
        }
        Nav::Stay
    }

    fn result(&mut self, intent: &Intent, outcome: &Outcome, _ui: &Ui<'_>) -> Nav {
        if let (Intent::Trace { .. }, Outcome::Request(tag)) = (intent, outcome) {
            self.tag = Some(*tag);
        }
        self.asked = false;
        Nav::Stay
    }

    fn footer(&self, _ui: &Ui<'_>) -> Footer {
        Footer {
            a_click: None,
            a_hold: Some("trace"),
            a_double: None,
            b_click: Some("back"),
            b_hold: None,
        }
    }
}

/// A contact or heard node whose key starts with `h`.
fn name_by_hash(ui: &Ui<'_>, h: &[u8]) -> Option<String> {
    let dev = ui.ctx.dev;
    if let Some(c) = dev
        .node
        .contacts
        .list
        .iter()
        .find(|c| c.pub_key.starts_with(h))
    {
        return Some(name_of(&c.name));
    }
    dev.heard()
        .sorted()
        .into_iter()
        .find(|n| n.key.starts_with(h))
        .map(|n| heard_name(ui, n))
}

// ------------------------------------------------------------------ signal

/// The Signal tool's values: the last packet, the noise floor, the packet rate, and what the
/// radio chip itself received since boot (readable, and "bad": too weak or too noisy to read,
/// but something was on the air).
fn signal_rows(ui: &Ui<'_>) -> Vec<(&'static str, String)> {
    let dev = ui.ctx.dev;
    let mins = dev.minutes();
    let per_min = mins.iter().last().map(|m| m.rx).unwrap_or(0);
    let hour: u32 = mins.iter().map(|m| u32::from(m.rx)).sum();
    let mut l = alloc::vec![
        (
            "Last",
            match dev.packets().recent().next() {
                Some(p) => format!("{} {} dBm", snr_text(p.snr4), p.rssi),
                None => String::from("-"),
            },
        ),
        ("Noise floor", super::noise_text(dev.noise_floor)),
        ("Packets", format!("{per_min}/min {hour}/h")),
    ];
    if let Some(rx) = ui.ctx.rx {
        l.push(("Radio", format!("{} ok, {} bad", rx.ok, rx.damaged)));
    }
    l
}

/// The last hour, a value a minute: the noise floor (dBm above −140) and the packets received.
fn signal_history(ui: &Ui<'_>) -> (Vec<u64>, Vec<u64>) {
    let mins = ui.ctx.dev.minutes();
    let noise = mins
        .iter()
        .map(|m| (i64::from(m.noise) + 140).max(0) as u64)
        .collect();
    let rx = mins.iter().map(|m| u64::from(m.rx)).collect();
    (noise, rx)
}

#[derive(Debug)]
pub struct Signal;

impl Screen for Signal {
    fn title(&self, _ui: &Ui<'_>) -> String {
        String::from("Signal")
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        let dev = ui.ctx.dev;
        let w = usize::from(body.width);
        let mins: Vec<_> = dev.minutes().iter().copied().collect();
        let l: Vec<String> = signal_rows(ui)
            .iter()
            .map(|(k, v)| chrome::spread(k, v, w))
            .collect();
        let n = l.len() as u16;
        lines_to(f, rows(body, 0, n), l);
        // The two charts share what is left under the lines and a blank row.
        let body = rows(body, n - 3, body.height.saturating_sub(n - 3));
        let half = body.height.saturating_sub(4) / 2;
        f.render_widget(Paragraph::new("Noise floor, 1 h"), rows(body, 4, 1));
        let noise: Vec<u64> = mins
            .iter()
            .map(|m| (i64::from(m.noise) + 140).max(0) as u64)
            .collect();
        f.render_widget(
            Sparkline::default().data(noise),
            rows(body, 5, half.saturating_sub(1)),
        );
        f.render_widget(
            Paragraph::new("Packets per minute"),
            rows(body, 4 + half, 1),
        );
        let rx: Vec<u64> = mins.iter().map(|m| u64::from(m.rx)).collect();
        f.render_widget(
            Sparkline::default().data(rx),
            rows(body, 5 + half, body.height.saturating_sub(5 + half)),
        );
    }

    /// On the panel: the values, then the last hour as two bar charts.
    fn scene(&mut self, w: u16, h: u16, ui: &Ui<'_>) -> Option<Scene> {
        let mut sc = Scene::new();
        let y = pixel::fields(&mut sc, w, 0, &signal_rows(ui));
        let (noise, rx) = signal_history(ui);
        // Two charts with their titles share the rest of the body.
        let chart_h = ((h as i16 - y - 4 - 2 * 13) / 2).clamp(16, 48) as u16;
        let y = pixel::chart(&mut sc, w, y + 2, chart_h, "Noise floor, last hour", &noise);
        pixel::chart(&mut sc, w, y + 2, chart_h, "Packets a minute", &rx);
        Some(sc)
    }

    fn footer(&self, _ui: &Ui<'_>) -> Footer {
        Footer::BACK
    }
}

// ------------------------------------------------------------------ packet log

fn type_name(t: u8) -> &'static str {
    match t {
        0 => "REQ",
        1 => "RESP",
        2 => "TXT",
        3 => "ACK",
        4 => "ADV",
        5 => "GTXT",
        6 => "GDAT",
        7 => "ANON",
        8 => "PATH",
        9 => "TRCE",
        10 => "MULT",
        11 => "CTRL",
        15 => "RAW",
        _ => "?",
    }
}

#[derive(Debug, Default)]
pub struct Packets {
    state: ListState,
}

impl Screen for Packets {
    fn title(&self, ui: &Ui<'_>) -> String {
        format!("Packets {}", ui.ctx.dev.packets().total)
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        let c = ui.ctx;
        let rows_: Vec<(String, String)> = c
            .dev
            .packets()
            .recent()
            .map(|p| {
                let t = clock::hhmm(clock::local(p.at, c.tz_offset_min));
                if !p.valid {
                    return (format!("{t} ??? {}B", p.len), snr_text(p.snr4));
                }
                let route = match p.route {
                    0 | 1 => "F",
                    _ => "D",
                };
                (
                    format!("{t} {:4} {route}{}", type_name(p.ptype), p.hops),
                    snr_text(p.snr4),
                )
            })
            .collect();
        if rows_.is_empty() {
            f.render_widget(Paragraph::new("No packet yet."), rows(body, 1, 1));
            return;
        }
        draw_list(f, body, &rows_, &mut self.state, ui);
    }

    /// On the panel: a line a packet, newest first: time, type, route and hops, its SNR.
    fn scene(&mut self, w: u16, h: u16, ui: &Ui<'_>) -> Option<Scene> {
        let c = ui.ctx;
        let rows_: Vec<Row> = c
            .dev
            .packets()
            .recent()
            .map(|p| {
                let t = clock::hhmm(clock::local(p.at, c.tz_offset_min));
                let what = if p.valid {
                    let route = match p.route {
                        0 | 1 => "flood",
                        _ => "direct",
                    };
                    format!("{t}  {} · {route} {}", type_name(p.ptype), p.hops)
                } else {
                    format!("{t}  ? · {} B", p.len)
                };
                Row::item(None, &what, snr_text(p.snr4))
            })
            .collect();
        let mut sc = Scene::new();
        if rows_.is_empty() {
            sc.text(
                pixel::MARGIN,
                24,
                Font::B10,
                Align::Left,
                true,
                "No packet yet.",
            );
            return Some(sc);
        }
        let sel = self.state.selected().unwrap_or(0).min(rows_.len() - 1);
        sc = rich_list(ui, w, h, &rows_, sel);
        Some(sc)
    }

    fn input(&mut self, input: Input, ui: &Ui<'_>) -> Nav {
        let n = ui.ctx.dev.packets().recent().count();
        let _ = step(&mut self.state, n, input);
        Nav::Stay
    }

    fn footer(&self, _ui: &Ui<'_>) -> Footer {
        Footer {
            a_click: Some("next"),
            a_hold: None,
            a_double: None,
            b_click: Some("back"),
            b_hold: None,
        }
    }
}

// ------------------------------------------------------------------ airtime

#[derive(Debug)]
pub struct Airtime;

fn secs(ms: u64) -> String {
    format!("{}.{} s", ms / 1000, ms % 1000 / 100)
}

const NO_DUTY_RULE: &str = "No duty-cycle rule for this frequency.";

/// The Airtime tool's values.
fn airtime_rows(ui: &Ui<'_>) -> Vec<(&'static str, String)> {
    let dev = ui.ctx.dev;
    let s = dev.node.disp.stats;
    alloc::vec![
        ("TX total", secs(s.tx_airtime_ms)),
        ("RX total", secs(s.rx_airtime_ms)),
        ("TX last hour", secs(dev.minutes().tx_ms())),
        ("Queue", format!("{}", dev.tx_queue())),
        (
            "Sent / received",
            format!(
                "{}/{}",
                s.sent_flood + s.sent_direct,
                s.recv_flood + s.recv_direct
            ),
        ),
    ]
}

/// The share of the last hour spent transmitting, in hundredths of a percent.
fn duty_used(ui: &Ui<'_>) -> u32 {
    (ui.ctx.dev.minutes().tx_ms() * 10_000 / 3_600_000) as u32
}

fn duty_text(used: u32, band: &str, limit: u32) -> String {
    format!(
        "Duty {}.{:02}% of {}.{:01}% ({band})",
        used / 100,
        used % 100,
        limit / 100,
        limit % 100 / 10
    )
}

impl Screen for Airtime {
    fn title(&self, _ui: &Ui<'_>) -> String {
        String::from("Airtime")
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        let dev = ui.ctx.dev;
        let w = usize::from(body.width);
        let used = duty_used(ui);
        let mut l: Vec<String> = airtime_rows(ui)
            .iter()
            .map(|(k, v)| chrome::spread(k, v, w))
            .collect();
        l.push(String::new());
        match regions::duty_rule(dev.node.prefs.radio.freq_khz) {
            Some((band, limit)) => {
                l.push(duty_text(used, &band, limit));
                let len = w;
                let filled = (len as u64 * u64::from(used) / u64::from(limit.max(1)))
                    .min(len as u64) as usize;
                l.push(format!(
                    "{}{}",
                    "█".repeat(filled),
                    ui.sym.dot.repeat(len - filled)
                ));
            }
            None => l.push(String::from(NO_DUTY_RULE)),
        }
        lines_to(f, body, l);
    }

    /// On the panel: the values, then the share of the hour used against the band's limit as
    /// a gauge.
    fn scene(&mut self, w: u16, _h: u16, ui: &Ui<'_>) -> Option<Scene> {
        let mut sc = Scene::new();
        let y = pixel::fields(&mut sc, w, 0, &airtime_rows(ui));
        match regions::duty_rule(ui.ctx.dev.node.prefs.radio.freq_khz) {
            Some((band, limit)) => {
                let used = duty_used(ui);
                let y = y + 14;
                sc.text(
                    pixel::MARGIN,
                    y,
                    Font::R8,
                    Align::Left,
                    true,
                    &duty_text(used, &band, limit),
                );
                let bw = w.saturating_sub(8);
                sc.rect(4, y + 4, bw, 11, 3, None, Some(true));
                let fill = (u64::from(bw.saturating_sub(4)) * u64::from(used)
                    / u64::from(limit.max(1)))
                .min(u64::from(bw.saturating_sub(4))) as u16;
                if fill > 0 {
                    sc.rect(6, y + 6, fill, 7, 1, Some(true), None);
                }
            }
            None => {
                pixel::note(&mut sc, w, y + 2, NO_DUTY_RULE);
            }
        }
        Some(sc)
    }

    fn footer(&self, _ui: &Ui<'_>) -> Footer {
        Footer::BACK
    }
}

// ------------------------------------------------------------------ GPS

#[derive(Debug, Default)]
pub struct Gps {
    state: ListState,
}

/// Contacts with a position, the nearest first: (name, distance m, bearing).
fn by_distance(ui: &Ui<'_>, me: (i32, i32)) -> Vec<(String, f32, u16)> {
    let mut v: Vec<(String, f32, u16)> = ui
        .ctx
        .dev
        .node
        .contacts
        .list
        .iter()
        .filter_map(|c| {
            let p = position_of(c)?;
            Some((
                name_of(&c.name),
                geo::distance_m(me, p),
                geo::bearing_deg(me, p),
            ))
        })
        .collect();
    v.sort_by(|a, b| a.1.total_cmp(&b.1));
    v
}

impl Screen for Gps {
    fn title(&self, _ui: &Ui<'_>) -> String {
        String::from("GPS")
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        let w = usize::from(body.width);
        let p = &ui.ctx.dev.node.prefs;
        let mut l = match ui.ctx.gps {
            Some(g) => alloc::vec![
                chrome::spread("Fix", &format!("{} satellites", g.satellites), w),
                chrome::spread("Lat", &geo::coord(g.lat, true), w),
                chrome::spread("Lon", &geo::coord(g.lon, false), w),
            ],
            None => alloc::vec![match (ui.ctx.dev.gps_active(), ui.ctx.gps_searching) {
                (false, _) => String::from("GPS off."),
                // The count tells an open sky from a room.
                (true, Some(n)) => chrome::spread("Searching", &format!("{n} satellites"), w),
                (true, None) => String::from("Waiting for a fix…"),
            }],
        };
        l.push(chrome::spread(
            "Shared position",
            if p.lat == 0 && p.lon == 0 {
                "none"
            } else {
                "set"
            },
            w,
        ));
        let n = l.len() as u16;
        lines_to(f, rows(body, 0, n), l);
        let mut items: Vec<(String, String)> = Vec::new();
        if ui.ctx.gps.is_some() {
            items.push((String::from("Share this position"), String::new()));
        }
        if let Some(me) = my_position(ui) {
            for (name, d, b) in by_distance(ui, me) {
                items.push((
                    name,
                    format!("{} {}", geo::distance_text(d), geo::compass(b)),
                ));
            }
        }
        draw_list(
            f,
            rows(body, n + 1, body.height.saturating_sub(n + 1)),
            &items,
            &mut self.state,
            ui,
        );
    }

    fn input(&mut self, input: Input, ui: &Ui<'_>) -> Nav {
        let me = my_position(ui);
        let n =
            usize::from(ui.ctx.gps.is_some()) + me.map(|m| by_distance(ui, m).len()).unwrap_or(0);
        if step(&mut self.state, n, input)
            && self.state.selected() == Some(0)
            && let Some(g) = ui.ctx.gps
        {
            return Nav::IntentToast(
                Intent::SetLocation {
                    lat: g.lat,
                    lon: g.lon,
                },
                "Position shared in adverts",
            );
        }
        Nav::Stay
    }

    fn footer(&self, ui: &Ui<'_>) -> Footer {
        // Only "Share this position" does something on a hold, and it is there only with a fix.
        let share = ui.ctx.gps.is_some() && self.state.selected().unwrap_or(0) == 0;
        Footer {
            a_hold: share.then_some("share"),
            ..Footer::LIST
        }
    }
}

// ------------------------------------------------------------------ map

/// Half-widths of the map, meters.
const ZOOMS: [u32; 6] = [500, 2_000, 10_000, 50_000, 200_000, 1_000_000];

#[derive(Debug)]
pub struct Map {
    zoom: usize,
}

impl Default for Map {
    fn default() -> Self {
        Self { zoom: 2 }
    }
}

impl Screen for Map {
    fn title(&self, _ui: &Ui<'_>) -> String {
        let z = ZOOMS[self.zoom];
        format!("Map {}", geo::distance_text(z as f32))
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        let Some(me) = my_position(ui) else {
            f.render_widget(
                Paragraph::new("No position: no GPS fix and none set by the app.")
                    .wrap(Wrap { trim: true }),
                rows(body, 1, body.height.saturating_sub(1)),
            );
            return;
        };
        let span = f64::from(ZOOMS[self.zoom]);
        // Local east/north meters around us.
        let cos = f64::from(libm::cosf((me.0 as f32 / 1e6).to_radians()));
        let to_xy = |p: (i32, i32)| -> (f64, f64) {
            let dy = f64::from(p.0 - me.0) / 1e6 * 111_320.0;
            let dx = f64::from(p.1 - me.1) / 1e6 * 111_320.0 * cos;
            (dx, dy)
        };
        let mut pts: Vec<(String, (f64, f64))> = Vec::new();
        for c in &ui.ctx.dev.node.contacts.list {
            if let Some(p) = position_of(c) {
                pts.push((name_of(&c.name), to_xy(p)));
            }
        }
        for n in ui.ctx.dev.heard().sorted() {
            if let Some(p) = n.location
                && !pts.iter().any(|(_, xy)| *xy == to_xy(p))
            {
                pts.push((heard_name(ui, n), to_xy(p)));
            }
        }
        let visible = pts
            .iter()
            .filter(|(_, (x, y))| x.abs() <= span && y.abs() <= span)
            .count();
        let canvas = Canvas::default()
            .marker(Marker::Braille)
            .x_bounds([-span, span])
            .y_bounds([-span, span])
            .paint(|ctx| {
                // Markers big enough to see on the panel: a ring per node, a cross for us.
                let r = span / 12.0;
                ctx.draw(&Line {
                    x1: -r,
                    y1: 0.0,
                    x2: r,
                    y2: 0.0,
                    color: Color::Reset,
                });
                ctx.draw(&Line {
                    x1: 0.0,
                    y1: -r,
                    x2: 0.0,
                    y2: r,
                    color: Color::Reset,
                });
                ctx.draw(&Circle {
                    x: 0.0,
                    y: 0.0,
                    radius: r * 0.7,
                    color: Color::Reset,
                });
                ctx.print(r * 1.2, -r, TextLine::from("me"));
                for (name, (x, y)) in &pts {
                    ctx.draw(&Circle {
                        x: *x,
                        y: *y,
                        radius: r * 0.6,
                        color: Color::Reset,
                    });
                    ctx.draw(&Circle {
                        x: *x,
                        y: *y,
                        radius: r * 0.3,
                        color: Color::Reset,
                    });
                    let short: String = name.chars().take(8).collect();
                    ctx.print(*x + r * 0.9, *y, TextLine::from(short));
                }
            });
        f.render_widget(canvas, rows(body, 0, body.height.saturating_sub(1)));
        let hidden = pts.len() - visible;
        let info = if hidden > 0 {
            format!("{visible} shown, {hidden} out of range")
        } else {
            format!("{visible} shown")
        };
        f.render_widget(
            Paragraph::new(info)
                .alignment(Alignment::Right)
                .style(theme::label()),
            rows(body, body.height.saturating_sub(1), 1),
        );
    }

    fn paints(&self, body: Rect, ui: &Ui<'_>) -> Vec<(Rect, crate::app::Paint)> {
        let Some(me) = my_position(ui) else {
            return Vec::new();
        };
        let span = ZOOMS[self.zoom] as f32;
        let cos = libm::cosf((me.0 as f32 / 1e6).to_radians());
        let scale = |p: (i32, i32)| -> Option<(i16, i16)> {
            let n = (p.0 - me.0) as f32 / 1e6 * 111_320.0 / span * 1000.0;
            let e = (p.1 - me.1) as f32 / 1e6 * 111_320.0 * cos / span * 1000.0;
            (e.abs() <= 1000.0 && n.abs() <= 1000.0).then_some((e as i16, n as i16))
        };
        // The same nodes as the text version and its count: contacts, then nodes heard with
        // a position that no contact has.
        let mut seen: Vec<(i32, i32)> = Vec::new();
        let mut pts: Vec<(String, i16, i16)> = Vec::new();
        for c in &ui.ctx.dev.node.contacts.list {
            if let Some(p) = position_of(c) {
                seen.push(p);
                if let Some((e, n)) = scale(p) {
                    pts.push((name_of(&c.name).chars().take(10).collect(), e, n));
                }
            }
        }
        for h in ui.ctx.dev.heard().sorted() {
            if let Some(p) = h.location
                && !seen.contains(&p)
            {
                seen.push(p);
                if let Some((e, n)) = scale(p) {
                    pts.push((heard_name(ui, h).chars().take(10).collect(), e, n));
                }
            }
        }
        alloc::vec![(
            rows(body, 0, body.height.saturating_sub(1)),
            crate::app::Paint::Map(pts)
        )]
    }

    fn input(&mut self, input: Input, _ui: &Ui<'_>) -> Nav {
        match input {
            Input::Next(_) => self.zoom = (self.zoom + 1).min(ZOOMS.len() - 1),
            Input::Prev => self.zoom = self.zoom.saturating_sub(1),
            Input::Open => self.zoom = 2,
        }
        Nav::Stay
    }

    fn footer(&self, _ui: &Ui<'_>) -> Footer {
        Footer {
            a_click: Some("out"),
            a_hold: Some("reset"),
            a_double: Some("in"),
            b_click: Some("back"),
            b_hold: None,
        }
    }
}
