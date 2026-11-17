//! Device storage in the `mcdata` partition (ADR-0006), using `sequential-storage`
//! (power-loss tolerant). Implements the `Storage` trait of `mc-node`.
//!
//! Keys (u32): 1 identity (96 B), 2 prefs, 3 contact count, 4 Bluetooth bonds, `0x0001_0000 + i` contact i
//! (C++ `/contacts3` record), `0x0002_0000 + i` channel i, `0x0003_0000 + i` history slot i,
//! `0x1000_0000 | seq` offline queue entry, `0x8000_0000 | key prefix` stored advert (preceded
//! by the full key, checked on read). Deleted entries are empty values (tombstones).
//!
//! A server (`mc_server::persist`, `docs/spec/server.md` §5) adds: 5 its settings, 6 client
//! count, `0x0004_0000 + i` client i, 7 the `/regions2` header and the entry count,
//! `0x0005_0000 + i` region entry i (a whole file does not fit in one item). It shares 1 and 2.

use embassy_futures::block_on;
use embedded_storage_async::nor_flash::NorFlash;
use mc_node::history::HISTORY_RECORD;
use mc_node::persist::{
    CHANNEL_RECORD, CONTACT_RECORD, PREFS_RECORD, PREFS_RECORD_V1, decode_channel, decode_contact,
    decode_prefs,
};
use mc_node::region::{FILE_ENTRY, FILE_HEADER, FILE_MAX, MAX as REGIONS_MAX};
use mc_node::{Device, Node};
use mc_server::Server;
use mc_server::persist::{CLIENT_RECORD, SETTINGS_RECORD, ServerStore};
use sequential_storage::cache::{Cache, Uncached};
use sequential_storage::map::{MapConfig, MapStorage};
use trouble_host::prelude::{
    AddrKind, Address, BdAddr, BondInformation, Identity, IdentityResolvingKey, LongTermKey,
    SecurityLevel,
};

const KEY_IDENTITY: u32 = 1;
const KEY_PREFS: u32 = 2;
const KEY_COUNT: u32 = 3;
const KEY_BONDS: u32 = 4;
const KEY_SERVER: u32 = 5;
const KEY_CLIENT_COUNT: u32 = 6;
const KEY_REGIONS: u32 = 7;
const CLIENT_BASE: u32 = 0x0004_0000;
const REGION_BASE: u32 = 0x0005_0000;
const CONTACT_BASE: u32 = 0x0001_0000;
const CHANNEL_BASE: u32 = 0x0002_0000;
const HISTORY_BASE: u32 = 0x0003_0000;
const QUEUE_BASE: u32 = 0x1000_0000;
const QUEUE_MASK: u32 = 0x0FFF_FFFF;
const BLOB_BASE: u32 = 0x8000_0000;
const BUF: usize = 512;

/// CRC-32 (IEEE) of a record, so that only modified contacts are written.
fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                (c >> 1) ^ 0xEDB8_8320
            } else {
                c >> 1
            };
        }
    }
    !c
}

/// Paired phones kept; beyond that the oldest is forgotten.
pub const BONDS_MAX: usize = 8;
/// One bond record.
pub const BOND_LEN: usize = 42;

/// Bond record (from loranet): `kind ‖ addr[6] ‖ ltk[16] ‖ irk_present ‖ irk[16] ‖ level ‖
/// bonded`.
pub fn encode_bond(b: &BondInformation) -> [u8; BOND_LEN] {
    let mut out = [0u8; BOND_LEN];
    out[0] = b.identity.addr.kind.as_raw();
    for (d, s) in out[1..7].iter_mut().zip(b.identity.addr.addr.raw()) {
        *d = *s;
    }
    out[7..23].copy_from_slice(&b.ltk.0.to_le_bytes());
    if let Some(irk) = b.identity.irk {
        out[23] = 1;
        out[24..40].copy_from_slice(&irk.0.get().to_le_bytes());
    }
    out[40] = match b.security_level {
        SecurityLevel::NoEncryption => 0,
        SecurityLevel::Encrypted => 1,
        SecurityLevel::EncryptedAuthenticated => 2,
    };
    out[41] = u8::from(b.is_bonded);
    out
}

