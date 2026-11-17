//! Supervision of the radio chip: when its state can no longer be trusted, the runner resets it
//! and sets it up again, instead of staying deaf until a reboot (ADR-0013).
//!
//! Two signs count:
//! - commands that fail in a row (a BUSY line that stays high, a dead SPI bus);
//! - the chip found outside receive mode at a periodic check, when it should be listening. The
//!   C++ firmware watches the same thing and only raises `ERR_EVENT_STARTRX_TIMEOUT` after 8 s
//!   (`Dispatcher::loop`, MeshCore companion-v1.18.0, MIT); it never recovers.
//!
//! A chip that does not come back is left alone until the next attempt: a broken radio then costs
//! one attempt every [`RETRY_MS`], not a stall on every pass of the main loop.

/// The chip's mode is read this often, ms.
pub const CHECK_MS: u64 = 10_000;
/// Failed commands in a row before a recovery.
pub const MAX_ERRORS: u8 = 8;
/// Checks in a row outside receive mode before a recovery.
pub const MAX_BAD_CHECKS: u8 = 3;
/// After a recovery that failed, the next attempt, ms.
pub const RETRY_MS: u64 = 30_000;

/// What the supervisor counted, for the screens and the logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Report {
    /// Recoveries that brought the chip back.
    pub recoveries: u32,
    /// Recoveries that did not.
    pub failures: u32,
    /// The chip is out of use until the next attempt.
    pub down: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RadioHealth {
    errors: u8,
    bad_checks: u8,
    next_check_ms: u64,
    retry_at_ms: Option<u64>,
    recoveries: u32,
    failures: u32,
}

impl Default for RadioHealth {
    fn default() -> Self {
        Self::new()
    }
}

impl RadioHealth {
    pub const fn new() -> Self {
        Self {
            errors: 0,
            bad_checks: 0,
            next_check_ms: CHECK_MS,
            retry_at_ms: None,
            recoveries: 0,
            failures: 0,
        }
    }

    /// The chip is out of use (it failed at boot, or a recovery failed): the runner asks nothing
    /// of it until [`RadioHealth::recover_due`].
    pub fn down(&self) -> bool {
        self.retry_at_ms.is_some()
    }

    /// The chip failed to start: out of use until the first attempt.
    pub fn failed_at_boot(&mut self, now_ms: u64) {
        self.failures = self.failures.saturating_add(1);
        self.retry_at_ms = Some(now_ms.saturating_add(CHECK_MS));
    }

    /// A command's outcome.
    pub fn command(&mut self, ok: bool) {
        if ok {
            self.errors = 0;
        } else {
            self.errors = self.errors.saturating_add(1);
        }
    }

    /// Whether to read the chip's mode now.
    pub fn check_due(&self, now_ms: u64) -> bool {
        !self.down() && now_ms >= self.next_check_ms
    }

    /// The chip's mode was read while it should be listening: `receiving` is `None` when the
    /// read itself failed (which [`RadioHealth::command`] counts). Returns true the moment the
    /// chip has been outside receive mode for [`MAX_BAD_CHECKS`] checks.
    pub fn checked(&mut self, now_ms: u64, receiving: Option<bool>) -> bool {
        self.next_check_ms = now_ms.saturating_add(CHECK_MS);
        match receiving {
            Some(true) => self.bad_checks = 0,
            Some(false) => self.bad_checks = self.bad_checks.saturating_add(1),
            None => {}
        }
        self.bad_checks == MAX_BAD_CHECKS
    }

    /// Whether the chip must be reset and set up again now.
    pub fn recover_due(&self, now_ms: u64) -> bool {
        match self.retry_at_ms {
            Some(t) => now_ms >= t,
            None => self.errors >= MAX_ERRORS || self.bad_checks >= MAX_BAD_CHECKS,
        }
    }

