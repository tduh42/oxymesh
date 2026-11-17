# ThinkNode M5 interface

Source of truth for `mc-ui`. On-screen text **in English**. 200×200 black and white e-paper panel (SSD1681), two buttons, brightness potentiometer, blue LED, buzzer.

## 1. Principles

- **Built on Ratatui** (`ratatui-core` + `ratatui-widgets`), drawn by `mousefood` into the panel frame (ADR-0008). Text is a grid of monospace cells.
- **The main screens are drawn in pixels on the panel** (ADR-0011): the header and footer of every screen, and the body of Home, the message list, conversations, compose, the new-message banner, the contact and channel cards, every menu (Tools, Settings and its pages, Identity, System) and picker, the hold-to-confirm popup, About, every tool (Heard nodes and a node's details, Discover, Trace route, Ping & range with its hits and misses and the answers' SNR, Signal with two bar charts of the last hour, Packet log, Airtime with a duty-cycle gauge, GPS, Repeater admin with its status and neighbours), the first-boot setup and the C++ import. The map and the QR cards draw their own pixel overlays over grid text. A screen returns a scene (`mc_ui::scene`): text in Helvetica, icons from Open Iconic, rounded boxes, circles, lines, a dither. The scene is painted over the grid. The grid version stays, and it is what the terminal emulator and the text snapshots show. Big digits, QR codes and the map are pixel overlays too.
- **The same code runs everywhere**: on the M5, in the terminal emulator (`mc ui`), and in snapshot tests. Screen logic is separate from drawing, so it is tested without a terminal.
- **Both buttons mean the same thing on every screen**, and the footer always says what they do right now.
- **Meaning never rests on style alone**: the selected row is reversed *and* marked; states are words or symbols, not just bold.
- **The screen never blocks the radio or Bluetooth**: the panel's BUSY line is polled, never waited on.
- **Nothing disappears from the app on its own**: the protocol's offline queue is untouched by the screen (`docs/spec/companion.md` §5).

## 2. Grids and glyphs

| Grid | Font | Cells | Spare pixels |
|---|---|---|---|
| Large (default) | 8×13 (bold 8×13 for titles) | 25 × 15 | 5 rows |
| Compact | 6×10 | 33 × 20 | 2 columns |

- Only glyphs present in the font are used. Each grid has its own symbol set (`theme::Symbols`), and a test checks every symbol against its font. A missing glyph would print as `?`.
- Missing from both fonts: ✓ ✗ ✉ ⚡. Missing from 6×10: ▸ ◂ ⏎ ★. Their fallbacks are words or ASCII.
- Screens split the area with plain arithmetic on the grid, never with Ratatui's `Layout` (ADR-0008).

## 3. Buttons and gestures

| Button | Click | Double click (< 300 ms) | Long press (≥ 600 ms) | Hold (≥ 1 s) |
|---|---|---|---|---|
| **A** (GPIO21, bottom) | next / down | previous / up | open, select, confirm | the same as a long press, once, at 1 s |
| **B** (GPIO14, top) | back (nothing on Home) | Home from anywhere | backlight on / off | — |

- A hold acts once, at 1 s, as a long press (holding is how people validate: on the M5, presses held past a second used to scroll instead). The rest of that hold is dropped, even when the press changed the screen. A hold-to-confirm popup counts only a hold that started on it: the hold that opened it never confirms it.
- **Home's shortcuts, button B:** three quick presses cycle the buzzer (off → quiet → on), four or more cycle message previews (shown → sender only → hidden, §8), each with a toast ("Buzzer: quiet", "Previews: hidden"). On other screens they act as a double click (Home). B counts its presses while each comes within 300 ms of the last, so its double click is reported once that window has passed; A's double click is still reported at once (it is "previous" everywhere).
- Recognizer (`mc_ui::input`): 30 ms debounce; a click is reported once the 300 ms double-click window has passed; a long press is reported on release; while held, a tick every 250 ms from 1 s on. A released after it started repeating does not also open.
- **While text is typed** (§7), B only deletes. Every short press is a click at its release: the recognizer neither waits for a double click nor counts presses (`Gestures::set_immediate_b`, set by the runner from `App::typing`), so quick presses delete that many characters, at once. A double click of B does not go Home there, since two quick deletes would throw the text away; presses that still arrive counted are taken as that many deletes, and stop with the press that cancels the entry.
- **While a phone is pairing** the PIN covers the screen (§6): presses do nothing, except B's long press for the light.
- Lists wrap: "next" on the last row goes to the first, "previous" on the first goes to the last.
- **Hold-to-confirm** for dangerous or outgoing actions (factory reset, remove contact, flood advert, region change): a popup asks to hold A for 2 s and shows the time left; releasing early cancels. On the panel it is a card over the faded screen, like the banner: a warning icon and the title, the text, `Hold A to confirm` with the time left, and a bar that fills while A is held. The screen under it stays drawn as it was.
- The first press while the backlight has timed out only turns it back on (if it was on before).

