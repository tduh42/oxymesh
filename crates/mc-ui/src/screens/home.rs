//! Home: the always-on face (big clock, date, what is new) and the launcher.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use ratatui_core::layout::{Alignment, Rect};
use ratatui_core::terminal::Frame;
use ratatui_widgets::list::ListState;
use ratatui_widgets::paragraph::Paragraph;

use super::{Dest, draw_list, name_of, open, rows, short_key, step};
use crate::app::{Input, Nav, Paint, Screen, Ui};
use crate::chrome::Footer;
use crate::clock;
use crate::panel::COMPACT;
use crate::scene::{self, Align, Font, Icon, IconSize, Scene};
use crate::theme;

/// Settings stays last: a double click from the first entry reaches it.
const LAUNCHER: [(&str, Dest, Icon); 6] = [
    ("Messages", Dest::Messages, Icon::Chat),
    ("Contacts", Dest::Contacts, Icon::People),
    ("Channels", Dest::Channels, Icon::Broadcast),
    ("Tools", Dest::Tools, Icon::Tools),
    ("Map", Dest::Map, Icon::Pin),
    ("Settings", Dest::Settings, Icon::Settings),
];

/// The advertised name, else the contact's, else the short key.
pub(crate) fn heard_name(ui: &Ui<'_>, n: &mc_node::heard::HeardNode) -> String {
    let own = name_of(&n.name);
    if !own.is_empty() {
        return own;
    }
    let contacts = &ui.ctx.dev.node.contacts;
    contacts
        .find(&n.key)
        .and_then(|s| contacts.get(s))
        .map(|c| name_of(&c.name))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| short_key(&n.key))
}

#[derive(Debug, Default)]
pub struct Home {
    state: ListState,
}

impl Home {
    fn clock_rows(ui: &Ui<'_>) -> u16 {
        if ui.grid == COMPACT { 5 } else { 4 }
    }

    fn info(ui: &Ui<'_>) -> Vec<String> {
        let c = ui.ctx;
        let dev = c.dev;
        let new = dev.unread_alerting();
        let unread = match new {
            0 => String::from("No new messages"),
            1 => String::from("1 new message"),
            n => format!("{n} new messages"),
        };
        let heard = match dev.heard().last() {
            Some(n) => format!(
                "Heard {} {} {}",
                heard_name(ui, n),
                ui.sym.dot,
                clock::stamp(n.at, c.unix, c.tz_offset_min)
            ),
            None => String::from("Nothing heard yet"),
        };
        let r = &dev.node.prefs.radio;
        let last = match dev.history().iter().filter(|m| !m.outgoing).last() {
            Some(_) if !super::previews(ui).sender() => String::from(super::HIDDEN_TEXT),
            Some(m) if !super::previews(ui).text() => {
                format!("{}: {}", super::peer_name(ui, m.peer), super::HIDDEN_TEXT)
            }
            Some(m) => {
                let text = String::from_utf8_lossy(&m.text);
                match m.peer {
                    // Channel texts already start with "Name: ".
                    mc_node::history::Peer::Channel(_) => String::from(text),
                    p => format!("{}: {text}", super::peer_name(ui, p)),
                }
            }
            None => String::new(),
        };
        alloc::vec![
            clock::date(c.time()),
            unread,
            last,
            heard,
            format!(
                "{} {} {}",
                name_of(&dev.node.prefs.name),
                ui.sym.dot,
                short_key(dev.node.id.pub_key())
            ),
            format!(
                "{}.{:03} MHz SF{} BW{}.{}",
                r.freq_khz / 1000,
                r.freq_khz % 1000,
                r.sf,
                r.bw_hz / 1000,
                r.bw_hz % 1000 / 100
            ),
        ]
    }
}

impl Screen for Home {
    fn title(&self, ui: &Ui<'_>) -> String {
        name_of(&ui.ctx.dev.node.prefs.name)
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        let ch = Self::clock_rows(ui);
        // Text clock: what the terminal shows; on the panel, the overlay covers it.
        let time = clock::hhmm(ui.ctx.time());
        f.render_widget(
            Paragraph::new(time)
                .alignment(Alignment::Center)
                .style(theme::label()),
            rows(body, ch / 2, 1),
        );
        let launcher_h = LAUNCHER.len() as u16;
        let room = body.height.saturating_sub(ch + launcher_h + 1);
        for (i, line) in Self::info(ui)
            .into_iter()
            .take(usize::from(room))
            .enumerate()
        {
            let align = if i == 0 {
                Alignment::Center
            } else {
                Alignment::Left
            };
            f.render_widget(
                Paragraph::new(line).alignment(align),
                rows(body, ch + i as u16, 1),
            );
        }
        let dev = ui.ctx.dev;
        let channels = dev.node.channels.0.iter().filter(|c| !c.is_empty()).count();
        let new = dev.unread_alerting();
        let launcher: Vec<(String, String)> = LAUNCHER
            .iter()
            .map(|(label, d, _)| {
                let detail = match d {
                    Dest::Messages if new > 0 => format!("{new} new"),
                    Dest::Contacts => format!("{}", dev.node.contacts.list.len()),
                    Dest::Channels => format!("{channels}"),
                    _ => String::new(),
                };
                (String::from(*label), detail)
            })
            .collect();
        draw_list(
            f,
            rows(body, body.height.saturating_sub(launcher_h), launcher_h),
            &launcher,
            &mut self.state,
            ui,
        );
    }

