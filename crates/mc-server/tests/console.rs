//! The repeater console (`docs/spec/server.md` §4): replies and effects, as the C++ gives them
//! (`MyMesh::handleCommand`, `CommonCLI.cpp`).

use mc_air::LocalIdentity;
use mc_core::{Duration, Instant};
use mc_node::cli::Reply;
use mc_node::{Node, Prefs};
use mc_server::Server;
use mc_server::server::{Action, Board, Gps};
use rand_chacha::ChaCha20Rng;
use rand_chacha::rand_core::SeedableRng;

const EPOCH: u32 = 1_793_900_000;

struct Rig {
    node: Node,
    r: Server,
    now: Instant,
    rng: ChaCha20Rng,
    serial: String,
}

impl Rig {
    fn new() -> Self {
        let id = LocalIdentity::from_seed(&[3; 32]);
        let mut prefs = Prefs::defaults(id.pub_key());
        prefs.name = [0; 32];
        prefs.name[..6].copy_from_slice(b"Morgan");
        let now = Instant::from_nanos(1_000_000_000);
        let mut node = Node::new(id, prefs, EPOCH, now);
        let mut r = Server::new(now, "v1.18.0-oxymesh", mc_server::Role::Repeater);
        r.board = Board {
            battery_mv: 4_012,
            manufacturer: "Elecrow ThinkNode M5",
            build_date: "10-Nov-2026",
            ..Default::default()
        };
        r.start(&mut node, now);
        Self {
            node,
            r,
            now,
            rng: ChaCha20Rng::seed_from_u64(9),
            serial: String::new(),
        }
    }

    /// One line, as from the serial port (`ts` 0) or over the air.
    fn at(&mut self, ts: u32, cmd: &str) -> String {
        let mut out = Reply::new();
        let mut serial = String::new();
        self.r.command(
            &mut self.node,
            ts,
            cmd.as_bytes(),
            &mut out,
            &mut serial,
            self.now,
            &mut self.rng,
        );
        self.serial = serial;
        out.to_string()
    }

    fn cli(&mut self, cmd: &str) -> String {
        self.at(0, cmd)
    }

    fn air(&mut self, cmd: &str) -> String {
        self.at(EPOCH + 10, cmd)
    }

    fn actions(&mut self) -> Vec<Action> {
        std::iter::from_fn(|| self.r.poll_action()).collect()
    }

    fn wait(&mut self, ms: u64) {
        self.now = self.now + Duration::from_millis(ms);
        self.r.tick(&mut self.node, self.now, &mut self.rng);
    }
}

#[test]
fn identity_and_version() {
    let mut r = Rig::new();
    assert_eq!(r.cli("ver"), "v1.18.0-oxymesh (Build: 10-Nov-2026)");
    assert_eq!(r.cli("board"), "Elecrow ThinkNode M5");
    assert_eq!(r.cli("get role"), "> repeater");
    assert_eq!(r.cli("get name"), "> Morgan");
    let pk = r.cli("get public.key");
    assert_eq!(pk.len(), 2 + 64);
    assert!(
        pk[2..]
            .chars()
            .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
    );
    assert_eq!(
        r.cli("power"),
        "batt:4012 mV usb:n/a solar_chg:n/a ext:no charger:no"
    );
    assert_eq!(r.cli("frobnicate"), "Unknown command");
    assert_eq!(r.cli("get frob"), "??: frob");
    assert_eq!(r.cli("set frob 1"), "unknown config: frob 1");
}

#[test]
fn the_companion_prefix_is_reflected() {
    let mut r = Rig::new();
    assert_eq!(r.air("ab|get role"), "ab|> repeater");
    assert_eq!(r.air("  get role"), "> repeater", "leading spaces skipped");
}

#[test]
fn private_key_from_the_serial_port_only() {
    let mut r = Rig::new();
    assert_eq!(r.cli("get prv.key").len(), 2 + 128);
    assert_eq!(r.air("get prv.key"), "??: prv.key");
}

#[test]
fn clock() {
    let mut r = Rig::new();
    // EPOCH = 2026-11-05 17:33:20 UTC.
    assert_eq!(r.cli("clock"), "17:33 - 5/11/2026 UTC");
    assert_eq!(r.cli("time 1700000000"), "(ERR: clock cannot go backwards)");
    assert_eq!(
        r.cli("time 1793903600"),
        "OK - clock set: 18:33 - 5/11/2026 UTC"
    );
    assert_eq!(r.at(EPOCH, "clock sync"), "ERR: clock cannot go backwards");
    assert_eq!(
        r.at(1_793_907_199, "clock sync"),
        "OK - clock set: 19:33 - 5/11/2026 UTC"
    );
}

