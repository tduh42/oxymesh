# ADR-0012 — Main loops sleep until something happens

- Status: accepted
- Date: 2026-11-14

## Context

Every runner (ADR-0009) turned its main loop once per millisecond: it polled the radio over SPI, the USB link, the buttons, the GPS and the screen, then slept 1 ms. On the M5 companion, a `debug-log` build measured about 650 passes a second, each costing about 0.36 ms with nothing to do. The loop kept the processor busy 24 % of the time, idle. That cost runs on battery around the clock, on companions and servers alike. Light sleep (P2) also needs a loop that knows how long it may sleep.

## Decision

- **A pass, then a wait.** Each loop ends by computing when it next has something to do. It sleeps until then, or until one of these wakes it:
  - the SX1262's DIO1 rising (RX done, CRC or header error, TX done, timeout);
  - a button edge;
  - bytes from the computer (an async read of UART0 or the USB-Serial-JTAG, kept for the next pass);
  - Bluetooth (`ble::Shared::wake`, signalled with every frame, state change and bond).
- **Deadlines** (`run::Deadline`): the earliest of
  - the core's (`Device::next_wakeup`, `Node::next_wakeup`, `Server::next_wakeup`);
  - the radio's (`Radio::next_deadline`: the next noise round, a transmission's give-up time; soon while DIO1 is already high, since a latched flag hides the next rising edge);
  - the battery reading (10 s);
  - the interface ticks, the gesture recognizer's timers, the backlight, the unread blink.
- **Ceilings.** The M5 companion sleeps at most 250 ms (its interface tick); servers and the Heltec companion 1 s. The core's periodic work outside its next wakeup (adverts, AGC, app message give-up) never waits longer than that.
- **Polled closer while needed:**
  - every 10 ms for 60 ms after a button edge: the recognizer drops edges closer than 30 ms as bounces, and must see the level again;
  - every 20 ms while the e-paper is busy and a frame waits;
  - every 50 ms while the GPS is on: UART1 at 9600 baud fills its 128-byte FIFO in about 130 ms.
- **The noise floor** takes a round's 64 samples in one burst (about 2 ms) instead of one per pass, every 2 s as in the C++.

## Consequences

- Measured on the M5 companion (`docs/measurements.md`): 20 passes a second with the GPS on, the processor busy 2-3 % instead of 24 %. USB round trips are unchanged (10/13/15 ms).
- Everything a pass does happens later by up to its wait. The radio, the buttons and the links wake it at once; only periodic work moves, and by less than the ceiling.
- A source that neither wakes the loop nor has a deadline is only seen at the ceiling. Anything new that needs a prompt answer needs a wake or a deadline.
- Rejected: shorter fixed sleeps (5-10 ms). They save less, and UART0 at 115,200 baud fills its FIFO in 11 ms.
- Rejected: one task per source. The core is a single state machine; one loop keeps it that way.
