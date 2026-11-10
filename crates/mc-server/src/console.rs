//! The repeater console (`MyMesh::handleCommand`, `examples/simple_repeater/MyMesh.cpp:1198-1287`;
//! `CommonCLI`, `src/helpers/CommonCLI.cpp`; MeshCore v1.18, MIT; `docs/spec/server.md` §4):
//! from admins over the air, and from the serial port (timestamp 0). Replies are the C++ texts.
//! Commands are matched like the C++ does (`memcmp`): by prefix, so `rebootx` reboots.

use core::fmt::Write;

use mc_air::LocalIdentity;
use mc_air::crypto::{PRV_KEY_LEN, PUB_KEY_LEN};
use mc_core::civil::Civil;
use mc_core::{Duration, Instant};
use mc_node::Node;
use mc_node::cli::{self, Effect, Reply};
use mc_node::dispatch::RadioParams;
use mc_node::node::Routing;
use mc_node::region::{DENY_FLOOD, is_name_char};
use rand_core::Rng;

use crate::acl;
use crate::server::{Action, Role, Server};

/// `MIN_LOCAL_ADVERT_INTERVAL`, minutes.
const MIN_LOCAL_ADVERT_MINS: u32 = 60;
/// `clkreboot` sets the clock to this (15 May 2024) before restarting.
const CLKREBOOT_EPOCH: u32 = 1_715_770_351;
/// `tempradio`: applied this long after the reply.
const TEMP_RADIO_DELAY_MS: u64 = 2000;
/// Neighbours listed while the reply is shorter than this (`formatNeighborsReply`).
const NEIGHBOURS_TEXT_MAX: usize = 134;

/// A region table being loaded line by line (`region load`, `MyMesh.cpp:1199-1231`).
#[derive(Debug, Clone)]
pub(crate) struct RegionLoad {
    /// The parent at each indent (region ids; 0 is the wildcard).
    stack: [Option<u16>; 8],
}

/// The C++ `_atoi`: decimal digits up to the first other character.
fn atoi_digits(s: &str) -> u32 {
    let mut n = 0u32;
    for b in s.bytes() {
        if !b.is_ascii_digit() {
            break;
        }
        n = n.wrapping_mul(10).wrapping_add(u32::from(b - b'0'));
    }
    n
}

/// `Utils::parseTextParts`: up to `max` pieces between `sep`; text after them is ignored, and
/// a trailing separator adds no empty piece.
fn parts(s: &str, sep: char, max: usize) -> heapless::Vec<&str, 5> {
    let mut v: heapless::Vec<&str, 5> = heapless::Vec::new();
    let mut it = s.split(sep).peekable();
    while let Some(p) = it.next() {
        if v.len() >= max {
            break;
        }
        if it.peek().is_none() && p.is_empty() {
            break;
        }
        let _ = v.push(p);
    }
    v
}

/// `Utils::fromHex`: exactly `out.len()` × 2 characters; other characters read as 0.
fn from_hex(s: &str, out: &mut [u8]) -> bool {
    if s.len() != out.len() * 2 {
        return false;
    }
    let val = |c: u8| match c {
        b'A'..=b'F' => c - b'A' + 10,
        b'a'..=b'f' => c - b'a' + 10,
        b'0'..=b'9' => c - b'0',
        _ => 0,
    };
    for (o, pair) in out.iter_mut().zip(s.as_bytes().chunks(2)) {
        let (Some(&h), Some(&l)) = (pair.first(), pair.get(1)) else {
            return false;
        };
        *o = (val(h) << 4) | val(l);
    }
    true
}

/// `Utils::toHex`: uppercase.
fn push_hex(out: &mut Reply, b: &[u8]) {
    for x in b {
        let _ = write!(out, "{x:02X}");
    }
}

fn push_bytes(out: &mut Reply, b: &[u8]) {
    for &c in b {
        if out.push(char::from(c)).is_err() {
            break;
        }
    }
}

/// `StrHelper::strncpy` into a C buffer of `N + 1` bytes.
fn copy_str<const N: usize>(dst: &mut heapless::String<N>, src: &str) {
    dst.clear();
    for c in src.chars() {
        if dst.push(c).is_err() {
            break;
        }
    }
}

/// `"%02d:%02d - %d/%d/%d UTC"` of an RTClib `DateTime`.
fn push_clock(out: &mut Reply, unix: u32) {
    let d = Civil::from_unix(unix);
    let _ = write!(
        out,
        "{:02}:{:02} - {}/{}/{} UTC",
        d.hour, d.minute, d.day, d.month, d.year
    );
}

/// `"%.2f"` of SNR × 4 / 4.
fn push_snr(out: &mut Reply, snr4: i8) {
    let h = i32::from(snr4) * 25;
    let sign = if h < 0 { "-" } else { "" };
    let a = h.unsigned_abs();
    let _ = write!(out, "{sign}{}.{:02}", a / 100, a % 100);
}

