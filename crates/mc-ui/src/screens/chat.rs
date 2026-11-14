//! Messages: the conversations, one conversation, and writing a message
//! (`docs/spec/ui.md` §6-7).

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use mc_node::history::{Msg, Peer, Status};
use ratatui_core::layout::{Alignment, Rect};
use ratatui_core::style::Style;
use ratatui_core::terminal::Frame;
use ratatui_core::text::Line;
use ratatui_widgets::list::ListState;
use ratatui_widgets::paragraph::{Paragraph, Wrap};

use super::pixel::{self, Badge, Dot, Row, initials, rich_list};
use super::{Contacts, draw_list, peer_name, rows, step};
use crate::act::{Intent, Outcome};
use crate::app::{Input, Nav, Screen, Ui};
use crate::chrome::{self, Footer};
use crate::clock;
use crate::keyboard::{Cand, Key, Keyboard, Mode, recent_words};
use crate::scene::{self, Align, Font, Icon, IconSize, Scene};
use crate::theme;

/// Longest message text (`MAX_TEXT_LEN`, 10 AES blocks).
pub const TEXT_MAX: usize = 160;

// ------------------------------------------------------------------ conversations

#[derive(Debug, Default)]
pub struct Messages {
    state: ListState,
}

/// Unread first (muted channels aside), then the most recent.
fn conversations(ui: &Ui<'_>) -> Vec<(Peer, String, String)> {
    let h = ui.ctx.dev.history();
    let prefs = &ui.ctx.dev.node.prefs.ui;
    let muted = |p: Peer| matches!(p, Peer::Channel(i) if prefs.is_muted(i));
    let mut v: Vec<(Peer, u32, usize)> = h
        .conversations()
        .iter()
        .map(|(p, m)| (*p, m.seq, h.unread_with(*p)))
        .collect();
    v.sort_by_key(|(p, seq, unread)| (*unread == 0 || muted(*p), core::cmp::Reverse(*seq)));
    v.into_iter()
        .map(|(p, seq, unread)| {
            let detail = if unread > 0 {
                let mark = if muted(p) {
                    ui.sym.quiet
                } else {
                    ui.sym.unread
                };
                format!("{mark}{unread}")
            } else {
                h.get(seq)
                    .map(|m| clock::stamp(m.at, ui.ctx.unix, ui.ctx.tz_offset_min))
                    .unwrap_or_default()
            };
            (p, peer_name(ui, p), detail)
        })
        .collect()
}

/// The conversations as pixel rows, in the same order as [`conversations`].
fn conversation_rows(ui: &Ui<'_>) -> Vec<Row> {
    let h = ui.ctx.dev.history();
    let prefs = &ui.ctx.dev.node.prefs.ui;
    let muted = |p: Peer| matches!(p, Peer::Channel(i) if prefs.is_muted(i));
    let mut v: Vec<(Peer, &Msg, usize)> = h
        .conversations()
        .iter()
        .map(|(p, m)| (*p, *m, h.unread_with(*p)))
        .collect();
    v.sort_by_key(|(p, m, unread)| (*unread == 0 || muted(*p), core::cmp::Reverse(m.seq)));
    v.into_iter()
        .map(|(p, m, unread)| {
            let name = peer_name(ui, p);
            let text = String::from_utf8_lossy(&m.text);
            let preview = if m.outgoing {
                format!("You: {text}")
            } else {
                String::from(text)
            };
            Row {
                badge: Some(match p {
                    Peer::Channel(_) => Badge::Hash,
                    Peer::Contact(_) => Badge::Initials(initials(&name)),
                }),
                title: name,
                bold: unread > 0,
                right: clock::ago(m.at, ui.ctx.unix),
                sub: Some(preview),
                dot: match (unread, muted(p)) {
                    (0, _) => Dot::None,
                    (_, true) => Dot::Muted,
                    (_, false) => Dot::Unread,
                },
            }
        })
        .collect()
}

impl Screen for Messages {
    fn scene(&mut self, w: u16, h: u16, ui: &Ui<'_>) -> Option<Scene> {
        let mut rows_ = alloc::vec![Row::action(Icon::Pencil, "New message")];
        rows_.extend(conversation_rows(ui));
        let sel = self.state.selected().unwrap_or(0).min(rows_.len() - 1);
        let mut sc = rich_list(ui, w, h, &rows_, sel);
        if rows_.len() == 1 {
            sc.text(8, 50, Font::R10, Align::Left, true, "No messages yet.");
        }
        Some(sc)
    }

