# ADR-0010 — Server roles in their own crate, on a node in server mode

- Status: accepted
- Date: 2026-11-10

## Context

oxymesh had only the companion role (ADR-0004). The repeater and the room server of the C++ firmware are separate builds (`examples/simple_repeater`, `examples/simple_room_server`) that share the mesh and the dispatcher with the companion, but differ in four ways:

- they forward by their own rules (`air.md` §4.6);
- they keep an access list of logged-in clients rather than contacts;
- they answer requests and console commands over the air;
- they run their own advert timers.

The companion's `Device` (app link, message queue, contacts from adverts) has no place on them.

## Decision

- **`mc-node` gains a server mode**, `Node::routing = Some(Routing)`. In that mode it:
  - forwards by the server rules;
  - adds no contacts from adverts;
  - hands what logged-in clients send to its runner as events: `ServerPeer`, `ServerPath`, `AnonRequest`, `AdvertHeard`, `ControlData`.
- **A new `no_std` crate, `mc-server`**, holds the roles (`docs/spec/server.md`):
  - the access list, neighbours and rate limiters;
  - logins, requests, the console over the air, discovery and adverts;
  - reply routing (`chooseReplyRoute`, `chooseReplyScope`).
  
  It works on `&mut Node` and has no IO. A runner feeds it the node's events and calls `tick`.
- **A logged-in client is also a node contact**, so that the node's decryption and path storage serve both roles. The access list holds what is the server's own: role, last timestamp, last activity.
- **One `Server`, with a `Role`** (repeater or room server). The two C++ servers share their settings, access list, console, storage, adverts and timers. What differs is in `mc_server::server` (repeater) and `mc_server::room`: logins, requests, neighbours and discovery, posts and pushes. A room server's pushes wait for ACKs, which the node reports from a list the server fills (`Node::server_acks`).
- **The simulator's repeater is `mc-server`.** It replaces the stand-in in `mc-sim/src/server.rs`, which reported firmware level 1 and gave guests role 1, where v1.18 gives 2 and 0.

## Consequences

- One mesh core for all three roles. A server build is a node, `mc-server` and a runner, with no companion `Device`.
- The repeater can be tested against our own companion in `mc-sim` (`crates/mc-sim/tests/server.rs`) and fuzzed (`fuzz/fuzz_targets/server_input.rs`).
- The access list holds 32 clients, the C++ default `MAX_CLIENTS` (`src/helpers/ClientACL.h:44-46`), and evicts like it: the least recently active client that is not an admin.
- Rejected: server code inside `mc-node`. It would mix two roles' state in one struct, and every companion build would carry it.
- The firmware's radio loop (`run::radio`) takes a companion `Device` today. The server binaries (S1d) need it to take a node. The simulator keeps a `Device` around every node, and the server ignores its companion side.
- Rejected: a `Device`-like wrapper owning the node for servers. The server needs none of the companion's app link, message queue or contacts from adverts.