/// Reads a bond record back. `None` if a field is out of range.
pub fn decode_bond(b: &[u8; BOND_LEN]) -> Option<BondInformation> {
    let mut addr = [0u8; 6];
    addr.copy_from_slice(&b[1..7]);
    let mut ltk = [0u8; 16];
    ltk.copy_from_slice(&b[7..23]);
    let mut irk = [0u8; 16];
    irk.copy_from_slice(&b[24..40]);
    let irk = match b[23] {
        0 if irk == [0; 16] => None,
        1 => Some(IdentityResolvingKey(core::num::NonZeroU128::new(
            u128::from_le_bytes(irk),
        )?)),
        _ => return None,
    };
    let level = match b[40] {
        1 => SecurityLevel::Encrypted,
        2 => SecurityLevel::EncryptedAuthenticated,
        _ => return None,
    };
    if b[41] != 1 {
        return None;
    }
    let identity = Identity {
        addr: Address {
            kind: AddrKind::new(b[0]),
            addr: BdAddr::new(addr),
        },
        irk,
    };
    Some(BondInformation::new(
        identity,
        LongTermKey(u128::from_le_bytes(ltk)),
        level,
        true,
    ))
}

fn blob_key(key: &[u8; 32]) -> u32 {
    BLOB_BASE | (u32::from_le_bytes([key[0], key[1], key[2], key[3]]) >> 1)
}

/// The storage could not be read (as opposed to holding nothing).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unreadable;

pub struct FlashStore<S: NorFlash> {
    map: MapStorage<u32, S, Cache<Uncached, Uncached, Uncached, u32>>,
    buf: [u8; BUF],
    /// CRC of the written contacts, by index; 0 = absent.
    saved: [u32; mc_node::contacts::MAX_CONTACTS],
    saved_count: usize,
    len_kb: u32,
    pub errors: u32,
}

impl<S: NorFlash> FlashStore<S> {
    pub fn new(flash: S, len: u32) -> Option<Self> {
        let erase = S::ERASE_SIZE as u32;
        let config = MapConfig::try_new(0..(len - len % erase)).ok()?;
        Some(Self {
            map: MapStorage::new(flash, config, Cache::new_uncached()),
            buf: [0; BUF],
            saved: [0; mc_node::contacts::MAX_CONTACTS],
            saved_count: 0,
            len_kb: len / 1024,
            errors: 0,
        })
    }

    fn put(&mut self, key: u32, value: &[u8]) {
        if block_on(self.map.store_item(&mut self.buf, &key, &value)).is_err() {
            self.errors += 1;
        }
    }

    /// Deletes an entry by replacing it with an empty value (tombstone): `remove_item` would
    /// require a multiwrite flash. An empty entry is ignored on load.
    fn remove(&mut self, key: u32) {
        self.put(key, &[]);
    }

    fn get(&mut self, key: u32, out: &mut [u8]) -> Option<usize> {
        let v: Option<&[u8]> = block_on(self.map.fetch_item(&mut self.buf, &key)).ok()?;
        let v = v?;
        out.get_mut(..v.len())?.copy_from_slice(v);
        Some(v.len())
    }

    /// Stored identity: expanded private key (64) then public key (32). `Err` when the storage
    /// cannot be read, which must not be taken for a first boot (that would replace the key).
    pub fn identity(&mut self) -> Result<Option<[u8; 64]>, Unreadable> {
        let mut b = [0u8; 96];
        let v: Option<&[u8]> =
            block_on(self.map.fetch_item(&mut self.buf, &KEY_IDENTITY)).map_err(|_| Unreadable)?;
        let Some(v) = v else { return Ok(None) };
        if v.len() != 96 {
            return Err(Unreadable);
        }
        b.copy_from_slice(v);
        let mut prv = [0u8; 64];
        prv.copy_from_slice(&b[..64]);
        Ok(Some(prv))
    }

