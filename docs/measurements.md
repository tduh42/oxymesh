# Hardware measurements and tests

Log of tests on the real devices. Each entry: date, setup, command, result.

## 2026-11-14 — repeater light sleep on the M5 (P2)

- Setup: `rpt-m5` over the companion store, `powersaving on` (answer `on - After 2 minutes`), the C++ Heltec as the other radio, `mc term` on the CH340 port (it does not reset the M5).
- Past 2 minutes of uptime, three zero-hop adverts from the Heltec 8 s apart all counted (`stats-packets` direct_rx 3): DIO1 wakes it.
- The console asleep: the first line (`x`) vanished, taken by the UART wake; the next commands answered, and the board stayed awake a minute.
- Time across sleeps: uptime moved 245 s while the Mac's clock moved 240-244 s over four mostly-asleep minutes; the node's clock read 22:23 then 22:27. esp-hal moves the system timer forward by the time slept, and embassy follows.
- Not measured yet: the current saved. Not run: the Heltec build (it runs the C++ reference). Afterwards `powersaving off` and the companion back (key 2E97BC45).

## 2026-11-14 — main loops that sleep (P1, ADR-0012)

- Setup: `mc-m5` with `debug-log`, which now logs every 10 s the main loop's passes and busy time, and keeps the battery voltage every 5 minutes in RAM (printed every minute, for runs on battery). On USB, screen idle on Home, GPS on (indoors, no fix), a phone connected now and then.

| M5 companion | Passes per 10 s | Busy |
|---|---|---|
| Polling every 1 ms (before) | 6,500-6,600 | 22-24 % (29 % with the noise burst) |
| Sleeping until something happens | 203-205 | 2.0-3.4 % |

- What a pass cost when polled (µs): radio 81, core tick 58, interface bookkeeping 50, GPS 31-45, buttons and events 35-39, Bluetooth and actions 31, drawing check 7-15, USB link 9. Nothing to optimise one by one: the cost was paying it 650 times a second.
- With the GPS off, the M5 companion's ceiling is its 250 ms interface tick: about 4 passes a second.
- USB round trips unchanged (`mc latency`: 10 / 13 / 15 ms).
- Radio: 12 minutes of evening listening heard nothing, with the new loop and with the old one (a quiet network, not the loop). Then three zero-hop adverts from the C++ Heltec all reached the M5, and two from the M5 reached the Heltec. `rpt-m5`: its console answers at once, it received two adverts from the Heltec; the M5 was given its companion back (key 2E97BC45).
- Not run on hardware: the Heltec builds (the Heltec runs the C++ reference).
- `mc stats`, `mc info` and the other commands that open the M5's port reset it (the CH340's control lines); `mc log` does not.

## 2026-11-14 — pixel screens on the M5 (U10), clock chip

- Setup: `mc-m5` with `debug-log`, which now logs each frame's draw time, overlay paint time and heap. Read with `mc log`, the screens moved by hand.
- Image: 1,533,120 B, 84 % of the app partition (72 % before U9: the Helvetica sizes, Spleen digits and Open Iconic).

| Per frame | Text screen | Pixel screen (Home) |
|---|---|---|
| First build: Ratatui draw + scene | 9-30 ms | 9 ms |
| First build: overlays painted | 10-15 ms | 13.4 ms |
| After the fixes below: draw | 8.5 ms | 5.8 ms |
| After the fixes below: overlays | 1.7 ms | 6.1 ms |

- Three fixes:
  - text is measured in one pass (glyph advances add up; a test checks it), where `wrap` and `ellipsize` re-measured the whole string per character;
  - on the panel, a screen with a scene no longer also draws its hidden text version;
  - the panel frame fills rectangles a byte at a time (`fill_solid`). Every overlay blanks its rectangle first, and filled shapes arrive as rows.