#[test]
fn restarts_say_nothing() {
    let mut r = Rig::new();
    assert_eq!(r.air("rebootx"), "", "prefix match, like memcmp");
    assert_eq!(r.actions(), [Action::Reboot]);
    assert_eq!(r.air("shutdown"), "");
    assert_eq!(r.actions(), [Action::PowerOff]);
    assert_eq!(r.air("erase"), "Unknown command", "serial only");
    assert_eq!(r.cli("erase"), "File system erase: OK");
    assert_eq!(r.actions(), [Action::Erase]);
}

#[test]
fn any_saved_setting_stops_the_2_minute_adverts() {
    let mut r = Rig::new();
    assert_eq!(r.cli("get advert.interval"), "> 2");
    assert_eq!(r.cli("set guest.password hello"), "OK");
    assert_eq!(r.cli("get guest.password"), "> hello");
    assert_eq!(r.cli("get advert.interval"), "> 0", "CommonCLI::savePrefs");
    assert_eq!(r.actions(), [Action::SavePrefs]);
    assert_eq!(
        r.cli("set advert.interval 30"),
        "Error: interval range is 60-240 minutes"
    );
    assert_eq!(r.cli("set advert.interval 90"), "OK");
    assert_eq!(r.cli("get advert.interval"), "> 90");
    assert_eq!(
        r.cli("set flood.advert.interval 2"),
        "Error: interval range is 3-168 hours"
    );
    assert_eq!(r.cli("set flood.advert.interval 0"), "OK");
    assert_eq!(r.cli("get flood.advert.interval"), "> 0");
}

#[test]
fn forwarding_settings() {
    let mut r = Rig::new();
    assert_eq!(r.cli("get repeat"), "> on");
    assert_eq!(r.cli("set repeat off"), "OK - repeat is now OFF");
    assert!(r.node.routing.as_ref().unwrap().disable_fwd);
    assert_eq!(r.cli("set repeat on"), "OK - repeat is now ON");
    assert_eq!(r.cli("set flood.max 70"), "Error, max 64");
    // `uint8_t m = atoi(...)`: 300 wraps to 44, which passes.
    assert_eq!(r.cli("set flood.max 300"), "OK");
    assert_eq!(r.cli("get flood.max"), "> 44");
    assert_eq!(r.cli("set flood.max.advert 4"), "OK");
    assert_eq!(r.cli("get flood.max.advert"), "> 4");
    assert_eq!(r.cli("set flood.max.unscoped 0"), "OK");
    assert_eq!(r.cli("get flood.max.unscoped"), "> 0");
    assert_eq!(r.cli("get loop.detect"), "> minimal");
    assert_eq!(r.cli("set loop.detect strict"), "OK");
    assert_eq!(r.cli("get loop.detect"), "> strict");
    assert_eq!(
        r.cli("set loop.detect some"),
        "Error, must be: off, minimal, moderate, or strict"
    );
}

#[test]
fn owner_info_keeps_its_bars() {
    let mut r = Rig::new();
    assert_eq!(r.cli("set owner.info Morgan|on 869.618"), "OK");
    assert_eq!(r.r.prefs.owner_info, "Morgan\non 869.618");
    assert_eq!(r.cli("get owner.info"), "> Morgan|on 869.618");
}

#[test]
fn position_and_name() {
    let mut r = Rig::new();
    assert_eq!(r.cli("set lat 45.5"), "OK");
    assert_eq!(r.node.prefs.lat, 45_500_000);
    assert_eq!(r.cli("get lat"), "> 45.5");
    assert_eq!(r.cli("set name Col*"), "Error, bad chars");
    assert_eq!(r.cli("set name Relay Col"), "OK");
    assert_eq!(r.cli("get name"), "> Relay Col");
}

#[test]
fn frequency_from_the_serial_port_only() {
    let mut r = Rig::new();
    assert_eq!(r.air("set freq 869.525"), "unknown config: freq 869.525");
    assert_eq!(r.cli("set freq 869.525"), "OK - reboot to apply");
    assert_eq!(r.node.prefs.radio.freq_khz, 869_525);
}

#[test]
fn radio_commands_are_shared_with_the_companion() {
    let mut r = Rig::new();
    assert!(r.cli("get radio").starts_with("> 869."));
    assert_eq!(r.cli("set tx 14"), "OK");
    assert_eq!(r.actions(), [Action::SavePrefs, Action::RadioChanged]);
}

