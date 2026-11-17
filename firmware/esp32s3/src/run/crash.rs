//! What ends a run, and getting out of it (ADR-0013).
//!
//! - **A panic** records where it happened, then restarts the chip. The stock handler stops
//!   forever, and an e-paper panel keeps showing its last image as if nothing were wrong.
//! - **The RTC watchdog** restarts a main loop that stopped turning ([`Watchdog`]).
//! - **A record in RTC memory** keeps the reset reasons of the last four boots, the number of
//!   panics and the last one's place, across every reset that leaves the power on.

use core::sync::atomic::{AtomicBool, Ordering};

use esp_hal::peripherals::RTC_TIMER;
use esp_hal::rtc_cntl::{Rtc, Rwdt, RwdtStage};

/// The main loop may stay silent this long before the chip is restarted. A pass lasts
/// milliseconds; the slowest things it does (a full e-paper refresh, a storage scan) are seconds.
pub const WATCHDOG_S: u64 = 30;

const MAGIC: u32 = 0x0C2A_5B02;
/// Bytes kept of a panic's file path (its end).
const FILE_MAX: usize = 40;
const WORDS: usize = 5 + FILE_MAX / 4;

const W_MAGIC: usize = 0;
/// Reset reasons of the last four boots, oldest in the high byte.
const W_BOOTS: usize = 1;
const W_PANICS: usize = 2;
const W_LINE: usize = 3;
/// Bit 0: a panic happened and no boot has seen it yet; bit 1: this boot followed a panic;
/// bits 8-15: length of the file path kept.
const W_FLAGS: usize = 4;
const W_FILE: usize = 5;

const PENDING: u32 = 1 << 0;
const AFTER_PANIC: u32 = 1 << 1;

/// Plain words, valid whatever the memory holds at power-on (`Persistable`).
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut RECORD: [u32; WORDS] = [0; WORDS];

fn load() -> [u32; WORDS] {
    // SAFETY: plain words, read whole; written only at boot and by the panic handler, which
    // never returns.
    let mut r = unsafe { (&raw const RECORD).read() };
    if r[W_MAGIC] != MAGIC {
        r = [0; WORDS];
        r[W_MAGIC] = MAGIC;
    }
    r
}

fn store(r: [u32; WORDS]) {
    // SAFETY: see `load`.
    unsafe { (&raw mut RECORD).write(r) }
}

/// At boot, before anything else: notes why the chip started.
pub fn note() {
    let reason = esp_hal::system::reset_reason().map_or(0, |r| r as u32) & 0xFF;
    let mut r = load();
    r[W_BOOTS] = (r[W_BOOTS] << 8) | reason;
    if r[W_FLAGS] & PENDING != 0 {
        r[W_FLAGS] = (r[W_FLAGS] & !PENDING) | AFTER_PANIC;
    } else {
        r[W_FLAGS] &= !AFTER_PANIC;
    }
    store(r);
}

/// Reset reasons of the last four boots, oldest first (`SocResetReason`; 0: none yet).
pub fn boots() -> [u8; 4] {
    load()[W_BOOTS].to_be_bytes()
}

/// Why the device last started, in words for the screen and the console.
pub fn restart() -> &'static str {
    let r = load();
    if r[W_FLAGS] & AFTER_PANIC != 0 {
        return "after an error";
    }
    // `SocResetReason` (esp-hal, `rtc_cntl/rtc/esp32s3.rs`).
    match r[W_BOOTS] & 0xFF {
        0x01 => "power on",
        0x03 | 0x0C => "restart",
        0x05 => "woke up",
        0x07..=0x09 | 0x0B | 0x0D | 0x10..=0x12 => "watchdog",
        0x0F => "low voltage",
        0x13 | 0x17 => "power glitch",
        0x15 | 0x16 => "USB reset",
        _ => "unknown",
    }
}

/// The last panic since the record began (a power-on clears it).
#[derive(Debug, Clone, Copy)]
pub struct Panic {
    /// How many there were.
    pub count: u32,
    pub line: u32,
    file: [u8; FILE_MAX],
    file_len: u8,
}