    /// Reloads prefs, contacts (in index order), channels, the screen's history and the offline
    /// queue, in one pass over the storage. Items come out oldest first, so the last version
    /// of a key wins, and a tombstone (an empty value) deletes: what `get` would return for
    /// each key, without walking the partition once per key (each walk took about 0.1-0.25 s;
    /// 130 contacts made a boot of 18 s, measured on the M5 on 2026-11-17).
    pub fn load(&mut self, dev: &mut Device) {
        use mc_node::contacts::{Channel, MAX_CHANNELS, MAX_CONTACTS};
        let mut prefs: Option<([u8; PREFS_RECORD], usize)> = None;
        let mut count = 0usize;
        // Contacts by their index in storage: the latest record of each, valid or not.
        let mut present = [false; MAX_CONTACTS];
        let mut crcs = [0u32; MAX_CONTACTS];
        dev.node.contacts.list.clear();
        let mut buf = [0u8; BUF];
        let Ok(mut it) = block_on(self.map.fetch_all_items(&mut buf)) else {
            self.errors += 1;
            return;
        };
        loop {
            let (key, value) = match block_on(it.next::<&[u8]>(&mut buf)) {
                Ok(Some(kv)) => kv,
                Ok(None) => break,
                Err(_) => {
                    self.errors += 1;
                    break;
                }
            };
            match key {
                KEY_PREFS => {
                    // Longer than a record: unreadable, as `get` would find it.
                    let mut b = [0u8; PREFS_RECORD];
                    prefs = match b.get_mut(..value.len()) {
                        Some(d) => {
                            d.copy_from_slice(value);
                            Some((b, value.len()))
                        }
                        None => None,
                    };
                }
                KEY_COUNT => {
                    count = match value {
                        [lo, hi] => usize::from(u16::from_le_bytes([*lo, *hi])).min(MAX_CONTACTS),
                        _ => 0,
                    };
                }
                k if (CONTACT_BASE..CONTACT_BASE + MAX_CONTACTS as u32).contains(&k) => {
                    let i = (k - CONTACT_BASE) as usize;
                    let c = (value.len() == CONTACT_RECORD)
                        .then(|| decode_contact(value))
                        .flatten();
                    let list = &mut dev.node.contacts.list;
                    match c {
                        Some(c) => {
                            // Placeholders up to `i`, replaced or dropped below.
                            while list.len() <= i {
                                if list.push(c.clone()).is_err() {
                                    break;
                                }
                            }
                            if let Some(slot) = list.get_mut(i) {
                                *slot = c;
                            }
                            present[i] = true;
                            crcs[i] = crc32(value);
                        }
                        None => present[i] = false,
                    }
                }
                k if (CHANNEL_BASE..CHANNEL_BASE + MAX_CHANNELS as u32).contains(&k) => {
                    let i = (k - CHANNEL_BASE) as usize;
                    let c = (value.len() == CHANNEL_RECORD)
                        .then(|| decode_channel(value))
                        .flatten();
                    if let Some(slot) = dev.node.channels.0.get_mut(i) {
                        *slot = c.unwrap_or(Channel::EMPTY);
                    }
                }
                k if (HISTORY_BASE..HISTORY_BASE + 0x1_0000).contains(&k) => {
                    if value.len() == HISTORY_RECORD {
                        dev.restore_history(value);
                    }
                }
                k if k & !QUEUE_MASK == QUEUE_BASE => {
                    if value.is_empty() {
                        dev.forget_queued(k & QUEUE_MASK);
                    } else {
                        dev.restore_queued(k & QUEUE_MASK, value);
                    }
                }
                _ => {}
            }
        }
        if let Some((b, n)) = prefs
            && n >= PREFS_RECORD_V1
            && let Some(p) = decode_prefs(&b[..n], &dev.node.prefs)
        {
            dev.node.prefs = p;
        }
        // Contacts below the stored count with a valid record, in index order; the CRC of what
        // is stored follows each to its place in the list.
        let mut kept = 0;
        let mut index = 0;
        dev.node.contacts.list.retain(|_| {
            let keep = index < count && present[index];
            if keep {
                self.saved[kept] = crcs[index];
                kept += 1;
            }
            index += 1;
            keep
        });
        self.saved_count = kept;
    }

