# ADR-0014 — Bluetooth off lets the M5 companion light-sleep

- Status: accepted
- Date: 2026-11-17

## Context

A companion never slept: the Bluetooth controller needs the chip awake, and `esp-radio` holds a wake lock for as long as it runs. P1 (ADR-0012) made the main loop idle 97 % of the time, but the chip stays powered between events. The repeater sleeps with `powersaving on` (P2) and wakes on a packet. A companion used from its own screen, without the phone app, could do the same if Bluetooth were off.

## Decision

- **A setting**, Settings → Bluetooth → Bluetooth (on / off), oxymesh's own (`UiPrefs::bluetooth_off`, byte 146 of the settings record; older records read as on). The app never sees it. Changing it asks for a hold, saves, and restarts: Bluetooth runs alongside the main loop for the whole run and is not stopped midway.
- **At boot**, the stored record is read before the controller would start (`FlashStore::bluetooth_off`, one walk of the storage). With Bluetooth off the controller is never started. The hardware RNG then takes its entropy from the ADC (`TrngSource`) while the identity and the seed are drawn, and the ADC goes back to the battery afterwards. With Bluetooth on, the boot is as before, except that the controller starts once the storage is open.
- **Light sleep on the M5**, as the repeater does (`run::sleep::Saver`): from 2 minutes after boot, for at most 30 s at a time. DIO1 (a packet) or a button wakes it, and a button keeps it awake a minute. It sleeps only when all of these hold:
  - Bluetooth is off;
  - the M5 is on battery (no USB power): USB gets a computer's bytes, and a UART woken from sleep loses the first ones;
  - the backlight is off;
  - nothing is being drawn, sent, played (a tune, 5 s), blinked or typed;
  - the GPS is off: its UART fills in 130 ms;
  - nothing on screen waits for a tick (`App::at_rest`: no toast, no running range test).
- A sleep ends by the next job at the latest: the node's next wake-up, the battery reading (10 s), Home's clock at the next minute, the unread blink. The noise floor rounds pause while asleep, as on the repeater.
- The Heltec V4 companion keeps Bluetooth on: it has no settings screen, and its USB-Serial-JTAG drops off the bus in light sleep.

## Consequences

- With Bluetooth off, no phone can connect; USB still works when plugged in, and then the device stays awake.
- What it saves is not measured yet: the battery runs of P1/P2 should compare Bluetooth on and off.
- A button press that wakes the M5 is the first press of the gesture: a very short click (shorter than the wake-up) can be lost; the press then only wakes it.
- The first packet that wakes it is received as usual: the SX1262 keeps listening and raises DIO1; the loop reads it on waking.
- Rejected: stopping Bluetooth without a restart. The host and the main loop are joined for the whole run; tearing the controller down and starting it again is a new path for a rare change.
- Rejected: light sleep on USB power. A computer's first bytes would be lost; and on USB, power is not short. Test builds can sleep anyway (`OXYMESH_SLEEP_ON_USB=1`) to watch it with `mc log`.
