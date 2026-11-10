//! What a repeater keeps across restarts (`docs/spec/server.md` §5): the node's settings (the
//! companion's record), the repeater's own settings, the access list and the region table. The
//! runner stores the records under its own keys ([`ServerStore`]); this module only encodes
//! them.
//!
//! - Settings: [`SETTINGS_RECORD`] bytes, versioned, described at [`encode_settings`].
//! - A client: the C++ `/s_contacts` record (`ClientACL::save`, MeshCore, MIT) without its
//!   last 32 bytes, the shared secret, which is derived again from our key: key (32),
//!   permissions, `sync_since` (u32 LE), 2 unused, `out_path_len`, `out_path` (64).
//! - Regions: the C++ `/regions2` file as is (`mc_node::region`).

use mc_air::crypto::{PRV_KEY_LEN, PUB_KEY_LEN};
use mc_core::{Reader, Writer};
use mc_node::Node;
use mc_node::contacts::PATH_UNKNOWN;
use mc_node::node::Routing;
use mc_node::persist::{PREFS_RECORD, encode_prefs};
use mc_node::region::{FILE_MAX, RegionMap};

use crate::acl::Client;
use crate::server::{Action, Server, ServerPrefs};

/// The repeater's settings record.
pub const SETTINGS_RECORD: usize = 164;
const SETTINGS_VERSION: u8 = 1;
/// A client in the access list.
pub const CLIENT_RECORD: usize = 32 + 1 + 4 + 2 + 1 + 64;
/// The same in the C++ `/s_contacts`, with the shared secret.
pub const CPP_CLIENT_RECORD: usize = CLIENT_RECORD + 32;

/// Where a server's records go (the firmware's flash map, a test's memory).
pub trait ServerStore {
    fn save_identity(&mut self, prv: &[u8; PRV_KEY_LEN], pub_key: &[u8; PUB_KEY_LEN]);
    /// The node's settings, in the companion's record (`mc_node::persist`).
    fn save_node_prefs(&mut self, rec: &[u8; PREFS_RECORD]);
    fn save_settings(&mut self, rec: &[u8; SETTINGS_RECORD]);
    /// The whole access list, replacing the stored one.
    fn save_clients(&mut self, records: &mut dyn Iterator<Item = [u8; CLIENT_RECORD]>);
    /// A `/regions2` file.
    fn save_regions(&mut self, file: &[u8]);
}

fn c_string<const N: usize>(b: &[u8]) -> heapless::String<N> {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    let mut s = heapless::String::new();
    for ch in core::str::from_utf8(b.get(..end).unwrap_or(&[]))
        .unwrap_or("")
        .chars()
    {
        if s.push(ch).is_err() {
            break;
        }
    }
    s
}

fn fixed<const N: usize>(s: &str) -> [u8; N] {
    let mut b = [0u8; N];
    for (o, c) in b.iter_mut().take(N - 1).zip(s.bytes()) {
        *o = c;
    }
    b
}

/// The settings record: version (1), admin password (16), guest password (16), owner info
/// (120), `advert_interval`, `flood_advert_interval`, flags (bit 0 `allow_read_only`, bit 1
/// `powersaving`, bit 2 `disable_fwd`), `flood_max`, `flood_max_unscoped`, `flood_max_advert`,
/// `loop_detect`, `discovery_mod_timestamp` (u32 LE). Strings are NUL padded.
pub fn encode_settings(p: &ServerPrefs, r: &Routing) -> [u8; SETTINGS_RECORD] {
    let mut b = [0u8; SETTINGS_RECORD];
    let mut w = Writer::new(&mut b);
    let flags = u8::from(p.allow_read_only)
        | (u8::from(p.powersaving) << 1)
        | (u8::from(r.disable_fwd) << 2);
    let _ = (|| {
        w.put_u8(SETTINGS_VERSION)?;
        w.put(&fixed::<16>(&p.admin_password))?;
        w.put(&fixed::<16>(&p.guest_password))?;
        w.put(&fixed::<120>(&p.owner_info))?;
        w.put(&[
            p.advert_interval,
            p.flood_advert_interval,
            flags,
            r.flood_max,
            r.flood_max_unscoped,
            r.flood_max_advert,
            r.loop_detect,
        ])?;
        w.put(&p.discovery_mod_timestamp.to_le_bytes())
    })();
    b
}

/// Reads the settings back into `p` and `r` (the regions untouched); false if the record is
/// not one of ours (they are then left as they were).
pub fn decode_settings(b: &[u8], p: &mut ServerPrefs, r: &mut Routing) -> bool {
    let mut rd = Reader::new(b);
    let Ok(SETTINGS_VERSION) = rd.u8() else {
        return false;
    };
    let (Ok(admin), Ok(guest), Ok(owner), Ok(n), Ok(ts)) = (
        rd.array::<16>(),
        rd.array::<16>(),
        rd.array::<120>(),
        rd.array::<7>(),
        rd.array::<4>(),
    ) else {
        return false;
    };
    p.admin_password = c_string(admin);
    p.guest_password = c_string(guest);
    p.owner_info = c_string(owner);
    let [adv, f_adv, flags, f_max, f_max_uns, f_max_adv, loop_detect] = *n;
    p.advert_interval = adv;
    p.flood_advert_interval = f_adv;
    p.allow_read_only = flags & 1 != 0;
    p.powersaving = flags & 2 != 0;
    r.disable_fwd = flags & 4 != 0;
    r.flood_max = f_max;
    r.flood_max_unscoped = f_max_uns;
    r.flood_max_advert = f_max_adv;
    r.loop_detect = loop_detect.min(3);
    p.discovery_mod_timestamp = u32::from_le_bytes(*ts);
    true
}

