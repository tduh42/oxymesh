//! Ctrl-C for the commands that change a device's settings while they run (`mc soak`,
//! `mc duo`): the first one asks them to wind down, so they put the settings back; a second
//! one, while the first is still being honoured, ends the process as usual.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

static STOP: OnceLock<Arc<AtomicBool>> = OnceLock::new();

/// From now on Ctrl-C sets a flag instead of ending the process. Other commands never call
/// this and stop at once, as before.
pub fn install() {
    let flag = STOP.get_or_init(|| Arc::new(AtomicBool::new(false)));
    // Registered first, so it runs first: with the flag already up, the default action.
    let _ = signal_hook::flag::register_conditional_default(
        signal_hook::consts::SIGINT,
        Arc::clone(flag),
    );
    let _ = signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(flag));
}

/// Ctrl-C was pressed.
pub fn requested() -> bool {
    STOP.get().is_some_and(|f| f.load(Ordering::Relaxed))
}

/// The wind-down starts: the commands that put things back must be allowed to run.
pub fn clear() {
    if let Some(f) = STOP.get() {
        f.store(false, Ordering::Relaxed);
    }
}

/// Sleeps `secs`, less if Ctrl-C comes.
pub fn sleep(secs: u64) {
    let end = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    while std::time::Instant::now() < end && !requested() {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
