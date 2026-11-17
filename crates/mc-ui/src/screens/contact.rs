//! One contact, one channel: what we know about it and what can be done with it
//! (`docs/spec/ui.md` §6).

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use mc_node::contacts::{Contact, PATH_UNKNOWN};
use mc_node::history::Peer;
use ratatui_core::layout::Rect;
use ratatui_core::terminal::Frame;
use ratatui_core::text::Line;
use ratatui_widgets::list::ListState;
use ratatui_widgets::paragraph::Paragraph;

use super::chat::{Compose, Conversation};
use super::{Card, draw_list, name_of, rows, short_key, step};
use crate::act::{Intent, Outcome};
use crate::app::{Confirm, Input, Nav, Screen, Ui};
use crate::clock;
use crate::geo;
use crate::scene::{self, Align, Font, Icon, IconSize, Scene};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Act {
    Write,
    Messages,
    Ping,
    Favourite,
    ResetRoute,
    Trace,
    Share,
    Remove,
}

/// The icons of [`ACTS`], for the tiles.
const ACT_ICONS: [Icon; 8] = [
    Icon::Pencil,
    Icon::Chat,
    Icon::Broadcast,
    Icon::Star,
    Icon::Refresh,
    Icon::Route,
    Icon::Qr,
    Icon::Trash,
];

const ACTS: [(Act, &str); 8] = [
    (Act::Write, "Write"),
    (Act::Messages, "Messages"),
    (Act::Ping, "Ping"),
    (Act::Favourite, "Favourite"),
    (Act::ResetRoute, "Reset route"),
    (Act::Trace, "Trace route"),
    (Act::Share, "Share (QR)"),
    (Act::Remove, "Remove"),
];

#[derive(Debug)]
pub struct ContactScreen {
    key: [u8; 32],
    state: ListState,
    /// The last ping from this screen: its request tag.
    ping: Option<u32>,
}

impl ContactScreen {
    pub fn new(key: [u8; 32]) -> Self {
        Self {
            key,
            state: ListState::default().with_selected(Some(0)),
            ping: None,
        }
    }

    fn contact<'a>(&self, ui: &Ui<'a>) -> Option<&'a Contact> {
        let c = &ui.ctx.dev.node.contacts;
        c.find(&self.key).and_then(|s| c.get(s))
    }

    fn peer(&self) -> Peer {
        let mut p = [0u8; 6];
        p.copy_from_slice(&self.key[..6]);
        Peer::Contact(p)
    }
}

pub(crate) fn kind_name(kind: u8) -> &'static str {
    match kind {
        1 => "Chat",
        2 => "Repeater",
        3 => "Room",
        4 => "Sensor",
        _ => "Node",
    }
}

pub(crate) fn route_of(c: &Contact) -> String {
    if c.out_path_len == PATH_UNKNOWN {
        return String::from("no route yet");
    }
    match c.out_path_len & 63 {
        0 => String::from("direct"),
        1 => String::from("1 hop"),
        n => format!("{n} hops"),
    }
}

impl Screen for ContactScreen {
    fn title(&self, ui: &Ui<'_>) -> String {
        self.contact(ui)
            .map(|c| name_of(&c.name))
            .unwrap_or_else(|| String::from("Contact"))
    }

