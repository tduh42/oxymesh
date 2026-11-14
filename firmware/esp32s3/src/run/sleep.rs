//! Light sleep for repeaters (`powersaving on`, `docs/spec/server.md` §4.7), like the C++
//! repeater on ESP32 (room servers keep the setting and stay awake, as in the C++) (`examples/simple_repeater/main.cpp` `loop`, `ESP32Board::sleep`,
//! MeshCore companion-v1.18.0, MIT):
//! - only with the setting on, from 2 minutes after boot (`POWERSAVING_FIRSTSLEEP_SECS`), and
//!   when nothing waits to be sent (`hasPendingWork`: the outbound queue);
//! - at most 30 s at a time; DIO1 high (a packet) ends it, and a packet already waiting
//!   skips it;
//! - our addition: a button or the console also wake the board, and keep it awake a minute, so
//!   it can be used (the C++ console loses what is typed while asleep, then sleeps again).
//!
//! esp-hal moves the system timer forward by the time slept, so embassy time stays right.

use embassy_time::{Duration, Instant};
use esp_hal::gpio::{Event, Input};
use esp_hal::rtc_cntl::sleep::{LowPower, RtcSleepConfig};
use esp_hal::rtc_cntl::{WakeupSource, wakeup_cause};

/// No sleep before this, after boot (`POWERSAVING_FIRSTSLEEP_SECS`).
pub const FIRST_SLEEP_MS: u64 = 120_000;
/// The longest sleep (`board.sleep(30)`).
pub const SLEEP_MAX: Duration = Duration::from_secs(30);
/// Awake this long after a button or the console.
pub const AWAKE_AFTER_USE_MS: u64 = 60_000;

pub struct Saver {
    awake_until: u64,
}

impl Default for Saver {
    fn default() -> Self {
        Self::new()
    }
}

impl Saver {
    pub const fn new() -> Self {
        Self { awake_until: 0 }
    }

    /// Someone used the board (a button, the console): awake for a minute.
    pub fn used(&mut self, now_ms: u64) {
        self.awake_until = now_ms + AWAKE_AFTER_USE_MS;
    }

    /// May the board sleep now? `on`: the setting; `pending`: something waits to be sent.
    pub fn may_sleep(&self, on: bool, pending: bool, now_ms: u64) -> bool {
        on && !pending && now_ms >= FIRST_SLEEP_MS && now_ms >= self.awake_until
    }

    /// Light sleep until `until` (at most [`SLEEP_MAX`]), DIO1 high, or one of `pins` at its
    /// level. Returns whether a person woke it (a pin, the console), not the radio or the time.
    pub fn sleep(
        &mut self,
        low: &mut LowPower<'_>,
        until: Instant,
        dio1: &mut Input<'_>,
        pins: &mut [(&mut Input<'_>, Event)],
    ) -> bool {
        let until = until.min(Instant::now() + SLEEP_MAX);
        let ahead = until.saturating_duration_since(Instant::now());
        // A packet already waiting, or too short a sleep to be worth it.
        if dio1.is_high() || ahead < Duration::from_millis(10) {
            return false;
        }
        dio1.listen(Event::HighLevel);
        for (p, ev) in pins.iter_mut() {
            p.listen(*ev);
        }
        low.set_wakeup_deadline(
            esp_hal::time::Instant::now() + esp_hal::time::Duration::from_micros(ahead.as_micros()),
        );
        low.sleep_light(RtcSleepConfig::default());
        low.clear_wakeup_deadline();
        let radio = dio1.is_high();
        dio1.unlisten();
        dio1.clear_interrupt();
        let mut person = false;
        for (p, _) in pins.iter_mut() {
            person |= p.is_low();
            p.unlisten();
            p.clear_interrupt();
        }
        let cause = wakeup_cause();
        let woke = !cause.is_empty() && !cause.contains(WakeupSource::Timer) && !radio;
        person || woke
    }
}
