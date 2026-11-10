//! `RateLimiter` (`examples/simple_repeater/RateLimiter.h`): at most `max` answers in a window
//! of `secs` that starts with the first one.

#[derive(Debug, Clone, Copy)]
pub struct RateLimiter {
    start: u32,
    secs: u32,
    max: u16,
    count: u16,
}

impl RateLimiter {
    pub const fn new(max: u16, secs: u32) -> Self {
        Self {
            start: 0,
            secs,
            max,
            count: 0,
        }
    }

    /// `allow(now)`, `now` in seconds of our clock.
    pub fn allow(&mut self, now: u32) -> bool {
        if now < self.start.saturating_add(self.secs) {
            self.count = self.count.saturating_add(1);
            self.count <= self.max
        } else {
            self.start = now;
            self.count = 1;
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window() {
        let mut l = RateLimiter::new(4, 120);
        for _ in 0..4 {
            assert!(l.allow(1000));
        }
        assert!(!l.allow(1100));
        assert!(l.allow(1120), "a new window");
    }
}
