//! A MeshCore C++ repeater's data (`examples/simple_repeater`, `src/helpers/CommonCLI.cpp`,
//! `ClientACL.cpp`, `RegionMap.cpp`; MeshCore, MIT; `docs/spec/server.md` §5), imported into
//! oxymesh's storage.
//!
//! - `/identity/_main.id`, as the companion's;
//! - `/prefs.json`: the node's settings (as the companion's) and the repeater's (`pass`,
//!   `guest`, `owner`, `adv_int`, `f_adv_int`, `repeat`, `room`, `power`); the older binary
//!   `/com_prefs` is not read, v1.18 rewrites it as `/prefs.json` at its first boot;
//! - `/s_contacts`: 136-byte client records, of which the shared secret is dropped;
//! - `/regions2`: the region table, the same format as oxymesh's.

use mc_node::dispatch::RadioParams;
use mc_node::node::{Prefs, Routing};
use mc_node::persist::encode_prefs;
use mc_node::region::{FILE_MAX, RegionMap};
use mc_server::persist::{
    CPP_CLIENT_RECORD, ServerStore, decode_client, encode_client, encode_settings,
};
use mc_server::repeater::{ServerPrefs, default_node_prefs};

use crate::cpp::{ImportError, PREFS, identity, prefs};
use crate::json::{Value, fixed, unescape, walk};
use crate::{Error, Flash, Spiffs};

pub const ACL: &str = "/s_contacts";
pub const REGIONS: &str = "/regions2";
/// Longest settings file read.
const PREFS_MAX: usize = 2048;
/// `MAX_CLIENTS`.
const CLIENTS_MAX: usize = 32;

/// What an import brings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub name: [u8; 32],
    pub key: [u8; 32],
    /// Clients in the access list.
    pub clients: u8,
    /// Regions, the wildcard not counted.
    pub regions: u8,
    /// `None` when the settings file is missing or unreadable.
    pub radio: Option<RadioParams>,
}

