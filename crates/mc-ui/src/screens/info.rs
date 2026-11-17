//! Screens that show information: About, the QR cards, the import from the C++ firmware.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use ratatui_core::layout::{Alignment, Rect};
use ratatui_core::terminal::Frame;
use ratatui_core::text::Line;
use ratatui_widgets::paragraph::{Paragraph, Wrap};

use mc_spiffs::cpp::{ImportError, Summary};

use super::{name_of, rows, short_key};
use crate::act::{Intent, Outcome};
use crate::app::{Confirm, Input, Nav, Paint, Screen, Ui};
use crate::chrome::{self, Footer};

/// The device's health for About (ADR-0013): why it last started, always; what went wrong since
/// the power came on, only when something did.
fn health_lines(h: &crate::app::Health<'_>) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    if !h.restart.is_empty() {
        out.push(("Started", String::from(h.restart)));
    }
    if h.panics > 0 {
        let at = match h.panic_at {
            // The file's name is enough on this width.
            Some((file, line)) => format!(" {}:{line}", file.rsplit('/').next().unwrap_or(file)),
            None => String::new(),
        };
        out.push(("Errors", format!("{}{at}", h.panics)));
    }
    let times = |n: u32, what: &str| format!("{n} {what}{}", if n == 1 { "" } else { "s" });
    if h.radio_down {
        out.push(("Radio", String::from("not answering")));
    } else if h.radio_recoveries > 0 {
        out.push(("Radio", times(h.radio_recoveries, "reset")));
    }
    if h.store_errors > 0 {
        out.push(("Storage", times(h.store_errors, "write error")));
    }
    out
}

/// System → About.
#[derive(Debug)]
pub struct About;

impl Screen for About {
    fn title(&self, _ui: &Ui<'_>) -> String {
        String::from("About")
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        let w = usize::from(body.width);
        let lines: Vec<Line<'_>> = about_rows(ui)
            .iter()
            .map(|(k, v)| Line::from(chrome::spread(k, v, w)))
            .collect();
        f.render_widget(Paragraph::new(lines), body);
    }

    /// On the panel: the same rows as labelled values.
    fn scene(&mut self, w: u16, _h: u16, ui: &Ui<'_>) -> Option<crate::scene::Scene> {
        let mut sc = crate::scene::Scene::new();
        super::pixel::fields(&mut sc, w, 0, &about_rows(ui));
        Some(sc)
    }

    fn footer(&self, _ui: &Ui<'_>) -> Footer {
        Footer::BACK
    }
}

/// What About shows: who this is, its health, the radio, what it holds.
fn about_rows(ui: &Ui<'_>) -> Vec<(&'static str, String)> {
    let c = ui.ctx;
    let dev = c.dev;
    let p = &dev.node.prefs;
    let r = &p.radio;
    let channels = dev.node.channels.0.iter().filter(|c| !c.is_empty()).count();
    let up = c.now_ms / 1000;
    let volts = if dev.battery_mv == 0 {
        String::from("--")
    } else {
        format!(
            "{}.{:02} V",
            dev.battery_mv / 1000,
            dev.battery_mv % 1000 / 10
        )
    };
    let battery = match c.battery_percent {
        Some(pc) => format!("{volts} {pc}%"),
        None => volts,
    };
    let mut pairs: Vec<(&str, String)> = alloc::vec![
        ("Name", name_of(&p.name)),
        ("Key", short_key(dev.node.id.pub_key())),
        ("Firmware", String::from(dev.info.version)),
    ];
    pairs.extend(health_lines(&c.health));
    pairs.extend([
        (
            "Frequency",
            format!("{}.{:03} MHz", r.freq_khz / 1000, r.freq_khz % 1000),
        ),
        (
            "Modulation",
            format!(
                "SF{} BW{}.{} CR4/{}",
                r.sf,
                r.bw_hz / 1000,
                r.bw_hz % 1000 / 100,
                r.cr
            ),
        ),
        ("Power", format!("{} dBm", r.tx_power_dbm)),
        (
            "Contacts",
            format!(
                "{}/{}",
                dev.node.contacts.list.len(),
                mc_node::contacts::MAX_CONTACTS
            ),
        ),
        (
            "Channels",
            format!("{channels}/{}", mc_node::contacts::MAX_CHANNELS),
        ),
        ("Battery", battery),
        ("Uptime", format!("{} h {:02} min", up / 3600, up / 60 % 60)),
        ("For the app", format!("{} queued", dev.queued_messages())),
    ]);
    pairs
}

