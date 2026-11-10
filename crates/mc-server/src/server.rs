//! A MeshCore server (MeshCore v1.18, MIT; `docs/spec/server.md`): the repeater
//! (`examples/simple_repeater/MyMesh.cpp`, §2) or the room server
//! (`examples/simple_room_server`, §3, in `crate::room`). What both do is here: logins,
//! requests from logged-in clients, the console for admins, adverts, settings and timers; the
//! repeater adds anonymous requests, neighbours and discovery. Forwarding itself is the node's
//! (`Node::routing`).

use core::fmt::Write;

use mc_air::advert::{TYPE_REPEATER, TYPE_ROOM};
use mc_air::crypto::{PRV_KEY_LEN, PUB_KEY_LEN};
use mc_air::{PacketBuf, PathLen, PayloadType, Route as AirRoute};
use mc_core::{Duration, Instant};
use mc_node::cli::Reply;
use mc_node::contacts::{Contact, PATH_UNKNOWN};
use mc_node::device::FrontEnd;
use mc_node::dispatch::RadioParams;
use mc_node::node::Routing;
use mc_node::region::RegionMap;
use mc_node::{Event, Node};
use rand_core::Rng;

use crate::acl::{self, Acl, Put};
use crate::limiter::RateLimiter;
use crate::neighbours::Neighbours;
use crate::reply::{self, Request, Route};

/// Which server (`FIRMWARE_ROLE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Repeater,
    Room,
}

impl Role {
    /// `FIRMWARE_ROLE` (`get role`).
    pub const fn name(self) -> &'static str {
        match self {
            Role::Repeater => "repeater",
            Role::Room => "room_server",
        }
    }

    /// `FIRMWARE_VER_LEVEL` (login reply).
    pub const fn fw_level(self) -> u8 {
        match self {
            Role::Repeater => 2,
            Role::Room => 1,
        }
    }

    /// `ADV_TYPE_*` in its adverts.
    pub const fn advert_kind(self) -> u8 {
        match self {
            Role::Repeater => TYPE_REPEATER,
            Role::Room => TYPE_ROOM,
        }
    }

    /// Its forwarding rules out of the box: a room server forwards nothing.
    pub const fn routing(self) -> Routing {
        let mut r = Routing::REPEATER;
        r.disable_fwd = matches!(self, Role::Room);
        r
    }
}
/// `RESP_SERVER_LOGIN_OK`.
pub(crate) const LOGIN_OK: u8 = 0;
const REQ_STATUS: u8 = 0x01;
const REQ_TELEMETRY: u8 = 0x03;
const REQ_ACCESS_LIST: u8 = 0x05;
const REQ_NEIGHBOURS: u8 = 0x06;
const REQ_OWNER_INFO: u8 = 0x07;
const ANON_REGIONS: u8 = 0x01;
const ANON_OWNER: u8 = 0x02;
const ANON_CLOCK: u8 = 0x03;
const DISCOVER_REQ: u8 = 0x80;
const DISCOVER_RESP: u8 = 0x90;
/// `TELEM_PERM_LOCATION`.
const TELEM_LOCATION: u8 = 0x02;
/// `LAZY_CONTACTS_WRITE_DELAY`.
const LAZY_SAVE_MS: u64 = 5000;
/// `ENABLE_ADVERT_ON_BOOT`: a zero-hop advert this long after start.
const BOOT_ADVERT_MS: u64 = 16_000;
/// How long discovery answers are taken after our request.
const DISCOVER_WINDOW_MS: u64 = 60_000;
/// `MAX_PACKET_PAYLOAD`: room for a reply.
pub(crate) const REPLY_MAX: usize = 184;
/// The neighbours list in a NEIGHBOURS reply.
const NEIGHBOURS_MAX: usize = 130;

/// The repeater's own settings, next to the node's (`NodePrefs`, `src/helpers/CommonCLI.h`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerPrefs {
    /// Admin password (`password`, 15 characters).
    pub admin_password: heapless::String<15>,
    pub guest_password: heapless::String<15>,
    /// Local (zero-hop) adverts every this many 2-minute steps; 0 = none.
    pub advert_interval: u8,
    /// Flood adverts every this many hours; 0 = none.
    pub flood_advert_interval: u8,
    /// Room server: an unknown password logs in read only.
    pub allow_read_only: bool,
    /// Free text, `|` for new lines (`owner.info`).
    pub owner_info: heapless::String<119>,
    /// Discovery answers only requests asking for changes since this (our clock).
    pub discovery_mod_timestamp: u32,
    /// `powersaving` (only boards that can sleep accept it).
    pub powersaving: bool,
}

