//! Mesh time (`docs/spec/companion.md` §9, 14): while nothing has set the clock, the time is
//! taken from the timestamps heard on the mesh, once [`QUORUM`] senders agree within
//! [`AGREE_S`]. Wrong clocks are common on a mesh (nodes counting from their own floor) and
//! scattered, so they rarely form a group; right clocks do.

use mc_core::Instant;

/// Samples kept, one per sender; the oldest is replaced.
pub const SLOTS: usize = 16;
/// Senders that must agree.
pub const QUORUM: usize = 3;
/// How close their estimates must be, in seconds.
pub const AGREE_S: u32 = 120;
/// A timestamp later than the floor by more than this is a broken clock.
pub const SPAN_S: u32 = 366 * 86_400;

#[derive(Debug, Clone, Copy)]
struct Sample {
    sender: u32,
    ts: u32,
    at: Instant,
}

impl Sample {
    /// What this sample says the time is at `now`.
    fn estimate(&self, now: Instant) -> u32 {
        let secs = now.saturating_duration_since(self.at).as_nanos() / 1_000_000_000;
        self.ts
            .saturating_add(u32::try_from(secs).unwrap_or(u32::MAX))
    }
}

/// The largest group of senders that agree, and the median of their estimates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Group {
    pub senders: usize,
    pub median: u32,
}

#[derive(Debug, Clone)]
pub struct MeshTime {
    samples: [Option<Sample>; SLOTS],
    next: usize,
}

impl Default for MeshTime {
    fn default() -> Self {
        Self::new()
    }
}

impl MeshTime {
    pub const fn new() -> Self {
        Self {
            samples: [None; SLOTS],
            next: 0,
        }
    }

    /// Notes a timestamp `ts` from `sender`, heard at `now`, with the clock's `floor` (no right
    /// time is older). Returns the time now once [`QUORUM`] senders agree.
    pub fn note(&mut self, sender: u32, ts: u32, now: Instant, floor: u32) -> Option<u32> {
        if ts < floor || ts > floor.saturating_add(SPAN_S) {
            return None;
        }
        let s = Sample {
            sender,
            ts,
            at: now,
        };
        let same = self
            .samples
            .iter()
            .position(|x| x.is_some_and(|x| x.sender == sender));
        let slot = match same {
            Some(i) => i,
            None => {
                let i = self.next % SLOTS;
                self.next = (i + 1) % SLOTS;
                i
            }
        };
        if let Some(x) = self.samples.get_mut(slot) {
            *x = Some(s);
        }
        self.group(now)
            .filter(|g| g.senders >= QUORUM)
            .map(|g| g.median)
    }

    /// Samples held.
    pub fn held(&self) -> usize {
        self.samples.iter().flatten().count()
    }

    /// The largest group of estimates within [`AGREE_S`] of each other at `now`.
    pub fn group(&self, now: Instant) -> Option<Group> {
        let mut est = [0u32; SLOTS];
        let mut n = 0;
        for s in self.samples.iter().flatten() {
            if let Some(e) = est.get_mut(n) {
                *e = s.estimate(now);
                n += 1;
            }
        }
        let est = est.get_mut(..n)?;
        est.sort_unstable();
        let mut best: Option<(usize, usize)> = None;
        let mut j = 0;
        for i in 0..est.len() {
            let lo = *est.get(i)?;
            j = j.max(i);
            while est
                .get(j + 1)
                .is_some_and(|&e| e.saturating_sub(lo) <= AGREE_S)
            {
                j += 1;
            }
            if best.is_none_or(|(a, b)| j - i > b - a) {
                best = Some((i, j));
            }
        }
        let (a, b) = best?;
        Some(Group {
            senders: b - a + 1,
            median: *est.get(a + (b - a) / 2)?,
        })
    }

    pub fn clear(&mut self) {
        *self = Self::new();
    }
}

/// A short tag for a sender: the first bytes of its key.
pub fn key_tag(key: &[u8]) -> u32 {
    let mut b = [0u8; 4];
    for (o, k) in b.iter_mut().zip(key) {
        *o = *k;
    }
    u32::from_le_bytes(b)
}