    fn title(&self, _ui: &Ui<'_>) -> String {
        // The header already counts the unread messages.
        String::from("Messages")
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        let mut rows_: Vec<(String, String)> =
            alloc::vec![(String::from("+ New message"), String::new())];
        rows_.extend(conversations(ui).into_iter().map(|(_, n, d)| (n, d)));
        draw_list(f, body, &rows_, &mut self.state, ui);
        if rows_.len() == 1 {
            f.render_widget(
                Paragraph::new("No messages yet.").wrap(Wrap { trim: true }),
                rows(body, 2, body.height.saturating_sub(2)),
            );
        }
    }

    fn input(&mut self, input: Input, ui: &Ui<'_>) -> Nav {
        let conv = conversations(ui);
        if !step(&mut self.state, conv.len() + 1, input) {
            return Nav::Stay;
        }
        match self.state.selected().unwrap_or(0) {
            0 => Nav::Push(Box::new(Contacts::picker())),
            i => match conv.get(i - 1) {
                Some((peer, _, _)) => {
                    Nav::PushWith(Box::new(Conversation::new(*peer)), Intent::MarkRead(*peer))
                }
                None => Nav::Stay,
            },
        }
    }
}

// ------------------------------------------------------------------ one conversation

/// One conversation; the newest messages at the bottom, A scrolls.
#[derive(Debug)]
pub struct Conversation {
    peer: Peer,
    /// Lines scrolled up from the bottom.
    up: usize,
    /// Lines last drawn, to bound scrolling.
    total: usize,
    page: usize,
    /// On the panel the same in pixels: drawn as bubbles (ADR-0011).
    px: Option<PxScroll>,
}

#[derive(Debug, Clone, Copy, Default)]
struct PxScroll {
    up: i32,
    total: i32,
    page: i32,
}

impl Conversation {
    pub fn new(peer: Peer) -> Self {
        Self {
            peer,
            up: 0,
            total: 0,
            page: 1,
            px: None,
        }
    }
}

/// What goes under a bubble: the time, then the route (received) or the status (sent).
/// A delivered message gets a check mark, drawn as an icon before its round trip.
fn meta(ui: &Ui<'_>, m: &Msg) -> (String, Option<(Icon, String)>) {
    let c = ui.ctx;
    let when = clock::stamp(m.at, c.unix, c.tz_offset_min);
    if !m.outgoing {
        let snr = if m.snr4 == 0 {
            String::new()
        } else {
            format!(" · {} dB", m.snr4 / 4)
        };
        return (format!("{when} · {}{snr}", route(m)), None);
    }
    match m.status {
        Status::Delivered { rtt_ms } => (
            when,
            Some((
                Icon::Check,
                format!("{}.{} s", rtt_ms / 1000, rtt_ms % 1000 / 100),
            )),
        ),
        Status::Failed => (when, Some((Icon::Cross, String::from("failed")))),
        _ => (format!("{when} · {}", status(m).replace('…', "...")), None),
    }
}

/// One message as a bubble: its height, and its drawing from y = 0.
fn bubble(ui: &Ui<'_>, m: &Msg, channel: bool, w: i16) -> (i16, Scene) {
    let s = pixel::sizes(ui);
    let font = s.body;
    let lh = font.line_height();
    let text = String::from_utf8_lossy(&m.text);
    let (sender, body) = match (channel && !m.outgoing, text.split_once(": ")) {
        (true, Some((who, t))) => (Some(String::from(who)), String::from(t)),
        (true, None) => (Some(String::from("?")), String::from(text.as_ref())),
        _ => (None, String::from(text.as_ref())),
    };
    let max_w = w * 7 / 10;
    let mut lines = scene::wrap(font, &body, max_w - 12);
    if lines.is_empty() {
        lines.push(String::new());
    }
    let (meta_text, mark) = meta(ui, m);
    let meta_w = scene::width(s.small, &meta_text)
        + mark
            .as_ref()
            .map_or(0, |(_, t)| 14 + scene::width(s.small, t));
    let text_w = lines
        .iter()
        .map(|l| scene::width(font, l))
        .max()
        .unwrap_or(0);
    let bw = (text_w + 12).max(meta_w.min(max_w)).max(24);
    let mut sc = Scene::new();
    let mut y = 0i16;
    if let Some(who) = sender {
        let who = scene::ellipsize(s.small_bold, &who, w - 16);
        sc.text(8, y + 8, s.small_bold, Align::Left, true, &who);
        y += 10;
    }
    let bh = lh * lines.len() as i16 + 8;
    let x = if m.outgoing { w - 4 - bw } else { 4 };
    sc.rect(x, y, bw as u16, bh as u16, 6, Some(m.outgoing), Some(true));
    for (i, l) in lines.iter().enumerate() {
        sc.text(
            x + 6,
            y + 4 + font.ascent() + lh * i as i16,
            font,
            Align::Left,
            !m.outgoing,
            l,
        );
    }
    let my = y + bh + 9;
    if m.outgoing {
        let mut right = w - 6;
        if let Some((icon, tail)) = &mark {
            sc.text(right, my, s.small, Align::Right, true, tail);
            right -= scene::width(s.small, tail) + 11;
            sc.icon(right, my - 7, *icon, IconSize::S8, true);
            right -= 3;
        }
        sc.text(right, my, s.small, Align::Right, true, &meta_text);
    } else {
        let t = scene::ellipsize(s.small, &meta_text, w - 8);
        sc.text(x + 2, my, s.small, Align::Left, true, &t);
    }
    (my + 5, sc)
}