    /// A card: the badge and name, what is known, then the actions as tiles.
    fn scene(&mut self, w: u16, h: u16, ui: &Ui<'_>) -> Option<Scene> {
        let mut sc = Scene::new();
        let Some(c) = self.contact(ui) else {
            sc.text(8, 24, Font::B10, Align::Left, true, "Contact removed.");
            return Some(sc);
        };
        let ctx = ui.ctx;
        let (wi, hi) = (w as i16, h as i16);
        let name = name_of(&c.name);
        // The badge, the name, a star for a favourite.
        sc.circle(20, 19, 34, true, true);
        let badge = super::pixel::initials(&name);
        sc.text(
            20,
            19 + Font::B12.ascent() / 2,
            Font::B12,
            Align::Center,
            false,
            &badge,
        );
        let fav = c.flags & 1 != 0;
        let room = wi - 44 - if fav { 14 } else { 4 };
        let shown = scene::ellipsize(Font::B12, &name, room);
        sc.text(44, 16, Font::B12, Align::Left, true, &shown);
        if fav {
            let x = 44 + scene::width(Font::B12, &shown) + 4;
            sc.icon(x, 6, Icon::Star, IconSize::S8, true);
        }
        let sub = format!("{} · key {}", kind_name(c.kind), short_key(&c.pub_key));
        sc.text(
            44,
            31,
            Font::R8,
            Align::Left,
            true,
            &scene::ellipsize(Font::R8, &sub, wi - 46),
        );
        // What is known: the route and when it was heard, where it is, the last ping.
        let mut lines: Vec<(Icon, String)> = Vec::new();
        let heard = if c.lastmod == 0 {
            String::from("never heard")
        } else {
            format!(
                "heard {}",
                clock::stamp(c.lastmod, ctx.unix, ctx.tz_offset_min)
            )
        };
        lines.push((Icon::Route, format!("{} · {heard}", route_of(c))));
        if let Some(p) = super::tools::position_of(c) {
            let place = match super::tools::my_position(ui) {
                Some(me) => format!(
                    "{} {}",
                    geo::distance_text(geo::distance_m(me, p)),
                    geo::compass(geo::bearing_deg(me, p))
                ),
                None => format!("{}, {}", geo::coord(p.0, true), geo::coord(p.1, false)),
            };
            lines.push((Icon::Pin, place));
        }
        if let Some(tag) = self.ping {
            lines.push((
                Icon::Signal,
                match ctx.dev.request_reply(tag) {
                    Some(r) if r.ok => {
                        format!("Ping {}.{:02} s", r.rtt_ms / 1000, r.rtt_ms % 1000 / 10)
                    }
                    Some(_) => String::from("Ping: no answer"),
                    None => String::from("Ping: waiting..."),
                },
            ));
        }
        let mut y = 40i16;
        for (icon, text) in &lines {
            sc.icon(4, y, *icon, IconSize::S8, true);
            let t = scene::ellipsize(Font::R8, text, wi - 20);
            sc.text(16, y + 7, Font::R8, Align::Left, true, &t);
            y += 11;
        }
        sc.hline(0, wi - 1, y + 1);
        // The actions, 3 × 3.
        let top = y + 4;
        let (tw, tile_x) = super::pixel::tile_columns(w, 3);
        let th = ((hi - top - 4) / 3).clamp(20, 40) as u16;
        let sel = self.state.selected().unwrap_or(0) % ACTS.len();
        let unread = ctx.dev.history().unread_with(self.peer());
        for (i, ((act, label), icon)) in ACTS.iter().zip(ACT_ICONS).enumerate() {
            let (col, row) = (i as i16 % 3, i as i16 / 3);
            let label = match act {
                Act::Favourite if fav => "Unfavourite",
                Act::Share => "Share",
                _ => *label,
            };
            let count = if *act == Act::Messages { unread } else { 0 };
            super::pixel::tile(
                &mut sc,
                tile_x(col),
                top + row * (th as i16 + super::pixel::TILE_GAP),
                tw,
                th,
                icon,
                label,
                count,
                i == sel,
            );
        }
        Some(sc)
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        let Some(c) = self.contact(ui) else {
            f.render_widget(Paragraph::new("Contact removed."), body);
            return;
        };
        let ctx = ui.ctx;
        let fav = c.flags & 1 != 0;
        let mut info = alloc::vec![
            format!("{} {} {}", kind_name(c.kind), ui.sym.dot, route_of(c)),
            format!(
                "Key {} {} {}",
                short_key(&c.pub_key),
                ui.sym.dot,
                if c.lastmod == 0 {
                    String::from("never heard")
                } else {
                    clock::stamp(c.lastmod, ctx.unix, ctx.tz_offset_min)
                }
            ),
        ];
        if let Some(tag) = self.ping {
            info.push(match ctx.dev.request_reply(tag) {
                Some(r) if r.ok => {
                    format!("Ping: {}.{:02} s", r.rtt_ms / 1000, r.rtt_ms % 1000 / 10)
                }
                Some(_) => String::from("Ping: no answer"),
                None => String::from("Ping: waiting…"),
            });
        }
        let n = info.len() as u16;
        let lines: Vec<Line<'_>> = info.into_iter().map(Line::from).collect();
        f.render_widget(Paragraph::new(lines), rows(body, 0, n));
        let acts: Vec<(String, String)> = ACTS
            .iter()
            .map(|(a, label)| {
                let detail = match a {
                    Act::Favourite => String::from(if fav { "yes" } else { "no" }),
                    Act::Messages => match ctx.dev.history().unread_with(self.peer()) {
                        0 => String::new(),
                        u => format!("{}{u}", ui.sym.unread),
                    },
                    _ => String::new(),
                };
                (String::from(*label), detail)
            })
            .collect();
        draw_list(
            f,
            rows(body, n + 1, body.height.saturating_sub(n + 1)),
            &acts,
            &mut self.state,
            ui,
        );
    }