/// A client and its path back.
pub fn encode_client(c: &Client, out_path_len: u8, out_path: &[u8]) -> [u8; CLIENT_RECORD] {
    let mut b = [0u8; CLIENT_RECORD];
    let mut path = [0u8; 64];
    for (o, v) in path.iter_mut().zip(out_path) {
        *o = *v;
    }
    let mut w = Writer::new(&mut b);
    let _ = (|| {
        w.put(&c.key)?;
        w.put_u8(c.permissions)?;
        w.put(&c.sync_since.to_le_bytes())?;
        w.put(&[0, 0])?;
        w.put_u8(out_path_len)?;
        w.put(&path)
    })();
    b
}

/// A client record (ours, or the C++'s with its secret after it, ignored): the client, its
/// path length and path.
pub fn decode_client(b: &[u8]) -> Option<(Client, u8, [u8; 64])> {
    let mut r = Reader::new(b);
    let key = *r.array::<PUB_KEY_LEN>().ok()?;
    let permissions = r.u8().ok()?;
    let sync_since = u32::from_le_bytes(*r.array::<4>().ok()?);
    r.array::<2>().ok()?;
    let out_path_len = r.u8().ok()?;
    let out_path = *r.array::<64>().ok()?;
    let mut c = Client::new(key, permissions);
    c.sync_since = sync_since;
    Some((c, out_path_len, out_path))
}

impl Server {
    /// Carries out a save action (`SaveAcl`, `SavePrefs`, `SaveRegions`, `SaveIdentity`);
    /// false for the others, which are the runner's.
    pub fn save(&self, node: &Node, action: Action, st: &mut impl ServerStore) -> bool {
        let fallback = Routing::REPEATER;
        let routing = node.routing.as_ref().unwrap_or(&fallback);
        match action {
            Action::SavePrefs => {
                st.save_node_prefs(&encode_prefs(&node.prefs));
                st.save_settings(&encode_settings(&self.prefs, routing));
            }
            Action::SaveAcl => {
                // Clients with permissions only, as `ClientACL::save` skips the others; the
                // room server keeps its admins only (`saveFilter`).
                let room = self.role == crate::server::Role::Room;
                let mut it = self
                    .acl
                    .clients
                    .iter()
                    .filter(|c| c.permissions != 0 && (!room || c.is_admin()))
                    .map(|c| {
                        let contact = node
                            .contacts
                            .find(&c.key)
                            .and_then(|i| node.contacts.get(i));
                        match contact {
                            Some(ct) => encode_client(c, ct.out_path_len, ct.out_path()),
                            None => encode_client(c, PATH_UNKNOWN, &[]),
                        }
                    });
                st.save_clients(&mut it);
            }
            Action::SaveRegions => {
                let mut file = [0u8; FILE_MAX];
                if let Some(n) = routing.regions.encode(&mut file) {
                    st.save_regions(file.get(..n).unwrap_or(&[]));
                }
            }
            Action::SaveIdentity(prv) => {
                if let Some(id) = crate::console::valid_private_key(&prv) {
                    st.save_identity(id.private_key(), id.pub_key());
                }
            }
            _ => return false,
        }
        true
    }

    /// What was stored, before [`Server::start`]: the settings, the clients (with their
    /// paths, as node contacts) and the regions. Missing or unreadable parts keep the
    /// defaults.
    pub fn restore<'a>(
        &mut self,
        node: &mut Node,
        settings: Option<&[u8]>,
        clients: impl Iterator<Item = &'a [u8]>,
        regions: Option<&[u8]>,
    ) {
        let routing = node.routing.get_or_insert(Routing::REPEATER);
        if let Some(b) = settings {
            decode_settings(b, &mut self.prefs, routing);
        }
        if let Some(m) = regions.and_then(RegionMap::decode) {
            routing.regions = m;
        }
        for rec in clients {
            let Some((c, len, path)) = decode_client(rec) else {
                continue;
            };
            if self.acl.clients.push(c).is_err() {
                break;
            }
            crate::server::ensure_contact(node, &c.key);
            if let Some(ct) = node
                .contacts
                .find(&c.key)
                .and_then(|i| node.contacts.get_mut(i))
            {
                let valid = mc_air::PathLen::from_byte(len).is_ok_and(|p| p.byte_len() <= 64);
                ct.out_path_len = if valid { len } else { PATH_UNKNOWN };
                ct.out_path = path;
            }
        }
    }
}