    /// Stored bonds, most recent first. An unreadable list is forgotten.
    pub fn bonds(&mut self) -> heapless::Vec<BondInformation, BONDS_MAX> {
        let mut raw = [0u8; BONDS_MAX * BOND_LEN];
        let mut out = heapless::Vec::new();
        let Some(n) = self.get(KEY_BONDS, &mut raw) else {
            return out;
        };
        if n % BOND_LEN != 0 {
            return out;
        }
        for chunk in raw[..n].chunks_exact(BOND_LEN) {
            let mut rec = [0u8; BOND_LEN];
            rec.copy_from_slice(chunk);
            let Some(b) = decode_bond(&rec) else {
                return heapless::Vec::new();
            };
            let _ = out.push(b);
        }
        out
    }

    /// Stores a bond first in the list; a peer already known is replaced.
    pub fn add_bond(&mut self, bond: &BondInformation) {
        let old = self.bonds();
        let mut raw = [0u8; BONDS_MAX * BOND_LEN];
        raw[..BOND_LEN].copy_from_slice(&encode_bond(bond));
        let mut n = BOND_LEN;
        for b in old
            .iter()
            .filter(|b| b.identity.addr != bond.identity.addr)
            .take(BONDS_MAX - 1)
        {
            raw[n..n + BOND_LEN].copy_from_slice(&encode_bond(b));
            n += BOND_LEN;
        }
        self.put(KEY_BONDS, &raw[..n]);
    }

    /// Forgets every paired phone.
    pub fn clear_bonds(&mut self) {
        self.remove(KEY_BONDS);
    }