    fn input(&mut self, input: Input, ui: &Ui<'_>) -> Nav {
        if !step(&mut self.state, ACTS.len(), input) {
            return Nav::Stay;
        }
        let Some(c) = self.contact(ui) else {
            return Nav::Pop;
        };
        let (act, _) = ACTS[self.state.selected().unwrap_or(0) % ACTS.len()];
        let key = self.key;
        match act {
            Act::Write => Nav::Push(Box::new(Compose::to(self.peer()))),
            Act::Messages => Nav::PushWith(
                Box::new(Conversation::new(self.peer())),
                Intent::MarkRead(self.peer()),
            ),
            Act::Ping => Nav::Intent(Intent::Ping(key)),
            Act::Favourite => {
                let on = c.flags & 1 == 0;
                Nav::IntentToast(
                    Intent::SetFavourite(key, on),
                    if on { "Favourite" } else { "Not a favourite" },
                )
            }
            Act::ResetRoute => Nav::IntentToast(Intent::ResetPath(key), "Route reset"),
            Act::Trace => Nav::Push(Box::new(super::tools::Trace::new(key))),
            Act::Share => Nav::Push(Box::new(Card::of(&name_of(&c.name), &c.pub_key))),
            Act::Remove => Nav::Confirm(
                Confirm::new(
                    "Remove",
                    "Remove this contact from the device?",
                    Intent::RemoveContact(key),
                )
                .with_toast("Removed"),
            ),
        }
    }

    fn result(&mut self, intent: &Intent, outcome: &Outcome, _ui: &Ui<'_>) -> Nav {
        match (intent, outcome) {
            (Intent::Ping(_), Outcome::Request(tag)) => self.ping = Some(*tag),
            (Intent::RemoveContact(_), Outcome::Ok) => return Nav::Pop,
            _ => {}
        }
        Nav::Stay
    }
}

#[derive(Debug)]
pub struct ChannelScreen {
    idx: u8,
    state: ListState,
}

impl ChannelScreen {
    pub fn new(idx: u8) -> Self {
        Self {
            idx,
            state: ListState::default().with_selected(Some(0)),
        }
    }
}

/// Messages, Write, Mute, Remove.
const CHANNEL_ACTS: [&str; 4] = ["Messages", "Write", "Mute", "Remove"];
const CHANNEL_ICONS: [Icon; 4] = [Icon::Chat, Icon::Pencil, Icon::Bell, Icon::Trash];

impl Screen for ChannelScreen {
    fn title(&self, ui: &Ui<'_>) -> String {
        ui.ctx
            .dev
            .node
            .channels
            .0
            .get(usize::from(self.idx))
            .map(|c| name_of(&c.name))
            .unwrap_or_default()
    }