impl Panic {
    /// The end of the source file's path.
    pub fn file(&self) -> &str {
        let b = self.file.get(..usize::from(self.file_len)).unwrap_or(&[]);
        core::str::from_utf8(b).unwrap_or("?")
    }
}

pub fn last_panic() -> Option<Panic> {
    let r = load();
    if r[W_PANICS] == 0 {
        return None;
    }
    let mut file = [0u8; FILE_MAX];
    for (i, c) in file.chunks_exact_mut(4).enumerate() {
        c.copy_from_slice(&r.get(W_FILE + i).copied().unwrap_or(0).to_le_bytes());
    }
    Some(Panic {
        count: r[W_PANICS],
        line: r[W_LINE],
        file,
        file_len: (((r[W_FLAGS] >> 8) & 0xFF) as u8).min(FILE_MAX as u8),
    })
}

fn record_panic(path: &str, line: u32) {
    let mut r = load();
    r[W_PANICS] = r[W_PANICS].saturating_add(1);
    r[W_LINE] = line;
    // The end of the path says more than its start; cut on a character boundary.
    let mut from = path.len().saturating_sub(FILE_MAX);
    while !path.is_char_boundary(from) {
        from += 1;
    }
    let tail = path.as_bytes().get(from..).unwrap_or(&[]);
    let mut file = [0u8; FILE_MAX];
    if let Some(dst) = file.get_mut(..tail.len()) {
        dst.copy_from_slice(tail);
    }
    for (i, c) in file.chunks_exact(4).enumerate() {
        if let Some(w) = r.get_mut(W_FILE + i) {
            *w = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
        }
    }
    r[W_FLAGS] = (r[W_FLAGS] & 0xFF) | PENDING | ((tail.len() as u32) << 8);
    store(r);
}

static PANICKING: AtomicBool = AtomicBool::new(false);

/// Records the place, then restarts. With `debug-log`, the message and the return addresses go
/// out first (in a release build UART0 or the USB port carries the companion protocol).
#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    // A panic while handling a panic: straight to the restart.
    if PANICKING.swap(true, Ordering::SeqCst) {
        esp_hal::system::software_reset();
    }
    let (file, line) = info.location().map_or(("?", 0), |l| (l.file(), l.line()));
    record_panic(file, line);
    #[cfg(feature = "debug-log")]
    {
        esp_println::println!("\n\n!! {info}");
        for f in esp_backtrace::Backtrace::capture().frames() {
            esp_println::println!("   {:#x}", f.program_counter());
        }
        // Let the last bytes leave the UART before the reset empties it.
        esp_hal::delay::Delay::new().delay_millis(50);
    }
    esp_hal::system::software_reset()
}

/// The RTC watchdog: restarts the chip when [`Watchdog::feed`] is not called for
/// [`WATCHDOG_S`]. It counts on the RTC's slow clock, whatever the processor does, and pauses
/// by itself while the chip sleeps (`wdt_pause_in_slp`), so light sleep needs nothing.
pub struct Watchdog(Rwdt);

impl Watchdog {
    pub fn start(rtc_timer: RTC_TIMER<'static>) -> Self {
        let mut w = Rtc::new(rtc_timer).rwdt;
        w.set_timeout(
            RwdtStage::Stage0,
            esp_hal::time::Duration::from_secs(WATCHDOG_S),
        );
        w.enable();
        w.feed();
        Self(w)
    }

    /// The loop is alive.
    pub fn feed(&mut self) {
        self.0.feed();
    }

    /// Before something long: erasing the storage (up to a minute by the flash chip's worst
    /// figures), or hibernation, which waits for a button to be released.
    pub fn stop(&mut self) {
        self.0.disable();
    }

    /// After [`Watchdog::stop`], when the run goes on.
    pub fn resume(&mut self) {
        self.0.enable();
        self.0.feed();
    }
}
