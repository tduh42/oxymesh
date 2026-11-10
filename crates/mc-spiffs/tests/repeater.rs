//! A C++ repeater's SPIFFS imported (`testdata/spiffs/cpp-repeater.bin`, made by
//! `testdata/spiffs/gen.py`: made-up data, an independent writer).

use mc_air::crypto::{PRV_KEY_LEN, PUB_KEY_LEN};
use mc_node::node::Routing;
use mc_node::persist::{PREFS_RECORD, decode_prefs};
use mc_node::region::RegionMap;
use mc_server::persist::{
    CLIENT_RECORD, SETTINGS_RECORD, ServerStore, decode_client, decode_settings,
};
use mc_server::repeater::ServerPrefs;
use mc_spiffs::{Spiffs, repeater};

const IMAGE: &[u8] = include_bytes!("../../../testdata/spiffs/cpp-repeater.bin");
const COMPANION: &[u8] = include_bytes!("../../../testdata/spiffs/cpp.bin");
const NODE: &str = "93eb0b364abd709af7f82a108b84966c97fb5f23dcc17a6d40381fe89d5ee97a";
const ADMIN: &str = "fc7bd2b78f9e07e50f2faae626bf9509b0c350ac27667acea0f6c5eb20c3de9b";

fn hex(s: &str) -> [u8; 32] {
    let mut k = [0u8; 32];
    for (i, o) in k.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap();
    }
    k
}

#[derive(Default)]
struct Mem {
    identity: Option<([u8; PRV_KEY_LEN], [u8; PUB_KEY_LEN])>,
    prefs: Option<[u8; PREFS_RECORD]>,
    settings: Option<[u8; SETTINGS_RECORD]>,
    clients: Vec<[u8; CLIENT_RECORD]>,
    regions: Vec<u8>,
}

impl ServerStore for Mem {
    fn save_identity(&mut self, prv: &[u8; PRV_KEY_LEN], pub_key: &[u8; PUB_KEY_LEN]) {
        self.identity = Some((*prv, *pub_key));
    }
    fn save_node_prefs(&mut self, rec: &[u8; PREFS_RECORD]) {
        self.prefs = Some(*rec);
    }
    fn save_settings(&mut self, rec: &[u8; SETTINGS_RECORD]) {
        self.settings = Some(*rec);
    }
    fn save_clients(&mut self, records: &mut dyn Iterator<Item = [u8; CLIENT_RECORD]>) {
        self.clients = records.collect();
    }
    fn save_regions(&mut self, file: &[u8]) {
        self.regions = file.to_vec();
    }
}

#[test]
fn a_repeater_is_told_from_a_companion() {
    let mut fs = Spiffs::new(IMAGE, IMAGE.len() as u32).unwrap();
    assert!(repeater::is_server(&mut fs).unwrap());
    let mut fs = Spiffs::new(COMPANION, COMPANION.len() as u32).unwrap();
    assert!(!repeater::is_server(&mut fs).unwrap());
}

#[test]
fn imports_everything() {
    let mut fs = Spiffs::new(IMAGE, IMAGE.len() as u32).unwrap();
    let mut st = Mem::default();
    let s = repeater::import(&mut fs, &mut st).unwrap();
    assert_eq!(s.key, hex(NODE));
    assert_eq!(&s.name[..12], b"C++ Repeater");
    assert_eq!((s.clients, s.regions), (2, 2));
    let radio = s.radio.unwrap();
    assert_eq!(
        (radio.freq_khz, radio.bw_hz, radio.sf, radio.cr),
        (869_618, 62_500, 8, 8)
    );
    assert_eq!(st.identity.unwrap().1, hex(NODE));

    let base = mc_node::Prefs::defaults(&hex(NODE));
    let p = decode_prefs(&st.prefs.unwrap(), &base).unwrap();
    assert_eq!(
        (p.lat, p.lon, p.advert_loc_policy),
        (45_900_000, 6_800_000, 2)
    );
    assert_eq!((p.tx_delay_milli, p.direct_tx_delay_milli), (500, 300));
    assert!(p.fem_rxgain);

    let mut sp = ServerPrefs::default();
    let mut r = Routing::REPEATER;
    assert!(decode_settings(&st.settings.unwrap(), &mut sp, &mut r));
    assert_eq!(sp.admin_password, "adm\"in");
    assert_eq!(sp.guest_password, "hello");
    assert_eq!(sp.owner_info, "Morgan\nCol du Midi");
    assert_eq!((sp.advert_interval, sp.flood_advert_interval), (0, 12));
    assert_eq!(
        (
            r.disable_fwd,
            r.flood_max,
            r.flood_max_unscoped,
            r.flood_max_advert,
            r.loop_detect
        ),
        (false, 32, 16, 4, 2)
    );

    let clients: Vec<_> = st
        .clients
        .iter()
        .map(|c| decode_client(c).unwrap())
        .collect();
    assert_eq!(clients[0].0.key, hex(ADMIN));
    assert_eq!((clients[0].0.permissions, clients[0].1), (3, 0xFF));
    assert_eq!((clients[1].0.permissions, clients[1].1), (1, 2));
    assert_eq!(clients[1].2[..2], [0x11, 0x22]);

    let m = RegionMap::decode(&st.regions).unwrap();
    let eu = m.find_by_name("#eu").unwrap();
    let fr = m.get(m.find_by_name("fr").unwrap()).unwrap();
    assert_eq!(fr.parent, eu);
    assert!(!fr.allows_flood());
    assert_eq!(m.home().unwrap().name, "fr");
    assert_eq!(m.default_region().unwrap().name, "#eu");
}

#[test]
fn the_summary_reads_the_same() {
    let mut fs = Spiffs::new(IMAGE, IMAGE.len() as u32).unwrap();
    let s = repeater::summary(&mut fs).unwrap();
    assert_eq!((s.clients, s.regions), (2, 2));
}
