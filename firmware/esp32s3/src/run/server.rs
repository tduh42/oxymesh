//! What the server binaries share (`docs/spec/server.md`): the text console on the serial
//! wire, as the C++ repeater's `main.cpp` runs it, and the boot identity of a server (stored,
//! imported from a C++ repeater or companion at first boot, or new).

use core::fmt::Write;

use log::info;
use mc_air::crypto::LocalIdentity;
use mc_node::Node;
use mc_server::{Role, Server};
use mc_spiffs::{Spiffs, repeater};
use rand_core::Rng;

use super::boot::Store;
use super::link::Wire;
use crate::partition::ReadOnly;

/// `char command[160]` (the room server's: `MAX_POST_TEXT_LEN + 1`, 152).
const LINE_MAX: usize = 160;
const ROOM_LINE_MAX: usize = 152;

/// Text out on the wire.
pub struct WireOut<'a, W: Wire>(pub &'a mut W);

impl<W: Wire> Write for WireOut<'_, W> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.0.write_all(s.as_bytes());
        Ok(())
    }
}

/// The serial console (repeater `main.cpp:155-190`, room server `main.cpp:104-130`):
/// characters are echoed and `\n` dropped; a line ends at `\r` (or when the buffer is full);
/// the reply comes as `  -> {reply}`. The repeater echoes a new line after the command, the
/// room server echoes the `\n` it was sent.
pub struct Console {
    role: Role,
    line: [u8; LINE_MAX],
    len: usize,
    /// Bytes read and not yet taken (what follows a complete line waits for the next poll).
    stash: [u8; 32],
    stashed: usize,
    taken: usize,
}

impl Console {
    pub const fn new(role: Role) -> Self {
        Self {
            role,
            line: [0; LINE_MAX],
            len: 0,
            stash: [0; 32],
            stashed: 0,
            taken: 0,
        }
    }

    fn line_max(&self) -> usize {
        match self.role {
            Role::Repeater => LINE_MAX,
            Role::Room => ROOM_LINE_MAX,
        }
    }

    /// `Repeater ID: {key hex}` (`Room ID: …`), at boot.
    pub fn banner<W: Wire>(&self, wire: &mut W, node: &Node) {
        let mut out = WireOut(wire);
        let _ = out.write_str(match self.role {
            Role::Repeater => "Repeater ID: ",
            Role::Room => "Room ID: ",
        });
        for b in node.id.pub_key() {
            let _ = write!(out, "{b:02X}");
        }
        let _ = out.write_str("\r\n");
    }

    /// Until something is typed (kept for the next `poll`); at once if something is kept.
    pub async fn wait_input<W: Wire>(&mut self, wire: &mut W) {
        if self.taken >= self.stashed {
            self.stashed = wire.read_async(&mut self.stash).await;
            self.taken = 0;
        }
    }

    /// One character, echoed: is the line complete?
    fn take<W: Wire>(&mut self, c: u8, wire: &mut W) -> bool {
        if c == b'\n' {
            if self.role == Role::Room {
                wire.write_all(&[c]);
            }
            return false;
        }
        if let Some(slot) = self.line.get_mut(self.len) {
            *slot = c;
            self.len += 1;
        }
        wire.write_all(&[c]);
        c == b'\r' || self.len >= self.line_max() - 1
    }

    /// Reads what came, runs a complete line.
    pub fn poll<W: Wire>(
        &mut self,
        wire: &mut W,
        r: &mut Server,
        node: &mut Node,
        now: mc_core::Instant,
        rng: &mut impl Rng,
    ) {
        let mut done = false;
        while !done {
            if self.taken >= self.stashed {
                if !wire.read_ready() {
                    break;
                }
                self.stashed = wire.read(&mut self.stash).min(self.stash.len());
                self.taken = 0;
                if self.stashed == 0 {
                    break;
                }
            }
            let c = self.stash.get(self.taken).copied().unwrap_or(b'\r');
            self.taken += 1;
            done = self.take(c, wire);
        }
        if !done {
            return;
        }
        // The line without its '\r'.
        let end = self.len.saturating_sub(usize::from(
            self.line.get(self.len.wrapping_sub(1)) == Some(&b'\r'),
        ));
        self.len = 0;
        if self.role == Role::Repeater {
            wire.write_all(b"\n");
        }
        let mut reply = mc_node::cli::Reply::new();
        let line = self.line;
        r.command(
            node,
            0,
            line.get(..end).unwrap_or(&[]),
            &mut reply,
            &mut WireOut(wire),
            now,
            rng,
        );
        if !reply.is_empty() {
            wire.write_all(b"  -> ");
            wire.write_all(reply.as_bytes());
            wire.write_all(b"\r\n");
        }
    }
}

/// A server's identity: the stored one; at first boot the C++ firmware's (a repeater's with
/// its settings, access list and regions; a companion's identity and node settings), or a new
/// one. A store that cannot be read is left alone (panic).
pub fn identity<'a, 'd: 'a>(
    role: mc_server::Role,
    store: &mut Store<'_, '_>,
    cpp: impl FnOnce() -> Option<Spiffs<ReadOnly<'a, 'd>>>,
    hw: &esp_hal::rng::Rng,
) -> LocalIdentity {
    match store.identity() {
        Ok(Some(prv)) => LocalIdentity::from_private(&prv),
        Err(_) => panic!("mcdata unreadable: the identity is left alone"),
        Ok(None) => {
            let imported = cpp().and_then(|mut fs| match repeater::import(&mut fs, role, store) {
                Ok(s) => {
                    info!(
                        "import: C++ node, {} clients, {} regions",
                        s.clients, s.regions
                    );
                    store.identity().ok().flatten()
                }
                Err(e) => {
                    info!("import: nothing from the C++ firmware ({e:?})");
                    None
                }
            });
            if let Some(prv) = imported {
                return LocalIdentity::from_private(&prv);
            }
            let id = loop {
                let mut seed = [0u8; 32];
                for c in seed.chunks_exact_mut(4) {
                    c.copy_from_slice(&hw.random().to_le_bytes());
                }
                let id = LocalIdentity::from_seed(&seed);
                if !id.is_reserved() {
                    break id;
                }
            };
            mc_node::Storage::save_identity(store, id.private_key(), id.pub_key());
            id
        }
    }
}