/// `LocalIdentity::validatePrivateKey` (`src/Identity.cpp:67-108`): the public key does not
/// start with 0x00 or 0xFF, and a key exchange with a known test key pair agrees both ways.
pub fn valid_private_key(prv: &[u8; PRV_KEY_LEN]) -> Option<LocalIdentity> {
    const TEST_PRV: [u8; PRV_KEY_LEN] = [
        0x70, 0x65, 0xe1, 0x8f, 0xd9, 0xfa, 0xbb, 0x70, 0xc1, 0xed, 0x90, 0xdc, 0xa1, 0x99, 0x07,
        0xde, 0x69, 0x8c, 0x88, 0xb7, 0x09, 0xea, 0x14, 0x6e, 0xaf, 0xd9, 0x3d, 0x9b, 0x83, 0x0c,
        0x7b, 0x60, 0xc4, 0x68, 0x11, 0x93, 0xc7, 0x9b, 0xbc, 0x39, 0x94, 0x5b, 0xa8, 0x06, 0x41,
        0x04, 0xbb, 0x61, 0x8f, 0x8f, 0xd7, 0xa8, 0x4a, 0x0a, 0xf6, 0xf5, 0x70, 0x33, 0xd6, 0xe8,
        0xdd, 0xcd, 0x64, 0x71,
    ];
    let id = LocalIdentity::from_private(prv);
    if id.is_reserved() {
        return None;
    }
    let test = LocalIdentity::from_private(&TEST_PRV);
    let ss1 = id.shared_secret(test.pub_key())?;
    let ss2 = test.shared_secret(id.pub_key())?;
    (ss1 == ss2 && ss1.iter().any(|&b| b != 0)).then_some(id)
}