    /// A card like a contact's: the `#` badge and the name, what is known, then the actions as
    /// four tiles.
    fn scene(&mut self, w: u16, h: u16, ui: &Ui<'_>) -> Option<Scene> {
        let mut sc = Scene::new();
        let (wi, hi) = (w as i16, h as i16);
        let peer = Peer::Channel(self.idx);
        let hist = ui.ctx.dev.history();
        let muted = ui.ctx.dev.node.prefs.ui.is_muted(self.idx);
        sc.circle(20, 19, 34, true, true);
        sc.text(
            20,
            19 + Font::B12.ascent() / 2,
            Font::B12,
            Align::Center,
            false,
            "#",
        );
        let name = scene::ellipsize(Font::B12, &self.title(ui), wi - 48);
        sc.text(44, 16, Font::B12, Align::Left, true, &name);
        let sub = format!("Channel · slot {}", self.idx);
        sc.text(44, 31, Font::R8, Align::Left, true, &sub);
        let unread = hist.unread_with(peer);
        let count = hist.with(peer).count();
        let lines = [
            (
                Icon::Chat,
                format!(
                    "{count} message{}, {unread} unread",
                    if count == 1 { "" } else { "s" }
                ),
            ),
            (
                Icon::Bell,
                String::from(if muted {
                    "Muted: no sound, no light"
                } else {
                    "Notifies like a direct message"
                }),
            ),
        ];
        let mut y = 40i16;
        for (icon, text) in &lines {
            sc.icon(4, y, *icon, IconSize::S8, true);
            let t = scene::ellipsize(Font::R8, text, wi - 20);
            sc.text(16, y + 7, Font::R8, Align::Left, true, &t);
            y += 11;
        }
        sc.hline(0, wi - 1, y + 1);
        // The actions, 2 × 2.
        let top = y + 4;
        let (tw, tile_x) = super::pixel::tile_columns(w, 2);
        let th = ((hi - top - 4) / 2).clamp(24, 44) as u16;
        let sel = self.state.selected().unwrap_or(0) % CHANNEL_ACTS.len();
        for (i, (label, icon)) in CHANNEL_ACTS.iter().zip(CHANNEL_ICONS).enumerate() {
            let (col, row) = (i as i16 % 2, i as i16 / 2);
            let label = if *label == "Mute" && muted {
                "Unmute"
            } else {
                label
            };
            let badge = if i == 0 { unread } else { 0 };
            super::pixel::tile(
                &mut sc,
                tile_x(col),
                top + row * (th as i16 + super::pixel::TILE_GAP),
                tw,
                th,
                icon,
                label,
                badge,
                i == sel,
            );
        }
        Some(sc)
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        let peer = Peer::Channel(self.idx);
        let h = ui.ctx.dev.history();
        let info = format!(
            "Slot {} {} {} messages {} {} unread",
            self.idx,
            ui.sym.dot,
            h.with(peer).count(),
            ui.sym.dot,
            h.unread_with(peer)
        );
        f.render_widget(
            Paragraph::new(crate::chrome::fit(&info, usize::from(body.width))),
            rows(body, 0, 1),
        );
        let muted = ui.ctx.dev.node.prefs.ui.is_muted(self.idx);
        let acts: Vec<(String, String)> = CHANNEL_ACTS
            .iter()
            .map(|l| {
                let detail = match *l {
                    "Mute" => String::from(if muted { "yes" } else { "no" }),
                    _ => String::new(),
                };
                (String::from(*l), detail)
            })
            .collect();
        draw_list(
            f,
            rows(body, 2, body.height.saturating_sub(2)),
            &acts,
            &mut self.state,
            ui,
        );
    }

    fn input(&mut self, input: Input, ui: &Ui<'_>) -> Nav {
        if !step(&mut self.state, CHANNEL_ACTS.len(), input) {
            return Nav::Stay;
        }
        let peer = Peer::Channel(self.idx);
        match self.state.selected().unwrap_or(0) {
            0 => Nav::PushWith(Box::new(Conversation::new(peer)), Intent::MarkRead(peer)),
            1 => Nav::Push(Box::new(Compose::to(peer))),
            2 => {
                let u = ui.ctx.dev.node.prefs.ui;
                let mute = !u.is_muted(self.idx);
                Nav::IntentToast(
                    Intent::SetUi(u.with_muted(self.idx, mute)),
                    if mute { "Muted" } else { "Unmuted" },
                )
            }
            _ => Nav::Confirm(
                Confirm::new(
                    "Remove",
                    "Leave this channel?",
                    Intent::RemoveChannel(self.idx),
                )
                .with_toast("Removed"),
            ),
        }
    }

    fn result(&mut self, intent: &Intent, outcome: &Outcome, _ui: &Ui<'_>) -> Nav {
        if matches!(intent, Intent::RemoveChannel(_)) && *outcome == Outcome::Ok {
            return Nav::Pop;
        }
        Nav::Stay
    }
}
