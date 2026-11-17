//! The screens (`docs/spec/ui.md` §6) and what they share: list drawing and cursor moves.

pub mod admin;
pub mod chat;
pub mod contact;
mod home;
mod info;
mod lists;
mod menu;
pub(crate) mod pixel;
pub mod settings;
pub mod setup;
pub mod tools;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use ratatui_core::layout::Rect;
use ratatui_core::terminal::Frame;
use ratatui_core::text::Line;
use ratatui_widgets::list::{List, ListState};

use crate::app::{Input, Screen, Ui};
use crate::chrome;
use crate::theme;

pub use chat::{Compose, Conversation, Messages, Target};
pub use contact::{ChannelScreen, ContactScreen};
pub use home::Home;
pub use info::{About, Card, contact_link as info_link};
pub use lists::{Channels, Contacts, Pick};
pub use menu::{Action, Item, Menu};

/// The noise floor as text. It reads 0 until the first sampling round ends (about 2 s after
/// boot or a radio reset), which is no measurement.
pub fn noise_text(dbm: i16) -> String {
    if dbm == 0 {
        String::from("-- dBm")
    } else {
        alloc::format!("{dbm} dBm")
    }
}

/// Where a menu item leads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dest {
    Messages,
    Contacts,
    Channels,
    Tools,
    Settings,
    Identity,
    System,
    About,
    MyCard,
    /// First-boot setup, again.
    Setup,
    /// Edit the node's name.
    Name,
    Heard,
    TracePick,
    RangePick,
    Signal,
    Packets,
    Airtime,
    Gps,
    Map,
    Discover,
    AdminPick,
    Radio,
    Bluetooth,
    ContactSettings,
    Display,
    Notifications,
    Telemetry,
    Power,
    Time,
}

pub fn open(d: Dest) -> Box<dyn Screen> {
    match d {
        Dest::Messages => Box::new(Messages::default()),
        Dest::Contacts => Box::new(Contacts::default()),
        Dest::Channels => Box::new(Channels::default()),
        Dest::Tools => Box::new(menu::tools()),
        Dest::Settings => Box::new(menu::settings()),
        Dest::Identity => Box::new(menu::identity()),
        Dest::System => Box::new(menu::system()),
        Dest::About => Box::new(About),
        Dest::MyCard => Box::new(Card::mine()),
        Dest::Setup => Box::new(setup_start()),
        Dest::Name => Box::new(Compose::new(Target::Name { setup: false }, "")),
        Dest::Heard => Box::new(tools::HeardNodes::default()),
        Dest::TracePick => Box::new(Contacts::pick_for(lists::Pick::Trace)),
        Dest::RangePick => Box::new(Contacts::pick_for(lists::Pick::Range)),
        Dest::Signal => Box::new(tools::Signal),
        Dest::Packets => Box::new(tools::Packets::default()),
        Dest::Airtime => Box::new(tools::Airtime),
        Dest::Gps => Box::new(tools::Gps::default()),
        Dest::Map => Box::new(tools::Map::default()),
        Dest::Discover => Box::new(admin::Discover::default()),
        Dest::AdminPick => Box::new(Contacts::pick_for(lists::Pick::Admin)),
        Dest::Radio => Box::new(settings::radio()),
        Dest::Bluetooth => Box::new(settings::bluetooth()),
        Dest::ContactSettings => Box::new(settings::contacts()),
        Dest::Display => Box::new(settings::display()),
        Dest::Notifications => Box::new(settings::notifications()),
        Dest::Telemetry => Box::new(settings::telemetry()),
        Dest::Power => Box::new(settings::power()),
        Dest::Time => Box::new(settings::time()),
    }
}

/// The first screen of the first-boot setup.
pub fn setup_start() -> setup::SetupRegion {
    setup::SetupRegion::default()
}

/// Shown in place of a message's text when previews are off (`docs/spec/ui.md` §8).
pub const HIDDEN_TEXT: &str = "Message hidden";

/// What notifications and previews show (Settings → Notifications → Message previews).
pub fn previews(ui: &Ui<'_>) -> mc_node::Previews {
    ui.ctx.dev.node.prefs.ui.previews
}

/// Who a new message is from, as a notification says it: the name, or "New message".
pub fn notice_sender(ui: &Ui<'_>, peer: mc_node::history::Peer) -> String {
    if previews(ui).sender() {
        peer_name(ui, peer)
    } else {
        String::from("New message")
    }
}

/// A message's text outside its conversation: the text, or [`HIDDEN_TEXT`].
pub fn preview_text(ui: &Ui<'_>, m: &mc_node::history::Msg) -> String {
    if previews(ui).text() {
        String::from(String::from_utf8_lossy(&m.text))
    } else {
        String::from(HIDDEN_TEXT)
    }
}

/// Who a conversation is with: contact name, channel name, or the key prefix.
pub fn peer_name(ui: &Ui<'_>, peer: mc_node::history::Peer) -> String {
    let node = &ui.ctx.dev.node;
    match peer {
        mc_node::history::Peer::Contact(p) => node
            .contacts
            .find_prefix(&p)
            .and_then(|s| node.contacts.get(s))
            .map(|c| name_of(&c.name))
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| short_key(&p)),
        mc_node::history::Peer::Channel(i) => node
            .channels
            .0
            .get(usize::from(i))
            .map(|c| name_of(&c.name))
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| format!("Channel {i}")),
    }
}

/// Moves the cursor of a list of `len` rows; true if the selected row is to be opened.
pub(crate) fn step(state: &mut ListState, len: usize, input: Input) -> bool {
    if len == 0 {
        return false;
    }
    let sel = state.selected().unwrap_or(0).min(len - 1);
    match input {
        Input::Next(n) => state.select(Some((sel + usize::from(n)) % len)),
        Input::Prev => state.select(Some((sel + len - 1) % len)),
        Input::Open => {
            state.select(Some(sel));
            return true;
        }
    }
    false
}

/// Rows of `label … detail`, the selected one reversed and marked.
pub(crate) fn draw_list(
    f: &mut Frame<'_>,
    area: Rect,
    rows: &[(String, String)],
    state: &mut ListState,
    ui: &Ui<'_>,
) {
    if state.selected().is_none() && !rows.is_empty() {
        state.select(Some(0));
    }
    let marker = ui.sym.selected;
    let width = usize::from(area.width).saturating_sub(marker.chars().count());
    let items: Vec<Line<'_>> = rows
        .iter()
        .map(|(l, d)| Line::from(chrome::spread(l, d, width)))
        .collect();
    f.render_stateful_widget(
        List::new(items)
            .highlight_style(theme::reversed())
            .highlight_symbol(marker),
        area,
        state,
    );
}

/// `n` rows down from the top of `body`, `h` rows tall (clipped to the body).
pub(crate) fn rows(body: Rect, n: u16, h: u16) -> Rect {
    let y = (body.y + n).min(body.bottom());
    Rect {
        y,
        height: h.min(body.bottom() - y),
        ..body
    }
}

/// Short public key: first 4 bytes in hex.
pub(crate) fn short_key(k: &[u8]) -> String {
    format!("{}", mc_core::Hex(k.get(..4).unwrap_or(k)))
}

pub(crate) fn name_of(b: &[u8]) -> String {
    String::from_utf8_lossy(mc_node::contacts::name_str(b)).into_owned()
}