    fn input(&mut self, input: Input, _ui: &Ui<'_>) -> Nav {
        if step(&mut self.state, LAUNCHER.len(), input) {
            let (_, d, _) = LAUNCHER[self.state.selected().unwrap_or(0) % LAUNCHER.len()];
            return Nav::Push(open(d));
        }
        Nav::Stay
    }

    fn footer(&self, _ui: &Ui<'_>) -> Footer {
        Footer {
            a_click: Some("next"),
            a_hold: Some("open"),
            a_double: None,
            b_click: None,
            // B only toggles the light here: `B☀`.
            b_hold: Some("☀"),
        }
    }

    fn header_time(&self) -> bool {
        false
    }

    /// The dashboard (ADR-0011): clock and date, GPS and noise, the last message, the tiles.
    fn scene(&mut self, w: u16, _h: u16, ui: &Ui<'_>) -> Option<Scene> {
        let c = ui.ctx;
        let dev = c.dev;
        let wi = w as i16;
        let t = c.time();
        let mut sc = Scene::new();
        sc.text(4, 34, Font::Digits, Align::Left, true, &clock::hhmm(t));
        let dx = 92;
        sc.text(dx, 16, Font::B12, Align::Left, true, &clock::weekday(t));
        let dm = scene::ellipsize(Font::R12, &clock::day_month(t), wi - dx - 2);
        sc.text(dx, 32, Font::R12, Align::Left, true, &dm);
        // GPS and noise floor.
        let gps = match (c.gps, c.gps_searching) {
            (Some(g), _) => format!("GPS fix, {} sats", g.satellites),
            _ if !dev.gps_active() => String::from("GPS off"),
            // Searching: the count tells an open sky from a room.
            (None, Some(n)) => format!("No fix, {n} sats"),
            (None, None) => String::from("GPS, no fix"),
        };
        sc.icon(4, 40, Icon::Pin, IconSize::S8, true);
        sc.text(15, 47, Font::R8, Align::Left, true, &gps);
        sc.icon(wi / 2 + 12, 40, Icon::Signal, IconSize::S8, true);
        sc.text(
            wi / 2 + 23,
            47,
            Font::R8,
            Align::Left,
            true,
            &super::noise_text(dev.noise_floor),
        );
        // The last message received.
        let (cy, ch) = (52i16, 43u16);
        sc.rect(3, cy, w - 6, ch, 5, None, Some(true));
        match dev.history().iter().filter(|m| !m.outgoing).last() {
            Some(m) => {
                let icon = if dev.unread_alerting() > 0 {
                    Icon::Mail
                } else {
                    Icon::Chat
                };
                sc.icon(8, cy + 5, icon, IconSize::S8, true);
                let when = clock::ago(m.at, c.unix);
                sc.text(wi - 8, cy + 12, Font::R8, Align::Right, true, &when);
                let who = if super::previews(ui).sender() {
                    super::peer_name(ui, m.peer)
                } else {
                    String::from("Message")
                };
                let room = wi - 20 - scene::width(Font::R8, &when) - 14;
                let who = scene::ellipsize(Font::B10, &who, room);
                sc.text(20, cy + 12, Font::B10, Align::Left, true, &who);
                let text = super::preview_text(ui, m);
                let lines = scene::wrap(Font::R10, &text, wi - 16);
                for (i, l) in lines.iter().take(2).enumerate() {
                    let l = if i == 1 && lines.len() > 2 {
                        scene::ellipsize(Font::R10, &format!("{l} ..."), wi - 16)
                    } else {
                        l.clone()
                    };
                    sc.text(8, cy + 25 + 12 * i as i16, Font::R10, Align::Left, true, &l);
                }
            }
            None => {
                let heard = match dev.heard().last() {
                    Some(n) => format!("Heard {} {}", heard_name(ui, n), clock::ago(n.at, c.unix)),
                    None => String::from("Nothing heard yet"),
                };
                sc.text(8, cy + 16, Font::B10, Align::Left, true, "No messages yet");
                let heard = scene::ellipsize(Font::R8, &heard, wi - 16);
                sc.text(8, cy + 32, Font::R8, Align::Left, true, &heard);
            }
        }
        // The launcher: 3 × 2 tiles.
        let sel = self.state.selected().unwrap_or(0) % LAUNCHER.len();
        let (tw, tile_x) = super::pixel::tile_columns(w, 3);
        let th = 33u16;
        let new = dev.unread_alerting();
        for (i, (label, d, icon)) in LAUNCHER.iter().enumerate() {
            let (col, row) = (i as i16 % 3, i as i16 / 3);
            let x = tile_x(col);
            let y = cy + ch as i16 + 4 + row * (th as i16 + super::pixel::TILE_GAP);
            let count = if *d == Dest::Messages { new } else { 0 };
            super::pixel::tile(&mut sc, x, y, tw, th, *icon, label, count, i == sel);
        }
        Some(sc)
    }

    fn paints(&self, body: Rect, ui: &Ui<'_>) -> Vec<(Rect, Paint)> {
        let t = ui.ctx.time().map(|t| (t.hour, t.minute));
        alloc::vec![(rows(body, 0, Self::clock_rows(ui)), Paint::Clock(t))]
    }
}
