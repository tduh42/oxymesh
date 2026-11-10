//! Regions a server knows (`src/helpers/RegionMap.cpp`, MeshCore, MIT; `docs/spec/server.md`):
//! a tree of named regions under the wildcard `*`, each allowing flood or not. A scoped flood
//! (TRANSPORT_FLOOD) is forwarded only if its transport code matches a region allowing flood;
//! a plain flood only if the wildcard allows it.
//!
//! Keys: a `#name` region, or a plain name (an implicit hashtag), uses `SHA256("#" ‖ name)[..16]`;
//! a `$name` region is private and its keys live in a key store the C++ does not have yet, so
//! it never matches. The file format is the C++'s `/regions2`.

use core::fmt::Write;

use mc_air::crypto;

/// `MAX_REGION_ENTRIES`.
pub const MAX: usize = 32;
/// `REGION_DENY_FLOOD`.
pub const DENY_FLOOD: u8 = 0x01;
/// Bytes of a name, NUL included (`char name[31]`).
pub const NAME_LEN: usize = 31;
/// `/regions2`: 3 reserved, default id, home id, wildcard flags, next id.
pub const FILE_HEADER: usize = 10;
/// `/regions2` entry: id, parent, name[31], flags, 128 reserved.
pub const FILE_ENTRY: usize = 2 + 2 + NAME_LEN + 1 + 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    pub id: u16,
    pub parent: u16,
    pub flags: u8,
    pub name: heapless::String<{ NAME_LEN - 1 }>,
}

impl Region {
    fn bare_name(&self) -> &str {
        self.name.strip_prefix('#').unwrap_or(&self.name)
    }

    pub fn allows_flood(&self) -> bool {
        self.flags & DENY_FLOOD == 0
    }
}

#[derive(Debug, Clone)]
pub struct RegionMap {
    next_id: u16,
    home_id: u16,
    default_id: u16,
    pub wildcard_flags: u8,
    regions: heapless::Vec<Region, MAX>,
}

impl Default for RegionMap {
    fn default() -> Self {
        Self::new()
    }
}

/// `RegionMap::is_name_char`: letters, digits, `-`, `$`, `#`, and anything from `A` up
/// (accented letters).
pub fn is_name_char(c: u8) -> bool {
    c == b'-' || c == b'$' || c == b'#' || c.is_ascii_digit() || c >= b'A'
}