- A frame now costs 10-12 ms before the 542 ms partial refresh, on par with U0's 6 ms grid demo plus the header and footer.
- Heap peak 57,336 of 98,300 B (61,004 B in the first build); stack peak 30,784 of 61,156 B.
- **Clock chip**: the M5 showed 2026-11-05 17:33, the firmware's floor. The PCF8563 had its VL flag set and counted from 2000-01-01, so it had lost power about 1 h 40 min before, with the battery at 4.1 V. The firmware rightly ignores such a time. `mc time` (new: SET_DEVICE_TIME only, nothing on air) set the clock, the M5 wrote it back, and VL cleared. The cause of the power loss is not known; the M5 has no separate cell for the chip. The `debug-log` status line now shows the chip's state.
- **Typing (U11)**: a long A that inserts (a new prediction over the ~4,000 words) takes about 9 ms on the M5, a click 0.17 ms; heap peak 72,952 B while typing, with a phone connected. The first try on the device found that presses held past 1 s moved on instead of validating; a held A now validates once in the keyboard.
- **GPS**: indoors, 0 satellites for several minutes after boot, with sentences flowing (about 340 B/s, no errors), as on 2026-11-08. No fix has been seen on this M5 yet: it needs a test with sky in view.

## 2026-11-10 — the repeater and the room server on the M5 (S1d, S2)

- Setup: the ThinkNode M5 flashed with `rpt-m5`, then `room-m5`, over its oxymesh companion store, then given its companion firmware back. `mc term` on the CH340 port.
- Images: `rpt-m5` 534,496 B (29 % of the app partition).
- **Repeater:**
  - the identity (2E97BC45) and node settings came from the companion's store;
  - the time came from the PCF8563;
  - the console answered `ver`, `get role`, `clock`, `get radio`, `neighbors`, `stats-*` as specified;
  - the boot advert went out zero hop after 16 s (`direct_tx` 1);
  - noise floor −115 dBm, battery 4,076 mV.
- **Room server:**
  - `get role` answered `> room_server` and `get guest.password` `> hello`;
  - `room.post` stored a post, and `set repeat on` was refused.
  - The first build said `get repeat` > on: the store's restore had given a fresh room server the repeater's routing (fixed in 7f44b3b, then checked: > off).
- After the companion firmware went back, `mc info` showed the same key, name and radio. The servers had written nothing to the store.
- **Over the air, the same evening**, from the Heltec running MeshCore C++ v1.18 (companion, 869.618 MHz, `mc remote`, boards side by side):
  - **Repeater:** the admin login answered with permissions 3 and firmware level 2. The status decoded (battery 4,075 mV, noise −111 dBm, SNR 12.25 dB). Telemetry gave 4.07 V and 39.0 °C (LPP 116 and 103).
  - **Room server:**
    - the room password logged in read-write at firmware level 1, and the 52-byte status decoded;
    - a post from the Heltec was acknowledged in 955 ms (`mc send`);
    - a post from the room's console was pushed to the Heltec once, and its ACK ended the push: one push in 40 s, status `posts 2, pushes 1`.
  - A second login with the password, and later requests, were refused as replays. The Heltec's clock (no clock chip, reset earlier) was 6,216 s behind the Mac's time, which `mc send` had put in the post. The C++ room server does the same. A blank password (a known client) logs in without that check. Setting the Heltec's clock let requests through.
- **Not done yet:** the console over the air from the Heltec. Its replies come through the companion's message queue, which held the owner's real messages. Also not done: a look at the e-paper pages.

## 2026-11-10 — the Heltec V4 on oxymesh (B2)