fn route(m: &Msg) -> String {
    match m.path_len {
        0xFF => String::from("direct"),
        p => match p & 63 {
            0 => String::from("direct"),
            1 => String::from("1 hop"),
            n => format!("{n} hops"),
        },
    }
}

fn status(m: &Msg) -> String {
    match m.status {
        Status::Sending { attempt: 0 } => String::from("sending…"),
        Status::Sending { attempt: 3 } => String::from("flood…"),
        Status::Sending { attempt } => format!("try {}…", attempt + 1),
        Status::Delivered { rtt_ms } => {
            format!("delivered {}.{} s", rtt_ms / 1000, rtt_ms % 1000 / 100)
        }
        Status::Sent => String::from("sent"),
        Status::Failed => String::from("failed"),
        Status::Unread | Status::Read => String::new(),
    }
}

/// The lines of the whole conversation: a header per message, then its wrapped text;
/// received on the left, sent on the right.
fn lines(ui: &Ui<'_>, peer: Peer, width: usize) -> Vec<(String, Alignment, Style)> {
    let c = ui.ctx;
    let mut out = Vec::new();
    for m in c.dev.history().with(peer) {
        let when = clock::stamp(m.at, c.unix, c.tz_offset_min);
        let text = String::from_utf8_lossy(&m.text).into_owned();
        let (head, body, align) = if m.outgoing {
            (
                format!("{when} {} {}", ui.sym.dot, status(m)),
                text,
                Alignment::Right,
            )
        } else if let Peer::Channel(_) = peer {
            // "Name: text" on channels.
            let (who, t) = match text.split_once(": ") {
                Some((w, t)) => (String::from(w), String::from(t)),
                None => (String::from("?"), text.clone()),
            };
            (format!("{who} {} {when}", ui.sym.dot), t, Alignment::Left)
        } else {
            (
                format!("{when} {} {}", ui.sym.dot, route(m)),
                text,
                Alignment::Left,
            )
        };
        out.push((chrome::fit(&head, width), align, theme::label()));
        for l in chrome::wrap(&body, width.saturating_sub(2)) {
            out.push((l, align, Style::new()));
        }
        out.push((String::new(), Alignment::Left, Style::new()));
    }
    out.pop();
    out
}

impl Screen for Conversation {
    fn title(&self, ui: &Ui<'_>) -> String {
        peer_name(ui, self.peer)
    }