## 4. Screen layout

- **Header** (row 0, reversed): the screen's title on the left (bold); on the right, in order: unread (`●3`), backlight (`☀` while on), `BT` while a phone is connected, time `HH:MM` (not on Home, which has the big clock), battery `87%` (`87%+` while charging). On the panel: a black bar, the title in Helvetica bold, then mail icon and count, sun, Bluetooth, time, battery icon (empty under 25 %, a bolt while charging) and percentage.
- **Body**: rows 1 to the last but one.
- **Footer** (last row): the current meaning of the buttons, A on the left, B on the right: `A▸next hold▸open  B◂back` (compact grid: ► ◄). On Home, where B only toggles the light: `B☀`. "hold" is a word: the 8×13 font's ⏎ reads as a music note on the panel. On the panel: a rule, then the buttons as black key caps with their hints, `A next · hold: open`, `B back`; a double click is named where it does something of its own (`2×: up` in a conversation). Home's `B☀` reads `hold: light` (no sun in Helvetica). On the large grid the footer runs down to the panel's edge.
- **Toasts**: short results, shown in place of the footer for 3 s, from the next refresh: "Sent" when a message leaves; "Delivered 2.4 s" or "Failed" when an outgoing message's status changes (from the screen or the app); "Failed" for any action that failed.
- **Popups**: centered, framed, modal. B click closes them (cancel).
- **New message banner**: over the top of the body, sender and the start of the text, for direct messages (channel messages only count as unread). The first A press opens the conversation and is never swallowed; B dismisses it (`B◂later`). No banner for the conversation already on screen. On the panel the screen stays under it, faded by a dither, with a card on top: the mail icon, the sender, how long ago, and up to three lines of the text.

## 5. Backlight and potentiometer

- The backlight stays on once enabled, so that its brightness can be adjusted with the potentiometer.
- The potentiometer is not readable by the firmware (`docs/spec/m5.md`): no dead zone is possible at the bottom of its travel. Turning it 100 % off is guaranteed by button B and the auto-off, which cut the LED power (IO5), whatever the knob position. The header always shows the backlight state.
- Adjustable auto-off: 30 s, 2 min (default), never; the timer restarts on each press.

## 6. Screens

```
Home (always-on face)
├─ Messages ── conversations (DMs + channels, unread first) ── Conversation ── Compose
├─ Contacts ── list (favourites, recent) ── Contact: info · message · ping · trace · telemetry
│                                          · reset path · favourite · share (QR) · remove
├─ Channels ── list ── channel view ── compose ; add #hashtag channel ; show QR ; remove
├─ Tools
│   ├─ Heard nodes   ├─ Discover    ├─ Trace route   ├─ Ping & range test   ├─ Signal
│   ├─ Packet log    ├─ Airtime     ├─ Repeater admin ├─ GPS               └─ Map
└─ Settings
    ├─ Radio ├─ Identity ├─ Bluetooth ├─ Contacts ├─ Display ├─ Notifications
    ├─ Telemetry ├─ Power ├─ Time └─ System (about, setup again, reboot, factory reset, import)
```

