# ADR-0013 — A crash or a hang restarts the device, and the device says so

- Status: accepted
- Date: 2026-11-16

## Context

A review of the firmware (2026-11-16) found that nothing brought a failed device back:

- A panic ended in `esp-backtrace`'s handler, which prints and then loops forever with interrupts off.
- No watchdog ran: `esp_hal::init` switches them all off and the firmware enabled none.
- The radio was never checked. SPI errors were dropped, and the chip was reset only at boot.

On the M5 the e-paper keeps its last image, so a dead device looks alive and just stops receiving. A repeater on a roof stays dead until someone cuts its power. And nothing told the owner afterwards: reset reasons were kept by one binary and shown only in `debug-log` builds.

## Decision

- **A panic records its place, then restarts** (`run::crash`). Our own panic handler stores the file and line and counts the panic, then calls `software_reset()`. With `debug-log` it prints the message and the return addresses first. A release build prints nothing: its serial port carries the companion protocol.
- **The RTC watchdog restarts a loop that stopped turning.** 30 s (`crash::WATCHDOG_S`), fed once per pass of every main loop, from right after `esp_hal::init`.
  - It pauses by itself while the chip sleeps (`wdt_pause_in_slp`), so light sleep (ADR-0012, P2) needs nothing.
  - It is stopped before hibernation and around a full storage erase, which the flash chip's worst figures put at about a minute.
- **A record in RTC memory** (plain words in `rtc_fast`, persistent) keeps the reset reasons of the last four boots, the panic count, and the last panic's file and line. It survives every reset that leaves the power on.
- **The radio is supervised** (`mc_core::radio_health`, used by `run::radio`):
  - commands that fail 8 times in a row, or
  - the chip found outside receive mode at 3 checks in a row, 10 s apart, while it should be listening,
  - end in a hardware reset and a fresh setup, as at boot. The noise floor and the receive watch start over.
  - The second case first raises `ERR_EVENT_STARTRX_TIMEOUT` (bit 2 of the error flags), the flag the C++ raises after 8 s outside receive (`Dispatcher::loop`); the C++ stops there.
  - A chip that does not come back is left alone and tried again every 30 s. A radio that failed at boot gets its first attempt after 10 s.
  - The noise sampling burst stops at its first failed read: a silent chip no longer holds a pass for 64 timeouts.
- **The device says it.**
  - M5 companion, Settings → System → About: `Started` (power on, restart, woke up, after an error, watchdog, low voltage, USB reset), always. `Errors` (count, file and line of the last panic), `Radio` (resets, or "not answering") and `Storage` (failed writes) only when there is something to say.
  - Servers: `get pwrmgt.bootreason` answers the same words in place of "Not available".
  - `debug-log` builds print a `health:` line every 10 s.
- **Proving it on a device** (`debug-log` only): B held 15 s panics; A and B held 15 s stop the loop. A build with `OXYMESH_CRASH_TEST=panic` or `=hang` does the same by itself, once, 20 s after a power-on.

## Consequences

- A fault costs a restart of a few seconds instead of a dead device. Messages waiting for the app, contacts and history are in flash and survive it; what was heard in RAM (heard nodes, packet log, statistics) starts over.
- A fault that repeats at every boot becomes a restart loop. The record keeps its place, and `mc log` on a `debug-log` build shows it.
- Nothing changes on the air or for the app. The new error flag is the C++'s own.
- A pass that legitimately takes more than 30 s would now restart the device. None does today; anything new that long must feed the watchdog or stop it. Boot's storage steps (identity, import from the C++ at first boot, loading the store) run with the watchdog stopped: the first try on the M5 found a load of 18 s with 130 contacts, so a full contact table would have restarted the device before its first pass (now one pass over the storage, 270 ms, `docs/measurements.md` 2026-11-17).
- Rejected: `esp-backtrace`'s `halt-cores` or `custom-halt`. They cannot see the panic's location, and a halt is what we are leaving.
- Rejected: the timer-group watchdogs. They stop with the main clocks; the RTC watchdog also covers a hang with interrupts off.