impl Default for ServerPrefs {
    fn default() -> Self {
        let mut admin_password = heapless::String::new();
        let _ = admin_password.push_str("password");
        Self {
            admin_password,
            guest_password: heapless::String::new(),
            advert_interval: 1,
            flood_advert_interval: 47,
            allow_read_only: false,
            owner_info: heapless::String::new(),
            discovery_mod_timestamp: 0,
            powersaving: false,
        }
    }
}

/// What the runner should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// The access list changed (admins only are kept, after the lazy delay).
    SaveAcl,
    /// The node's or the repeater's settings changed.
    SavePrefs,
    /// The region table changed.
    SaveRegions,
    /// `set prv.key`: the identity used from the next start.
    SaveIdentity([u8; PRV_KEY_LEN]),
    Reboot,
    PowerOff,
    /// `erase` (serial only): wipe oxymesh's own store, never the C++ SPIFFS.
    Erase,
    /// Transmit power, RX gain or front end changed: apply them to the radio now.
    RadioChanged,
    /// `tempradio`: these settings on the radio until `RestoreRadio`; not saved.
    TempRadio(RadioParams),
    /// Back to the node's radio settings.
    RestoreRadio,
    /// Turn the GPS on or off.
    Gps(bool),
    /// Set the clock from the GPS.
    GpsSync,
    /// GPS update interval, seconds.
    GpsInterval(u32),
    /// The console set the clock: a board with a clock chip writes it.
    ClockSet,
}

/// The GPS as the board sees it (`LocationProvider`).
#[derive(Debug, Clone, Default)]
pub struct Gps {
    /// Powered (`isEnabled`).
    pub enabled: bool,
    /// Asked to run (setting `gps`).
    pub active: bool,
    pub fix: bool,
    pub sats: i32,
    /// `gps diag` (`formatDiagnostics`).
    pub diag: heapless::String<120>,
}

/// What the board reports, set by the runner.
#[derive(Debug, Clone, Default)]
pub struct Board {
    pub battery_mv: u16,
    pub mcu_temp_deci: Option<i16>,
    pub noise_floor: i16,
    /// `board` (`getManufacturerName`).
    pub manufacturer: &'static str,
    /// `ver` (`FIRMWARE_BUILD_DATE`).
    pub build_date: &'static str,
    /// `power`: `None` where the board cannot tell.
    pub usb_powered: Option<bool>,
    pub solar_charging: Option<bool>,
    pub external_power: bool,
    pub charging: bool,
    /// A GPS (`ENV_INCLUDE_GPS`, and detected): the `gps` commands and the `gps` setting.
    pub gps: Option<Gps>,
    /// The board can sleep (`powersaving on`).
    pub powersaving: bool,
    /// `radio.fem.rxgain`: the Heltec V4's front end.
    pub front_end: FrontEnd,
    /// `get pwrmgt.bootreason`.
    pub reset_reason: &'static str,
    pub shutdown_reason: &'static str,
}

#[derive(Debug)]
pub struct Server {
    pub role: Role,
    pub prefs: ServerPrefs,
    pub acl: Acl,
    pub neighbours: Neighbours,
    pub board: Board,
    /// Version reported to clients (`FIRMWARE_VERSION`).
    pub version: &'static str,
    /// Login reply byte 5: v1.18 sends 0 (older servers sent a keep-alive interval / 16 s).
    /// Settable so that clients of such servers can be tested.
    pub legacy_keep_alive: u8,
    discover_limiter: RateLimiter,
    anon_limiter: RateLimiter,
    pending_discover: Option<(u32, Instant)>,
    next_local_advert: Option<Instant>,
    next_flood_advert: Option<Instant>,
    save_acl_at: Option<Instant>,
    started: Instant,
    actions: heapless::Deque<Action, 16>,
    /// `tempradio`: the settings, when to apply them (once), when to go back.
    pub(crate) temp_radio: Option<(RadioParams, Option<Instant>, Instant)>,
    /// `region load` in progress, into `temp_regions`.
    pub(crate) region_load: Option<crate::console::RegionLoad>,
    pub(crate) temp_regions: RegionMap,
    /// The room server's posts and pushes.
    pub room: crate::room::Room,
}

/// The node's settings a server starts from (repeater `MyMesh.cpp:893-929`, room server
/// `MyMesh.cpp:648-694`): the room server's direct retransmit delay factor is 0.2, not 0.3.
pub fn default_node_prefs(p: &mut mc_node::Prefs, role: Role) {
    p.airtime_factor_milli = 1000;
    p.rx_delay_base_milli = 0;
    p.tx_delay_milli = 500;
    p.direct_tx_delay_milli = match role {
        Role::Repeater => 300,
        Role::Room => 200,
    };
    p.interference_threshold = 0;
    p.cad_enabled = false;
    p.advert_loc_policy = 2; // ADVERT_LOC_PREFS
}