- **Home**: big clock (overlay) and date; the next lines show unread DMs and channels, the last message (sender and start of the text), the radio (preset, last heard), and the GPS fix when enabled. At the bottom, a launcher (`Messages · Contacts · Channels · Tools · Map · Settings`) moved with A and opened with a long A; Settings stays last, one double click from the first. On the panel: the clock, the weekday and date, the GPS state and the noise floor, a card with the last message received (or the last node heard), then the launcher as 3 × 2 tiles with icons, the unread count on Messages.
- **Messages**: `+ New message` (pick a contact, then write), then the conversations: unread first, then the most recent; the right column shows the unread count or the time. On the panel each row has a badge (initials, `#` for a channel), the name (bold while unread), how long ago, a one-line preview (`You: …` for our own), and a dot while unread (a ring in a muted channel). Contacts and Channels use the same rows: type and route under a contact's name, an icon for repeaters, rooms and sensors.
- **Conversation**: received on the left, sent on the right, wrapped, newest at the bottom; A scrolls (click down, double up), a long A writes. Sent: `sending…`, `try 2…`, `flood…`, `delivered 2.4 s`, `failed`; channels: `sent`. Received: `direct` or `3 hops`; on channels, the sender's name. Messages arriving while the conversation is open are read at once. On the panel: bubbles at most 70 % of the width, received outlined on the left, sent filled on the right; under each the time and the route and SNR, or the status (a check mark and the round trip once delivered); on channels the sender's name above. Scrolling moves half a body at a time, and a bar on the right edge shows where the window is.
- **Contact**: type and route (`direct`, `2 hops`, `no route yet`), short key, last heard; then Write, Messages, Ping (round trip shown on the screen), Favourite, Reset route, Trace route (U4), Share (QR card), Remove (hold-to-confirm). On the panel: a card with the initials, the name (a star for a favourite), type and key, route and last heard, distance and bearing when both positions are known (else the contact's position), the last ping; then the actions as 3 × 3 icon tiles.
- **Channels**: `+ Add #channel` (name typed, secret derived from it like the app), then the channels with their unread count. A channel with unread messages opens on its conversation; otherwise on its screen: Messages, Write, Mute (yes / no), Remove (hold-to-confirm). On the panel the channel's screen is a card like a contact's: the `#` badge, the name and slot, the message and unread counts, whether it notifies, then the four actions as 2 × 2 tiles (Mute reads Unmute on a muted channel).
- **Menus and pickers on the panel** are pixel lists, one line per item with its current value on the right. The top menus give each item an icon (Tools: heard nodes, discover, trace, ping, signal, packet log, airtime, admin, GPS, map; Settings: radio, identity, Bluetooth, contacts, display, notifications, telemetry, power, time, system). A settings page has no icons and starts at the margin; a picker marks the current value with a check.
- **Muted channels** (oxymesh only: neither MeshCore nor its app has them, no protocol byte changes): a muted channel keeps its own unread count, marked `○3` instead of `●3` (`muted` when nothing is unread), and no longer floats to the top of Messages. It never counts in the header or on Home, never blinks the LED and never plays a tune. The mute belongs to the slot: when a slot gets another channel (a new secret, from the screen or the app's `SET_CHANNEL`), its mute and its old messages go; a rename keeps both. Stored as a 40-bit mask in the prefs record (bytes 134-138, zero in older records).
- **Compose and quick replies**: §7. 8 quick replies by default: OK, Yes, No, On my way, Call me, Where are you?, Running late, Thanks; plus "My position: 45.12345, 5.67890" while the GPS has a fix (five decimals, about a metre). Sent from the device: 3 direct attempts, then `RESET_PATH` and flood (deliberate deviation 6).
- **Tools** (U4):
  - Signal strength is shown as bars from the SNR: `▂▄▆█` at 10 dB and above, `▂▄▆` from 5, `▂▄` from 0, `▂` from −7.
  - The map is a pixel overlay on the panel (a cross for us, rings and names for the nodes; the braille canvas is only the terminal's fallback): braille dots are about 400 m apart at 10 km, too coarse to see.
  - Airtime compares the last hour of transmission with the duty cycle of the sub-band the frequency falls in (`config/eu868.toml`), as a filled bar: a line gauge is invisible in black and white.
  - Ping & range: a telemetry request every 5 s while the screen is open (hold A pauses), answered count, RTT, the SNR of each answer, and a strip of hits (`█`) and misses (`·`).
  - Trace route: along the contact's route and back (turning at a repeater contact itself, else at the last repeater of the route); per-hop SNR, then the SNR back here.
  - Heard nodes (name, type, signal bars and SNR, hops, distance, last heard; repeaters first).
  - Discover (zero-hop discovery, SNR both ways, at most once every 5 s).
  - Trace route (per-hop SNR bars, both directions).
  - Ping & range test (success %, RTT, SNR sparkline).
  - Signal (last RSSI/SNR, noise floor sparkline, packet rate). On the device, a `Radio` line counts what the chip itself received since boot: `412 ok, 15 bad`, readable packets and ones with a checksum or header error (something was on the air, too weak or too noisy to read).
  - Packet log (time, type, route, hops, SNR, summary).
  - Airtime (TX/RX time, regulatory duty-cycle gauge, queue).
  - Repeater admin (login as guest or with a password typed on the keyboard, status, neighbours strongest first), in the formats of MeshCore's `simple_repeater` v1.18.
  - Answers to the screen's requests (pings, traces, discovery, logins, status, neighbours) go to the screen only: the app never sees them, and the app's own pending requests are untouched.
  - GPS (fix, satellites, position, distance and bearing to contacts).
  - Map (braille canvas, self and contacts, zoom).
- **Settings** (U5): every value goes through the device's settings API, so the app sees it at once (`SELF_INFO`, `DEVICE_INFO`, the scope and auto-add getters).
  - A value is picked on a list where the current one is marked `●`; lists longer than 8 put it first. Toggles flip on a long A with a toast.
  - Radio: region (presets from `config/regions.toml`), frequency (typed in kHz), bandwidth, spreading factor, coding rate (all four hold-to-confirm: every node you talk to must match), power, path hash size, flood scope (none, or a region's scope).
  - Identity: my card, name, share location in adverts, adverts. Bluetooth: on / off (hold-to-confirm, then a restart; off, no phone can connect and on battery the device light-sleeps between events, ADR-0014), status (off, waiting, pairing, connected); the PIN, fixed (six digits) or random (drawn at boot, a new one on demand), shown with its value; paired phones, with a count, forgotten all at once behind a hold-to-confirm.
  - Contacts: add every node heard or only chosen types (chat, repeater, room, sensor), replace the oldest when full, max hops.
  - Display: text size (the grid changes at once), light timeout (30 s, 2 min, 10 min, never), invert, rotate 180° (applied on the panel in U6). Notifications: buzzer off / quiet / on, LED on unread, message previews (§8).
  - Telemetry: who may read the battery, the location, the sensors (nobody, favourites, everyone). Power: GPS on/off, GPS updates (how often the node takes the GPS position: 1 s to 1 h, the `gps_interval` the app can set too), hibernate (hold-to-confirm). Time: time zone (UTC−12 to UTC+14 in 30-minute steps), clock.
- **My card**: full-screen QR overlay of `meshcore://contact/add?name=<name>&public_key=<64 hex>&type=1`, scannable by the official app, with the name and short key below.
- **Pairing**: as soon as a phone starts pairing, a panel covers the body: "A phone wants to pair. Type this PIN on it:" and the PIN in seven-segment digits (`482 913`, overlay); in the terminal, the PIN as text. Gone when pairing ends, either way.
- **Hibernate**: leaves a badge on the panel (name, my-card QR, "off — press B") and wakes on B.
- **Timestamps are fixed** ("14:02", "yesterday") or whole minutes and more on the panel's lists and cards ("now", "5 min", "3 h", "2 d"), never seconds, so the clock alone never forces a redraw beyond its minute tick.
- **First boot**: a setup flow (region, name, time from GPS or the app). It can be skipped, and it comes back from Settings → System.
- **No time until the clock is set.** Until a clock chip, an app or the GPS gives the time, the header and Home show `--:--`, Home's date line reads `Clock not set`, and Settings → Time → Clock shows `--:--`. The node itself counts from its boot floor (`docs/spec/m5.md` §5), which is no time to show. Ages ("5 min") still work: they are differences.
- **GPS state**: `GPS fix, 9 sats`; while searching `No fix, 4 sats` on Home and `Searching  4 satellites` in the GPS tool (satellites in use from the last GGA: an open sky reads differently from a room); `GPS off`. The tool's footer offers `hold: share` only on "Share this position", which is there only with a fix.
- **Noise floor**: `-- dBm` until its first sampling round has ended (it reads 0 before).
- **About** (Settings → System): name, key, firmware, radio settings, contacts and channels used, battery, uptime, messages waiting for the app. After the firmware line, the device's health (ADR-0013): `Started` with the reason of the last start (`power on`, `restart`, `woke up`, `after an error`, `watchdog`, `low voltage`, `USB reset`), always; then only when there is something to say, `Errors` (panics since the power came on, and the last one's file and line), `Radio` (`2 resets`, or `not answering`) and `Storage` (`3 write errors`).

## 7. Two-button text entry

- Text at the top with the byte count and the 160-byte limit, live; below it what a long A can insert.
- **Two levels.** The first is a ring of stops; a click moves to the next one, a double click to the previous one. A long A inserts a letter or a word, or opens a row. In an open row a click moves along it and a long A inserts, then the cursor is back on the first stop. B leaves an open row; otherwise B deletes the last character, and on an empty text it cancels. Each press deletes at once, however fast they come (§3).
- **The ring, in order:**
  - the quick replies (empty text only; in place of the text on the panel);
  - the 4 letters most likely to come next, best first;
  - 2 completions of the word being typed;
  - the rows: `a b c d e f`, `g h i j k l`, `m n o p q r`, `s t u v w x`, `y z é è à ç ê ù`, then space, punctuation, digits, the rarer accents (`â ô î û œ ë ï ü`) and symbols in the last;
  - the final action (`Send`, `Save`, `Add`, `Login`) last, one double click from the first stop.
- **Prediction** (`mc_ui::keyboard`): each next character scores the words that continue the text with it, a word weighing by its rank in the built-in lists (about 1,900 French and 2,100 English words, most frequent first, written for oxymesh) and more for the words of your own sent messages. A whole word scores the space, and the punctuation after it (`, . ? !`, `?` and `!` also after a space, French style). After an apostrophe the word that follows is predicted too (`J'arr` → `J'arrive`). A word no list knows offers the space first.
- An opened row starts on its most likely character among those not already a stop of their own.
- The first letter of a sentence is upper case. Hashtag names: lower-case ASCII letters, digits and `-`; names and passwords get no prediction; passwords have rows of lower and upper case letters, digits and symbols; digits (PIN, frequency) are a plain ring.
- **On the panel:** the text in Helvetica 12 with a cursor; one line of keys with the predicted letters (the space drawn as `⎵`), or the open row's characters; the completions as chips with the final action on the right; the rows in two columns, the selected one filled, the open one framed.
- **Measured** (`crates/mc-ui/tests/typing.rs`): 14 French and English messages take 1,067 presses, 2,864 with the single ring this replaced (37 %). The test fails above 60 %.
- Long messages still come from the phone app.

## 8. Notifications

- **Blue LED**: blinks briefly every 5 s while something is unread (muted channels left out, like the header and Home).
- **Buzzer** (GPIO9, LEDC): the C++ tunes (`UITask.cpp:770-778`, RTTTL, `mc_ui::tune`): `MsgRcv3` for a DM, `kerplop` for a message on an unmuted channel, `ack` when a message sent from the screen is delivered. Off / quiet (the same tunes at 3 % duty) / on (50 %). Silent while a phone holds the Bluetooth link, like the C++ (`UITask.cpp:814-816`). USB does not count as an app: it is also the charger, and the C++'s USB `isConnected()` is always true. The C++ flag `buzzer_quiet` follows "off", for the import and a return to C++.
- We never switch screens on the user's behalf while they are navigating; a new DM shows the banner (§4).
- **Message previews** (Settings → Notifications → Previews), for a screen others can read: *shown* (the default), *sender only* or *hidden*. Outside an opened conversation, *sender only* replaces a message's text with "Message hidden" in the banner, on Home and in the Messages and Channels lists (and on the status pages' Messages page); *hidden* also replaces the sender with "New message" in the banner and "Message" on Home's card. Lists keep their conversation names: opening a list is a choice, as is opening a conversation, which always shows everything. Stored in the prefs record (byte 145; 0 in older records reads as *shown*). oxymesh only: the phone app is not affected. While previews are not *shown*, a lock shows in the header. Four quick presses of B on Home cycle the mode (§3).

## 9. Local history and unread

- The last 200 messages (DMs and channels, received, sent from the screen and sent from the app) are kept in flash, independently of the protocol's offline queue: peer, direction, time, status, text.
- A message from the app (`SEND_TXT_MSG` type 0, `SEND_CHANNEL_TXT_MSG`) is one entry, whatever the number of attempts (same contact and timestamp). Its status follows the attempts; its acknowledgement marks it delivered and still goes to the app. With no acknowledgement for 2 minutes after the last attempt it shows as sent, not failed: only the app knows when it gives up. Channel messages show as sent (nothing acknowledges them).
- A message becomes read when it is opened on the device **or** when the app fetches it with `SYNC_NEXT_MESSAGE`.

## 10. E-paper refresh

- Partial refresh for everything; a full refresh is decided by a **debt** (`mc_ui::refresh`):
  - each partial refresh adds 100 (150 once the debt has reached 1,000);
  - a full refresh brings the debt back to 0;
  - once the debt reaches 1,000, a full refresh runs 60 s after the last input;
  - at 3,000, the next screen change is a full refresh;
  - a screen change right after sending something is a full refresh.
- No refresh when the frame has not changed (hash of the panel bytes).
- The clock redraws once a minute; the battery percentage redraws only when it moves by 5 points or more.
- Measured (U0): partial refresh visible 549 ms after the input, full refresh 2.0 s.

## 11. Emulator and verification

- `mc ui` runs the real `mc-ui` App in a terminal at the device's grid size, hosted by `mc-sim`: Alice (an echo bot who writes first), Bob (posts on Public, answers pings) and a relay that forwards and answers like a MeshCore repeater (admin password `secret`). The emulator shows a fixed GPS fix. Keys: `j`/↓ = A click, `k`/↑ = A double click, Enter = A long, Space (held) = A hold, Esc/Backspace = B click, `h` = B double click, `l` = B long, `g` = switch grid, `p` = a simulated phone pairs, then connects, then leaves, `q` = quit. The simulated phone has paired once (PIN `482913`).
- `mc ui --png <dir>` renders every screen through mousefood into PNG files: the exact panel pixels, overlays included.
- Snapshot tests (`insta`) of every screen and state at both grid sizes.
- On the device (U6): navigation with the two buttons, a message composed and delivered with its tick, tools on real traffic, backlight and potentiometer, no packet lost during a refresh (M5 log compared with the Heltec's).

## 12. Status pages (one button, small screen)

The Heltec V4 (`docs/spec/heltec-v4.md`): a 128×64 OLED and one button, too small for the screens above. Like the C++ firmware on that board (`examples/companion_radio/ui-new/UITask.cpp`), it shows pages; the phone does the rest (`mc_ui::status`).

- **Button:** click = next page, double click = previous page, long press = act on the page. The pages go round.
- **Layout:** 6×10 font, 21 columns. A header line (node name, page `n/6`, battery `87%`, `+87%` while charging), a rule, 5 lines.
- **Pages:**

  | Page | Shows | Long press |
  |---|---|---|
  | Home | time and date, unread count (muted channels left out), Bluetooth state, noise floor | — |
  | Messages | the newest messages, newest first: `*` unread, `>` sent | marks the newest unread conversation read |
  | Heard | nodes heard lately: name, hops, how long ago | — |
  | Radio | frequency, SF / bandwidth / coding rate, TX setting, noise floor | — |
  | Advert | — | sends a flood advert |
  | Off | — | powers off (hibernation); the button wakes it |

- An acknowledgement (`> Advert sent`, `> Marked read`, `> Failed`) takes the last line for 2.5 s.
- While a phone pairs, its PIN covers the page in the large font.
- `mc ui --board heltec-v4` runs the pages in a terminal; tests snapshot every page as text and two of them as pixels (`crates/mc-ui/tests/status.rs`).


## 13. Server pages

A repeater or a room server (`rpt-*`, `room-*`; `docs/spec/server.md`) shows the same kind of pages as the companion's status pages (§12), with its own content (`mc_ui::server`). The C++ repeater shows a single screen; these pages are oxymesh's own and nothing else depends on them.

- **Buttons:** on the Heltec, click = next page, double click = previous, long press = act. On the M5, the lower button does the same and the top one shows the previous page. After power-off, the top button wakes the M5 and the only button wakes the Heltec.
- **Layout:** the header and 5 lines of §12 (21 columns). The Heltec draws them in the 6×10 font. The M5 uses 9×15 (bold header, a rule, one line every 30 px) and redraws at most every 30 s unless a button is pressed, with the e-paper refresh policy of §10.
- **Pages:**

  | Page | Shows | Long press |
  |---|---|---|
  | Home | `Repeater` (or `Room server`) and the time; forwarding on or off (a room server: posts stored and pushed); uptime; clients logged in with a role (`Logins`) and, for a repeater, neighbours (`Nbrs`) | — |
  | Radio | frequency, bandwidth / SF / coding rate, TX setting, noise floor, airtime (TX and RX, seconds) | — |
  | Traffic | packets heard, sent and duplicates, flood and direct; the last RSSI | — |
  | Neighbours (repeater) | how many, then the newest: key prefix, SNR, how long ago | — |
  | Posts (room server) | how many, then the newest: author's key prefix and text | — |
  | Advert | — | sends a flood advert (default scope) |
  | Off | — | powers off (deep sleep) |

- `crates/mc-ui/tests/server.rs` snapshots every page as text, on a simulated relay with a neighbour.
