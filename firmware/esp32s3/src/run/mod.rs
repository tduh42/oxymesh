//! What every board's runner shares (ADR-0009): the radio side of the main loop, the link to
//! the app (USB framing and Bluetooth frames), the boot steps (partitions, identity, import from
//! the C++ firmware) and the Bluetooth host. A board brings its pins, its radio front end, its
//! screen and its buttons.

pub mod boot;
pub mod bt;
pub mod crash;
pub mod link;
pub mod radio;
pub mod server;
pub mod sleep;

/// The clock at boot, until something sets it: the time of the commit this firmware was built
/// from (`build.rs`). The node stamps its packets from here; the screens show no time until
/// the clock is set. No clock chip time before it can be right, so none is trusted: that is
/// how another firmware's stale time is told from a good one.
pub const BOOT_EPOCH: u32 = decimal(env!("OXYMESH_BUILD_EPOCH"));

/// The build date the device reports (`DD-Mon-YYYY`, UTC), from the same commit.
pub const BUILD_DATE: &str = env!("OXYMESH_BUILD_DATE");

const fn decimal(s: &str) -> u32 {
    let b = s.as_bytes();
    let mut v = 0u32;
    let mut i = 0;
    while i < b.len() {
        v = v * 10 + (b[i] - b'0') as u32;
        i += 1;
    }
    v
}

/// The earliest of the times something is due, for the main loop's sleep.
pub struct Deadline(embassy_time::Instant);

impl Deadline {
    /// At most `cap` from now.
    pub fn within(cap: embassy_time::Duration) -> Self {
        Self(embassy_time::Instant::now() + cap)
    }

    pub fn at(&mut self, t: embassy_time::Instant) {
        self.0 = self.0.min(t);
    }

    pub fn at_ms(&mut self, ms: u64) {
        self.at(embassy_time::Instant::from_millis(ms));
    }

    /// A time of the protocol core (`mc_now`'s clock).
    pub fn at_mc(&mut self, t: Option<mc_core::Instant>) {
        if let Some(t) = t {
            self.at(embassy_time::Instant::from_micros(t.as_nanos() / 1000));
        }
    }

    pub fn get(&self) -> embassy_time::Instant {
        self.0
    }
}

/// `mc-core`'s clock: embassy's, in nanoseconds.
pub fn mc_now() -> mc_core::Instant {
    mc_core::Instant::from_nanos(
        embassy_time::Instant::now()
            .as_micros()
            .saturating_mul(1000),
    )
}