    /// Bubbles, the newest at the bottom; scrolled in pixels.
    fn scene(&mut self, w: u16, h: u16, ui: &Ui<'_>) -> Option<Scene> {
        let (wi, hi) = (w as i16, h as i16);
        let channel = matches!(self.peer, Peer::Channel(_));
        let mut sc = Scene::new();
        let blocks: Vec<(i16, Scene)> = ui
            .ctx
            .dev
            .history()
            .with(self.peer)
            .map(|m| bubble(ui, m, channel, wi))
            .collect();
        if blocks.is_empty() {
            self.px = Some(PxScroll {
                page: 1,
                ..PxScroll::default()
            });
            sc.icon(wi / 2 - 8, hi / 2 - 30, Icon::Chat, IconSize::S16, true);
            sc.text(
                wi / 2,
                hi / 2,
                Font::B10,
                Align::Center,
                true,
                "No messages yet",
            );
            sc.text(
                wi / 2,
                hi / 2 + 16,
                Font::R8,
                Align::Center,
                true,
                "Hold A to write.",
            );
            return Some(sc);
        }
        let top = 4i32;
        let total: i32 = top + blocks.iter().map(|(bh, _)| i32::from(*bh)).sum::<i32>();
        let mut px = self.px.unwrap_or_default();
        px.total = total;
        px.page = (i32::from(hi) / 2).max(1);
        px.up = px.up.clamp(0, (total - i32::from(hi)).max(0));
        self.px = Some(px);
        // The window's top, in content pixels: the start if it all fits, else from the bottom.
        let from = (total - i32::from(hi) - px.up).max(0);
        let mut y = top - from;
        for (bh, b) in blocks {
            let bh = i32::from(bh);
            if y + bh > 0 && y < i32::from(hi) {
                sc.append(b, 0, y as i16);
            }
            y += bh;
        }
        // Where the window is, on the right edge.
        if total > i32::from(hi) {
            sc.vline(wi - 1, 0, hi - 1);
            let thumb = (i32::from(hi) * i32::from(hi) / total).max(8);
            let ty = (from * (i32::from(hi) - thumb) / (total - i32::from(hi)).max(1)) as i16;
            sc.rect(wi - 3, ty, 3, thumb as u16, 0, Some(true), None);
        }
        Some(sc)
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        let all = lines(ui, self.peer, usize::from(body.width));
        let h = usize::from(body.height);
        self.total = all.len();
        self.page = (h / 2).max(1);
        self.up = self.up.min(all.len().saturating_sub(h));
        if all.is_empty() {
            f.render_widget(
                Paragraph::new("No messages yet. Hold A to write.").wrap(Wrap { trim: true }),
                rows(body, 1, body.height.saturating_sub(1)),
            );
            return;
        }
        let end = all.len() - self.up;
        let start = end.saturating_sub(h);
        for (i, (text, align, style)) in all[start..end].iter().enumerate() {
            f.render_widget(
                Paragraph::new(Line::from(text.as_str()))
                    .alignment(*align)
                    .style(*style),
                rows(body, i as u16, 1),
            );
        }
    }

    fn input(&mut self, input: Input, _ui: &Ui<'_>) -> Nav {
        if let Some(px) = &mut self.px {
            match input {
                Input::Next(n) => px.up = (px.up - px.page * i32::from(n)).max(0),
                Input::Prev => px.up = (px.up + px.page).min(px.total),
                Input::Open => return Nav::Push(Box::new(Compose::to(self.peer))),
            }
            return Nav::Stay;
        }
        match input {
            // Down: towards the newest.
            Input::Next(n) => self.up = self.up.saturating_sub(self.page * usize::from(n)),
            Input::Prev => self.up = (self.up + self.page).min(self.total),
            Input::Open => return Nav::Push(Box::new(Compose::to(self.peer))),
        }
        Nav::Stay
    }

    fn footer(&self, _ui: &Ui<'_>) -> Footer {
        Footer {
            a_click: Some("down"),
            a_hold: Some("write"),
            a_double: Some("up"),
            b_click: Some("back"),
            b_hold: None,
        }
    }

    fn peer(&self) -> Option<Peer> {
        Some(self.peer)
    }
}

// ------------------------------------------------------------------ writing

/// What the text is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Peer(Peer),
    /// A new `#hashtag` channel.
    Hashtag,
    /// The node's name; `setup`: a step of the first-boot setup.
    Name {
        setup: bool,
    },
    /// A server's password, then log in.
    Password([u8; 32]),
    /// The radio frequency in kHz (hold-to-confirm).
    Frequency,
    /// A fixed Bluetooth PIN, six digits.
    Pin,
}

#[derive(Debug)]
pub struct Compose {
    target: Target,
    kb: Option<Keyboard>,
    initial: String,
}

impl Compose {
    pub fn to(peer: Peer) -> Self {
        Self {
            target: Target::Peer(peer),
            kb: None,
            initial: String::new(),
        }
    }

    pub fn new(target: Target, initial: &str) -> Self {
        Self {
            target,
            kb: None,
            initial: String::from(initial),
        }
    }