#[test]
fn passwords() {
    let mut r = Rig::new();
    assert_eq!(
        r.cli("password a-very-long-password"),
        "password now: a-very-long-pas"
    );
    assert_eq!(r.r.prefs.admin_password, "a-very-long-pas");
}

#[test]
fn temporary_radio_settings() {
    let mut r = Rig::new();
    assert_eq!(r.cli("tempradio 869.5,62.5,8"), "Error, invalid params");
    assert_eq!(
        r.cli("tempradio 869.5,62.5,8,5,2"),
        "OK - temp params for 2 mins"
    );
    r.wait(1000);
    assert_eq!(r.actions(), []);
    r.wait(1000);
    let a = r.actions();
    assert!(
        matches!(a.as_slice(), [Action::TempRadio(p)] if p.freq_khz == 869_500 && p.bw_hz == 62_500 && p.sf == 8 && p.cr == 5),
        "{a:?}"
    );
    r.wait(2 * 60 * 1000);
    assert_eq!(r.actions(), [Action::RestoreRadio]);
}

#[test]
fn neighbours() {
    let mut r = Rig::new();
    assert_eq!(r.cli("neighbors"), "-none-");
    let clock = r.node.rtc.now(r.now);
    r.r.neighbours.put(&[0xAB; 32], 1, clock - 30, -6);
    r.r.neighbours.put(&[0x12; 32], 1, clock - 5, 22);
    assert_eq!(r.cli("neighbors"), "12121212:5:22\nABABABAB:30:-6");
    assert_eq!(r.cli("neighbor.remove abc"), "ERR: bad pubkey");
    assert_eq!(r.cli("neighbor.remove abab"), "OK");
    assert_eq!(r.cli("neighbors"), "12121212:5:22");
    assert_eq!(
        r.cli("discover.neighbors now"),
        "Err - discover.neighbors has no options"
    );
    assert_eq!(r.cli("discover.neighbors"), "OK - Discover sent");
}

#[test]
fn access_list_by_hand() {
    let mut r = Rig::new();
    let key = "AB".repeat(32);
    assert_eq!(r.cli("setperm abcd"), "Err - bad params");
    assert_eq!(
        r.cli(&format!("setperm {} 3", &key[..10])),
        "Err - invalid params",
        "whole key needed"
    );
    assert_eq!(r.cli(&format!("setperm {key} 3")), "OK");
    assert!(r.node.contacts.find(&[0xAB; 32]).is_some());
    r.cli("get acl");
    assert_eq!(r.serial, format!("ACL:\n03 {key}\n"));
    assert_eq!(
        r.cli("setperm abab 0"),
        "OK",
        "a guest role removes, by prefix"
    );
    assert!(r.r.acl.is_empty());
    assert!(r.node.contacts.find(&[0xAB; 32]).is_none());
    r.wait(5000);
    assert!(r.actions().contains(&Action::SaveAcl));
    // A restart before the lazy save is due: the runner is told the save still waits, once.
    assert!(!r.r.take_pending_acl_save());
    assert_eq!(r.cli(&format!("setperm {key} 3")), "OK");
    assert!(r.r.take_pending_acl_save());
    assert!(!r.r.take_pending_acl_save());
    r.wait(5000);
    assert!(!r.actions().contains(&Action::SaveAcl));
}

#[test]
fn regions() {
    let mut r = Rig::new();
    assert_eq!(r.cli("region put eu"), "OK - (flood allowed)");
    assert_eq!(r.cli("region put fr eu"), "OK - (flood allowed)");
    assert_eq!(r.cli("region put xx nowhere"), "Err - unknown parent");
    assert_eq!(r.cli("region get fr"), " fr (eu) F");
    assert_eq!(r.cli("region denyf fr"), "OK");
    assert_eq!(r.cli("region get fr"), " fr (eu) ");
    assert_eq!(r.cli("region list denied"), "fr");
    assert_eq!(
        r.cli("region list maybe"),
        "Err - use 'allowed' or 'denied'"
    );
    assert_eq!(r.cli("region home"), " home is *");
    assert_eq!(r.cli("region home f"), " home is now fr");
    assert_eq!(r.cli("region default"), " default scope is <null>");
    assert_eq!(r.cli("region default be"), " default scope is now be");
    assert_eq!(r.actions(), [Action::SaveRegions]);
    assert_eq!(r.cli("region remove eu"), "Err - not empty");
    assert_eq!(r.cli("region remove e"), "Err - not found", "exact name");
    assert_eq!(r.cli("region remove fr"), "OK");
    assert_eq!(r.cli("region frob"), "Err - ??");
    assert_eq!(r.cli("region save"), "OK");
    assert_eq!(r.actions(), [Action::SavePrefs, Action::SaveRegions]);
    assert_eq!(r.r.prefs.discovery_mod_timestamp, r.node.rtc.now(r.now));
}