/// A tag for a channel message's sender: the channel and the name before ": " (FNV-1a), or
/// `None` without a name.
pub fn channel_tag(idx: u8, text: &[u8]) -> Option<u32> {
    let end = text.windows(2).position(|w| w == b": ")?;
    let name = text.get(..end)?;
    if name.is_empty() {
        return None;
    }
    let mut h: u32 = 0x811c_9dc5;
    for b in core::iter::once(&idx).chain(name) {
        h ^= u32::from(*b);
        h = h.wrapping_mul(0x0100_0193);
    }
    Some(h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_core::Duration;

    const FLOOR: u32 = 1_794_000_000;
    const TRUE: u32 = 1_794_100_000;

    fn at(s: u64) -> Instant {
        Instant::from_nanos(0) + Duration::from_secs(s)
    }

    #[test]
    fn three_agreeing_senders_give_the_time_two_do_not() {
        let mut m = MeshTime::new();
        assert_eq!(m.note(1, TRUE, at(0), FLOOR), None);
        assert_eq!(m.note(2, TRUE + 70, at(60), FLOOR), None, "two only");
        // 100 s later; estimates now: TRUE+100, TRUE+110, TRUE+95.
        assert_eq!(m.note(3, TRUE + 95, at(100), FLOOR), Some(TRUE + 100));
    }

    #[test]
    fn the_same_sender_counts_once() {
        let mut m = MeshTime::new();
        for i in 0..5 {
            assert_eq!(m.note(7, TRUE + i * 10, at(u64::from(i) * 10), FLOOR), None);
        }
        assert_eq!(m.held(), 1);
        assert_eq!(m.note(8, TRUE + 50, at(50), FLOOR), None);
        assert_eq!(m.note(9, TRUE + 50, at(50), FLOOR), Some(TRUE + 50));
    }

    #[test]
    fn scattered_wrong_clocks_and_old_times_are_ignored() {
        let mut m = MeshTime::new();
        // Before the floor: refused outright.
        assert_eq!(m.note(1, FLOOR - 1, at(0), FLOOR), None);
        // Far in the future: refused.
        assert_eq!(m.note(2, FLOOR + SPAN_S + 1, at(0), FLOOR), None);
        assert_eq!(m.held(), 0);
        // Wrong clocks, each on its own.
        assert_eq!(m.note(3, FLOOR + 5, at(0), FLOOR), None);
        assert_eq!(m.note(4, TRUE + 86_400, at(0), FLOOR), None);
        assert_eq!(m.note(5, TRUE - 3_600, at(0), FLOOR), None);
        // Right ones agree, the median wins.
        assert_eq!(m.note(6, TRUE - 30, at(0), FLOOR), None);
        assert_eq!(m.note(7, TRUE + 2, at(0), FLOOR), None);
        assert_eq!(m.note(8, TRUE + 40, at(0), FLOOR), Some(TRUE + 2));
        let g = m.group(at(0)).unwrap();
        assert_eq!(g.senders, 3);
    }

    #[test]
    fn two_groups_the_larger_wins() {
        let mut m = MeshTime::new();
        // Two nodes counting from the same wrong floor, booted together.
        assert_eq!(m.note(1, FLOOR + 10, at(0), FLOOR), None);
        assert_eq!(m.note(2, FLOOR + 12, at(0), FLOOR), None);
        assert_eq!(m.note(3, TRUE, at(0), FLOOR), None);
        assert_eq!(m.note(4, TRUE + 5, at(0), FLOOR), None);
        assert_eq!(m.note(5, TRUE + 9, at(0), FLOOR), Some(TRUE + 5));
    }

    #[test]
    fn slots_recycle_the_oldest() {
        let mut m = MeshTime::new();
        for i in 0..(SLOTS as u32 + 4) {
            // Each its own wrong clock, a day apart.
            let _ = m.note(100 + i, FLOOR + i * 86_400, at(0), FLOOR);
        }
        assert_eq!(m.held(), SLOTS);
    }

    #[test]
    fn sender_tags() {
        assert_eq!(key_tag(&[1, 2, 3, 4, 5]), u32::from_le_bytes([1, 2, 3, 4]));
        let a = channel_tag(0, b"Morgan: hello").unwrap();
        assert_eq!(channel_tag(0, b"Morgan: again"), Some(a));
        assert_ne!(channel_tag(1, b"Morgan: hello"), Some(a), "another channel");
        assert_eq!(channel_tag(0, b"no name here"), None);
        assert_eq!(channel_tag(0, b": empty"), None);
    }
}
