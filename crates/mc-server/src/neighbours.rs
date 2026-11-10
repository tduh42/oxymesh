//! Neighbouring repeaters (`examples/simple_repeater/MyMesh.cpp:63-88`, MeshCore, MIT):
//! repeaters heard zero-hop (adverts, discovery answers), 50 kept, the least recently heard
//! replaced.

use mc_air::crypto::PUB_KEY_LEN;

/// `MAX_NEIGHBOURS` as the repeater builds use it.
pub const MAX: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Neighbour {
    pub key: [u8; PUB_KEY_LEN],
    /// Their advert's timestamp.
    pub advert_ts: u32,
    /// When we heard it, our clock.
    pub heard: u32,
    /// SNR × 4.
    pub snr4: i8,
}

#[derive(Debug, Clone)]
pub struct Neighbours {
    slots: [Option<Neighbour>; MAX],
}

impl Default for Neighbours {
    fn default() -> Self {
        Self::new()
    }
}

impl Neighbours {
    pub const fn new() -> Self {
        Self { slots: [None; MAX] }
    }

    /// `putNeighbour`: the neighbour's slot, or the least recently heard one (an empty slot
    /// counts as heard at 0).
    pub fn put(&mut self, key: &[u8; PUB_KEY_LEN], advert_ts: u32, heard: u32, snr4: i8) {
        let mut pick = 0;
        let mut oldest = u32::MAX;
        for (i, s) in self.slots.iter().enumerate() {
            let h = match s {
                Some(n) if n.key == *key => {
                    pick = i;
                    break;
                }
                Some(n) => n.heard,
                None => 0,
            };
            if h < oldest {
                pick = i;
                oldest = h;
            }
        }
        if let Some(s) = self.slots.get_mut(pick) {
            *s = Some(Neighbour {
                key: *key,
                advert_ts,
                heard,
                snr4,
            });
        }
    }

    /// `removeNeighbor`: those whose key starts with `prefix`.
    pub fn remove(&mut self, prefix: &[u8]) {
        for s in &mut self.slots {
            if s.is_some_and(|n| n.key.starts_with(prefix)) {
                *s = None;
            }
        }
    }

    /// The known neighbours (slot order).
    pub fn iter(&self) -> impl Iterator<Item = &Neighbour> {
        self.slots.iter().flatten().filter(|n| n.heard > 0)
    }

    /// Sorted for a NEIGHBOURS request: 0 newest first, 1 oldest first, 2 strongest first,
    /// 3 weakest first, otherwise slot order.
    pub fn sorted(&self, order: u8) -> heapless::Vec<Neighbour, MAX> {
        let mut v: heapless::Vec<Neighbour, MAX> = self.iter().copied().collect();
        match order {
            0 => v.sort_unstable_by_key(|n| core::cmp::Reverse(n.heard)),
            1 => v.sort_unstable_by_key(|n| n.heard),
            2 => v.sort_unstable_by_key(|n| core::cmp::Reverse(n.snr4)),
            3 => v.sort_unstable_by_key(|n| n.snr4),
            _ => {}
        }
        v
    }
}