/// Does this look like a server's file system (an access list or a region table)?
pub fn is_server<F: Flash>(fs: &mut Spiffs<F>) -> Result<bool, Error> {
    for name in [ACL, REGIONS] {
        match fs.open(name) {
            Ok(_) => return Ok(true),
            Err(Error::NotFound) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(false)
}

fn string<const N: usize>(v: Value<'_>) -> Option<heapless::String<N>> {
    let Value::Str(raw) = v else { return None };
    let mut b = [0u8; 160];
    let n = unescape(raw, b.get_mut(..N)?);
    let mut s = heapless::String::new();
    for ch in core::str::from_utf8(b.get(..n)?).ok()?.chars() {
        s.push(ch).ok()?;
    }
    Some(s)
}

/// The repeater's own settings from `/prefs.json` over `p` and `r`; false without a readable
/// file (they are then left as they were).
pub fn settings<F: Flash>(fs: &mut Spiffs<F>, p: &mut ServerPrefs, r: &mut Routing) -> bool {
    let mut buf = [0u8; PREFS_MAX];
    let Ok(src) = fs.read_all(PREFS, &mut buf) else {
        return false;
    };
    let (mut np, mut nr) = (p.clone(), r.clone());
    let mut ok = true;
    let parsed = walk(src, |path, key, v| {
        let int = || match v {
            Value::Bare(b) => fixed(b, 0),
            Value::Str(_) => None,
        };
        let u8v = || int().and_then(|x| u8::try_from(x).ok());
        let section = path.first().copied().unwrap_or(b"");
        let mut set = || -> Option<()> {
            match (section, key) {
                (b"", b"pass") => np.admin_password = string(v)?,
                (b"", b"guest") => np.guest_password = string(v)?,
                (b"", b"owner") => np.owner_info = string(v)?,
                (b"", b"adv_int") => np.advert_interval = u8v()?,
                (b"", b"f_adv_int") => np.flood_advert_interval = u8v()?,
                (b"repeat", b"disable") => nr.disable_fwd = int()? != 0,
                (b"repeat", b"f_max") => nr.flood_max = u8v()?,
                (b"repeat", b"f_max_uns") => nr.flood_max_unscoped = u8v()?,
                (b"repeat", b"f_max_adv") => nr.flood_max_advert = u8v()?,
                (b"repeat", b"loop") => nr.loop_detect = u8v()?.min(3),
                (b"room", b"rd_only") => np.allow_read_only = int()? != 0,
                (b"power", b"pwr_sav_en") => np.powersaving = int()? != 0,
                _ => {}
            }
            Some(())
        };
        if set().is_none() {
            ok = false;
        }
    });
    if !parsed || !ok {
        return false;
    }
    *p = np;
    *r = nr;
    true
}

/// The access list's records (the shared secrets dropped), at most `MAX_CLIENTS`.
fn clients<F: Flash>(
    fs: &mut Spiffs<F>,
) -> Result<heapless::Vec<[u8; mc_server::persist::CLIENT_RECORD], CLIENTS_MAX>, Error> {
    let mut out = heapless::Vec::new();
    let f = match fs.open(ACL) {
        Ok(f) => f,
        Err(Error::NotFound) => return Ok(out),
        Err(e) => return Err(e),
    };
    let n = (f.size as usize / CPP_CLIENT_RECORD).min(CLIENTS_MAX);
    for i in 0..n {
        let mut rec = [0u8; CPP_CLIENT_RECORD];
        if fs.read_at(&f, (i * CPP_CLIENT_RECORD) as u32, &mut rec)? != CPP_CLIENT_RECORD {
            break;
        }
        if let Some((c, len, path)) = decode_client(&rec) {
            let _ = out.push(encode_client(&c, len, &path));
        }
    }
    Ok(out)
}

/// The region table, if there is a readable one.
fn regions<F: Flash>(fs: &mut Spiffs<F>) -> Result<Option<RegionMap>, Error> {
    let mut buf = [0u8; FILE_MAX];
    match fs.read_all(REGIONS, &mut buf) {
        Ok(data) => Ok(RegionMap::decode(data)),
        Err(Error::NotFound) => Ok(None),
        Err(e) => Err(e),
    }
}

/// What [`import`] would bring, without writing anything.
pub fn summary<F: Flash>(fs: &mut Spiffs<F>) -> Result<Summary, ImportError> {
    let id = identity(fs)?;
    let mut base = Prefs::defaults(id.pub_key());
    default_node_prefs(&mut base);
    let p = prefs(fs, &base);
    Ok(Summary {
        name: p.as_ref().map_or([0; 32], |p| p.name),
        key: *id.pub_key(),
        clients: clients(fs)?.len() as u8,
        regions: regions(fs)?.map_or(0, |m| m.len() as u8),
        radio: p.map(|p| p.radio),
    })
}

/// Writes the C++ repeater into `st`: identity, the node's and the repeater's settings, the
/// access list and the regions. From a C++ companion, only its identity and node settings.
/// Everything is read before anything is written. The runner restarts afterwards to load
/// them.
pub fn import<F: Flash>(
    fs: &mut Spiffs<F>,
    st: &mut impl ServerStore,
) -> Result<Summary, ImportError> {
    let id = identity(fs)?;
    let mut base = Prefs::defaults(id.pub_key());
    default_node_prefs(&mut base);
    let node = prefs(fs, &base);
    // From a companion only the identity and the node's settings: its `repeat.disable` (on by
    // default there) must not switch forwarding off on the repeater.
    let mut sp = ServerPrefs::default();
    let mut routing = Routing::REPEATER;
    if is_server(fs)? {
        settings(fs, &mut sp, &mut routing);
    }
    let acl = clients(fs)?;
    let map = regions(fs)?;

    st.save_identity(id.private_key(), id.pub_key());
    st.save_node_prefs(&encode_prefs(node.as_ref().unwrap_or(&base)));
    st.save_settings(&encode_settings(&sp, &routing));
    st.save_clients(&mut acl.iter().copied());
    let mut file = [0u8; FILE_MAX];
    if let Some(n) = map.as_ref().and_then(|m| m.encode(&mut file)) {
        st.save_regions(file.get(..n).unwrap_or(&[]));
    }
    Ok(Summary {
        name: node.as_ref().map_or(base.name, |p| p.name),
        key: *id.pub_key(),
        clients: acl.len() as u8,
        regions: map.map_or(0, |m| m.len() as u8),
        radio: node.map(|p| p.radio),
    })
}