    /// Erases the whole partition (`FACTORY_RESET`).
    /// A server's records back into `node` and `r` (before `Server::start`).
    /// A server's settings, clients and regions, in one pass over the storage (as
    /// [`FlashStore::load`]): the last version of each key wins, as `get` would return it.
    pub fn load_server(&mut self, node: &mut Node, r: &mut Server) {
        const CLIENTS_MAX: usize = 32;
        let mut prefs: Option<([u8; PREFS_RECORD], usize)> = None;
        let mut settings: Option<[u8; SETTINGS_RECORD]> = None;
        let mut client_count = 0usize;
        let mut clients = [[0u8; CLIENT_RECORD]; CLIENTS_MAX];
        let mut client_ok = [false; CLIENTS_MAX];
        let mut head: Option<[u8; FILE_HEADER + 1]> = None;
        // The region entries in their place in the file, after its header.
        let mut file = alloc::vec![0u8; FILE_MAX];
        let mut entry_ok = [false; REGIONS_MAX];
        let mut buf = [0u8; BUF];
        let Ok(mut it) = block_on(self.map.fetch_all_items(&mut buf)) else {
            self.errors += 1;
            return;
        };
        loop {
            let (key, value) = match block_on(it.next::<&[u8]>(&mut buf)) {
                Ok(Some(kv)) => kv,
                Ok(None) => break,
                Err(_) => {
                    self.errors += 1;
                    break;
                }
            };
            match key {
                KEY_PREFS => {
                    // Longer than a record: unreadable, as `get` would find it.
                    let mut b = [0u8; PREFS_RECORD];
                    prefs = match b.get_mut(..value.len()) {
                        Some(d) => {
                            d.copy_from_slice(value);
                            Some((b, value.len()))
                        }
                        None => None,
                    };
                }
                KEY_SERVER => settings = value.try_into().ok(),
                KEY_CLIENT_COUNT => {
                    client_count = match value {
                        [n] => usize::from(*n),
                        _ => 0,
                    };
                }
                KEY_REGIONS => head = value.try_into().ok(),
                k if (CLIENT_BASE..CLIENT_BASE + CLIENTS_MAX as u32).contains(&k) => {
                    let i = (k - CLIENT_BASE) as usize;
                    match <[u8; CLIENT_RECORD]>::try_from(value) {
                        Ok(rec) => {
                            clients[i] = rec;
                            client_ok[i] = true;
                        }
                        Err(_) => client_ok[i] = false,
                    }
                }
                k if (REGION_BASE..REGION_BASE + REGIONS_MAX as u32).contains(&k) => {
                    let i = (k - REGION_BASE) as usize;
                    let at = FILE_HEADER + i * FILE_ENTRY;
                    match (value.len() == FILE_ENTRY, file.get_mut(at..at + FILE_ENTRY)) {
                        (true, Some(slot)) => {
                            slot.copy_from_slice(value);
                            entry_ok[i] = true;
                        }
                        _ => entry_ok[i] = false,
                    }
                }
                _ => {}
            }
        }
        if let Some((b, n)) = prefs
            && n >= PREFS_RECORD_V1
            && let Some(p) = decode_prefs(&b[..n], &node.prefs)
        {
            node.prefs = p;
        }
        // Clients below the stored count with a whole record, in order.
        let kept = (0..client_count.min(CLIENTS_MAX))
            .filter(|&i| client_ok[i])
            .map(|i| clients[i]);
        let kept: heapless::Vec<[u8; CLIENT_RECORD], CLIENTS_MAX> = kept.collect();
        // The regions file: its header, then the entries up to the first missing one.
        let mut len = 0;
        if let Some(h) = head {
            file[..FILE_HEADER].copy_from_slice(&h[..FILE_HEADER]);
            let n = usize::from(h[FILE_HEADER]).min(REGIONS_MAX);
            let whole = entry_ok.iter().take(n).take_while(|ok| **ok).count();
            len = FILE_HEADER + whole * FILE_ENTRY;
        }
        r.restore(
            node,
            settings.as_ref().map(|s| &s[..]),
            kept.iter().map(|c| &c[..]),
            (len > 0).then_some(&file[..len]),
        );
    }

    pub fn erase_all(&mut self) {
        if block_on(self.map.erase_all()).is_err() {
            self.errors += 1;
        }
        self.saved = [0; mc_node::contacts::MAX_CONTACTS];
        self.saved_count = 0;
    }
}

impl<S: NorFlash> mc_node::Storage for FlashStore<S> {
    fn save_identity(&mut self, prv: &[u8; 64], pub_key: &[u8; 32]) {
        let mut b = [0u8; 96];
        b[..64].copy_from_slice(prv);
        b[64..].copy_from_slice(pub_key);
        self.put(KEY_IDENTITY, &b);
    }

    fn save_prefs(&mut self, rec: &[u8; PREFS_RECORD]) {
        self.put(KEY_PREFS, rec);
    }

    fn save_contacts(&mut self, records: &mut dyn Iterator<Item = [u8; CONTACT_RECORD]>) {
        let mut n = 0usize;
        for rec in records {
            let crc = crc32(&rec);
            if self.saved.get(n) != Some(&crc) {
                self.put(CONTACT_BASE + n as u32, &rec);
                if let Some(s) = self.saved.get_mut(n) {
                    *s = crc;
                }
            }
            n += 1;
        }
        for i in n..self.saved_count {
            self.remove(CONTACT_BASE + i as u32);
            if let Some(s) = self.saved.get_mut(i) {
                *s = 0;
            }
        }
        if n != self.saved_count {
            self.put(KEY_COUNT, &(n as u16).to_le_bytes());
            self.saved_count = n;
        }
    }

    fn save_channel(&mut self, idx: u8, rec: &[u8; CHANNEL_RECORD]) {
        self.put(CHANNEL_BASE + u32::from(idx), rec);
    }