/// A contact card as a QR the official app scans: ours (Identity → My card) or a contact's.
#[derive(Debug, Default)]
pub struct Card {
    /// `None`: this device.
    of: Option<(String, [u8; 32])>,
}

impl Card {
    pub fn mine() -> Self {
        Self { of: None }
    }

    pub fn of(name: &str, key: &[u8; 32]) -> Self {
        Self {
            of: Some((String::from(name), *key)),
        }
    }

    fn who(&self, ui: &Ui<'_>) -> (String, [u8; 32]) {
        match &self.of {
            Some((n, k)) => (n.clone(), *k),
            None => (
                name_of(&ui.ctx.dev.node.prefs.name),
                *ui.ctx.dev.node.id.pub_key(),
            ),
        }
    }
}

/// `meshcore://contact/add?name=…&public_key=…&type=1`, the official app's contact link.
pub fn contact_link(name: &str, key: &[u8; 32]) -> String {
    let mut n = String::new();
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            n.push(char::from(b));
        } else {
            n.push_str(&format!("%{b:02X}"));
        }
    }
    format!(
        "meshcore://contact/add?name={n}&public_key={}&type=1",
        mc_core::Hex(key)
    )
}

impl Screen for Card {
    fn title(&self, _ui: &Ui<'_>) -> String {
        String::from(if self.of.is_none() { "My card" } else { "Card" })
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        let (name, key) = self.who(ui);
        let qr = rows(body, 0, body.height.saturating_sub(1));
        // Text fallback under the QR (the terminal cannot show it).
        f.render_widget(
            Paragraph::new(alloc::vec![
                Line::from(""),
                Line::from("QR code shown"),
                Line::from("on the device:"),
                Line::from("scan it with the"),
                Line::from("MeshCore app."),
            ])
            .alignment(Alignment::Center),
            qr,
        );
        f.render_widget(
            Paragraph::new(format!("{name} {} {}", ui.sym.dot, short_key(&key)))
                .alignment(Alignment::Center),
            rows(body, body.height.saturating_sub(1), 1),
        );
    }

    fn footer(&self, _ui: &Ui<'_>) -> Footer {
        Footer::BACK
    }

    fn paints(&self, body: Rect, ui: &Ui<'_>) -> Vec<(Rect, Paint)> {
        let (name, key) = self.who(ui);
        let link = contact_link(&name, &key);
        alloc::vec![(
            rows(body, 0, body.height.saturating_sub(1)),
            Paint::Qr(link)
        )]
    }
}

/// System → Import from C++ (MC6): what the MeshCore C++ firmware left on the device, and a
/// hold to bring it over. The runner reads the partition (`Intent::CppFind`).
#[derive(Debug, Default)]
pub struct ImportCpp {
    found: Option<Result<Summary, ImportError>>,
}

