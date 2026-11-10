# Server roles: repeater and room server

The repeater (`examples/simple_repeater/MyMesh.cpp`, "rpt" below) and, later, the room server (`examples/simple_room_server`) of MeshCore v1.18 (`companion-v1.18.0`, 81dbae5; MIT). The C++ code is authoritative. Forwarding itself is in `air.md` §4.6.

## 1. How oxymesh splits it (ADR-0010)

- `mc-node` in server mode (`Node::routing = Some(Routing)`):
  - forwards by the server rules (`air.md` §4.6);
  - keeps no contacts from adverts. It reports them as `Event::AdvertHeard` (with the advert timestamp and whether it came as a share);
  - hands over, once decrypted, what logged-in clients send: `Event::ServerPeer` for REQ and TXT_MSG, and `Event::ServerPath` for a PATH, whose path it stores in the client's contact entry. Anonymous requests come as `Event::AnonRequest`, with their arrival path and region.
- `mc-server` (`no_std`, no allocation) holds the role:
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
| `MAX_NEIGHBOURS` | 50 |
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

To be written with milestone S2.

## 4. The repeater console

To be written with milestone S1c. Until then the repeater answers the radio commands (`get`/`set` of `CommonRadioPrefs`, `companion.md` §6.1) and replies "Unknown command" to anything else.