#[test]
fn region_def_builds_a_path() {
    let mut r = Rig::new();
    let out = r.cli("region def eu fr|eu de");
    let names: Vec<&str> = r
        .node
        .routing
        .as_ref()
        .unwrap()
        .regions
        .iter()
        .map(|g| g.name.as_str())
        .collect();
    assert_eq!(names, ["eu", "fr", "de"]);
    assert!(out.contains("eu"), "{out}");
    assert_eq!(r.cli("region def"), "Err - empty def");
    assert_eq!(r.cli("region def a|"), "Err - empty jump");
    assert_eq!(r.cli("region def b|zz"), "Err - unknown jump: zz");
}

#[test]
fn region_load_line_by_line() {
    let mut r = Rig::new();
    assert_eq!(r.cli("region put old"), "OK - (flood allowed)");
    assert_eq!(r.cli("region load"), "");
    assert_eq!(r.cli(" eu F"), "");
    assert_eq!(r.cli("  fr"), "");
    assert_eq!(r.cli(" old"), "");
    assert_eq!(r.cli(""), "OK - loaded 3 regions");
    let regions = &r.node.routing.as_ref().unwrap().regions;
    let get = |n: &str| regions.get(regions.find_by_name(n).unwrap()).unwrap();
    assert!(get("eu").allows_flood());
    assert!(!get("fr").allows_flood(), "no F: denied");
    assert_eq!(get("fr").parent, get("eu").id);
    assert!(get("old").allows_flood(), "flags carried over");
    assert_eq!(r.cli("get role"), "> repeater", "back to commands");
}

#[test]
fn stats_from_the_serial_port() {
    let mut r = Rig::new();
    r.node.last_snr4 = -9;
    assert_eq!(
        r.cli("stats-core"),
        "{\"battery_mv\":4012,\"uptime_secs\":0,\"errors\":0,\"queue_len\":1}"
    );
    assert!(r.cli("stats-radio").contains("\"last_snr\":-2.25,"));
    assert!(r.cli("stats-packets").starts_with("{\"recv\":0,"));
    assert_eq!(r.air("stats-core"), "Unknown command");
    assert_eq!(r.cli("clear stats"), "(OK - stats reset)");
}

#[test]
fn gps_only_with_a_gps() {
    let mut r = Rig::new();
    assert_eq!(r.cli("gps"), "Unknown command");
    assert_eq!(r.cli("sensor list"), "no custom var");
    r.r.board.gps = Some(Gps {
        enabled: true,
        active: true,
        fix: false,
        sats: 3,
        ..Default::default()
    });
    assert_eq!(r.cli("gps"), "on, active, no fix, 3 sats");
    assert_eq!(r.cli("sensor get gps"), "> 1");
    assert_eq!(r.cli("sensor list"), "1 vars\ngps=1");
    assert_eq!(r.cli("sensor set gps 0"), "ok");
    assert_eq!(r.cli("sensor set nope 1"), "can't find custom var");
    assert_eq!(r.cli("gps advert"), "> none");
    assert_eq!(r.cli("gps advert share"), "ok");
    assert_eq!(r.node.prefs.advert_loc_policy, 1);
    assert_eq!(r.cli("gps off"), "ok");
    assert!(r.actions().contains(&Action::Gps(false)));
}

#[test]
fn unsupported_on_these_boards() {
    let mut r = Rig::new();
    assert_eq!(r.cli("powersaving on"), "Board not supported");
    assert_eq!(r.cli("powersaving"), "off");
    assert_eq!(r.cli("start ota"), "Error");
    assert_eq!(r.cli("log start"), "Error: unsupported");
    assert_eq!(r.cli("get bridge.type"), "> none");
    assert_eq!(r.cli("get pwrmgt.support"), "> unsupported");
    assert_eq!(
        r.cli("set radio.fem.rxgain on"),
        "unknown config: radio.fem.rxgain on"
    );
}

#[test]
fn a_new_private_key() {
    let mut r = Rig::new();
    assert_eq!(r.cli("set prv.key 1234"), "Error, bad key");
    let id = LocalIdentity::from_seed(&[5; 32]);
    let hex: String = id
        .private_key()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let out = r.cli(&format!("set prv.key {hex}"));
    let want: String = id.pub_key().iter().map(|b| format!("{b:02X}")).collect();
    assert_eq!(out, format!("OK, reboot to apply! New pubkey: {want}"));
    assert_eq!(r.actions(), [Action::SaveIdentity(*id.private_key())]);
}
