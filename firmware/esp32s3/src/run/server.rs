//! What the server binaries share (`docs/spec/server.md`): the text console on the serial
//! wire, as the C++ repeater's `main.cpp` runs it, and the boot identity of a server (stored,
//! imported from a C++ repeater or companion at first boot, or new).

use core::fmt::Write;

use log::info;
use mc_air::crypto::LocalIdentity;
use mc_node::Node;
use mc_server::Repeater;
use mc_spiffs::{Spiffs, repeater};
use rand_core::Rng;

use super::boot::Store;
use super::link::Wire;
use crate::partition::ReadOnly;

/// `char command[160]`.
const LINE_MAX: usize = 160;

/// Text out on the wire.
pub struct WireOut<'a, W: Wire>(pub &'a mut W);

impl<W: Wire> Write for WireOut<'_, W> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.0.write_all(s.as_bytes());
        Ok(())
    }
}

/// The serial console (`main.cpp:155-190`): characters are echoed, `\n` dropped, a line ends
/// at `\r` (or when 159 characters are waiting); the reply comes as `  -> {reply}`.
pub struct Console {
    line: [u8; LINE_MAX],
    len: usize,
}

impl Default for Console {
    fn default() -> Self {
        Self::new()
    }
}

impl Console {
    pub const fn new() -> Self {
        Self {
            line: [0; LINE_MAX],
            len: 0,
        }
    }

    /// `Repeater ID: {key hex}`, at boot.
    pub fn banner<W: Wire>(wire: &mut W, node: &Node) {
        let mut out = WireOut(wire);
        let _ = out.write_str("Repeater ID: ");
        for b in node.id.pub_key() {
            let _ = write!(out, "{b:02X}");
        }
        let _ = out.write_str("\r\n");
    }

    /// Reads what came, runs a complete line.
    pub fn poll<W: Wire>(
        &mut self,
        wire: &mut W,
        r: &mut Repeater,
        node: &mut Node,
        now: mc_core::Instant,
        rng: &mut impl Rng,
    ) {
        let mut buf = [0u8; 32];
        let mut done = false;
        while !done && wire.read_ready() {
            let n = wire.read(&mut buf);
            if n == 0 {
                break;
            }
            for &c in buf.get(..n).unwrap_or(&[]) {
                if c == b'\n' {
                    continue;
                }
                if let Some(slot) = self.line.get_mut(self.len) {
                    *slot = c;
                    self.len += 1;
                }
                wire.write_all(&[c]);
                if c == b'\r' || self.len >= LINE_MAX - 1 {
                    done = true;
                    break;
                }
            }
        }
        if !done {
            return;
        }
        // The line without its '\r'.
        let end = self.len.saturating_sub(usize::from(
            self.line.get(self.len.wrapping_sub(1)) == Some(&b'\r'),
        ));
        self.len = 0;
        wire.write_all(b"\n");
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
    store: &mut Store<'_, '_>,
    cpp: impl FnOnce() -> Option<Spiffs<ReadOnly<'a, 'd>>>,
    hw: &esp_hal::rng::Rng,
) -> LocalIdentity {
    match store.identity() {
        Ok(Some(prv)) => LocalIdentity::from_private(&prv),
        Err(()) => panic!("mcdata unreadable: the identity is left alone"),
        Ok(None) => {
            let imported = cpp().and_then(|mut fs| match repeater::import(&mut fs, store) {
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
