//! Real-time clock (Unix seconds) on top of the runner's monotonic clock.

use mc_core::{Duration, Instant};

/// Current Unix time, derived from a sync point and the monotonic clock.
#[derive(Debug, Clone)]
pub struct Rtc {
    base_epoch: u32,
    base: Instant,
    last_unique: u32,
    /// Something gave the time since boot (a clock chip, an app, the GPS).
    set: bool,
}

impl Rtc {
    /// A clock counting from `epoch`, the runner's floor: good enough to stamp packets (they
    /// must not go back in time), not to show as the time of day.
    pub const fn new(epoch: u32, now: Instant) -> Self {
        Self {
            base_epoch: epoch,
            base: now,
            last_unique: 0,
            set: false,
        }
    }

    /// Whether something set the clock since boot. Until then the screens show no time.
    pub fn is_set(&self) -> bool {
        self.set
    }

    pub fn now(&self, now: Instant) -> u32 {
        let secs = now.saturating_duration_since(self.base).as_nanos() / 1_000_000_000;
        self.base_epoch
            .saturating_add(u32::try_from(secs).unwrap_or(u32::MAX))
    }

    pub fn set(&mut self, now: Instant, epoch: u32) {
        self.base_epoch = epoch;
        self.base = now;
        self.set = true;
    }

    /// Time strictly increasing from one call to the next (`getCurrentTimeUnique`, MeshCore.h:119).
    pub fn unique(&mut self, now: Instant) -> u32 {
        let t = self.now(now);
        if t <= self.last_unique {
            self.last_unique = self.last_unique.wrapping_add(1);
        } else {
            self.last_unique = t;
        }
        self.last_unique
    }
}

/// Milliseconds since the origin of the monotonic clock (for C++-style durations).
pub const fn ms(d: Duration) -> u64 {
    d.as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_counts_from_its_floor_and_knows_when_it_was_set() {
        let t0 = Instant::from_nanos(0);
        let mut rtc = Rtc::new(1_793_900_000, t0);
        assert!(!rtc.is_set());
        let later = t0 + Duration::from_secs(90);
        assert_eq!(rtc.now(later), 1_793_900_090);
        assert!(!rtc.is_set(), "counting is not being set");
        rtc.set(later, 1_800_000_000);
        assert!(rtc.is_set());
        assert_eq!(rtc.now(later + Duration::from_secs(5)), 1_800_000_005);
    }
}
