//! A repeater's state survives a restart (`docs/spec/server.md` §5).

use mc_air::LocalIdentity;
use mc_air::crypto::{PRV_KEY_LEN, PUB_KEY_LEN};
use mc_core::Instant;
use mc_node::cli::Reply;
use mc_node::persist::{PREFS_RECORD, decode_prefs};
use mc_node::{Node, Prefs};
use mc_server::Server;
use mc_server::persist::{
    CLIENT_RECORD, SETTINGS_RECORD, ServerStore, decode_client, decode_settings, encode_settings,
};
use rand_chacha::ChaCha20Rng;
use rand_chacha::rand_core::SeedableRng;

#[derive(Default)]
struct Mem {
    identity: Option<[u8; PRV_KEY_LEN]>,
    prefs: Option<[u8; PREFS_RECORD]>,
    settings: Option<[u8; SETTINGS_RECORD]>,
    clients: Vec<[u8; CLIENT_RECORD]>,
    regions: Vec<u8>,
}

impl ServerStore for Mem {
    fn save_identity(&mut self, prv: &[u8; PRV_KEY_LEN], _: &[u8; PUB_KEY_LEN]) {
        self.identity = Some(*prv);
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

fn fresh(now: Instant) -> (Node, Server) {
    let id = LocalIdentity::from_seed(&[3; 32]);
    let prefs = Prefs::defaults(id.pub_key());
    (
        Node::new(id, prefs, 1_793_900_000, now),
        Server::new(now, "v1.18.0-oxymesh", mc_server::Role::Repeater),
    )
}

#[test]
fn settings_clients_and_regions_come_back() {
    let now = Instant::from_nanos(1_000_000_000);
    let (mut node, mut r) = fresh(now);
    r.start(&mut node, now);
    let mut rng = ChaCha20Rng::seed_from_u64(1);
    let admin = "CD".repeat(32);
    for line in [
        "password s3cret",
        "set guest.password hello",
        "set owner.info Morgan|Col du Midi",
        "set flood.max 12",
        "set flood.max.advert 3",
        "set loop.detect moderate",
        "set repeat off",
        "set name Relay Col",
        "set advert.interval 120",
        "region put eu",
        "region put fr eu",
        "region home fr",
        &format!("setperm {admin} 3"),
        "region save",
    ] {
        let mut out = Reply::new();
        let mut serial = String::new();
        r.command(
            &mut node,
            0,
            line.as_bytes(),
            &mut out,
            &mut serial,
            now,
            &mut rng,
        );
    }
    // The client's path, learned meanwhile.
    let k = [0xCD; 32];
    let ct = node
        .contacts
        .find(&k)
        .and_then(|i| node.contacts.get_mut(i))
        .unwrap();
    ct.out_path_len = 2;
    ct.out_path[..2].copy_from_slice(&[0x11, 0x22]);

    let mut st = Mem::default();
    for a in [
        mc_server::server::Action::SavePrefs,
        mc_server::server::Action::SaveAcl,
        mc_server::server::Action::SaveRegions,
    ] {
        assert!(r.save(&node, a, &mut st));
    }
    assert_eq!(st.clients.len(), 1);

    let (mut node2, mut r2) = fresh(now);
    node2.prefs = decode_prefs(&st.prefs.unwrap(), &node2.prefs).unwrap();
    r2.restore(
        &mut node2,
        st.settings.as_ref().map(|s| &s[..]),
        st.clients.iter().map(|c| &c[..]),
        Some(&st.regions),
    );
    r2.start(&mut node2, now);
    assert_eq!(r2.prefs, r.prefs);
    assert_eq!(r2.prefs.owner_info, "Morgan\nCol du Midi");
    assert_eq!(r2.prefs.advert_interval, 60);
    let (a, b) = (
        node.routing.as_ref().unwrap(),
        node2.routing.as_ref().unwrap(),
    );
    assert_eq!(
        (
            b.disable_fwd,
            b.flood_max,
            b.flood_max_advert,
            b.loop_detect
        ),
        (true, 12, 3, 2)
    );
    let names = |m: &mc_node::region::RegionMap| -> Vec<String> {
        m.iter().map(|g| g.name.to_string()).collect()
    };
    assert_eq!(names(&b.regions), names(&a.regions));
    assert_eq!(b.regions.home().unwrap().name, "fr");
    assert_eq!(&node2.prefs.name[..9], b"Relay Col");
    let ct = node2
        .contacts
        .get(node2.contacts.find(&k).unwrap())
        .unwrap();
    assert_eq!(ct.out_path(), [0x11, 0x22]);
    assert_eq!(r2.acl.clients[0].permissions, 3);
}

#[test]
fn a_cpp_client_record_reads_without_its_secret() {
    let mut rec = vec![0x42; 32];
    rec.push(2); // read-write
    rec.extend_from_slice(&7u32.to_le_bytes());
    rec.extend_from_slice(&[0, 0, 1]);
    rec.extend_from_slice(&[0x99; 64]);
    rec.extend_from_slice(&[0xEE; 32]); // shared secret
    let (c, len, path) = decode_client(&rec).unwrap();
    assert_eq!(
        (c.key, c.permissions, c.sync_since, len),
        ([0x42; 32], 2, 7, 1)
    );
    assert_eq!(path[0], 0x99);
    assert!(decode_client(&rec[..40]).is_none());
}

#[test]
fn unknown_settings_are_ignored() {
    let mut p = mc_server::server::ServerPrefs::default();
    let mut r = mc_node::node::Routing::REPEATER;
    let mut rec = encode_settings(&p, &r);
    rec[0] = 9;
    p.guest_password.push_str("x").unwrap();
    assert!(!decode_settings(&rec, &mut p, &mut r));
    assert_eq!(p.guest_password, "x");
}