impl Screen for ImportCpp {
    fn title(&self, _ui: &Ui<'_>) -> String {
        String::from("Import from C++")
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        let w = usize::from(body.width);
        let lines: Vec<String> = match &self.found {
            None => alloc::vec![String::from(
                "Looking for the MeshCore C++ firmware's data…"
            )],
            Some(Err(ImportError::NoIdentity)) => alloc::vec![String::from(
                "No MeshCore C++ data on this device: nothing to import."
            )],
            Some(Err(e)) => alloc::vec![
                String::from("The C++ data cannot be read:"),
                format!("{e:?}"),
            ],
            Some(Ok(s)) => {
                let radio = s.radio.map_or(String::from("defaults"), |r| {
                    format!(
                        "{}.{:03} MHz SF{} CR{}",
                        r.freq_khz / 1000,
                        r.freq_khz % 1000,
                        r.sf,
                        r.cr
                    )
                });
                let mut v = alloc::vec![
                    format!("Name {}", name_of(&s.name)),
                    format!("Key {}", short_key(&s.key)),
                    format!(
                        "{} contacts {} {} channels",
                        s.contacts, ui.sym.dot, s.channels
                    ),
                    format!("Radio {radio}"),
                    String::new(),
                ];
                let same = s.key == *ui.ctx.dev.node.id.pub_key();
                v.push(String::from(if same {
                    "This device already has this identity."
                } else {
                    "Hold A: it replaces this device's identity, contacts, channels and settings, then reboots."
                }));
                v
            }
        };
        let text: Vec<Line<'_>> = lines
            .iter()
            .map(|l| Line::from(chrome::fit(l, w * 4)))
            .collect();
        f.render_widget(
            Paragraph::new(text).wrap(Wrap { trim: true }),
            rows(body, 0, body.height),
        );
    }

    /// On the panel: what the C++ firmware left, as labelled values, then what a hold does.
    fn scene(&mut self, w: u16, _h: u16, ui: &Ui<'_>) -> Option<crate::scene::Scene> {
        let mut sc = crate::scene::Scene::new();
        match &self.found {
            None => {
                super::pixel::note(
                    &mut sc,
                    w,
                    4,
                    "Looking for the MeshCore C++ firmware's data…",
                );
            }
            Some(Err(ImportError::NoIdentity)) => {
                super::pixel::note(
                    &mut sc,
                    w,
                    4,
                    "No MeshCore C++ data on this device: nothing to import.",
                );
            }
            Some(Err(e)) => {
                let y = super::pixel::note(&mut sc, w, 4, "The C++ data cannot be read:");
                super::pixel::note(&mut sc, w, y, &format!("{e:?}"));
            }
            Some(Ok(s)) => {
                let radio = s.radio.map_or(String::from("defaults"), |r| {
                    format!(
                        "{}.{:03} MHz SF{} CR{}",
                        r.freq_khz / 1000,
                        r.freq_khz % 1000,
                        r.sf,
                        r.cr
                    )
                });
                let rows = [
                    ("Name", name_of(&s.name)),
                    ("Key", short_key(&s.key)),
                    ("Contacts", format!("{}", s.contacts)),
                    ("Channels", format!("{}", s.channels)),
                    ("Radio", radio),
                ];
                let y = super::pixel::fields(&mut sc, w, 0, &rows);
                let same = s.key == *ui.ctx.dev.node.id.pub_key();
                super::pixel::note(
                    &mut sc,
                    w,
                    y + 4,
                    if same {
                        "This device already has this identity."
                    } else {
                        "Hold A: it replaces this device's identity, contacts, channels and settings, then reboots."
                    },
                );
            }
        }
        Some(sc)
    }

    fn input(&mut self, input: Input, _ui: &Ui<'_>) -> Nav {
        match (&self.found, input) {
            (Some(Ok(_)), Input::Open) => Nav::Confirm(
                Confirm::new(
                    "Import",
                    "Replace this device's identity, contacts, channels and settings with the C++ firmware's, then reboot.",
                    Intent::CppImport,
                )
                .with_toast("Importing"),
            ),
            _ => Nav::Stay,
        }
    }

    fn result(&mut self, _intent: &Intent, outcome: &Outcome, _ui: &Ui<'_>) -> Nav {
        if let Outcome::Cpp(r) = outcome {
            self.found = Some(*r);
        }
        Nav::Stay
    }

    fn footer(&self, _ui: &Ui<'_>) -> Footer {
        match self.found {
            Some(Ok(_)) => Footer {
                a_click: None,
                a_hold: Some("import"),
                a_double: None,
                b_click: Some("back"),
                b_hold: None,
            },
            _ => Footer::BACK,
        }
    }
}