- Setup: Heltec V4.3 (KCT8103L front end) that ran MeshCore C++ v1.18; its whole flash read twice into a temp folder outside the repository (identical, it holds the private key) to put the C++ back afterwards; partition table read from it (the C++ 16 MB layout, as `partitions-heltec-v4.csv` assumed). `scripts/flash.sh heltec-v4`: image 947,552 bytes.
- **First boot**: the C++ node imported (same key and name, 869.618 MHz / 62.5 kHz / SF8 / CR 4/8, TX 10 dBm, its contacts and channels); front end found as the KCT8103L ("Heltec V4.3 OLED", like the C++); console `radio.fem.rxgain` answers.
- **LNA**: noise floor −79 dBm with the LNA on at boot, −104 to −102 dBm with it off (about the 21 dB it adds). Found and fixed: a new LNA setting only took effect after the next transmission. Switching it back on leaves the floor low until an AGC reset or a restart, as in the C++ sampler (it only takes samples near the current floor).
- **`mc duo` M5 ↔ Heltec, both oxymesh**: 2 runs, 12/12 each; DM round trips 1078 to 1240 ms, pings 1023 to 1032 ms.
- **Screen and button** (checked by hand): status pages, click / double click / long press as specified; Vext is active low.
- Battery: 4395 mV read with the starting multiplier (no battery fitted; to compare with the C++'s reading).

## 2026-11-09/10 — radio housekeeping, console, board layer, GPS rework on the M5 (R1, R2, B1, G2)

- Setup: ThinkNode M5 (board v1.0) on USB, firmware after B1/G2 (`scripts/flash.sh m5`); Heltec V4.3 running MeshCore C++ v1.18 on USB as the other end of `mc duo`. Region fr (869.618 MHz, SF8, 62.5 kHz).
- **Flashing over the previous oxymesh**: identity, radio settings and the clock chip's time kept; the settings record written by the previous firmware reads back with the new fields at their C++ defaults (AGC reset off, boosted RX gain on).
- **Console** (`mc cli`): 22 commands, every reply in the C++ format (`> 869.6179809,62.5,8,5`, `> 0.1999999` for the direct TX delay, `ab|> name`, `Unknown command` for `clock` and `radio.fem.rxgain` on the M5, `Error, bad chars`). `poweroff` gets no reply and hibernates the M5; the top button wakes it.
- **USB latency** (`mc latency --count 150`): 10 / 12 / 12 ms (min / median / max), none over 100 ms.
- **AGC reset every 8 s, `int.thresh` 10, CAD on**: adverts go out, no CAD-timeout flag; noise floor steady at −111/−112 dBm over 7 minutes; `mc duo` with the Heltec: adverts, DMs both ways, channel datagrams and pings received (12/13 and 8/13, see below).
- **GPS** (`debug-log` build): the L76K sends about 560 bytes and 2 sentences per second, no read errors (no fix indoors).
- **`mc duo` M5 ↔ Heltec**: 26 runs. Early in the session, 4 of the first 5 runs (AGC/CAD/threshold on and off alike) went wrong around a DM from the Heltec: twice its delivery acknowledgement was lost, twice the host's serial port broke ("Broken pipe"). The M5 had restarted after two of them (uptime); in one the port broke while the M5 kept running, so the USB bridge itself went away. Not seen again in the 21 runs since, including 6 right after a wake from hibernation and 8 with a phone app connected (those 8 fail on cross-talk only: with a phone connected the device's frames also reach USB, deviation 10, so `mc duo` reads the app's replies). Open: the cause. Firmware builds now keep the reset reasons of their last four boots in RTC memory (`boots` on the `debug-log` status line, read with `mc log`, which does not reset the device); a full chip reset (flashing, `espflash monitor`) clears them.
- RAM and size: image 1,333,392 bytes of 1,835,008 (72.7 %).

## 2026-11-05 — `mc airtest`: mc-air against MeshCore C++ v1.18 (MC1)

- Setup: Heltec V4 running `companion_radio_usb` v1.18.0 (reference, `/dev/cu.usbmodem141101`); ThinkNode M5 running `companion_radio_usb` v1.18.0-81dbae5 (device under test, `/dev/cu.wchusbserial120`). Region fr: 869.618 MHz, 62.5 kHz, SF8, CR 4/8.
- Command: `mc airtest --ref /dev/cu.usbmodem141101 --dut /dev/cu.wchusbserial120`.
- Everything zero-hop (direct route, empty path): nothing is forwarded on the network.
- Result: **10 checks out of 10** on the first try.

| Check | Result |
|---|---|
| Advert signed by mc-air (orlp test identity) accepted by the C++ | contact added (ADVERT) |
| DM encrypted by mc-air, read by the C++ | identical text; `path_len` 0xFF (direct) |
| C++ ACK equal to the CRC computed by mc-air | identical, 6 bytes |
| `SEND_TXT_MSG` on the C++ sent direct, not flood | `SENT` with flood = 0 |
| DM from the C++, decrypted by mc-air | identical text |
| ACK CRC computed by mc-air equal to the one reported by the C++ in `SENT` | identical |
| ACK built by mc-air recognized by the C++ | `SEND_CONFIRMED`, round trip 824 ms |
| Advert from the C++ verified by mc-air | signature ok, identical name |
| Channel datagram from the C++ (zero-hop) decrypted by mc-air | identical |
| Channel message from mc-air read by the C++ | identical |

Other findings:
- The M5 reports 869618 kHz in `SELF_INFO`: deviation 2 as predicted by the study (869617) does not occur in the usual bands (ADR-0001 corrected).
- `mc sniff` on the M5 for 2 min (22:15): no packet heard, the network was quiet at that time.

## 2026-11-08 — clock chip, noise, temperature, GPS, C++ import (H1, H2, MC6)

- Setup: ThinkNode M5 on `/dev/cu.wchusbserial210`, `debug-log` build (status line every 10 s on UART0, uptime first: the boot lines can be lost on the CH340, and the line at 5 s comes before anything has arrived). A phone connected over Bluetooth. Indoors.
- **C++ SPIFFS of the M5** read with `espflash read-flash --block-size 0x400 --max-in-flight 1 0x290000 0x160000` (the default block size dropped bytes on the CH340), kept outside the repository, never committed. `mc spiffs`: 8 files (identity, `/prefs.json`, `/contacts3`, `/channels2`, 4 stored adverts); identity checked (the private key gives the public key); settings parsed (869.618 MHz, 62.5 kHz, SF8, CR 8, 22 dBm); 5 contacts, 1 channel. The ESP-IDF layout constants hold on a real image. Not imported yet: it would replace the M5's current identity.
- **Clock chip**: `mc advert` set the clock (SET_DEVICE_TIME); after a reset the M5 started at that time (1794168574 at boot against 1794168576 on the Mac when read) instead of 2026-11-05. Power-off not checked yet.
- **Noise floor**: −111 to −115 dBm once the first round has ended (0 before, like the C++).
- **Chip temperature**: 42.0 to 49.9 °C over the session (default range DAC); not compared with the Heltec yet.
- **GPS**: board v1.0 (IO7 low 10 times out of 10), switch on (GPIO10 high). The L76K sends about 325 bytes/s; 18 RMC/GGA sentences per 10 s parsed, no read error; 0 satellites and no fix indoors. A first round of tests read "0 B" only because every look was at the first status line; `gps-probe` (a small binary: UART1 RX 19 at 9600 receives about 340 bytes/s, GPIO19 toggles) ruled out the wiring. The pins swapped (RX 20) receive nothing.
- RAM (`debug-log`, GPS off at the time): stack peak 41,224 of 67,460 B; heap peak 54,044 of 98,300 B. Flash: `mc-m5` 1,313,664 of 1,835,008 B (72 %).

## 2026-11-06 — `mc airtest` with oxymesh as the device under test (MC4)

- Setup: Heltec V4 running `companion_radio_usb` v1.18.0 (reference, `/dev/cu.usbmodem2141101`); ThinkNode M5 running **oxymesh** `mc-m5` over USB (device under test, `/dev/cu.wchusbserial2120`). Same radio settings as above.
- Command: `mc airtest --ref /dev/cu.usbmodem2141101 --dut /dev/cu.wchusbserial2120`.
- In this run, "the C++ firmware" in the check names is oxymesh: the test identity talks to the device under test, the Heltec only listens and injects raw packets.
- Result: **10 checks out of 10**, the same checks as the C++ run above. ACK round trip 692 ms (824 ms with the C++ firmware on the M5).

## 2026-11-06 — `mc duo`: oxymesh (M5) and C++ (Heltec) talk to each other (MC4)

- Setup: A = ThinkNode M5 running oxymesh `mc-m5` (`/dev/cu.wchusbserial210`); B = Heltec V4 running `companion_radio_usb` v1.18.0 (`/dev/cu.usbmodem1101`). Region fr. No injected frames: each device builds its own packets from companion commands. Everything zero-hop.
- Command: `mc duo --a /dev/cu.wchusbserial210 --b /dev/cu.usbmodem1101 --pings 10`.
- Result: **12 checks out of 12**.

| Check | A → B | B → A |
|---|---|---|
| Zero-hop advert reported (`ADVERT`) | after 2.5 s | after 1.2 s |
| DM sent direct (`SENT`) | ok | ok |
| DM received (`MSG_WAITING`, `SYNC_NEXT_MESSAGE`) | identical text, `path_len` 0xFF, SNR 12.0 dB | identical text, `path_len` 0xFF, SNR 13.25 dB |
| Delivery confirmed (`SEND_CONFIRMED`) | round trip 1125 ms | round trip 1257 ms |
| Channel datagram, zero hop (`CHANNEL_DATA_RECV`) | `path_len` 0xFF, SNR 13.0 dB | `path_len` 0xFF, SNR 12.25 dB |
| Telemetry requests as pings | 10/10, RTT 1035 / 1040 / 1047 ms | 10/10, RTT 1008 / 1015 / 1025 ms |

Findings:
- **Bug, fixed:** oxymesh reported a zero-hop channel message or datagram with `path_len` 0x00. The C++ firmware reports 0xFF for any direct packet (M:572, 617). companion.md §5 now says so, and an mc-sim test covers it.
- **Test artifact:** GRP_DATA has no nonce. The same data on the same channel within one second gives byte-identical packets, and the receiver drops the second one as already seen (the C++ firmware does the same). `mc duo` puts the sender in the data.
- **Open:** telemetry pings that start right after `SET_OTHER_PARAMS` (which `mc duo` uses to allow telemetry) lose their second exchange.
  - Without a pause, in 6 runs, A → B lost ping 2 in 5 and B → A in 2; the other pings were all answered.
  - The lost packet is always the one oxymesh sends and the C++ firmware never sees: A counts it as sent, while B's receive counter and `LOG_RX_DATA` miss it, with no CRC error.
  - Zero-hop datagrams sent every 600 ms during the 4 s after a prefs save, on either device, all arrived (48/48), so it is not simply "deaf after a flash write".
  - With 100 to 300 ms before each ping: 0 lost in 2 runs. With 3 s between the prefs change and the first ping: 0 lost in 5 runs (100 pings). `mc duo` now waits 3 s.
  - Not reproduced between two oxymesh nodes in mc-sim. Cause unknown; next step is a run with the C++ firmware on both ends.

## 2026-11-06 — Bluetooth (MC5)

- M5 `/dev/cu.wchusbserial120` running oxymesh with Bluetooth, advertised as `MeshCore-2E97BC45`; Heltec (C++) `/dev/cu.usbmodem14301`; the Mac as the central (`mc`, btleplug).
- **First pairing** (`mc info ble:MeshCore-2E97BC45`, the PIN shown on the M5's panel typed into macOS's dialog):
  - passkey entry, the Mac typing and the M5 displaying;
  - EncryptedAuthenticated, bond stored in `mcdata`;
  - ATT MTU 247, so a 176-byte frame fits one notification.
- `mc duo --a ble:MeshCore-2E97BC45 --b /dev/cu.usbmodem14301 --pings 3 --reboot`: **13/13**.
  - DM delivery 1.08–1.24 s; ping RTT 1.14 s (M5 → Heltec) and 1.02 s.
  - After the reboot, the Mac reconnects in 2.0 s with the bond read back from flash, without a PIN.
  - The same over USB with this build: 13/13.
- `mc latency`: over Bluetooth, 35 / 49 / 85 ms (min / median / max); over USB, 10 / 12 / 37 ms.
- **RAM** (`debug-log` build: stack painted at boot, heap peak counted by esp-alloc):

| | Size | Idle | Peak seen |
|---|---|---|---|
| Heap (24 KB in main DRAM + 72 KB in `dram2`) | 98,300 B | 52,564 B | 53,968 B (a connection adds nothing: the controller allocates at boot) |
| Main stack | 68,180 B | 28,736 B | 37,112 B pairing (P-256), 39,888 B after the duo runs |

  - Bluetooth's share of the heap is ≈ 31 KB.
  - 15 minutes idle with Bluetooth on: the heap did not move.
- **First build: the stack overflowed at boot.** It was 27,300 B; the stack guard caught it. The fix moved 40 KB of the UI heap to `dram2`.
- **Controller trims** (`esp_radio::ble::Config`):
  - `max_connections(1)`: advertising fails ("Memory Capacity Exceeded");
  - `max_connections(2)` saves 3.3 KB;
  - a scan duplicate list of 10 (we never scan) saves 1.1 KB.
- **Flash:** image 1,269,568 B of 1,835,008 (69 %). Bluetooth adds ≈ 300 KB of code and 27 KB of IRAM.
- **Interface heap on the host** (`crates/mc-ui/tests/heap.rs`): worst screen is compose on the compact grid, 62.7 KB on the host. The host is 64-bit, ≈ 1.45× the device, so ≈ 43 KB on the M5.
- **One connection only.** An app that reconnects at once (seen here: a phone paired with the M5, running the companion startup sequence: APP_START, DEVICE_QUERY, default scope, contacts) holds it, and the M5 stops advertising, like the C++. `mc` then cannot get in. `mc` reaches a companion it knows by its CoreBluetooth identifier, cached in `~/Library/Caches/oxymesh/ble-peripherals`.

## 2026-11-06 — message bursts while the M5 redraws (U6)

- `mc burst --a <receiver> --b <sender> --count N --gap MS`: the sender's companion sends N direct messages MS apart without waiting for them, then counts its delivery confirmations; the receiver's waiting messages are fetched last. Each message the M5 receives redraws its screen (partial refresh: 542 ms, see U0).
- Radio: 869.618 MHz, SF8, 62.5 kHz; the Heltec at CR 4/8, the M5 at CR 4/5. A 38-byte DM takes 321 to 443 ms on air, its direct ACK 157 to 181 ms.
- Ports: M5 `/dev/cu.wchusbserial120`, Heltec `/dev/cu.usbmodem14301`.

| Sender → receiver | Gap | Sent | Confirmed to the sender | Received |
|---|---|---|---|---|
| Heltec → M5 | 0.7 s | 30 | 0 | 15 (every odd one missing) |
| M5 → Heltec | 0.7 s | 12 | 0 | 6 (every odd one missing) |
| Heltec → M5 | 1.5 s | 20 | 20 | 20 |
| M5 → Heltec | 1.5 s (twice) | 20 | 20 | 16 queued (see below) |
| Heltec → M5 | 2.5 s | 12 | 12 | 12 |

- **At 0.7 s the loss is the same when the C++ firmware receives.** The ACK for message *n* goes out while message *n + 1* arrives, and both radios are half-duplex. The receiver misses *n + 1*, and the sender, busy transmitting, misses the ACK. That is the air at this spacing, not the screen.
- **At 1.5 s every message reached the M5 and was acknowledged, each one redrawn: no packet lost during a refresh.**
- The 4 messages missing on the Heltec were acknowledged, so they were received. The C++ USB build then dropped them because it keeps 16 waiting messages (`OFFLINE_QUEUE_SIZE` defaults to 16, `MyMesh.h:61-62`; the BLE and Wi-Fi builds, and every M5 build, set 256). Same result on a second run. Not a radio loss.
- `mc latency /dev/cu.wchusbserial120 --count 150` measures USB command round trips (GET_DEVICE_TIME). It ran while the Heltec sent the M5 6 DMs with `mc send`, each delivered in 975 to 1218 ms and redrawn. Result: min 10 ms, median 12 ms, max 37 ms, none over 100 ms. The Heltec, idle: 10 / 12 / 17 ms. The panel refresh never stalls the main loop: BUSY is polled.

## 2026-11-06 — the interface on the M5 (U6, part 1)

- Firmware `mc-m5` with `mc-ui`: 64 KB heap, one Ratatui terminal drawing into the panel frame, overlays, refresh-debt policy, gestures from the two buttons, light auto-off, unread LED, hibernation.
- Image 900,384 bytes, against a 1.75 MB app partition.
- `mc duo --a /dev/cu.wchusbserial210 --b /dev/cu.usbmodem1101 --pings 3 --reboot`: 13/13 with the interface running in the same loop as the radio.
- Checked by hand: a quick reply ("On my way") written on the M5 reached the Heltec (flood, SNR 12.25 dB) and showed "delivered" on the M5; the Heltec's answer (`mc send`, direct, acknowledged in 1.1 s) brought up the new-message banner.
- Still to check by hand: the tools on real traffic, light and potentiometer, the unread LED. No packet lost during a refresh: see the bursts below.

## 2026-11-06 — offline queue in flash, screen history (U2)

- Firmware with the offline queue and the 200-message history in the `mcdata` partition (index in RAM), prefs record v2. The M5's v1 prefs record (128 bytes) was read back: same name, key and radio after flashing.
- `mc duo --a /dev/cu.wchusbserial210 --b /dev/cu.usbmodem1101 --pings 3`: 12/12, every DM and sync now going through flash.
- `mc duo … --pings 0 --reboot`: Bob writes to the M5, the M5 reboots before the app fetches it, and the DM is still delivered after the reboot (11/11).
- `Device`: 146,232 bytes (147,240 before): the 40 KB history, the heard-nodes ring (2.8 KB) and the packet log (1 KB) cost less than the 47 KB queue that moved to flash.
- After the settings API refactor (U2, part 2): `mc duo … --pings 3 --reboot` 13/13; the companion arms now go through the shared setters.

## 2026-11-06 — U0: Ratatui on the M5's e-paper (ADR-0008)

- Setup: ThinkNode M5 running `ui-spike` (`firmware/esp32s3/src/bin/ui-spike.rs`): `mc_ui::demo` (header, list with a reversed selection, wrapped paragraph, sparkline, footer) drawn by Ratatui through mousefood into the panel frame, `esp-alloc` heap of 96 KB with `internal-heap-stats`. A frame every 4 s, grid switch every 6 frames. Logs `local/ui-spike-*.log`.
- Flash: the spike image is 346 KB with esp-hal and the fonts; the companion `mc-m5` is 493 KB, for a 1.75 MB app partition.

| | Large 25×15 | Compact 33×20 |
|---|---|---|
| Heap, steady (two cell buffers) | 21,008 B | 36,968 B |
| Heap, peak while drawing | 22,584 B | 39,376 B |
| Draw, whole screen | 6 ms (15 ms the first time) | 6 ms (11 ms the first time) |
| Frame sent to the panel (SPI 4 MHz) | 20 ms | 20 ms |
| Partial refresh until BUSY drops | 542 ms | 542 ms |
| Input to visible | 549 ms | 549 ms |

- Full refresh: 2007 ms.
- First version, with `Layout`: peak 55.5 KB (large) and 75.2 KB (compact), draw 21 ms. The constraint solver accounted for about 30 KB and 15 ms per draw.
- First version, with `..EmbeddedBackendConfig::default()`: 3.8 KB more heap after each grid switch. The default config builds the 6×10 font atlas and leaks it (`FontAtlas::leak`); it now has zero leak (host counting allocator: 0 B after create, draw and drop).
- RAM of `mc-m5` (static): `.data` 156.5 KB (of which `Device` 147 KB), `.bss` 16.8 KB, `.rwdata` 10.7 KB. The stack takes the rest of the 334 KB main DRAM (≈ 158 KB); `dram2` (72 KB) is free for Bluetooth.

## 2026-11-05 — one-hour fuzz runs

| Target | Executions | Duration | Crashes |
|---|---|---|---|
| `air_decode` (MC1) | 351,327,535 | 3601 s | 0 |
| `cmd_decode` (MC2, 2026-11-06) | 388,902,314 | 3601 s | 0 |
