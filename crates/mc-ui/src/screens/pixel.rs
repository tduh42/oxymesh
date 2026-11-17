//! What the pixel screens share (`docs/spec/ui.md` §1, ADR-0011): sizes by text size, initials,
//! the rich list (badge, name, age, preview, unread dot) and icon tiles.

use alloc::string::String;

use crate::app::Ui;
use crate::panel::COMPACT;
use crate::scene::{self, Align, Font, Icon, IconSize, Scene};

/// Left margin of text that starts a line on a pixel screen (lists without badges, notes,
/// cards).
pub const MARGIN: i16 = 8;

/// Fonts and row heights by text size: large (default) or compact (smaller text, more rows).
#[derive(Debug, Clone, Copy)]
pub struct Sizes {
    pub title: Font,
    pub title_bold: Font,
    pub body: Font,
    pub small: Font,
    pub small_bold: Font,
    /// A list row with a preview line.
    pub row2: i16,
    /// A list row of one line.
    pub row1: i16,
    pub badge: u16,
}

pub fn sizes(ui: &Ui<'_>) -> Sizes {
    if ui.grid == COMPACT {
        Sizes {
            title: Font::R8,
            title_bold: Font::B8,
            body: Font::R8,
            small: Font::R8,
            small_bold: Font::B8,
            row2: 25,
            row1: 17,
            badge: 19,
        }
    } else {
        Sizes {
            title: Font::R10,
            title_bold: Font::B10,
            body: Font::R10,
            small: Font::R8,
            small_bold: Font::B8,
            row2: 33,
            row1: 22,
            badge: 22,
        }
    }
}

/// Up to two letters: the first of the first two words, else the first two letters.
pub fn initials(name: &str) -> String {
    let words: alloc::vec::Vec<&str> = name.split_whitespace().collect();
    let mut out = String::new();
    match words.as_slice() {
        [] => out.push('?'),
        [one] => out.extend(one.chars().filter(|c| c.is_alphanumeric()).take(2)),
        [a, b, ..] => {
            out.extend(a.chars().filter(|c| c.is_alphanumeric()).take(1));
            out.extend(b.chars().filter(|c| c.is_alphanumeric()).take(1));
        }
    }
    if out.is_empty() {
        out.push('?');
    }
    out.to_uppercase()
}

/// What sits on the left of a row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Badge {
    /// Initials in a ring.
    Initials(String),
    /// `#` in a ring (a channel).
    Hash,
    Icon(Icon),
}

/// The unread mark on the right of a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dot {
    None,
    Unread,
    /// Unread in a muted channel: a ring, not a disc.
    Muted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub badge: Option<Badge>,
    pub title: String,
    pub bold: bool,
    pub right: String,
    /// A second line (preview), or none for a one-line row.
    pub sub: Option<String>,
    pub dot: Dot,
}

impl Row {
    /// A one-line row of a menu or a picker: a label, its value on the right, an icon or none.
    pub fn item(icon: Option<Icon>, label: &str, value: String) -> Self {
        Self {
            badge: icon.map(Badge::Icon),
            title: String::from(label),
            bold: false,
            right: value,
            sub: None,
            dot: Dot::None,
        }
    }

    /// A one-line row: an icon and a label (`+ New message`).
    pub fn action(icon: Icon, label: &str) -> Self {
        Self {
            badge: Some(Badge::Icon(icon)),
            title: String::from(label),
            bold: true,
            right: String::new(),
            sub: None,
            dot: Dot::None,
        }
    }
}