    /// The outcome of a reset and setup.
    pub fn recovered(&mut self, now_ms: u64, ok: bool) {
        self.errors = 0;
        self.bad_checks = 0;
        self.next_check_ms = now_ms.saturating_add(CHECK_MS);
        if ok {
            self.recoveries = self.recoveries.saturating_add(1);
            self.retry_at_ms = None;
        } else {
            self.failures = self.failures.saturating_add(1);
            self.retry_at_ms = Some(now_ms.saturating_add(RETRY_MS));
        }
    }

    pub fn report(&self) -> Report {
        Report {
            recoveries: self.recoveries,
            failures: self.failures,
            down: self.down(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_healthy_chip_is_left_alone() {
        let mut h = RadioHealth::new();
        assert!(!h.check_due(CHECK_MS - 1));
        assert!(h.check_due(CHECK_MS));
        for i in 1..100u64 {
            h.command(true);
            assert!(!h.checked(i * CHECK_MS, Some(true)));
            assert!(!h.recover_due(i * CHECK_MS));
        }
        assert_eq!(h.report(), Report::default());
    }

    #[test]
    fn errors_in_a_row_ask_for_a_recovery_and_a_success_forgets_them() {
        let mut h = RadioHealth::new();
        for _ in 0..MAX_ERRORS - 1 {
            h.command(false);
        }
        assert!(!h.recover_due(0));
        h.command(true);
        for _ in 0..MAX_ERRORS - 1 {
            h.command(false);
        }
        assert!(!h.recover_due(0));
        h.command(false);
        assert!(h.recover_due(0));
        h.recovered(500, true);
        assert!(!h.recover_due(500));
        assert!(!h.down());
        assert_eq!(h.report().recoveries, 1);
        // The next check is a full period after the recovery.
        assert!(!h.check_due(500 + CHECK_MS - 1));
        assert!(h.check_due(500 + CHECK_MS));
    }

    #[test]
    fn three_checks_outside_receive_ask_for_a_recovery() {
        let mut h = RadioHealth::new();
        assert!(!h.checked(10_000, Some(false)));
        assert!(!h.checked(20_000, Some(false)));
        // One good check in between starts the count again.
        assert!(!h.checked(30_000, Some(true)));
        assert!(!h.checked(40_000, Some(false)));
        assert!(!h.checked(50_000, Some(false)));
        assert!(!h.recover_due(50_000));
        // A failed read does not count either way.
        assert!(!h.checked(60_000, None));
        assert!(!h.recover_due(60_000));
        assert!(h.checked(70_000, Some(false)));
        assert!(h.recover_due(70_000));
    }

    #[test]
    fn a_chip_that_stays_dead_is_tried_again_every_retry_period() {
        let mut h = RadioHealth::new();
        for _ in 0..MAX_ERRORS {
            h.command(false);
        }
        h.recovered(1_000, false);
        assert!(h.down());
        // Out of use: no checks, and errors no longer bring the attempt forward.
        assert!(!h.check_due(1_000 + CHECK_MS));
        for _ in 0..50 {
            h.command(false);
        }
        assert!(!h.recover_due(1_000 + RETRY_MS - 1));
        assert!(h.recover_due(1_000 + RETRY_MS));
        h.recovered(1_000 + RETRY_MS, false);
        assert!(!h.recover_due(1_000 + 2 * RETRY_MS - 1));
        assert!(h.recover_due(1_000 + 2 * RETRY_MS));
        h.recovered(1_000 + 2 * RETRY_MS, true);
        assert!(!h.down());
        assert_eq!(
            h.report(),
            Report {
                recoveries: 1,
                failures: 2,
                down: false
            }
        );
    }

    #[test]
    fn a_chip_that_failed_at_boot_gets_a_first_attempt_soon() {
        let mut h = RadioHealth::new();
        h.failed_at_boot(200);
        assert!(h.down());
        assert!(!h.recover_due(200 + CHECK_MS - 1));
        assert!(h.recover_due(200 + CHECK_MS));
    }
}
