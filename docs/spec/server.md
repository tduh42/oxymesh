# Server roles: repeater and room server

The repeater (`examples/simple_repeater/MyMesh.cpp`, "rpt" below) and the room server (`examples/simple_room_server/MyMesh.cpp`, "room" below) of MeshCore v1.18 (`companion-v1.18.0`, 81dbae5; MIT). The C++ code is authoritative. Forwarding itself is in `air.md` §4.6.

## 1. How oxymesh splits it (ADR-0010)

- `mc-node` in server mode (`Node::routing = Some(Routing)`):
  - forwards by the server rules (`air.md` §4.6);
  - keeps no contacts from adverts. It reports them as `Event::AdvertHeard` (with the advert timestamp and whether it came as a share);
  - hands over, once decrypted, what logged-in clients send: `Event::ServerPeer` for REQ and TXT_MSG, and `Event::ServerPath` for a PATH, whose path it stores in the client's contact entry. Anonymous requests come as `Event::AnonRequest`, with their arrival path and region.
- In server mode the node also keeps the ACKs the server waits for (`Node::server_acks`, a room server's pushes). One that arrives, alone, in a multipart or riding a PATH, is taken off, not relayed (`markDoNotRetransmit`), and reported as `Event::ServerAck`.
- `mc-server` (`no_std`, no allocation) holds the role in one `Server` with a `Role` (repeater or room):
  - the access list (`acl`, 32 clients) and the neighbours (`neighbours`, 50 slots);
  - the rate limiters, logins, requests and the console;
  - discovery and the advert timers;
  - reply routing (`reply`).
- A logged-in client is also a contact of the node, so that its packets decrypt. Its path back is that contact's `out_path`. The C++ keeps it in `ClientInfo`, which is invisible from outside.

## 2. Repeater

### 2.1 Constants (rpt:1-60, `MyMesh.h`)

| Name | Value |
|---|---|
| `FIRMWARE_VER_LEVEL` (login reply) | 2 |
| `SERVER_RESPONSE_DELAY` | 300 ms |
| `TXT_ACK_DELAY` | 200 ms |
| `CLI_REPLY_DELAY_MILLIS` | 600 ms |
| `LAZY_CONTACTS_WRITE_DELAY` (access list save) | 5 s |
| `MAX_NEIGHBOURS` | per board build: 8 on the M5, 50 on the Heltec V4 (`Neighbours::with_limit`) |
| Discovery answers | at most 4 per 120 s |
| Anonymous requests (regions, owner, clock) | at most 4 per 180 s |
| Roles (`permissions & 3`) | 0 guest, 1 read only, 2 read write, 3 admin |

### 2.2 Login (ANON_REQ whose data starts with 0 or a byte ≥ `' '`; rpt:90-145)

1. A blank password logs in a client already in the access list, with its stored role.
2. Otherwise the admin password gives role 3, the guest password role 0. Anything else gets no answer.
3. The client is added to the access list, or the least recently active one is evicted. The request is refused if its timestamp is not newer than the client's last one (replay, or a full table).
4. A role other than guest schedules an access-list save 5 s later.
5. A login by flood forgets the client's path.
6. Reply (13 bytes): our unique time (4), `0` (OK), `0` (legacy keep-alive), is admin (1), permissions, 4 random bytes, `FIRMWARE_VER_LEVEL`.

### 2.3 Anonymous requests (rpt:147-209, 571-618)

Types 1 (regions), 2 (owner) and 3 (clock) are answered only to a **direct** request, within the rate limit. The data after the type is `{reply path length}{reply path}`; an invalid length gets no answer. Each reply starts with the request's timestamp and our clock (4 + 4 bytes), then:

| Type | Then |
|---|---|
| 1 | the names of the regions that allow flood (`exportNamesTo`) |
| 2 | `name\nowner_info` |
| 3 | features: `0x80` if forwarding is disabled |

Route (`chooseReplyRoute`, `src/helpers/RoutingPolicy.h:39-44`): a request by flood gets a PATH back (flooded) carrying the reply. Otherwise the reply goes direct along the supplied path, then along the client's stored path, and is flooded as a last resort. A direct login also uses the stored path.

### 2.4 Requests from clients (REQ; rpt:211-372, 663-707)

The client must be in the access list, and the timestamp newer than its last one. Unknown types get no answer. Each reply starts with the request's timestamp. A request by flood gets a PATH back; otherwise the reply goes direct if the client's path is known, flooded if not.

| Type | Who | Reply |
|---|---|---|
| 0x01 status | any role | `RepeaterStats`, 56 bytes LE: battery mV (u16), TX queue (u16), noise floor (i16), last RSSI (i16), received, sent, TX airtime s, uptime s (u32 each), sent flood, sent direct, received flood, received direct (u32 each), error flags (u16), last SNR × 4 (i16), direct duplicates, flood duplicates (u16 each), RX airtime s, receive errors (u32 each) |
| 0x03 telemetry | any role | Cayenne LPP: voltage (channel 1, type 116); GPS (136) if allowed by `~byte 1` and a fix exists; MCU temperature (103). A guest gets the voltage and temperature only |
| 0x05 access list | admin; bytes 1 and 2 zero | 7 bytes per client with a role other than 0: key prefix (6), permissions; up to 176 bytes |
| 0x06 neighbours | any role; version 0 | total (u16), count (u16), then per neighbour: key prefix (`byte 6` bytes, at most 32), seconds since heard (u32), SNR × 4 (i8). `byte 2` count, `bytes 3-4` offset, `byte 5` order (0 newest, 1 oldest, 2 strongest, 3 weakest first). At most 130 bytes of entries |
| 0x07 owner | any role | `version\nname\nowner_info` |

The keep-alive request (0x02) is not answered in v1.18.

### 2.5 The console over the air (TXT_MSG from an admin; rpt:708-764)

1. Only text types 0 (plain), 1 (CLI data) and 3 are handled. The timestamp must not be older than the client's last one; an equal one is a retry, which gets no reply.
2. Plain text (the legacy console) is acknowledged after 200 ms with `SHA256(timestamp ‖ flags ‖ text ‖ client key)[0..4]`, sent direct if the path is known, flooded if not.
3. The reply is a TXT_MSG of type CLI data, after 600 ms, with our unique time (+1 if it equals the request's). It goes direct or flooded like the acknowledgement. An empty reply sends nothing.
4. The command set is the repeater console (§4).

### 2.6 Neighbours (rpt:63-88, 650-661)

- A **zero-hop** advert from a repeater, not received as a share (transport codes 0 and 0), puts it in the table: its advert timestamp, our clock, its SNR × 4.
- So does an answer to our own discovery request (§2.7).
- A neighbour already known is updated in place. Otherwise the least recently heard slot is replaced.

### 2.7 Discovery (CONTROL; rpt:789-857)

- **A request** (`0x80` in the high nibble, at least 6 bytes) is answered when all of these hold:
  - forwarding is on, the rate limit allows it, and the node is not hidden;
  - the filter (byte 1) includes repeaters (`1 << 2`);
  - `discovery_mod_timestamp` ≥ the optional "since" field (bytes 6-9).
- **The answer**, zero hop:
  - `0x92`, the request's SNR × 4, its tag (bytes 2-5), then our key: 8 bytes if bit 0 of the request is set, 32 if not;
  - delayed by four times the random retransmit delay.
- `discover.neighbors` sends `0x80`, `1 << 2` and a random tag. For 60 s, answers carrying that tag and a whole key add neighbours; our own key is ignored.

### 2.8 Adverts (rpt:1033-1060, 1289-1306; `main.cpp`)

- 16 s after start: a zero-hop advert.
- Every `advert_interval` × 2 minutes (default 1): a zero-hop advert.
- Every `flood_advert_interval` hours (default 47): a flood advert.
  - It goes within the default scope, if a default region with a key exists.
  - Its path hashes are `path_hash_mode + 1` bytes.
  - It also restarts the local advert timer.
- A node with both intervals at 0 is hidden: it sends no adverts and does not answer discovery.

### 2.9 Defaults (rpt:890-929)

| Setting | Repeater default |
|---|---|
| Admin password | `password` |
| Guest password | empty |
| `tx_delay_factor`, `direct_tx_delay_factor` | 0.5, 0.3 |
| `rx_delay_base` | 0 |
| `airtime_factor` | 1.0 |
| `interference_threshold`, CAD | off |
| `flood_max`, `flood_max_unscoped`, `flood_max_advert` | 64, 64, 8 |
| `loop_detect` | minimal |
| `advert_loc_policy` | prefs (the configured position) |

### 2.10 Flooded replies (rpt:413-432)

Replies are flooded with path hashes the size of the request's (`chooseReplyScope`):

1. within the request's region, if it came scoped and that region has a key;
2. unscoped, if it came as a plain flood (the wildcard region);
3. otherwise (a direct request, or an unresolved region) within the default scope if there is one, else unscoped.

### 2.11 Where oxymesh differs

These differences are invisible on the air:

- The status counts "received" and "sent" are the node's flood plus direct counters, as for the companion's `GET_STATS`. The C++ takes the radio driver's counts, which also include packets that failed to decode.
- Telemetry has no environment sensors.

## 3. Room server

A bulletin board: logged-in clients post plain text, and every post goes to every other client, one at a time, until each acknowledges it (`mc_server::room`). Everything not listed here is as for the repeater: requests and their routes (§2.4), the console (§4), adverts (§2.8), flooded replies (§2.10), storage (§5).

| | Room server |
|---|---|
| `FIRMWARE_VER_LEVEL`, `FIRMWARE_ROLE` | 1, `room_server` |
| Advert type | room (3) |
| Forwarding | off (`disable_fwd`); `set repeat on` answers `ERR - repeat now deprecated` |
| `direct_tx_delay_factor` | 0.2 |
| Room password (`guest_password`) | `hello` in the M5 and Heltec builds (`ROOM_PASSWORD`) |
| Anonymous requests other than the login, neighbours, discovery, `REQ` 0x06 and 0x07 | none |
| Posts (`MAX_UNSYNCED_POSTS`) | 32 in a ring, 150 characters each, in memory only |

### 3.1 Login (ANON_REQ; room:324-411)

The data after the timestamp is `{sync since (u32 LE)}{password}`: the client's newest post already received.

1. A blank password logs in a client already in the access list.
2. Otherwise:
   - the admin password gives admin (3);
   - the room password gives read-write (2);
   - with `allow.read.only`, any other password gives guest (0);
   - without it, there is no answer.
3. Replay check as for the repeater. Then:
   - `sync_since` is taken from the request, and the push state is reset;
   - the access list is saved 5 s later, whatever the role.
4. The reply is as for the repeater, except:
   - byte 6 is 1 for an admin, 2 for permissions 0, 0 otherwise;
   - the firmware level is 1.
5. Pushes then wait 2 s, so the reply goes first. The route: a PATH back for a flood login, otherwise direct if the path is known, flooded if not.

### 3.2 From a client (TXT_MSG, REQ; room:433-599)

- **TXT_MSG** (types 0, 1, 3; timestamp not older than the last; an equal one is a retry):
  - the client's activity is updated and its failed pushes reset;
  - **plain text** from anyone but a guest is a post (unless a retry). It is acknowledged after 200 ms with `SHA256(timestamp ‖ flags ‖ text ‖ client key)[0..4]`:
    - flooded if the path is unknown;
    - otherwise direct, preceded by a multipart copy 300 ms earlier when `multi.acks` is on;
  - **CLI data or command** from an admin is a console command (unless a retry). It gets no ACK. The reply is CLI data with our unique time, sent 300 ms after the ACK's delay (none here), direct or flooded.
- **REQ** (timestamp not older than the last, an equal one accepted):
  - **keep-alive** (0x02), direct only. The bytes 5-8 (`{last post time received}`, 0 if missing) move the client's sync point when not 0. Any push waiting for its ACK is dropped. If the client's path is known, the answer is a direct ACK of `SHA256(request[0..9] ‖ client key)[0..4]` followed by one byte: the posts waiting for that client;
  - **status** (0x01), `ServerStats`, 52 bytes: the repeater's first 48 bytes (§2.4), then posts stored (u16) and pushes sent (u16);
  - **telemetry** (0x03), as the repeater's;
  - **access list** (0x05, admin), as the repeater's, but listing admins only.

### 3.3 Pushing posts (room:41-131, 997-1040)

- A post (`storePost`) gets our unique time and goes into the ring. Pushes start 2 s later.
- **Every round**, when due:
  1. The waits for ACKs that timed out are given up: the client's failures count up.
  2. The next client in turn (round robin) is considered if:
     - no ACK is awaited from it;
     - it was active since the start (`last_activity` not 0);
     - its failures are under 3.
  3. It gets the oldest post that is at least 6 s old, newer than its sync point, and not its own. The next round comes 1.2 s later, or 150 ms if there was nothing to push.
- **A push** is a TXT_MSG of type signed plain: `{post time}{flags: 2 << 2 | random attempt (0-3)}{author key[0..4]}{text}`.
  - Its ACK is `SHA256(those bytes ‖ client key)[0..4]`.
  - With the client's path known, it goes direct, and the wait ends after 4 s + 2 s × (hops + 1). Otherwise it is flooded in the default scope, with hashes of `path_hash_mode + 1` bytes, and the wait ends after 12 s.
- **The ACK** (`processAck`) ends the wait and moves the client's sync point to the post's time.
- `room.post {text}` on the console posts as the room itself: `OK`, `ERR empty message`.

### 3.4 Where oxymesh differs

- An ACK the server did not ask for is relayed (or not) as the node's rules say. The C++ asks `processAck` about every ACK, so an ACK matching a push that has already timed out would also be kept back there.

## 4. The console

`mc_server::console` (`Repeater::command`), a port of `MyMesh::handleCommand` (rpt:1198-1287) and `CommonCLI` (`src/helpers/CommonCLI.cpp`). Lines come from admins over the air (§2.5) and from the serial port, where the timestamp is 0. Replies are the C++ texts, at most 160 bytes.

### 4.1 How a line is read

1. During `region load`, every line is a region (§4.5) until a blank one.
2. Leading spaces are skipped. An `xx|` prefix (the companion's remote console) is copied to the start of the reply.
3. Commands match **by prefix**, like the C++'s `memcmp`. So `rebootx` reboots, and `get flood.max.advert` is tested before `get flood.max`.
4. They are tried in this order:
   1. the repeater's own commands (§4.2);
   2. the radio commands shared with the companion (`companion.md` §6.1);
   3. the board's commands: `radio.fem.rxgain` on a Heltec V4 with a switchable LNA;
   4. the common commands (§4.3, §4.4).

   Anything else gets `Unknown command`.
5. A setting saved from the console also turns off the 2-minute local adverts if `advert.interval` is under 60 minutes (`CommonCLI::savePrefs`). The default of 2 minutes is meant for new installs only.

### 4.2 Each server's own commands

The room server has `room.post` and refuses `set repeat on` (§3); it has no `discover.neighbors`, which falls to `Unknown command`. `get role` answers `> room_server`.

| Command | Reply, effect |
|---|---|
| `setperm {key hex} {permissions}` | role 0 removes a client found by key prefix; any other role needs the whole key, then adds or updates it. `OK`, `Err - bad params`, `Err - bad pubkey`, `Err - invalid params`. The access list is saved 5 s later |
| `get acl` (serial) | prints `ACL:` and one `{permissions hex} {key hex}` line per client whose permissions are not 0; empty reply |
| `discover.neighbors` (repeater) | sends a discovery request (§2.7): `OK - Discover sent`; with anything after it: `Err - discover.neighbors has no options` |

### 4.3 Common commands

| Command | Reply, effect |
|---|---|
| `poweroff`, `shutdown`, `reboot` | no reply; the runner powers off or restarts |
| `clkreboot` | clock set to 1715770351 (15 May 2024), then a restart |
| `advert`, `advert.zerohop` | a flood (default scope) or zero-hop advert 1.5 s later: `OK - Advert sent`, `OK - zerohop advert sent` |
| `clock` | `HH:MM - D/M/YYYY UTC` |
| `clock sync` | over the air: the sender's time + 1 s if later than ours, `OK - clock set: …`; otherwise `ERR: clock cannot go backwards` |
| `time {epoch}` | the same from a number, `(ERR: clock cannot go backwards)` |
| `neighbors` | `{key[0..4] hex}:{seconds ago}:{SNR × 4}` lines, newest first, while the reply is under 134 bytes; `-none-` |
| `neighbor.remove {hex}` | removes the neighbours whose key starts with it (all of them for an empty prefix): `OK`, `ERR: bad pubkey` (odd length) |
| `tempradio {MHz},{kHz},{SF},{CR},{minutes}` | those radio settings 2 s later, unsaved, back after the minutes: `OK - temp params for N mins`, `Error, invalid params` |
| `password {text}` | admin password (15 characters kept): `password now: {text}` |
| `clear stats` | counters to 0: `(OK - stats reset)` |
| `erase` (serial) | the runner wipes oxymesh's own store: `File system erase: OK` |
| `ver`, `board`, `power` | `{version} (Build: {date})`, the manufacturer, `batt:{mV} mV usb:{yes/no/n/a} solar_chg:… ext:… charger:…` |
| `sensor get/set/list` | the `gps` setting on boards with a GPS (`gps_interval` can be set too): `> 1`, `null`, `ok`, `can't find custom var`, `1 vars\ngps=1`, `no custom var` |
| `region …` | §4.5 |
| `gps`, `gps on/off/sync/diag/setloc`, `gps advert [none/share/prefs]` | boards with a GPS only (the M5) |
| `powersaving [on/off]` | `Board not supported` for `on` (§4.6); `off`; `on`/`off` |
| `stats-core`, `stats-radio`, `stats-packets` (serial) | the C++'s JSON (`StatsFormatHelper.h`) |

### 4.4 `get` and `set`

| Setting | Get | Set |
|---|---|---|
| `name` | `> {name}` | `OK`, `Error, bad chars` (`[ ] \ : , ? *`) |
| `lat`, `lon` | `> {ftoa}` | `OK` |
| `repeat` | `> on/off` | `OK - repeat is now ON/OFF` |
| `flood.max`, `flood.max.unscoped`, `flood.max.advert` | `> N` | `OK`, `Error, max 64`. The value goes through `uint8_t`, so 300 becomes 44 |
| `loop.detect` | `> off/minimal/moderate/strict` | `OK`, `Error, must be: off, minimal, moderate, or strict` |
| `advert.interval` | `> {minutes}` | 0 or 60-240: `OK`, `Error: interval range is 60-240 minutes` |
| `flood.advert.interval` | `> {hours}` | 0 or 3-168: `OK`, `Error: interval range is 3-168 hours` |
| `guest.password` | `> {text}` | `OK` |
| `allow.read.only` | `> on/off` | `OK` |
| `owner.info` | `> {text}`, new lines shown as `|` | `|` stored as new lines: `OK` |
| `prv.key` | serial only: 64 bytes in hex | `OK, reboot to apply! New pubkey: {hex}` for a key that passes `validatePrivateKey`; `Error, bad key` |
| `freq` | (radio command) | serial only: `OK - reboot to apply`, not checked |
| `public.key`, `role` | `> {hex}`, `> repeater` | |
| `bridge.type` | `> none` | |
| `bootloader.ver`, `adc.multiplier` | `Error: unsupported` | `adc.multiplier`: `Error: unsupported` |
| `pwrmgt.support`, `pwrmgt.source`, `pwrmgt.bootmv`, `pwrmgt.bootreason` | `> unsupported`, `ERROR: Power management not supported`, `> Reset: …; Shutdown: …` | |
| `extra.sf` | `No extra SF configured` | |
| anything else | `??: {key}` | `unknown config: {rest}` |

Hex output is uppercase, as `Utils::toHex` writes it.

### 4.5 Regions (`CommonCLI.cpp:889-1027`, rpt:1199-1231)

| Command | Reply, effect |
|---|---|
| `region` | the tree (`exportTo`) |
| `region def {name}[|{jump}] …` | puts each name under the previous one, or under `jump` (found by prefix), with flood allowed; then the tree. `Err - empty def`, `Err - empty name`, `Err - empty jump`, `Err - put failed: …`, `Err - unknown jump: …` |
| `region put {name} [{parent}]` | `OK - (flood allowed)`, `Err - unknown parent`, `Err - unable to put` |
| `region remove {name}` | exact name: `OK`, `Err - not empty`, `Err - not found` |
| `region get {prefix}` | ` {name} ({parent}) F` or ` {name} F` (`F` when flood is allowed), `Err - unknown region` |
| `region allowf/denyf {prefix}` | `OK` |
| `region home [{prefix}]` | ` home is now {name}`; ` home is {name}` or `*` |
| `region default [{prefix}/<null>]` | creates the region if needed, flood allowed, saves the table: ` default scope is now {name}`; ` default scope is {name}` or `<null>`; `Err - region table full` |
| `region list allowed/denied` | names, comma-separated, `-none-`; `Err - use 'allowed' or 'denied'` |
| `region save` | `discovery_mod_timestamp` = now; saves settings and table: `OK` |
| `region load` | then one region per line, indented by depth (1 to 7 spaces), ` F` to allow flood. A region already known keeps its id and flags. A blank line ends it: `OK - loaded N regions`. As in the C++, the table loaded has no home and no default region |
| anything else | `Err - ??` |

### 4.6 Where the console differs from the C++

- There is no packet log: `log start`, `log stop`, `log erase` and `log` answer `Error: unsupported`.
- There is no power saving: `powersaving on` answers `Board not supported`. The C++ gives this answer on boards without power saving.
- There is no OTA update. `start ota` answers `Error`, as the C++ does on boards without OTA.
- There are no bridges and no ADC multiplier.
- After `region load`, the default scope used for adverts follows the new table at once. The C++ keeps the key it had computed until `region default` or a restart.
- `stats-packets` "recv" and "sent" are the node's counters (§2.11).

## 5. Storage and import

### 5.1 What is kept (`mc_server::persist`)

The runner stores four records under its own keys (`ServerStore`), on the actions `SavePrefs`, `SaveAcl` and `SaveRegions` (`Repeater::save`). At start it hands them back (`Repeater::restore`) before `Repeater::start`.

| Record | Content |
|---|---|
| node settings | the companion's settings record (`mc_node::persist`): name, position, radio, delays, GPS |
| repeater settings (164 bytes) | version 1; admin and guest passwords (16 each, NUL padded); owner info (120); `advert_interval`, `flood_advert_interval`; flags: bit 0 `allow_read_only`, bit 1 `powersaving`, bit 2 `disable_fwd`; `flood_max`, `flood_max_unscoped`, `flood_max_advert`, `loop_detect`; `discovery_mod_timestamp` (u32 LE) |
| access list | per client: the C++ `/s_contacts` record without its last 32 bytes. That is the key (32), permissions, `sync_since` (u32 LE), 2 unused bytes, `out_path_len`, `out_path` (64). Only clients with permissions are kept, as `ClientACL::save` does; a room server keeps its admins only (`saveFilter`). Their paths come back as the node contacts' paths |
| regions | the C++ `/regions2` file as is (`mc_node::region`) |

The C++ saves the shared secret with each client. oxymesh derives it again from its own key, as the C++ does on load, so it never stores it.

### 5.2 Import from the C++ repeater (`mc_spiffs::repeater`)

A SPIFFS holding `/s_contacts` or `/regions2` is a server's (`is_server`). The import reads everything before it writes anything:

- `/identity/_main.id`, checked as for the companion (`companion.md` §8.1);
- `/prefs.json`:
  - the node's settings, as the companion's, starting from the repeater defaults (§2.9);
  - the repeater's own keys: `pass`, `guest`, `owner`, `adv_int`, `f_adv_int`, `repeat.{disable, f_max, f_max_uns, f_max_adv, loop}`, `room.rd_only`, `power.pwr_sav_en`;
  - `bridge`, `custom` and `power.adc_mult` are not used;
  - the older binary `/com_prefs` is not read: v1.18 rewrites it as `/prefs.json` at its first boot;
- `/s_contacts`: 136-byte records, at most 32. Each loses its shared secret;
- `/regions2`, decoded then written again in the same format.

`discovery_mod_timestamp` is not in the C++'s `/prefs.json`, so it restarts at 0. `mc spiffs <image>` shows what an import would bring. The test image is `testdata/spiffs/cpp-repeater.bin`, made up by `testdata/spiffs/gen.py`.

To be written with milestone S2.

## 6. Firmware (`rpt-*`, `room-*`)

`firmware/esp32s3/src/bin/server/{m5,heltec_v4}.rs`, one per board, take the role: `rpt-*` binaries run a repeater, `room-*` binaries a room server. They sit on the shared runner (ADR-0009). The radio loop takes a `run::radio::Mesh`: a companion `Device`, or here a bare `Node`. Flash with `scripts/flash.sh m5-repeater|heltec-v4-repeater|m5-room|heltec-v4-room <port>`.

- **Each pass:**
  1. the serial console;
  2. the radio;
  3. the node's events to `Repeater::on_event`;
  4. `Repeater::tick`;
  5. the actions: saves go to the store (`Repeater::save`); the runner handles restarts, power-off, radio changes, the temporary radio and the GPS.
- **Serial console** (`run::server::Console`, `main.cpp:155-190`):
  - at boot it prints `Repeater ID: {key hex}` (`Room ID: …`);
  - every character is echoed, `\n` is dropped and `\r` ends the line (as does the 159th character, the 151st for a room server);
  - the repeater echoes a new line after the command, the room server the `\n` it is sent;
  - a non-empty reply is printed as `  -> {reply}`.

  On the M5 it runs on UART0 (CH340, 115,200 baud), on the Heltec on the chip's own USB. `mc term <port> "line" …` sends lines and prints what comes back.
- **Storage:** `mcdata`, the keys of `flashstore.rs` (§5.1). The identity and node settings use the same keys as the companion, so a board that ran the oxymesh companion keeps its identity and radio settings. The companion's contacts stay, unused.
- **First boot** (no stored identity): the C++ SPIFFS is imported (§5.2), or a new identity is made.
- **No Bluetooth**, like the C++ repeater.

| | ThinkNode M5 | Heltec V4 |
|---|---|---|
| Name at first start (`ADVERT_NAME`) | `Thinknode M5 Repeater`, `Thinknode M5 Room Server` | `Heltec Repeater`, `Heltec Room` |
| Neighbours (`MAX_NEIGHBOURS`) | 8 | 50 |
| GPS | yes: `gps` commands, position, clock | no |
| Clock chip | PCF8563, read at boot, written when the clock is set | none: the clock starts at 2026-11-05 until `time` or `clock sync` |
| Front end | none | GC1109 or KCT8103L (`radio.fem.rxgain`, regulatory gain) |
| Screen | e-paper, §13 of `ui.md` | OLED, §13 of `ui.md` |

From a companion, `mc remote <port> <key prefix> <password> status "get role" …` logs in over the air, then requests the status or runs console commands.