impl Server {
    /// One console line (`handleCommand`). `ts` is the sender's timestamp, 0 from the serial
    /// port; `serial` receives what the C++ prints there directly (`get acl`).
    #[allow(clippy::too_many_arguments)]
    pub fn command(
        &mut self,
        node: &mut Node,
        ts: u32,
        text: &[u8],
        out: &mut Reply,
        serial: &mut dyn Write,
        now: Instant,
        rng: &mut impl Rng,
    ) {
        let text = mc_node::contacts::name_str(text);
        let Ok(mut c) = core::str::from_utf8(text) else {
            let _ = out.push_str("Unknown command");
            return;
        };
        if self.region_load.is_some() {
            self.region_load_line(node, c, out);
            return;
        }
        c = c.trim_start_matches(' ');
        // Optional "xx|" prefix (companion radio CLI), reflected back.
        if c.len() > 4
            && c.as_bytes().get(2) == Some(&b'|')
            && let (Some(tag), Some(rest)) = (c.get(..3), c.get(3..))
        {
            let _ = out.push_str(tag);
            c = rest;
        }
        // The rest of the reply, after the prefix (the C++ writes past it).
        let mut body = Reply::new();
        self.dispatch(node, ts, c, &mut body, serial, now, rng);
        for ch in body.chars() {
            if out.push(ch).is_err() {
                break;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn dispatch(
        &mut self,
        node: &mut Node,
        ts: u32,
        c: &str,
        out: &mut Reply,
        serial: &mut dyn Write,
        now: Instant,
        rng: &mut impl Rng,
    ) {
        if let Some(rest) = c.strip_prefix("setperm ") {
            self.setperm(node, rest, out, now);
        } else if ts == 0 && c == "get acl" {
            let _ = serial.write_str("ACL:\n");
            for cl in self.acl.clients.iter().filter(|cl| cl.permissions != 0) {
                let _ = write!(serial, "{:02X} ", cl.permissions);
                for b in &cl.key {
                    let _ = write!(serial, "{b:02X}");
                }
                let _ = serial.write_str("\n");
            }
        } else if self.role == Role::Room && c.starts_with("room.post") {
            // `room.post {text}`: a post by the room itself.
            let msg = c.get(9..).unwrap_or("").trim_start_matches(' ');
            if msg.is_empty() {
                let _ = out.push_str("ERR empty message");
            } else {
                let id = *node.id.pub_key();
                self.add_post(node, &id, msg, now);
                let _ = out.push_str("OK");
            }
        } else if self.role == Role::Room && c.starts_with("set repeat on") {
            let _ = out.push_str("ERR - repeat now deprecated");
        } else if let Some(sub) = c
            .strip_prefix("discover.neighbors")
            .filter(|_| self.role == Role::Repeater)
        {
            if sub.trim_start_matches(' ').is_empty() {
                self.discover(node, now, rng);
                let _ = out.push_str("OK - Discover sent");
            } else {
                let _ = out.push_str("Err - discover.neighbors has no options");
            }
        } else {
            self.common(node, ts, c, out, now);
        }
    }

    /// `setperm {pubkey-hex} {permissions}`.
    fn setperm(&mut self, node: &mut Node, rest: &str, out: &mut Reply, now: Instant) {
        let Some((hex, perms)) = rest.split_once(' ') else {
            let _ = out.push_str("Err - bad params");
            return;
        };
        // The C++ counts the NUL it put in place of the space (`sp - hex`).
        let n = (hex.len() + 1).min(PUB_KEY_LEN * 2) / 2;
        let mut key = [0u8; PUB_KEY_LEN];
        let Some(k) = key.get_mut(..n) else { return };
        if !from_hex(hex, k) {
            let _ = out.push_str("Err - bad pubkey");
            return;
        }
        let perms = cli::atoi(perms) as u8;
        match self.acl.apply(k, perms) {
            Some(put) => {
                // A client removed, or evicted to make room: no longer a contact.
                if let acl::Put::Evicted(_, gone) = put {
                    crate::server::forget_contact(node, &gone);
                }
                if perms & acl::ROLE_MASK != acl::GUEST
                    && let Ok(full) = <[u8; PUB_KEY_LEN]>::try_from(k as &[u8])
                {
                    crate::server::ensure_contact(node, &full);
                }
                self.save_acl_soon(now);
                let _ = out.push_str("OK");
            }
            None => {
                let _ = out.push_str("Err - invalid params");
            }
        }
    }

    /// `CommonCLI::savePrefs`: a node configured by hand stops its 2-minute local adverts.
    fn save_prefs(&mut self) {
        if u32::from(self.prefs.advert_interval) * 2 < MIN_LOCAL_ADVERT_MINS {
            self.prefs.advert_interval = 0;
        }
        self.push(Action::SavePrefs);
    }

    /// The node's forwarding rules, the role's defaults if it has none yet.
    fn routing(role: Role, node: &mut Node) -> &mut Routing {
        node.routing.get_or_insert(role.routing())
    }

    /// `CommonCLI::handleCommand`.
    fn common(&mut self, node: &mut Node, ts: u32, c: &str, out: &mut Reply, now: Instant) {
        if let Some(effect) = cli::radio_command(&mut node.prefs, c, out) {
            if effect != Effect::None {
                self.save_prefs();
            }
            if matches!(effect, Effect::TxPower | Effect::RxGain) {
                self.push(Action::RadioChanged);
            }
            return;
        }
        if self.board_command(node, c, out) {
            return;
        }
        let clock = node.rtc.now(now);
        let w = |out: &mut Reply, s: &str| {
            let _ = out.push_str(s);
        };
        if c.starts_with("poweroff") || c.starts_with("shutdown") {
            self.push(Action::PowerOff);
        } else if c.starts_with("reboot") {
            self.push(Action::Reboot);
        } else if c.starts_with("clkreboot") {
            node.rtc.set(now, CLKREBOOT_EPOCH);
            self.push(Action::ClockSet);
            self.push(Action::Reboot);
        } else if c
            .strip_prefix("advert.zerohop")
            .is_some_and(|r| r.is_empty() || r.starts_with(' '))
        {
            self.send_advert(node, now, 1500, false);
            w(out, "OK - zerohop advert sent");
        } else if c.starts_with("advert") {
            self.send_advert(node, now, 1500, true);
            w(out, "OK - Advert sent");
        } else if c.starts_with("clock sync") {
            if ts > clock {
                node.rtc.set(now, ts.wrapping_add(1));
                self.push(Action::ClockSet);
                w(out, "OK - clock set: ");
                push_clock(out, node.rtc.now(now));
            } else {
                w(out, "ERR: clock cannot go backwards");
            }
        } else if c.starts_with("start ota") {
            w(out, "Error");
        } else if c.starts_with("clock") {
            push_clock(out, clock);
        } else if let Some(v) = c.strip_prefix("time ") {
            let secs = atoi_digits(v);
            if secs > clock {
                node.rtc.set(now, secs);
                self.push(Action::ClockSet);
                w(out, "OK - clock set: ");
                push_clock(out, node.rtc.now(now));
            } else {
                w(out, "(ERR: clock cannot go backwards)");
            }
        } else if c.starts_with("neighbors") {
            self.neighbours_text(clock, out);
        } else if let Some(hex) = c.strip_prefix("neighbor.remove ") {
            let n = hex.len().min(PUB_KEY_LEN * 2) / 2;
            let mut key = [0u8; PUB_KEY_LEN];
            let k = key.get_mut(..n).unwrap_or(&mut []);
            if from_hex(hex, k) {
                self.neighbours.remove(k);
                w(out, "OK");
            } else {
                w(out, "ERR: bad pubkey");
            }
        } else if let Some(args) = c.strip_prefix("tempradio ") {
            self.temp_radio(node, args, out, now);
        } else if let Some(pw) = c.strip_prefix("password ") {
            copy_str(&mut self.prefs.admin_password, pw);
            self.save_prefs();
            w(out, "password now: ");
            let _ = out.push_str(&self.prefs.admin_password);
        } else if c.starts_with("clear stats") {
            node.disp.stats = mc_node::dispatch::Stats::ZERO;
            node.seen.direct_dups = 0;
            node.seen.flood_dups = 0;
            w(out, "(OK - stats reset)");
        } else if c.starts_with("get ") {
            self.get(node, ts, c.get(4..).unwrap_or(""), out, now);
        } else if c.starts_with("set ") {
            self.set(node, ts, c.get(4..).unwrap_or(""), out, now);
        } else if ts == 0 && c == "erase" {
            self.push(Action::Erase);
            w(out, "File system erase: OK");
        } else if c.starts_with("ver") {
            let _ = write!(out, "{} (Build: {})", self.version, self.board.build_date);
        } else if c.starts_with("board") {
            w(out, self.board.manufacturer);
        } else if c == "power" {
            let yn = |b: bool| if b { "yes" } else { "no" };
            let opt = |b: Option<bool>| b.map_or("n/a", yn);
            let b = &self.board;
            let _ = write!(
                out,
                "batt:{} mV usb:{} solar_chg:{} ext:{} charger:{}",
                b.battery_mv,
                opt(b.usb_powered),
                opt(b.solar_charging),
                yn(b.external_power),
                yn(b.charging)
            );
        } else if let Some(key) = c.strip_prefix("sensor get ") {
            match self.sensor_value(node, key) {
                Some(v) => {
                    w(out, "> ");
                    w(out, v);
                }
                None => w(out, "null"),
            }
        } else if let Some(args) = c.strip_prefix("sensor set ") {
            let p = parts(args, ' ', 2);
            let key = p.first().copied().unwrap_or("");
            let value = p.get(1).copied().unwrap_or("null");
            if self.set_sensor(node, key, value) {
                w(out, "ok");
            } else {
                w(out, "can't find custom var");
            }
        } else if c.starts_with("sensor list") {
            self.sensor_list(node, c, out);
        } else if c.starts_with("region") {
            self.region_command(node, c, out, now);
        } else if self.board.gps.is_some() && c.starts_with("gps") {
            self.gps_command(node, c, out);
        } else if c.starts_with("powersaving on") {
            if self.board.powersaving {
                self.prefs.powersaving = true;
                self.save_prefs();
                w(out, "on - After 2 minutes");
            } else {
                w(out, "Board not supported");
            }
        } else if c.starts_with("powersaving off") {
            self.prefs.powersaving = false;
            self.save_prefs();
            w(out, "off");
        } else if c.starts_with("powersaving") {
            w(out, if self.prefs.powersaving { "on" } else { "off" });
        } else if c.starts_with("log start")
            || c.starts_with("log stop")
            || c.starts_with("log erase")
            || (ts == 0 && c.starts_with("log"))
        {
            // No packet log is kept (`docs/spec/server.md` §2.11).
            w(out, "Error: unsupported");
        } else if ts == 0 && Self::word(c, "stats-packets") {
            let s = &node.disp.stats;
            let _ = write!(
                out,
                "{{\"recv\":{},\"sent\":{},\"flood_tx\":{},\"direct_tx\":{},\"flood_rx\":{},\"direct_rx\":{},\"recv_errors\":{}}}",
                s.recv_flood.wrapping_add(s.recv_direct),
                s.sent_flood.wrapping_add(s.sent_direct),
                s.sent_flood,
                s.sent_direct,
                s.recv_flood,
                s.recv_direct,
                s.recv_errors
            );
        } else if ts == 0 && Self::word(c, "stats-radio") {
            let s = &node.disp.stats;
            let _ = write!(
                out,
                "{{\"noise_floor\":{},\"last_rssi\":{},\"last_snr\":",
                self.board.noise_floor, node.last_rssi
            );
            push_snr(out, node.last_snr4);
            let _ = write!(
                out,
                ",\"tx_air_secs\":{},\"rx_air_secs\":{}}}",
                s.tx_airtime_ms / 1000,
                s.rx_airtime_ms / 1000
            );
        } else if ts == 0 && Self::word(c, "stats-core") {
            let _ = write!(
                out,
                "{{\"battery_mv\":{},\"uptime_secs\":{},\"errors\":{},\"queue_len\":{}}}",
                self.board.battery_mv,
                self.uptime_secs(now),
                node.disp.stats.err_flags,
                node.disp.len()
            );
        } else {
            w(out, "Unknown command");
        }
    }

    /// `c` is `name`, alone or followed by a space.
    fn word(c: &str, name: &str) -> bool {
        c.strip_prefix(name)
            .is_some_and(|r| r.is_empty() || r.starts_with(' '))
    }

    /// The board's own commands (`board.handleCommand`): the Heltec V4's front-end LNA
    /// (`variants/heltec_v4/HeltecV4Board.cpp:89-122`).
    fn board_command(&mut self, node: &mut Node, c: &str, out: &mut Reply) -> bool {
        let get = c == "get radio.fem.rxgain";
        let set = c.strip_prefix("set radio.fem.rxgain ");
        if (!get && set.is_none()) || self.board.front_end == mc_node::device::FrontEnd::None {
            return false;
        }
        if self.board.front_end == mc_node::device::FrontEnd::Pa {
            let _ = out.push_str("Error: unsupported");
            return true;
        }
        if get {
            let _ = out.push_str(if node.prefs.fem_rxgain {
                "> on"
            } else {
                "> off"
            });
        } else if let Some(v) = set {
            let on = if v.starts_with("on") {
                true
            } else if v.starts_with("off") {
                false
            } else {
                let _ = out.push_str("Error: state must be on or off");
                return true;
            };
            node.prefs.fem_rxgain = on;
            self.save_prefs();
            self.push(Action::RadioChanged);
            let _ = write!(
                out,
                "OK - LoRa FEM RX gain {}",
                if on { "on" } else { "off" }
            );
        }
        true
    }

    /// `formatNeighborsReply`: `XXXXXXXX:secs:snr4` lines, newest first.
    fn neighbours_text(&self, clock: u32, out: &mut Reply) {
        let sorted = self.neighbours.sorted(0);
        for (i, n) in sorted.iter().enumerate() {
            if out.len() >= NEIGHBOURS_TEXT_MAX {
                break;
            }
            if i > 0 {
                let _ = out.push('\n');
            }
            push_hex(out, n.key.get(..4).unwrap_or(&[]));
            let _ = write!(out, ":{}:{}", clock.wrapping_sub(n.heard) as i32, n.snr4);
        }
        if sorted.is_empty() {
            let _ = out.push_str("-none-");
        }
    }

    /// `tempradio {freq},{bw},{sf},{cr},{minutes}`: other radio settings for a while, unsaved.
    fn temp_radio(&mut self, node: &Node, args: &str, out: &mut Reply, now: Instant) {
        let p = parts(args, ',', 5);
        let get = |i: usize| p.get(i).copied();
        let freq = get(0).map_or(0, cli::atof_micro);
        let bw = get(1).map_or(0, cli::atof_micro);
        let sf = get(2).map_or(0, |s| cli::atoi(s) as u8);
        let cr = get(3).map_or(0, |s| cli::atoi(s) as u8);
        let mins = get(4).map_or(0, cli::atoi);
        let ok = (150_000_000..=2_500_000_000).contains(&freq)
            && (5..=12).contains(&sf)
            && (5..=8).contains(&cr)
            && (7_000_000..=500_000_000).contains(&bw)
            && mins > 0;
        if !ok {
            let _ = out.push_str("Error, invalid params");
            return;
        }
        let milli = |micro: i64| u32::try_from((micro + 500) / 1000).unwrap_or(0);
        let params = RadioParams {
            freq_khz: milli(freq),
            bw_hz: milli(bw),
            sf,
            cr,
            tx_power_dbm: node.prefs.radio.tx_power_dbm,
        };
        let apply = now + Duration::from_millis(TEMP_RADIO_DELAY_MS);
        let revert = apply + Duration::from_millis(u64::from(mins.unsigned_abs()) * 60 * 1000);
        self.temp_radio = Some((params, Some(apply), revert));
        let _ = write!(out, "OK - temp params for {mins} mins");
    }

    // --------------------------------------------------------------- get / set

    /// `handleGetCmd`.
    fn get(&mut self, node: &Node, ts: u32, k: &str, out: &mut Reply, _now: Instant) {
        let w = |out: &mut Reply, s: &str| {
            let _ = out.push_str(s);
        };
        let on_off = |b: bool| if b { "> on" } else { "> off" };
        let fallback = self.role.routing();
        let routing = node.routing.as_ref().unwrap_or(&fallback);
        if k.starts_with("allow.read.only") {
            w(out, on_off(self.prefs.allow_read_only));
        } else if k.starts_with("flood.advert.interval") {
            let _ = write!(out, "> {}", self.prefs.flood_advert_interval);
        } else if k.starts_with("advert.interval") {
            let _ = write!(out, "> {}", u32::from(self.prefs.advert_interval) * 2);
        } else if k.starts_with("guest.password") {
            let _ = write!(out, "> {}", self.prefs.guest_password);
        } else if ts == 0 && k.starts_with("prv.key") {
            w(out, "> ");
            push_hex(out, node.id.private_key());
        } else if k.starts_with("name") {
            w(out, "> ");
            push_bytes(out, mc_node::contacts::name_str(&node.prefs.name));
        } else if k.starts_with("repeat") {
            w(out, if routing.disable_fwd { "> off" } else { "> on" });
        } else if k.starts_with("lat") {
            w(out, "> ");
            cli::ftoa(out, cli::f32_bits(i64::from(node.prefs.lat), 1_000_000));
        } else if k.starts_with("lon") {
            w(out, "> ");
            cli::ftoa(out, cli::f32_bits(i64::from(node.prefs.lon), 1_000_000));
        } else if k.starts_with("flood.max.advert") {
            let _ = write!(out, "> {}", routing.flood_max_advert);
        } else if k.starts_with("flood.max.unscoped") {
            let _ = write!(out, "> {}", routing.flood_max_unscoped);
        } else if k.starts_with("flood.max") {
            let _ = write!(out, "> {}", routing.flood_max);
        } else if k.starts_with("owner.info") {
            w(out, "> ");
            for ch in self.prefs.owner_info.chars() {
                if out.len() >= 159 || out.push(if ch == '\n' { '|' } else { ch }).is_err() {
                    break;
                }
            }
        } else if k.starts_with("loop.detect") {
            w(
                out,
                match routing.loop_detect {
                    0 => "> off",
                    1 => "> minimal",
                    2 => "> moderate",
                    _ => "> strict",
                },
            );
        } else if k.starts_with("public.key") {
            w(out, "> ");
            push_hex(out, node.id.pub_key());
        } else if k.starts_with("role") {
            let _ = write!(out, "> {}", self.role.name());
        } else if k.starts_with("bridge.type") {
            w(out, "> none");
        } else if k.starts_with("bootloader.ver") || k.starts_with("adc.multiplier") {
            w(out, "Error: unsupported");
        } else if k.starts_with("pwrmgt.support") {
            w(out, "> unsupported");
        } else if k.starts_with("pwrmgt.source") || k.starts_with("pwrmgt.bootmv") {
            w(out, "ERROR: Power management not supported");
        } else if k.starts_with("pwrmgt.bootreason") {
            let _ = write!(
                out,
                "> Reset: {}; Shutdown: {}",
                self.board.reset_reason, self.board.shutdown_reason
            );
        } else if k.starts_with("extra.sf") {
            w(out, "No extra SF configured");
        } else {
            let _ = write!(out, "??: {k}");
        }
    }

    /// `handleSetCmd`.
    fn set(&mut self, node: &mut Node, ts: u32, k: &str, out: &mut Reply, now: Instant) {
        let w = |out: &mut Reply, s: &str| {
            let _ = out.push_str(s);
        };
        let max64 = |out: &mut Reply, v: &str, slot: &mut u8| -> bool {
            let m = cli::atoi(v) as u8;
            if m <= 64 {
                *slot = m;
                w(out, "OK");
                true
            } else {
                w(out, "Error, max 64");
                false
            }
        };
        if let Some(v) = k.strip_prefix("allow.read.only ") {
            self.prefs.allow_read_only = v.starts_with("on");
            self.save_prefs();
            w(out, "OK");
        } else if let Some(v) = k.strip_prefix("flood.advert.interval ") {
            let hours = atoi_digits(v);
            if (hours > 0 && hours < 3) || hours > 168 {
                w(out, "Error: interval range is 3-168 hours");
            } else {
                self.prefs.flood_advert_interval = hours as u8;
                self.update_flood_advert_timer(now);
                self.save_prefs();
                w(out, "OK");
            }
        } else if let Some(v) = k.strip_prefix("advert.interval ") {
            let mins = atoi_digits(v);
            if (mins > 0 && mins < MIN_LOCAL_ADVERT_MINS) || mins > 240 {
                let _ = write!(
                    out,
                    "Error: interval range is {MIN_LOCAL_ADVERT_MINS}-240 minutes"
                );
            } else {
                self.prefs.advert_interval = (mins / 2) as u8;
                self.update_advert_timer(now);
                self.save_prefs();
                w(out, "OK");
            }
        } else if let Some(v) = k.strip_prefix("guest.password ") {
            copy_str(&mut self.prefs.guest_password, v);
            self.save_prefs();
            w(out, "OK");
        } else if let Some(v) = k.strip_prefix("prv.key ") {
            let mut prv = [0u8; PRV_KEY_LEN];
            match from_hex(v, &mut prv)
                .then(|| valid_private_key(&prv))
                .flatten()
            {
                Some(id) => {
                    self.push(Action::SaveIdentity(prv));
                    w(out, "OK, reboot to apply! New pubkey: ");
                    push_hex(out, id.pub_key());
                }
                None => w(out, "Error, bad key"),
            }
        } else if let Some(v) = k.strip_prefix("name ") {
            if cli::valid_name(v.as_bytes()) {
                node.prefs.name = [0; 32];
                for (d, s) in node.prefs.name.iter_mut().take(31).zip(v.bytes()) {
                    *d = s;
                }
                self.save_prefs();
                w(out, "OK");
            } else {
                w(out, "Error, bad chars");
            }
        } else if let Some(v) = k.strip_prefix("repeat ") {
            let off = v.starts_with("off");
            Self::routing(self.role, node).disable_fwd = off;
            self.save_prefs();
            w(
                out,
                if off {
                    "OK - repeat is now OFF"
                } else {
                    "OK - repeat is now ON"
                },
            );
        } else if let Some(v) = k.strip_prefix("lat ") {
            node.prefs.lat = i32::try_from(cli::atof_micro(v)).unwrap_or(0);
            self.save_prefs();
            w(out, "OK");
        } else if let Some(v) = k.strip_prefix("lon ") {
            node.prefs.lon = i32::try_from(cli::atof_micro(v)).unwrap_or(0);
            self.save_prefs();
            w(out, "OK");
        } else if let Some(v) = k.strip_prefix("flood.max.unscoped ") {
            if max64(
                out,
                v,
                &mut Self::routing(self.role, node).flood_max_unscoped,
            ) {
                self.save_prefs();
            }
        } else if let Some(v) = k.strip_prefix("flood.max.advert ") {
            if max64(out, v, &mut Self::routing(self.role, node).flood_max_advert) {
                self.save_prefs();
            }
        } else if let Some(v) = k.strip_prefix("flood.max ") {
            if max64(out, v, &mut Self::routing(self.role, node).flood_max) {
                self.save_prefs();
            }
        } else if let Some(v) = k.strip_prefix("owner.info ") {
            self.prefs.owner_info.clear();
            for ch in v.chars() {
                if self
                    .prefs
                    .owner_info
                    .push(if ch == '|' { '\n' } else { ch })
                    .is_err()
                {
                    break;
                }
            }
            self.save_prefs();
            w(out, "OK");
        } else if let Some(v) = k.strip_prefix("loop.detect ") {
            let mode = if v.starts_with("off") {
                Some(0)
            } else if v.starts_with("minimal") {
                Some(1)
            } else if v.starts_with("moderate") {
                Some(2)
            } else if v.starts_with("strict") {
                Some(3)
            } else {
                None
            };
            match mode {
                Some(m) => {
                    Self::routing(self.role, node).loop_detect = m;
                    self.save_prefs();
                    w(out, "OK");
                }
                None => w(out, "Error, must be: off, minimal, moderate, or strict"),
            }
        } else if let Some(v) = k.strip_prefix("freq ").filter(|_| ts == 0) {
            let micro = cli::atof_micro(v);
            node.prefs.radio.freq_khz = u32::try_from((micro + 500) / 1000).unwrap_or(0);
            self.save_prefs();
            w(out, "OK - reboot to apply");
        } else if k.starts_with("adc.multiplier ") {
            w(out, "Error: unsupported");
        } else {
            w(out, "unknown config: ");
            for ch in k.chars() {
                if out.len() >= 159 || out.push(ch).is_err() {
                    break;
                }
            }
        }
    }

    // --------------------------------------------------------------- sensors and GPS

    /// `EnvironmentSensorManager` settings: `gps` (on boards with a GPS).
    fn sensor_value(&self, node: &Node, key: &str) -> Option<&'static str> {
        let gps = self.board.gps.as_ref()?;
        let _ = node;
        (key == "gps").then_some(if gps.active { "1" } else { "0" })
    }

    fn set_sensor(&mut self, node: &mut Node, key: &str, value: &str) -> bool {
        if self.board.gps.is_none() {
            return false;
        }
        match key {
            "gps" => {
                self.push(Action::Gps(value != "0"));
                true
            }
            "gps_interval" => {
                let s = cli::atoi(value).unsigned_abs().max(1);
                node.prefs.gps_interval = s;
                self.push(Action::GpsInterval(s));
                true
            }
            _ => false,
        }
    }

    /// `sensor list [start]`.
    fn sensor_list(&self, node: &Node, c: &str, out: &mut Reply) {
        let end = usize::from(self.board.gps.is_some());
        let start = c.get(12..).map_or(0, |s| atoi_digits(s) as usize);
        if start >= end {
            let _ = out.push_str("no custom var");
            return;
        }
        let _ = writeln!(out, "{end} vars");
        let mut i = start;
        while i < end && out.len() < 134 {
            let _ = writeln!(out, "gps={}", self.sensor_value(node, "gps").unwrap_or("0"));
            i += 1;
        }
        if i < end {
            let _ = write!(out, "... next:{i}");
        } else {
            let _ = out.pop();
        }
    }

    /// The `gps` commands (`ENV_INCLUDE_GPS`).
    fn gps_command(&mut self, node: &mut Node, c: &str, out: &mut Reply) {
        let w = |out: &mut Reply, s: &str| {
            let _ = out.push_str(s);
        };
        let Some(gps) = self.board.gps.clone() else {
            return;
        };
        if c.starts_with("gps on") || c.starts_with("gps off") {
            let on = c.starts_with("gps on");
            node.prefs.gps_enabled = on;
            self.push(Action::Gps(on));
            self.save_prefs();
            w(out, "ok");
        } else if c.starts_with("gps sync") {
            self.push(Action::GpsSync);
            w(out, "ok");
        } else if c == "gps diag" {
            let _ = write!(out, "req:{} ", u8::from(gps.active));
            let _ = out.push_str(&gps.diag);
        } else if c.starts_with("gps setloc") {
            let (lat, lon) = node.gps_pos.unwrap_or((0, 0));
            node.prefs.lat = lat;
            node.prefs.lon = lon;
            self.save_prefs();
            w(out, "ok");
        } else if c.starts_with("gps advert") {
            let arg = c.get(11..).unwrap_or("");
            if c.len() == 10 {
                w(
                    out,
                    match node.prefs.advert_loc_policy {
                        0 => "> none",
                        2 => "> prefs",
                        1 => "> share",
                        _ => "error",
                    },
                );
            } else if let Some(p) = [("none", 0u8), ("share", 1), ("prefs", 2)]
                .iter()
                .find(|(name, _)| arg.starts_with(name))
                .map(|(_, p)| *p)
            {
                node.prefs.advert_loc_policy = p;
                self.save_prefs();
                w(out, "ok");
            } else {
                w(out, "error");
            }
        } else if gps.enabled {
            let _ = write!(
                out,
                "on, {}, {}, {} sats",
                if gps.active { "active" } else { "deactivated" },
                if gps.fix { "fix" } else { "no fix" },
                gps.sats
            );
        } else {
            w(out, "off");
        }
    }

    // --------------------------------------------------------------- regions

    /// `handleRegionCmd` (`CommonCLI.cpp:889-1027`).
    fn region_command(&mut self, node: &mut Node, command: &str, out: &mut Reply, now: Instant) {
        let cmd = command.trim_start_matches(' ');
        if let Some(rest) = cmd.strip_prefix("region def")
            && (rest.is_empty() || rest.starts_with(' '))
        {
            let payload = rest.trim_matches(' ');
            if payload.is_empty() {
                let _ = out.push_str("Err - empty def");
                return;
            }
            let regions = &mut Self::routing(self.role, node).regions;
            let mut cursor = 0u16;
            for tok in payload.split(' ').filter(|t| !t.is_empty()) {
                let (name, jump) = match tok.find(['|', ',']) {
                    Some(i) => (
                        tok.get(..i).unwrap_or(""),
                        Some(tok.get(i + 1..).unwrap_or("").trim_matches(' ')),
                    ),
                    None => (tok, None),
                };
                if name.is_empty() {
                    let _ = out.push_str("Err - empty name");
                    return;
                }
                if jump == Some("") {
                    let _ = out.push_str("Err - empty jump");
                    return;
                }
                let Some(id) = regions.put(name, cursor, 0) else {
                    let _ = write!(out, "Err - put failed: {name}");
                    return;
                };
                regions.set_flags(id, 0);
                cursor = match jump {
                    Some(j) => match regions.find_by_name_prefix(j) {
                        Some(r) => r,
                        None => {
                            let _ = write!(out, "Err - unknown jump: {j}");
                            return;
                        }
                    },
                    None => id,
                };
            }
            regions.export(out);
            return;
        }
        let p = parts(command, ' ', 4);
        let n = p.len();
        let arg = |i: usize| p.get(i).copied().unwrap_or("");
        let sub = arg(1);
        let w = |out: &mut Reply, s: &str| {
            let _ = out.push_str(s);
        };
        let unknown = |out: &mut Reply| w(out, "Err - unknown region");
        let regions = &mut Self::routing(self.role, node).regions;
        if n == 1 {
            regions.export(out);
        } else if sub == "load" {
            self.temp_regions.reset_from(regions);
            let mut stack = [None; 8];
            stack[0] = Some(0);
            self.region_load = Some(RegionLoad { stack });
        } else if sub == "save" {
            self.prefs.discovery_mod_timestamp = node.rtc.now(now);
            self.save_prefs();
            self.push(Action::SaveRegions);
            w(out, "OK");
        } else if n >= 3 && (sub == "allowf" || sub == "denyf") {
            match regions.find_by_name_prefix(arg(2)) {
                Some(id) => {
                    let flags = regions.get(id).map_or(0, |r| r.flags);
                    let f = if sub == "allowf" {
                        flags & !DENY_FLOOD
                    } else {
                        flags | DENY_FLOOD
                    };
                    regions.set_flags(id, f);
                    w(out, "OK");
                }
                None => unknown(out),
            }
        } else if n >= 3 && sub == "get" {
            match regions
                .find_by_name_prefix(arg(2))
                .and_then(|id| regions.get(id))
            {
                Some(r) => {
                    let f = if r.allows_flood() { "F" } else { "" };
                    match regions.get(r.parent).filter(|p| p.id != 0) {
                        Some(parent) => {
                            let _ = write!(out, " {} ({}) {f}", r.name, parent.name);
                        }
                        None => {
                            let _ = write!(out, " {} {f}", r.name);
                        }
                    }
                }
                None => unknown(out),
            }
        } else if n >= 3 && sub == "home" {
            match regions
                .find_by_name_prefix(arg(2))
                .and_then(|id| regions.get(id))
            {
                Some(r) => {
                    regions.set_home(r.id);
                    let _ = write!(out, " home is now {}", r.name);
                }
                None => unknown(out),
            }
        } else if n == 2 && sub == "home" {
            match regions.home() {
                Some(r) => {
                    let _ = write!(out, " home is {}", r.name);
                }
                None => w(out, " home is *"),
            }
        } else if n >= 3 && sub == "default" {
            if arg(2) == "<null>" {
                regions.set_default(0);
                self.push(Action::SaveRegions);
                w(out, " default scope is now <null>");
            } else {
                let id = regions
                    .find_by_name_prefix(arg(2))
                    .or_else(|| regions.put(arg(2), 0, 0));
                match id.and_then(|id| {
                    regions.set_flags(id, 0);
                    regions.set_default(id);
                    regions.get(id)
                }) {
                    Some(r) => {
                        let _ = write!(out, " default scope is now {}", r.name);
                        self.push(Action::SaveRegions);
                    }
                    None => w(out, "Err - region table full"),
                }
            }
        } else if n == 2 && sub == "default" {
            match regions.default_region() {
                Some(r) => {
                    let _ = write!(out, " default scope is {}", r.name);
                }
                None => w(out, " default scope is <null>"),
            }
        } else if n >= 3 && sub == "put" {
            let parent = if n >= 4 {
                regions.find_by_name_prefix(arg(3))
            } else {
                Some(0)
            };
            match parent {
                None => w(out, "Err - unknown parent"),
                Some(parent) => match regions.put(arg(2), parent, 0) {
                    Some(id) => {
                        regions.set_flags(id, 0);
                        w(out, "OK - (flood allowed)");
                    }
                    None => w(out, "Err - unable to put"),
                },
            }
        } else if n >= 3 && sub == "remove" {
            match regions.find_by_name(arg(2)) {
                Some(id) => w(
                    out,
                    if regions.remove(id) {
                        "OK"
                    } else {
                        "Err - not empty"
                    },
                ),
                None => w(out, "Err - not found"),
            }
        } else if n >= 3 && sub == "list" {
            let invert = match arg(2) {
                "allowed" => false,
                "denied" => true,
                _ => {
                    w(out, "Err - use 'allowed' or 'denied'");
                    return;
                }
            };
            regions.export_names(out, DENY_FLOOD, invert);
            if out.is_empty() {
                w(out, "-none-");
            }
        } else {
            w(out, "Err - ??");
        }
    }

    /// One line of `region load`: `{indent}{name}[ F]`, a blank line to finish.
    fn region_load_line(&mut self, node: &mut Node, line: &str, out: &mut Reply) {
        if line.bytes().all(|b| b == b' ') {
            let regions = &mut Self::routing(self.role, node).regions;
            *regions = self.temp_regions.clone();
            self.region_load = None;
            let _ = write!(out, "OK - loaded {} regions", regions.len());
            return;
        }
        let Some(load) = self.region_load.as_mut() else {
            return;
        };
        let np = line.trim_start_matches(' ');
        let indent = line.len() - np.len();
        let end = np
            .bytes()
            .position(|b| !is_name_char(b))
            .unwrap_or(np.len());
        let name = np.get(..end).unwrap_or("");
        let flood = np
            .get(end..)
            .is_some_and(|r| r.get(1..).is_some_and(|r| r.contains('F')));
        if indent == 0 || indent >= 8 || name.is_empty() {
            return;
        }
        let Some(parent) = load.stack.get(indent - 1).copied().flatten() else {
            return;
        };
        let current = node.routing.as_ref().map(|r| &r.regions);
        let old = current
            .and_then(|r| r.find_by_name(name))
            .and_then(|id| current.and_then(|r| r.get(id)));
        if let Some(id) = self
            .temp_regions
            .put(name, parent, old.as_ref().map_or(0, |o| o.id))
        {
            let flags = match &old {
                Some(o) => o.flags,
                None if flood => 0,
                None => DENY_FLOOD,
            };
            self.temp_regions.set_flags(id, flags);
            if let Some(s) = load.stack.get_mut(indent) {
                *s = Some(id);
            }
        }
    }
}