impl RegionMap {
    pub const fn new() -> Self {
        Self {
            next_id: 1,
            home_id: 0,
            default_id: 0,
            wildcard_flags: 0,
            regions: heapless::Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.regions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Region> {
        self.regions.iter()
    }

    /// The wildcard as a region (id 0, `*`).
    pub fn wildcard(&self) -> Region {
        let mut name = heapless::String::new();
        let _ = name.push('*');
        Region {
            id: 0,
            parent: 0,
            flags: self.wildcard_flags,
            name,
        }
    }

    /// `putRegion`: a new region (flood denied by default) or an existing one moved under
    /// `parent`. Its id, or `None` (bad name, full, or a region made its own parent).
    pub fn put(&mut self, name: &str, parent: u16, id: u16) -> Option<u16> {
        if !name.bytes().all(is_name_char) {
            return None;
        }
        if let Some(r) = self.find_by_name_mut(name) {
            if r.id == parent {
                return None;
            }
            r.parent = parent;
            return Some(r.id);
        }
        if id == 0 && self.regions.is_full() {
            return None;
        }
        let id = if id == 0 {
            let i = self.next_id;
            self.next_id = self.next_id.wrapping_add(1);
            i
        } else {
            id
        };
        let mut n = heapless::String::new();
        for c in name.chars() {
            if n.push(c).is_err() {
                break;
            }
        }
        self.regions
            .push(Region {
                id,
                parent,
                flags: DENY_FLOOD,
                name: n,
            })
            .ok()?;
        Some(id)
    }

    fn find_by_name_mut(&mut self, name: &str) -> Option<&mut Region> {
        let name = name.strip_prefix('#').unwrap_or(name);
        self.regions.iter_mut().find(|r| r.bare_name() == name)
    }

    /// `findByName`: `*` is the wildcard (id 0); a leading `#` is ignored.
    pub fn find_by_name(&self, name: &str) -> Option<u16> {
        if name == "*" {
            return Some(0);
        }
        let name = name.strip_prefix('#').unwrap_or(name);
        self.regions
            .iter()
            .find(|r| r.bare_name() == name)
            .map(|r| r.id)
    }

    /// `findByNamePrefix`: a whole name first, else the last region the prefix starts.
    pub fn find_by_name_prefix(&self, prefix: &str) -> Option<u16> {
        if prefix == "*" {
            return Some(0);
        }
        let prefix = prefix.strip_prefix('#').unwrap_or(prefix);
        let mut partial = None;
        for r in &self.regions {
            if r.bare_name() == prefix {
                return Some(r.id);
            }
            if r.bare_name().starts_with(prefix) {
                partial = Some(r.id);
            }
        }
        partial
    }

    pub fn get(&self, id: u16) -> Option<Region> {
        if id == 0 {
            return Some(self.wildcard());
        }
        self.regions.iter().find(|r| r.id == id).cloned()
    }

    pub fn get_mut(&mut self, id: u16) -> Option<&mut Region> {
        self.regions.iter_mut().find(|r| r.id == id)
    }

    /// Flags of a region, the wildcard included.
    pub fn set_flags(&mut self, id: u16, flags: u8) {
        if id == 0 {
            self.wildcard_flags = flags;
        } else if let Some(r) = self.get_mut(id) {
            r.flags = flags;
        }
    }

    pub fn home(&self) -> Option<Region> {
        self.get(self.home_id)
    }

    pub fn set_home(&mut self, id: u16) {
        self.home_id = id;
    }

    /// The default region (scope of this node's own floods), if one is set.
    pub fn default_region(&self) -> Option<Region> {
        (self.default_id != 0)
            .then(|| self.get(self.default_id))
            .flatten()
    }

    pub fn set_default(&mut self, id: u16) {
        self.default_id = id;
    }

    /// `removeRegion`: not the wildcard, not one with children.
    pub fn remove(&mut self, id: u16) -> bool {
        if id == 0 || self.regions.iter().any(|r| r.parent == id) {
            return false;
        }
        match self.regions.iter().position(|r| r.id == id) {
            Some(i) => {
                self.regions.remove(i);
                true
            }
            None => false,
        }
    }

    pub fn clear(&mut self) {
        self.regions.clear();
    }

    /// `resetFrom` (`RegionMap.h:52`): no regions, ids continuing from `src`'s. The home, the
    /// default and the wildcard's flags stay this map's own.
    pub fn reset_from(&mut self, src: &RegionMap) {
        self.regions.clear();
        self.next_id = src.next_id;
    }

    /// The transport key of a region (`getTransportKeysFor`): none for the wildcard or a
    /// private (`$`) region.
    pub fn key(r: &Region) -> Option<[u8; 16]> {
        if r.id == 0 || r.name.starts_with('$') {
            return None;
        }
        Some(crypto::region_key(&r.name))
    }

    /// `findMatch`: the region a scoped packet belongs to. Code 0 means the wildcard
    /// (`Some(0)`); otherwise the first region not denied by `mask` whose key gives the code.
    pub fn find_match(&self, code: u16, ptype: u8, payload: &[u8], mask: u8) -> Option<u16> {
        if code == 0 {
            return Some(0);
        }
        self.regions
            .iter()
            .filter(|r| r.flags & mask == 0)
            .find(|r| {
                Self::key(r).is_some_and(|k| crypto::transport_code(&k, ptype, payload) == code)
            })
            .map(|r| r.id)
    }

    /// The region tree as the CLI prints it (`exportTo`): one line per region, indented by
    /// depth, `^` on the home region, ` F` where flood is allowed.
    pub fn export(&self, out: &mut impl Write) {
        self.export_from(&self.wildcard(), 0, out);
    }

    fn export_from(&self, r: &Region, depth: usize, out: &mut impl Write) {
        for _ in 0..depth {
            let _ = out.write_char(' ');
        }
        let home = if r.id == self.home_id { "^" } else { "" };
        let f = if r.allows_flood() { " F" } else { "" };
        let _ = writeln!(out, "{}{home}{f}", r.bare_name());
        if depth > 8 {
            return;
        }
        for c in self
            .regions
            .iter()
            .filter(|c| c.parent == r.id && c.id != 0)
        {
            self.export_from(c, depth + 1, out);
        }
    }

    /// `exportNamesTo`: comma-separated names (wildcard first) whose flags match `mask`
    /// (`invert`: whose flags have it).
    pub fn export_names(&self, out: &mut impl Write, mask: u8, invert: bool) {
        let pick = |flags: u8| {
            if invert {
                flags & mask != 0
            } else {
                flags & mask == 0
            }
        };
        let mut first = true;
        let mut put = |out: &mut dyn Write, s: &str| {
            if !first {
                let _ = out.write_char(',');
            }
            first = false;
            let _ = out.write_str(s);
        };
        if pick(self.wildcard_flags) {
            put(out, "*");
        }
        for r in self.regions.iter().filter(|r| pick(r.flags)) {
            put(out, r.bare_name());
        }
    }

    /// `/regions2` header.
    pub fn encode_header(&self) -> [u8; FILE_HEADER] {
        let mut b = [0u8; FILE_HEADER];
        let mut w = mc_core::Writer::new(&mut b);
        let _ = (|| {
            w.put(&[0, 0, 0])?;
            w.put(&self.default_id.to_le_bytes())?;
            w.put(&self.home_id.to_le_bytes())?;
            w.put_u8(self.wildcard_flags)?;
            w.put(&self.next_id.to_le_bytes())
        })();
        b
    }

    /// `/regions2` entry.
    pub fn encode_entry(r: &Region) -> [u8; FILE_ENTRY] {
        let mut b = [0u8; FILE_ENTRY];
        let mut name = [0u8; NAME_LEN];
        for (o, c) in name.iter_mut().take(NAME_LEN - 1).zip(r.name.bytes()) {
            *o = c;
        }
        let mut w = mc_core::Writer::new(&mut b);
        let _ = (|| {
            w.put(&r.id.to_le_bytes())?;
            w.put(&r.parent.to_le_bytes())?;
            w.put(&name)?;
            w.put_u8(r.flags)
        })();
        b
    }

    /// Reads a `/regions2` file (`RegionMap::load`): `None` if the header is short; entries
    /// are read until the data ends or `MAX`, a partial entry ends the list.
    pub fn decode(file: &[u8]) -> Option<Self> {
        let mut r = mc_core::Reader::new(file);
        r.array::<3>().ok()?;
        let default_id = u16::from_le_bytes(*r.array::<2>().ok()?);
        let home_id = u16::from_le_bytes(*r.array::<2>().ok()?);
        let wildcard_flags = r.u8().ok()?;
        let next_id = u16::from_le_bytes(*r.array::<2>().ok()?);
        let mut m = Self {
            next_id,
            home_id,
            default_id,
            wildcard_flags,
            regions: heapless::Vec::new(),
        };
        while !m.regions.is_full() {
            let Ok(e) = r.array::<FILE_ENTRY>() else {
                break;
            };
            let mut er = mc_core::Reader::new(e);
            let (Ok(id), Ok(parent), Ok(raw), Ok(flags)) = (
                er.array::<2>(),
                er.array::<2>(),
                er.array::<NAME_LEN>(),
                er.u8(),
            ) else {
                break;
            };
            let id = u16::from_le_bytes(*id);
            let end = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
            let mut name = heapless::String::new();
            for c in core::str::from_utf8(raw.get(..end).unwrap_or(&[]))
                .unwrap_or("")
                .chars()
            {
                if name.push(c).is_err() {
                    break;
                }
            }
            let _ = m.regions.push(Region {
                id,
                parent: u16::from_le_bytes(*parent),
                flags,
                name,
            });
            if id >= m.next_id {
                m.next_id = id.wrapping_add(1);
            }
        }
        Some(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_and_matching() {
        let mut m = RegionMap::new();
        let fr = m.put("fr", 0, 0).unwrap();
        let alps = m.put("#alps", fr, 0).unwrap();
        assert_eq!((fr, alps), (1, 2));
        assert_eq!(m.put("bad name", 0, 0), None);
        assert_eq!(m.put("fr", fr, 0), None, "its own parent");
        assert_eq!(m.find_by_name("#fr"), Some(fr));
        assert_eq!(m.find_by_name("alps"), Some(alps));
        assert_eq!(m.find_by_name_prefix("al"), Some(alps));
        assert_eq!(m.find_by_name("*"), Some(0));
        // New regions deny flood: a code from their key matches nothing until allowed.
        let payload = [1, 2, 3];
        let code = crypto::transport_code(&crypto::region_key("fr"), 4, &payload);
        assert_eq!(m.find_match(code, 4, &payload, DENY_FLOOD), None);
        m.set_flags(fr, 0);
        assert_eq!(m.find_match(code, 4, &payload, DENY_FLOOD), Some(fr));
        assert_eq!(m.find_match(0, 4, &payload, DENY_FLOOD), Some(0));
        // Private regions never match.
        let p = m.put("$club", 0, 0).unwrap();
        m.set_flags(p, 0);
        assert!(RegionMap::key(&m.get(p).unwrap()).is_none());
        // Tree print and names.
        m.set_home(alps);
        let mut s: heapless::String<128> = heapless::String::new();
        m.export(&mut s);
        assert_eq!(s, "* F\n fr F\n  alps^\n $club F\n");
        let mut n: heapless::String<64> = heapless::String::new();
        m.export_names(&mut n, DENY_FLOOD, false);
        assert_eq!(n, "*,fr,$club");
        assert!(!m.remove(fr), "has a child");
        assert!(m.remove(alps) && m.remove(fr));
    }

    #[test]
    fn file_round_trip() {
        let mut m = RegionMap::new();
        let fr = m.put("fr", 0, 0).unwrap();
        m.put("#alps", fr, 0);
        m.set_default(fr);
        m.wildcard_flags = DENY_FLOOD;
        let mut f = m.encode_header().to_vec();
        for r in m.iter() {
            f.extend_from_slice(&RegionMap::encode_entry(r));
        }
        assert_eq!(f.len(), FILE_HEADER + 2 * FILE_ENTRY);
        let back = RegionMap::decode(&f).unwrap();
        assert_eq!(back.iter().count(), 2);
        assert_eq!(back.default_region().map(|r| r.id), Some(fr));
        assert_eq!(back.wildcard_flags, DENY_FLOOD);
        assert_eq!(back.find_by_name("alps"), Some(2));
        assert_eq!(back.next_id, 3);
        // A partial entry ends the list.
        assert_eq!(RegionMap::decode(&f[..FILE_HEADER + 100]).unwrap().len(), 0);
        assert!(RegionMap::decode(&f[..5]).is_none());
    }
}