    fn mode(&self, ui: &Ui<'_>) -> Mode {
        match self.target {
            Target::Peer(Peer::Contact(_)) => Mode::Message { limit: TEXT_MAX },
            Target::Peer(Peer::Channel(_)) => {
                // The sender's name and ": " travel in the same text.
                let name = mc_node::contacts::name_str(&ui.ctx.dev.node.prefs.name).len();
                Mode::Message {
                    limit: TEXT_MAX.saturating_sub(name + 2),
                }
            }
            Target::Hashtag => Mode::Hashtag,
            Target::Name { .. } => Mode::Name,
            Target::Password(_) => Mode::Password,
            Target::Frequency => Mode::Digits { limit: 7 },
            Target::Pin => Mode::Digits { limit: 6 },
        }
    }

    /// Created on first use: the recent words come from the history.
    fn kb(&mut self, ui: &Ui<'_>) -> &mut Keyboard {
        let mode = self.mode(ui);
        if self.initial.is_empty()
            && let Target::Name { .. } = self.target
        {
            self.initial = super::name_of(&ui.ctx.dev.node.prefs.name);
        }
        let initial = &self.initial;
        self.kb.get_or_insert_with(|| {
            let h = ui.ctx.dev.history();
            let sent: Vec<&[u8]> = h
                .iter()
                .filter(|m| m.outgoing)
                .map(|m| m.text.as_slice())
                .collect();
            Keyboard::new(mode, initial, recent_words(sent.into_iter()))
                .with_position(ui.ctx.gps.map(|g| (g.lat, g.lon)))
        })
    }

    fn finish(&self, text: String, ui: &Ui<'_>) -> Nav {
        match self.target {
            Target::Peer(Peer::Contact(prefix)) => {
                let contacts = &ui.ctx.dev.node.contacts;
                match contacts.find_prefix(&prefix).and_then(|s| contacts.get(s)) {
                    Some(c) => Nav::PopWith(
                        Intent::SendText {
                            key: c.pub_key,
                            text,
                        },
                        None,
                    ),
                    None => Nav::Toast(String::from("Unknown contact")),
                }
            }
            Target::Peer(Peer::Channel(idx)) => {
                Nav::PopWith(Intent::SendChannel { idx, text }, None)
            }
            Target::Hashtag => Nav::PopWith(Intent::AddChannel(text), Some("Channel added")),
            Target::Name { setup: false } => {
                Nav::PopWith(Intent::SetName(text), Some("Name saved"))
            }
            Target::Name { setup: true } => Nav::Intent(Intent::SetName(text)),
            Target::Frequency => match text.parse::<u32>() {
                Ok(f) if (150_000..=2_500_000).contains(&f) => {
                    let r = &ui.ctx.dev.node.prefs.radio;
                    Nav::Confirm(
                        crate::app::Confirm::new(
                            "Frequency",
                            &format!(
                                "Every node you talk to must use the same frequency. Switch to {}.{:03} MHz.",
                                f / 1000,
                                f % 1000
                            ),
                            Intent::SetRadio {
                                freq_khz: f,
                                bw_hz: r.bw_hz,
                                sf: r.sf,
                                cr: r.cr,
                            },
                        )
                        .with_toast("Frequency saved"),
                    )
                }
                _ => Nav::Toast(String::from("150000 to 2500000 kHz")),
            },
            Target::Pin => match text.parse::<u32>() {
                Ok(p) if text.len() == 6 && (100_000..=999_999).contains(&p) => Nav::PopWith(
                    Intent::SetPin(p),
                    Some("PIN saved, used from the next reboot"),
                ),
                _ => Nav::Toast(String::from("Six digits, not starting with 0")),
            },
            Target::Password(key) => Nav::PopWith(
                Intent::Login {
                    key,
                    password: text,
                },
                None,
            ),
        }
    }
}