pub(crate) fn le32(b: &[u8], i: usize) -> u32 {
    let mut a = [0u8; 4];
    for (o, v) in a.iter_mut().zip(b.iter().skip(i)) {
        *o = *v;
    }
    u32::from_le_bytes(a)
}

/// A client's stored path, or a flood.
pub(crate) fn direct_or_flood(p: &Option<(u8, heapless::Vec<u8, 64>)>) -> Route<'_> {
    match p {
        Some((len, p)) => Route::OutPath(*len, p),
        None => Route::Flood,
    }
}

pub(crate) fn c_str(b: &[u8]) -> &[u8] {
    mc_node::contacts::name_str(b)
}

impl Server {
    pub fn new(now: Instant, version: &'static str, role: Role) -> Self {
        Self {
            role,
            prefs: ServerPrefs::default(),
            acl: Acl::new(),
            neighbours: Neighbours::new(),
            board: Board::default(),
            version,
            legacy_keep_alive: 0,
            discover_limiter: RateLimiter::new(4, 120),
            anon_limiter: RateLimiter::new(4, 180),
            pending_discover: None,
            next_local_advert: None,
            next_flood_advert: None,
            save_acl_at: None,
            started: now,
            actions: heapless::Deque::new(),
            temp_radio: None,
            region_load: None,
            temp_regions: RegionMap::new(),
            room: crate::room::Room::new(),
        }
    }

    pub fn poll_action(&mut self) -> Option<Action> {
        self.actions.pop_front()
    }

    pub(crate) fn push(&mut self, a: Action) {
        if self.actions.is_full() {
            let _ = self.actions.pop_front();
        }
        let _ = self.actions.push_back(a);
    }

    /// The access list is saved `LAZY_CONTACTS_WRITE_DELAY` from now.
    pub(crate) fn save_acl_soon(&mut self, now: Instant) {
        self.save_acl_at = Some(now + Duration::from_millis(LAZY_SAVE_MS));
    }