    fn put_blob(&mut self, key: &[u8; 32], raw: &[u8]) {
        let mut b = [0u8; 32 + 255];
        let n = 32 + raw.len().min(255);
        b[..32].copy_from_slice(key);
        b[32..n].copy_from_slice(&raw[..n - 32]);
        self.put(blob_key(key), &b[..n]);
    }

    fn get_blob(&mut self, key: &[u8; 32], out: &mut [u8]) -> Option<usize> {
        let mut b = [0u8; 32 + 255];
        let n = self.get(blob_key(key), &mut b)?;
        if n < 32 || &b[..32] != key {
            return None;
        }
        let raw = &b[32..n];
        out.get_mut(..raw.len())?.copy_from_slice(raw);
        Some(raw.len())
    }

    fn remove_blob(&mut self, key: &[u8; 32]) {
        self.remove(blob_key(key));
    }

    fn put_queued(&mut self, seq: u32, entry: &[u8]) {
        self.put(QUEUE_BASE | (seq & QUEUE_MASK), entry);
    }

    fn get_queued(&mut self, seq: u32, out: &mut [u8]) -> Option<usize> {
        self.get(QUEUE_BASE | (seq & QUEUE_MASK), out)
            .filter(|&n| n > 0)
    }

    fn remove_queued(&mut self, seq: u32) {
        self.remove(QUEUE_BASE | (seq & QUEUE_MASK));
    }

    fn save_history(&mut self, slot: u16, rec: &[u8; HISTORY_RECORD]) {
        self.put(HISTORY_BASE + u32::from(slot), rec);
    }

    fn usage_kb(&self) -> (u32, u32) {
        // Estimate: contacts and stored adverts.
        let used = (self.saved_count * (CONTACT_RECORD + 64 + 128)) as u32 / 1024;
        (used, self.len_kb)
    }
}

impl<S: NorFlash> ServerStore for FlashStore<S> {
    fn save_identity(&mut self, prv: &[u8; 64], pub_key: &[u8; 32]) {
        mc_node::Storage::save_identity(self, prv, pub_key);
    }

    fn save_node_prefs(&mut self, rec: &[u8; PREFS_RECORD]) {
        self.put(KEY_PREFS, rec);
    }

    fn save_settings(&mut self, rec: &[u8; SETTINGS_RECORD]) {
        self.put(KEY_SERVER, rec);
    }

    fn save_clients(&mut self, records: &mut dyn Iterator<Item = [u8; CLIENT_RECORD]>) {
        let mut cnt = [0u8; 1];
        let old = match self.get(KEY_CLIENT_COUNT, &mut cnt) {
            Some(1) => u32::from(cnt[0]),
            _ => 0,
        };
        let mut n = 0u32;
        for rec in records.take(32) {
            self.put(CLIENT_BASE + n, &rec);
            n += 1;
        }
        for i in n..old {
            self.remove(CLIENT_BASE + i);
        }
        self.put(KEY_CLIENT_COUNT, &[n as u8]);
    }

    fn save_regions(&mut self, file: &[u8]) {
        let (Some(header), Some(entries)) = (file.get(..FILE_HEADER), file.get(FILE_HEADER..))
        else {
            return;
        };
        let mut head = [0u8; FILE_HEADER + 1];
        let mut old = [0u8; FILE_HEADER + 1];
        let old_n = match self.get(KEY_REGIONS, &mut old) {
            Some(k) if k == FILE_HEADER + 1 => u32::from(old[FILE_HEADER]),
            _ => 0,
        };
        let mut n = 0u32;
        for e in entries.chunks_exact(FILE_ENTRY).take(REGIONS_MAX) {
            self.put(REGION_BASE + n, e);
            n += 1;
        }
        for i in n..old_n {
            self.remove(REGION_BASE + i);
        }
        head[..FILE_HEADER].copy_from_slice(header);
        head[FILE_HEADER] = n as u8;
        self.put(KEY_REGIONS, &head);
    }
}