impl Screen for Compose {
    fn title(&self, ui: &Ui<'_>) -> String {
        match self.target {
            Target::Peer(p) => format!("To {}", peer_name(ui, p)),
            Target::Hashtag => String::from("New #channel"),
            Target::Name { setup: true } => String::from("2/3 Name"),
            Target::Name { setup: false } => String::from("Your name"),
            Target::Password(_) => String::from("Password"),
            Target::Frequency => String::from("Frequency (kHz)"),
            Target::Pin => String::from("Bluetooth PIN"),
        }
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        let w = usize::from(body.width);
        let hashtag = self.target == Target::Hashtag;
        let kb = self.kb(ui);
        // Text box: the last lines of the wrapped text, with a cursor.
        let shown = if hashtag {
            format!("#{}_", kb.text)
        } else {
            format!("{}_", kb.text)
        };
        let box_h = (body.height / 2).saturating_sub(1).max(1);
        let wrapped = chrome::wrap(&shown, w);
        let skip = wrapped.len().saturating_sub(usize::from(box_h));
        let text: Vec<Line<'_>> = wrapped.into_iter().skip(skip).map(Line::from).collect();
        f.render_widget(Paragraph::new(text), rows(body, 0, box_h));
        // Rule with the byte count.
        let count = format!(" {}/{}", kb.bytes(), kb.mode.limit());
        let rule = format!(
            "{}{count}",
            "─".repeat(w.saturating_sub(count.chars().count()))
        );
        f.render_widget(Paragraph::new(rule), rows(body, box_h, 1));
        // Candidates, flowed into the rows left; the page holding the selection.
        let area = rows(body, box_h + 1, body.height.saturating_sub(box_h + 1));
        let mut lines: Vec<Vec<(usize, String)>> = alloc::vec![Vec::new()];
        let mut len = 0usize;
        for (i, c) in kb.candidates().iter().enumerate() {
            let label = chrome::fit(&c.label(kb.mode), w);
            let l = label.chars().count();
            if len > 0 && len + 1 + l > w {
                lines.push(Vec::new());
                len = 0;
            }
            len += l + usize::from(len > 0);
            if let Some(last) = lines.last_mut() {
                last.push((i, label));
            }
        }
        let per_page = usize::from(area.height).max(1);
        let sel_line = lines
            .iter()
            .position(|l| l.iter().any(|(i, _)| *i == kb.sel))
            .unwrap_or(0);
        let first = sel_line / per_page * per_page;
        for (row, line) in lines.iter().skip(first).take(per_page).enumerate() {
            let mut spans = Vec::new();
            for (k, (i, label)) in line.iter().enumerate() {
                if k > 0 {
                    spans.push(ratatui_core::text::Span::raw(" "));
                }
                let style = if *i == kb.sel {
                    theme::reversed()
                } else {
                    Style::new()
                };
                spans.push(ratatui_core::text::Span::styled(label.clone(), style));
            }
            f.render_widget(Paragraph::new(Line::from(spans)), rows(area, row as u16, 1));
        }
    }

    fn input(&mut self, input: Input, ui: &Ui<'_>) -> Nav {
        let kb = self.kb(ui);
        match input {
            Input::Next(n) => kb.next(usize::from(n)),
            Input::Prev => kb.prev(),
            Input::Open => {
                if let Key::Done(text) = kb.insert() {
                    return self.finish(text, ui);
                }
            }
        }
        Nav::Stay
    }

    fn back(&mut self, ui: &Ui<'_>) -> Option<Nav> {
        match self.kb(ui).delete() {
            Key::Cancel => Some(match self.target {
                // Setup: keep the current name and go on.
                Target::Name { setup: true } => Nav::Replace(Box::new(super::setup::SetupTime)),
                _ => Nav::Pop,
            }),
            _ => Some(Nav::Stay),
        }
    }

    fn footer(&self, _ui: &Ui<'_>) -> Footer {
        let empty = self.kb.as_ref().is_none_or(|k| k.text.is_empty());
        let done = self
            .kb
            .as_ref()
            .and_then(|k| k.selected())
            .is_some_and(|c| *c == Cand::Done);
        Footer {
            a_click: Some("next"),
            a_hold: Some(if done { "done" } else { "add" }),
            a_double: None,
            b_click: Some(if empty { "cancel" } else { "delete" }),
            b_hold: None,
        }
    }

    fn result(&mut self, intent: &Intent, outcome: &Outcome, _ui: &Ui<'_>) -> Nav {
        // Setup: the name is saved, next step.
        if matches!(intent, Intent::SetName(_))
            && self.target == (Target::Name { setup: true })
            && *outcome == Outcome::Ok
        {
            return Nav::Replace(Box::new(super::setup::SetupTime));
        }
        if matches!(intent, Intent::SetRadio { .. })
            && self.target == Target::Frequency
            && *outcome == Outcome::Ok
        {
            return Nav::Pop;
        }
        Nav::Stay
    }

    fn keyboard(&self) -> Option<&Keyboard> {
        self.kb.as_ref()
    }
}