/// The rows from the top of a `w`×`h` body, the selected one reversed, scrolled so that it
/// shows.
pub fn rich_list(ui: &Ui<'_>, w: u16, h: u16, rows: &[Row], selected: usize) -> Scene {
    let s = sizes(ui);
    let w = w as i16;
    let height = |r: &Row| if r.sub.is_some() { s.row2 } else { s.row1 };
    // The first row so that the selected one fits.
    let mut first = 0;
    while first < selected {
        let used: i16 = rows
            .get(first..=selected)
            .unwrap_or(&[])
            .iter()
            .map(height)
            .sum();
        if used <= h as i16 {
            break;
        }
        first += 1;
    }
    // A list with no badge at all (a settings page) starts at the margin; one badge anywhere
    // (the current choice's check mark) keeps every row aligned past it.
    let badges = rows.iter().any(|r| r.badge.is_some());
    let mut sc = Scene::new();
    let mut y = 0i16;
    for (i, r) in rows.iter().enumerate().skip(first) {
        let rh = height(r);
        if y >= h as i16 {
            break;
        }
        let sel = i == selected;
        let ink = !sel;
        if sel {
            sc.rect(0, y, w as u16, rh as u16, 0, Some(true), None);
        }
        let left = if badges {
            4 + s.badge as i16 + 6
        } else {
            MARGIN
        };
        let cy = y + rh / 2;
        match &r.badge {
            Some(Badge::Initials(t)) => {
                sc.circle(4 + s.badge as i16 / 2, cy, s.badge, false, ink);
                sc.text(
                    4 + s.badge as i16 / 2,
                    cy + Font::B8.ascent() / 2,
                    Font::B8,
                    Align::Center,
                    ink,
                    t,
                );
            }
            Some(Badge::Hash) => {
                sc.circle(4 + s.badge as i16 / 2, cy, s.badge, false, ink);
                sc.text(
                    4 + s.badge as i16 / 2,
                    cy + Font::B8.ascent() / 2,
                    s.small_bold,
                    Align::Center,
                    ink,
                    "#",
                );
            }
            Some(Badge::Icon(icon)) => {
                let (size, px) = if s.badge >= 20 {
                    (IconSize::S16, 16)
                } else {
                    (IconSize::S8, 8)
                };
                sc.icon(4 + (s.badge as i16 - px) / 2, cy - px / 2, *icon, size, ink);
            }
            None => {}
        }
        let title_font = if r.bold { s.title_bold } else { s.title };
        let rw = scene::width(s.small, &r.right);
        let dot_room = if r.dot == Dot::None { 0 } else { 10 };
        let base1 = if r.sub.is_some() {
            y + title_font.ascent() + 3
        } else {
            cy + title_font.ascent() / 2
        };
        if !r.right.is_empty() {
            sc.text(w - 4, base1, s.small, Align::Right, ink, &r.right);
        }
        let room = w - left - 4 - if rw > 0 { rw + 6 } else { dot_room };
        let t = scene::ellipsize(title_font, &r.title, room);
        sc.text(left, base1, title_font, Align::Left, ink, &t);
        if let Some(sub) = &r.sub {
            let base2 = y + rh - 5;
            let p = scene::ellipsize(s.small, sub, w - left - 4 - dot_room);
            sc.text(left, base2, s.small, Align::Left, ink, &p);
            match r.dot {
                Dot::Unread => sc.circle(w - 8, base2 - 3, 6, true, ink),
                Dot::Muted => sc.circle(w - 8, base2 - 3, 6, false, ink),
                Dot::None => {}
            }
        } else {
            match r.dot {
                Dot::Unread => sc.circle(w - 8, cy, 6, true, ink),
                Dot::Muted => sc.circle(w - 8, cy, 6, false, ink),
                Dot::None => {}
            }
        }
        y += rh;
        if !sel && rows.get(i + 1).is_some() && i + 1 != selected {
            sc.hline(left, w - 4, y - 1);
        }
    }
    sc
}

/// Gap between two tiles, and between the tiles and the body's edges (twice).
pub const TILE_GAP: i16 = 2;

/// The width of a tile and the x of each column, for `columns` tiles across a `w`-wide body
/// (Home's launcher and a contact's actions: 3; a channel's: 2).
pub fn tile_columns(w: u16, columns: u16) -> (u16, impl Fn(i16) -> i16) {
    let gaps = 2 * TILE_GAP as u16 * 2 + (columns.max(1) - 1) * TILE_GAP as u16;
    let tw = w.saturating_sub(gaps) / columns.max(1);
    (tw, move |col: i16| {
        2 * TILE_GAP + col * (tw as i16 + TILE_GAP)
    })
}

/// An icon tile with its label, reversed when selected, with a count badge if `count` > 0.
#[allow(clippy::too_many_arguments)]
pub fn tile(
    sc: &mut Scene,
    x: i16,
    y: i16,
    w: u16,
    h: u16,
    icon: Icon,
    label: &str,
    count: usize,
    selected: bool,
) {
    let ink = !selected;
    sc.rect(x, y, w, h, 4, Some(selected), Some(true));
    let cx = x + w as i16 / 2;
    let icon_y = y + (h as i16 - 16 - 10) / 2;
    sc.icon(cx - 8, icon_y, icon, IconSize::S16, ink);
    let l = scene::ellipsize(Font::R8, label, w as i16 - 4);
    sc.text(cx, y + h as i16 - 4, Font::R8, Align::Center, ink, &l);
    if count > 0 {
        let bx = x + w as i16 - 9;
        let by = y + 8;
        sc.circle(bx, by, 13, true, ink);
        let n = if count > 99 {
            String::from("99")
        } else {
            alloc::format!("{count}")
        };
        sc.text(bx, by + 4, Font::B8, Align::Center, !ink, &n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_of_names() {
        assert_eq!(initials("Bob"), "BO");
        assert_eq!(initials("Relay Aiguille"), "RA");
        assert_eq!(initials("  zoé  b "), "ZB");
        assert_eq!(initials("#hiking"), "HI");
        assert_eq!(initials(""), "?");
        assert_eq!(initials("!!"), "?");
    }
}