    /// When `tick` has something to do next.
    pub fn next_wakeup(&self) -> Option<Instant> {
        let temp = self
            .temp_radio
            .as_ref()
            .map(|(_, apply, revert)| apply.unwrap_or(*revert));
        [
            self.next_local_advert,
            self.next_flood_advert,
            self.save_acl_at,
            temp,
            (self.role == Role::Room)
                .then_some(self.room.next_push)
                .flatten(),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// Puts the node in server mode (keeping routing rules already loaded), puts the clients
    /// in its contact table, starts the advert timers and schedules the boot advert
    /// (`MyMesh::begin`, `main.cpp:117-120`).
    pub fn start(&mut self, node: &mut Node, now: Instant) {
        if node.routing.is_none() {
            node.routing = Some(self.role.routing());
        }
        node.advert_kind = self.role.advert_kind();
        for c in self.acl.clients.clone() {
            ensure_contact(node, &c.key);
        }
        self.started = now;
        self.update_advert_timer(now);
        self.update_flood_advert_timer(now);
        if let Ok(p) = node.self_advert(now) {
            node.send_zero_hop(p, None, BOOT_ADVERT_MS, now);
        }
    }

    pub fn uptime_secs(&self, now: Instant) -> u32 {
        u32::try_from(now.saturating_duration_since(self.started).as_millis() / 1000)
            .unwrap_or(u32::MAX)
    }

    /// `isHiddenNode`: no adverts at all.
    pub fn is_hidden(&self) -> bool {
        self.prefs.advert_interval == 0 && self.prefs.flood_advert_interval == 0
    }

    /// `updateAdvertTimer`.
    pub fn update_advert_timer(&mut self, now: Instant) {
        self.next_local_advert = (self.prefs.advert_interval > 0).then(|| {
            now + Duration::from_millis(u64::from(self.prefs.advert_interval) * 2 * 60 * 1000)
        });
    }

    /// `updateFloodAdvertTimer`.
    pub fn update_flood_advert_timer(&mut self, now: Instant) {
        self.next_flood_advert = (self.prefs.flood_advert_interval > 0).then(|| {
            now + Duration::from_millis(
                u64::from(self.prefs.flood_advert_interval) * 60 * 60 * 1000,
            )
        });
    }

    /// `sendSelfAdvertisement`: flooded within the default scope, or zero hop.
    pub fn send_advert(&mut self, node: &mut Node, now: Instant, delay_ms: u64, flood: bool) {
        let Ok(p) = node.self_advert(now) else { return };
        if flood {
            let key = node
                .routing
                .as_ref()
                .and_then(|r| reply::default_scope(&r.regions));
            let size = node.prefs.path_hash_size();
            reply::flood_scoped(node, p, key, delay_ms, size, now);
        } else {
            node.send_zero_hop(p, None, delay_ms, now);
        }
    }

    /// The loop's timers (`MyMesh::loop`): adverts, the lazy access-list save.
    pub fn tick(&mut self, node: &mut Node, now: Instant, rng: &mut impl Rng) {
        if self.role == Role::Room {
            self.room_tick(node, now, rng);
        }
        if self.next_flood_advert.is_some_and(|t| now >= t) {
            self.send_advert(node, now, 0, true);
            self.update_flood_advert_timer(now);
            self.update_advert_timer(now);
        } else if self.next_local_advert.is_some_and(|t| now >= t) {
            self.send_advert(node, now, 0, false);
            self.update_advert_timer(now);
        }
        if self.save_acl_at.is_some_and(|t| now >= t) {
            self.save_acl_at = None;
            self.push(Action::SaveAcl);
        }
        if let Some((params, apply, revert)) = self.temp_radio {
            if apply.is_some_and(|t| now >= t) {
                self.temp_radio = Some((params, None, revert));
                self.push(Action::TempRadio(params));
            }
            if now >= revert {
                self.temp_radio = None;
                self.push(Action::RestoreRadio);
            }
        }
    }

    /// One event from the node.
    pub fn on_event(&mut self, node: &mut Node, ev: &Event, now: Instant, rng: &mut impl Rng) {
        let clock = node.rtc.now(now);
        match ev {
            Event::AnonRequest {
                key,
                ts,
                data,
                flood,
                in_path_len,
                in_path,
                region,
            } => {
                let req = Request {
                    key,
                    flood: *flood,
                    in_path_len: *in_path_len,
                    in_path,
                    region: *region,
                };
                match self.role {
                    Role::Repeater => self.on_anon(node, &req, *ts, data, now, rng),
                    Role::Room => self.room_login(node, &req, *ts, data, now, rng),
                }
            }
            Event::ServerPeer {
                key,
                ptype,
                data,
                flood,
                in_path_len,
                in_path,
                region,
            } => {
                let req = Request {
                    key,
                    flood: *flood,
                    in_path_len: *in_path_len,
                    in_path,
                    region: *region,
                };
                match self.role {
                    Role::Repeater => self.on_peer(node, &req, *ptype, data, now, rng),
                    Role::Room => self.room_peer(node, &req, *ptype, data, now, rng),
                }
            }
            Event::ServerPath { key } => {
                if let Some(i) = self.acl.get(key)
                    && let Some(c) = self.acl.clients.get_mut(i)
                {
                    c.last_activity = clock;
                }
            }
            Event::ServerAck { crc } => {
                if self.role == Role::Room {
                    self.room_ack(*crc);
                }
            }
            Event::AdvertHeard {
                key,
                kind,
                path_len,
                snr4,
                ts,
                shared,
                ..
            } => {
                // Zero-hop repeater adverts, not shares (`onAdvertRecv`).
                let hops = PathLen::from_byte(*path_len).map_or(1, |p| p.hops());
                if self.role == Role::Repeater && hops == 0 && !*shared && *kind == TYPE_REPEATER {
                    self.neighbours.put(key, *ts, clock, *snr4);
                }
            }
            Event::ControlData { snr4, data, .. } if self.role == Role::Repeater => {
                self.on_control(node, *snr4, data, now, rng);
            }
            _ => {}
        }
    }

    // --------------------------------------------------------------- anonymous requests

    /// `onAnonDataRecv`: a login, or (direct only) regions, owner or clock.
    fn on_anon(
        &mut self,
        node: &mut Node,
        req: &Request<'_>,
        ts: u32,
        data: &[u8],
        now: Instant,
        rng: &mut impl Rng,
    ) {
        let first = data.first().copied().unwrap_or(0);
        let mut out: heapless::Vec<u8, REPLY_MAX> = heapless::Vec::new();
        let mut supplied: Option<(u8, heapless::Vec<u8, 64>)> = None;
        let clock = node.rtc.now(now);
        if first == 0 || first >= b' ' {
            if !self.login(node, req, ts, data, now, rng, &mut out) {
                return;
            }
        } else if !req.flood && matches!(first, ANON_REGIONS | ANON_OWNER | ANON_CLOCK) {
            if !self.anon_limiter.allow(clock) {
                return;
            }
            // `{reply-path-len}{reply-path}`.
            let rest = data.get(1..).unwrap_or(&[]);
            let Some(&len) = rest.first() else { return };
            let Ok(pl) = PathLen::from_byte(len) else {
                return;
            };
            let path = rest.get(1..1 + pl.byte_len()).unwrap_or(&[]);
            supplied = Some((len, heapless::Vec::from_slice(path).unwrap_or_default()));
            let _ = out.extend_from_slice(&ts.to_le_bytes());
            let _ = out.extend_from_slice(&clock.to_le_bytes());
            match first {
                ANON_REGIONS => {
                    let mut names: heapless::String<{ REPLY_MAX - 12 }> = heapless::String::new();
                    if let Some(r) = &node.routing {
                        r.regions
                            .export_names(&mut names, mc_node::region::DENY_FLOOD, false);
                    }
                    let _ = out.extend_from_slice(names.as_bytes());
                }
                ANON_OWNER => {
                    let mut s: heapless::String<{ REPLY_MAX - 8 }> = heapless::String::new();
                    let name = c_str(&node.prefs.name);
                    let _ = write!(
                        s,
                        "{}\n{}",
                        core::str::from_utf8(name).unwrap_or(""),
                        self.prefs.owner_info
                    );
                    let _ = out.extend_from_slice(s.as_bytes());
                }
                _ => {
                    let disabled = node.routing.as_ref().is_some_and(|r| r.disable_fwd);
                    let _ = out.push(if disabled { 0x80 } else { 0 });
                }
            }
        } else {
            return;
        }
        // A direct login answers along the stored path, as a REQ does.
        let out_path = self.client_path(node, req.key);
        let route = match (&supplied, &out_path) {
            _ if req.flood => Route::PathReturn,
            (Some((len, path)), _) => Route::Supplied(*len, path),
            (None, Some((len, path))) => Route::OutPath(*len, path),
            _ => Route::Flood,
        };
        let rnd = rng.next_u32().to_le_bytes();
        reply::send(
            node,
            req,
            route,
            PayloadType::RESPONSE,
            &out,
            crate::RESPONSE_DELAY_MS,
            rnd,
            now,
        );
    }

    /// `handleLoginReq`: false if refused (wrong password, replay).
    #[allow(clippy::too_many_arguments)]
    fn login(
        &mut self,
        node: &mut Node,
        req: &Request<'_>,
        ts: u32,
        data: &[u8],
        now: Instant,
        rng: &mut impl Rng,
        out: &mut heapless::Vec<u8, REPLY_MAX>,
    ) -> bool {
        let password = c_str(data);
        let clock = node.rtc.now(now);
        let mut idx = if password.is_empty() {
            self.acl.get(req.key)
        } else {
            None
        };
        if idx.is_none() {
            let perms = if password == self.prefs.admin_password.as_bytes() {
                acl::ADMIN
            } else if password == self.prefs.guest_password.as_bytes() {
                acl::GUEST
            } else {
                return false;
            };
            let put = self.acl.put(req.key, 0);
            if let Put::Evicted(_, gone) = put {
                forget_contact(node, &gone);
            }
            let i = put.index();
            let Some(c) = self.acl.clients.get_mut(i) else {
                return false;
            };
            if ts <= c.last_timestamp {
                return false; // replay, or a full table
            }
            c.last_timestamp = ts;
            c.last_activity = clock;
            c.permissions = (c.permissions & !acl::ROLE_MASK) | perms;
            ensure_contact(node, req.key);
            if perms != acl::GUEST {
                self.save_acl_soon(now);
            }
            idx = Some(i);
        }
        let Some(c) = idx.and_then(|i| self.acl.clients.get(i)).copied() else {
            return false;
        };
        if req.flood
            && let Some(slot) = node.contacts.find(req.key)
            && let Some(ct) = node.contacts.get_mut(slot)
        {
            ct.out_path_len = PATH_UNKNOWN; // to be learned again
        }
        let unique = node.rtc.unique(now);
        let _ = out.extend_from_slice(&unique.to_le_bytes());
        let _ = out.push(LOGIN_OK);
        let _ = out.push(self.legacy_keep_alive);
        let _ = out.push(u8::from(c.is_admin()));
        let _ = out.push(c.permissions);
        let _ = out.extend_from_slice(&rng.next_u32().to_le_bytes());
        let _ = out.push(self.role.fw_level());
        true
    }

    pub(crate) fn client_path(
        &self,
        node: &Node,
        key: &[u8; PUB_KEY_LEN],
    ) -> Option<(u8, heapless::Vec<u8, 64>)> {
        self.acl.get(key)?;
        let c = node.contacts.get(node.contacts.find(key)?)?;
        (c.out_path_len != PATH_UNKNOWN).then(|| {
            (
                c.out_path_len,
                heapless::Vec::from_slice(c.out_path()).unwrap_or_default(),
            )
        })
    }

    // --------------------------------------------------------------- clients

    /// `onPeerDataRecv`: a request, or a console command from an admin.
    fn on_peer(
        &mut self,
        node: &mut Node,
        req: &Request<'_>,
        ptype: PayloadType,
        data: &[u8],
        now: Instant,
        rng: &mut impl Rng,
    ) {
        let Some(i) = self.acl.get(req.key) else {
            return;
        };
        let clock = node.rtc.now(now);
        let ts = le32(data, 0);
        let Some(client) = self.acl.clients.get(i).copied() else {
            return;
        };
        if ptype == PayloadType::REQ {
            if ts <= client.last_timestamp {
                return; // replay
            }
            let mut out: heapless::Vec<u8, REPLY_MAX> = heapless::Vec::new();
            let _ = out.extend_from_slice(&ts.to_le_bytes());
            if !self.request(node, &client, data.get(4..).unwrap_or(&[]), now, &mut out) {
                return;
            }
            if let Some(c) = self.acl.clients.get_mut(i) {
                c.last_timestamp = ts;
                c.last_activity = clock;
            }
            let path = self.client_path(node, req.key);
            let route = match &path {
                _ if req.flood => Route::PathReturn,
                Some((len, p)) => Route::OutPath(*len, p),
                None => Route::Flood,
            };
            let rnd = rng.next_u32().to_le_bytes();
            reply::send(
                node,
                req,
                route,
                PayloadType::RESPONSE,
                &out,
                crate::RESPONSE_DELAY_MS,
                rnd,
                now,
            );
        } else if ptype == PayloadType::TXT_MSG && data.len() > 5 && client.is_admin() {
            let flags = data.get(4).copied().unwrap_or(0) >> 2;
            if !matches!(flags, 0 | 1 | 3) || ts < client.last_timestamp {
                return;
            }
            let retry = ts == client.last_timestamp;
            if let Some(c) = self.acl.clients.get_mut(i) {
                c.last_timestamp = ts;
                c.last_activity = clock;
            }
            let text = c_str(data.get(5..).unwrap_or(&[]));
            let path = self.client_path(node, req.key);
            if flags == 0 {
                // Plain text (legacy console): acknowledged, `SHA256(ts ‖ flags ‖ text ‖ key)`.
                let mut ack = [0u8; 4];
                let head = data.get(..5).unwrap_or(&[]);
                let h: [u8; 4] = mc_air::crypto::sha256_trunc(&[head, text, req.key]);
                ack.copy_from_slice(&h);
                if let Ok(p) = mc_air::payload::ack(&ack) {
                    match direct_or_flood(&path) {
                        Route::OutPath(len, pp) => {
                            node.send_direct(p, len, pp, crate::TXT_ACK_DELAY_MS, now);
                        }
                        _ => reply::flood_reply(
                            node,
                            p,
                            req.region,
                            req.in_path_len,
                            crate::TXT_ACK_DELAY_MS,
                            now,
                        ),
                    }
                }
            }
            let mut answer = Reply::new();
            if !retry {
                self.command(node, ts, text, &mut answer, &mut Discard, now, rng);
            }
            if answer.is_empty() {
                return;
            }
            let mut stamp = node.rtc.unique(now);
            if stamp == ts {
                stamp = stamp.wrapping_add(1);
            }
            let mut out: heapless::Vec<u8, REPLY_MAX> = heapless::Vec::new();
            let _ = out.extend_from_slice(&stamp.to_le_bytes());
            let _ = out.push(1 << 2); // TXT_TYPE_CLI_DATA
            let _ = out.extend_from_slice(answer.as_bytes());
            reply::send(
                node,
                req,
                direct_or_flood(&path),
                PayloadType::TXT_MSG,
                &out,
                crate::CLI_REPLY_DELAY_MS,
                [0; 4],
                now,
            );
        }
    }

    /// `handleRequest`: the reply after the tag, false for an unknown or refused request.
    pub(crate) fn request(
        &mut self,
        node: &mut Node,
        client: &acl::Client,
        payload: &[u8],
        now: Instant,
        out: &mut heapless::Vec<u8, REPLY_MAX>,
    ) -> bool {
        let b = |i: usize| payload.get(i).copied().unwrap_or(0);
        match b(0) {
            REQ_STATUS => {
                let s = &node.disp.stats;
                let mut w = |v: &[u8]| {
                    let _ = out.extend_from_slice(v);
                };
                w(&self.board.battery_mv.to_le_bytes());
                w(&(node.disp.len() as u16).to_le_bytes());
                w(&self.board.noise_floor.to_le_bytes());
                w(&node.last_rssi.to_le_bytes());
                w(&(s.recv_flood.wrapping_add(s.recv_direct)).to_le_bytes());
                w(&(s.sent_flood.wrapping_add(s.sent_direct)).to_le_bytes());
                w(&u32::try_from(s.tx_airtime_ms / 1000)
                    .unwrap_or(u32::MAX)
                    .to_le_bytes());
                w(&self.uptime_secs(now).to_le_bytes());
                w(&s.sent_flood.to_le_bytes());
                w(&s.sent_direct.to_le_bytes());
                w(&s.recv_flood.to_le_bytes());
                w(&s.recv_direct.to_le_bytes());
                w(&s.err_flags.to_le_bytes());
                w(&i16::from(node.last_snr4).to_le_bytes());
                w(&(node.seen.direct_dups as u16).to_le_bytes());
                w(&(node.seen.flood_dups as u16).to_le_bytes());
                match self.role {
                    // `RepeaterStats`: 56 bytes.
                    Role::Repeater => {
                        w(&u32::try_from(s.rx_airtime_ms / 1000)
                            .unwrap_or(u32::MAX)
                            .to_le_bytes());
                        w(&s.recv_errors.to_le_bytes());
                    }
                    // `ServerStats`: 52 bytes, posts stored and pushed last.
                    Role::Room => {
                        w(&self.room.posted.to_le_bytes());
                        w(&self.room.pushes.to_le_bytes());
                    }
                }
                true
            }
            REQ_TELEMETRY => {
                // Byte 1 is the inverse of a permission mask; guests get the base only.
                let mut mask = !b(1);
                if client.role() == acl::GUEST {
                    mask = 0;
                }
                let cv = self.board.battery_mv / 10;
                let _ = out.extend_from_slice(&[1, 116]);
                let _ = out.extend_from_slice(&cv.to_be_bytes());
                if mask & TELEM_LOCATION != 0
                    && let Some((lat, lon)) = node.gps_pos
                {
                    let b3 = |x: i32| {
                        let b = x.to_be_bytes();
                        [b[1], b[2], b[3]]
                    };
                    let _ = out.extend_from_slice(&[1, 136]);
                    let _ = out.extend_from_slice(&b3(lat / 100));
                    let _ = out.extend_from_slice(&b3(lon / 100));
                    let _ = out.extend_from_slice(&b3(node.gps_alt_mm / 10));
                }
                if let Some(t) = self.board.mcu_temp_deci {
                    let _ = out.extend_from_slice(&[1, 103]);
                    let _ = out.extend_from_slice(&t.to_be_bytes());
                }
                true
            }
            REQ_ACCESS_LIST if client.is_admin() => {
                if b(1) != 0 || b(2) != 0 {
                    return false;
                }
                // The room server lists its admins only.
                let room = self.role == Role::Room;
                for c in self.acl.clients.iter().filter(|c| {
                    if room {
                        c.is_admin()
                    } else {
                        c.permissions != 0
                    }
                }) {
                    if out.len() + 7 > REPLY_MAX - 4 {
                        break;
                    }
                    let _ = out.extend_from_slice(c.key.get(..6).unwrap_or(&[]));
                    let _ = out.push(c.permissions);
                }
                true
            }
            REQ_NEIGHBOURS if b(1) == 0 && self.role == Role::Repeater => {
                let count = usize::from(b(2));
                let offset = usize::from(u16::from_le_bytes([b(3), b(4)]));
                let order = b(5);
                let prefix = usize::from(b(6)).min(PUB_KEY_LEN);
                let sorted = self.neighbours.sorted(order);
                let clock = node.rtc.now(now);
                let mut results: heapless::Vec<u8, NEIGHBOURS_MAX> = heapless::Vec::new();
                let mut n = 0u16;
                for nb in sorted.iter().skip(offset).take(count) {
                    if results.len() + prefix + 5 > NEIGHBOURS_MAX {
                        break;
                    }
                    let _ = results.extend_from_slice(nb.key.get(..prefix).unwrap_or(&[]));
                    let _ = results.extend_from_slice(&clock.wrapping_sub(nb.heard).to_le_bytes());
                    let _ = results.push(nb.snr4 as u8);
                    n += 1;
                }
                let _ = out.extend_from_slice(&(sorted.len() as u16).to_le_bytes());
                let _ = out.extend_from_slice(&n.to_le_bytes());
                let _ = out.extend_from_slice(&results);
                true
            }
            REQ_OWNER_INFO if self.role == Role::Repeater => {
                let mut s: heapless::String<{ REPLY_MAX - 4 }> = heapless::String::new();
                let name = core::str::from_utf8(c_str(&node.prefs.name)).unwrap_or("");
                let _ = write!(s, "{}\n{}\n{}", self.version, name, self.prefs.owner_info);
                let _ = out.extend_from_slice(s.as_bytes());
                true
            }
            _ => false,
        }
    }

    // --------------------------------------------------------------- discovery

    /// `onControlDataRecv`: answer a discovery request for repeaters; take answers to ours.
    fn on_control(
        &mut self,
        node: &mut Node,
        snr4: i8,
        data: &[u8],
        now: Instant,
        rng: &mut impl Rng,
    ) {
        let Some(&head) = data.first() else { return };
        let clock = node.rtc.now(now);
        let disabled = node.routing.as_ref().is_some_and(|r| r.disable_fwd);
        if head & 0xF0 == DISCOVER_REQ
            && data.len() >= 6
            && !disabled
            && self.discover_limiter.allow(clock)
            && !self.is_hidden()
        {
            let filter = data.get(1).copied().unwrap_or(0);
            let tag = data.get(2..6).unwrap_or(&[0; 4]);
            let since = if data.len() >= 10 { le32(data, 6) } else { 0 };
            if filter & (1 << TYPE_REPEATER) != 0 && self.prefs.discovery_mod_timestamp >= since {
                let prefix_only = head & 1 != 0;
                let mut resp: heapless::Vec<u8, 38> = heapless::Vec::new();
                let _ = resp.push(DISCOVER_RESP | TYPE_REPEATER);
                let _ = resp.push(snr4 as u8);
                let _ = resp.extend_from_slice(tag);
                let key = node.id.pub_key();
                let _ = resp
                    .extend_from_slice(key.get(..if prefix_only { 8 } else { 32 }).unwrap_or(&[]));
                let mut p = PacketBuf::new(AirRoute::Direct, PayloadType::CONTROL);
                if p.set_payload(&resp).is_ok() {
                    // Several nodes may answer: a random delay, widened × 4.
                    let tx_delay = node.prefs.tx_delay_milli;
                    let d = node.retransmit_delay(&p, tx_delay, rng) * 4;
                    node.send_zero_hop(p, None, d, now);
                }
            }
        } else if head & 0xF0 == DISCOVER_RESP && data.len() >= 6 {
            if head & 0x0F != TYPE_REPEATER || data.len() < 6 + PUB_KEY_LEN {
                return;
            }
            let Some((tag, until)) = self.pending_discover else {
                return;
            };
            if now >= until {
                self.pending_discover = None;
                return;
            }
            if le32(data, 2) != tag {
                return;
            }
            let Some(key) = data
                .get(6..6 + PUB_KEY_LEN)
                .and_then(|k| <[u8; 32]>::try_from(k).ok())
            else {
                return;
            };
            if key == *node.id.pub_key() {
                return;
            }
            self.neighbours.put(&key, clock, clock, snr4);
        }
    }

    /// `sendNodeDiscoverReq` (`discover.neighbors`): repeaters answering within 60 s become
    /// neighbours.
    pub fn discover(&mut self, node: &mut Node, now: Instant, rng: &mut impl Rng) {
        let tag = rng.next_u32();
        self.pending_discover = Some((tag, now + Duration::from_millis(DISCOVER_WINDOW_MS)));
        let mut data = [0u8; 10];
        data[0] = DISCOVER_REQ;
        data[1] = 1 << TYPE_REPEATER;
        data[2..6].copy_from_slice(&tag.to_le_bytes());
        let mut p = PacketBuf::new(AirRoute::Direct, PayloadType::CONTROL);
        if p.set_payload(&data).is_ok() {
            node.send_zero_hop(p, None, 0, now);
        }
    }
}

/// Where serial-only output goes when a command came over the air.
pub(crate) struct Discard;

impl Write for Discard {
    fn write_str(&mut self, _: &str) -> core::fmt::Result {
        Ok(())
    }
}

/// A logged-in client is a contact of the node, so that its requests decrypt.
pub(crate) fn ensure_contact(node: &mut Node, key: &[u8; PUB_KEY_LEN]) {
    if node.contacts.find(key).is_none() {
        let _ = node.contacts.list.push(Contact::new(*key));
    }
}

pub(crate) fn forget_contact(node: &mut Node, key: &[u8; PUB_KEY_LEN]) {
    if let Some(i) = node.contacts.list.iter().position(|c| c.pub_key == *key) {
        node.contacts.list.remove(i);
    }
}
