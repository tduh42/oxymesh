//! Keep-alive connections to servers (`BaseChatMesh.h:43-53`, `BaseChatMesh.cpp:717-832`,
//! MeshCore, MIT; `docs/spec/companion.md` §6.2). A server whose login reply asks for keep-alive
//! (byte 5 × 16 s) gets a slot; `HAS_CONNECTION` reads the table, `LOGOUT` empties the slot,
//! messages from the server and a keep-alive ACK refresh it.
//!
//! As in v1.18, the table is never pinged: the companion's `checkConnections()` call is
//! commented out (`MyMesh.cpp:2410-2411`), and v1.18 servers always answer 0 in byte 5, so the
//! table stays empty against them.

use mc_air::crypto::PUB_KEY_LEN;

/// `MAX_CONNECTIONS`.
pub const MAX: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Slot {
    key: [u8; PUB_KEY_LEN],
    keep_alive_ms: u32,
    /// Our clock, seconds.
    last_activity: u32,
    expected_ack: u32,
}

#[derive(Debug, Clone, Default)]
pub struct Connections {
    slots: [Option<Slot>; MAX],
}

impl Connections {
    pub const fn new() -> Self {
        Self { slots: [None; MAX] }
    }

    /// `startConnection`: the server's slot, or a free one (the last free one, like the C++'s
    /// scan). False when the table is full.
    pub fn start(&mut self, key: &[u8; PUB_KEY_LEN], keep_alive_secs: u16, clock: u32) -> bool {
        let mut use_idx = None;
        for (i, s) in self.slots.iter().enumerate() {
            match s {
                None => use_idx = Some(i),
                Some(s) if s.key == *key => {
                    use_idx = Some(i);
                    break;
                }
                Some(_) => {}
            }
        }
        let Some(slot) = use_idx.and_then(|i| self.slots.get_mut(i)) else {
            return false;
        };
        *slot = Some(Slot {
            key: *key,
            keep_alive_ms: u32::from(keep_alive_secs) * 1000,
            last_activity: clock,
            expected_ack: 0,
        });
        true
    }

    /// `stopConnection` (`LOGOUT`).
    pub fn stop(&mut self, key: &[u8]) {
        if let Some(s) = self
            .slots
            .iter_mut()
            .find(|s| s.is_some_and(|s| key.get(..PUB_KEY_LEN) == Some(&s.key[..])))
        {
            *s = None;
        }
    }

    /// `hasConnectionTo` (`HAS_CONNECTION`).
    pub fn has(&self, key: &[u8]) -> bool {
        self.slots
            .iter()
            .flatten()
            .any(|s| key.get(..PUB_KEY_LEN) == Some(&s.key[..]))
    }

    /// `markConnectionActive`: something came from this server.
    pub fn mark_active(&mut self, key: &[u8; PUB_KEY_LEN], clock: u32) {
        if let Some(s) = self.slots.iter_mut().flatten().find(|s| s.key == *key) {
            s.last_activity = clock;
        }
    }

    /// `checkConnectionsAck`: an ACK nobody else expected, matched against keep-alive requests
    /// (none are sent, so only an all-zero ACK matches, as in the C++). The server's key.
    pub fn check_ack(&mut self, crc: u32, clock: u32) -> Option<[u8; PUB_KEY_LEN]> {
        let s = self
            .slots
            .iter_mut()
            .flatten()
            .find(|s| s.expected_ack == crc)?;
        s.expected_ack = 0;
        s.last_activity = clock;
        Some(s.key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table() {
        let mut c = Connections::new();
        let (a, b) = ([1u8; 32], [2u8; 32]);
        assert!(!c.has(&a));
        assert!(c.start(&a, 16 * 4, 100));
        assert!(c.has(&a) && !c.has(&b));
        // The same server keeps its slot.
        assert!(c.start(&a, 16, 200));
        assert_eq!(c.slots.iter().flatten().count(), 1);
        for i in 0..15u8 {
            assert!(c.start(&[10 + i; 32], 16, 0));
        }
        assert!(!c.start(&b, 16, 0), "full");
        c.stop(&a);
        assert!(!c.has(&a));
        assert!(c.start(&b, 16, 0));
        assert_eq!(c.check_ack(0x1234_5678, 5), None);
        // Slots fill from the end (the C++ keeps the last free one it saw); the scan for an ACK
        // starts at slot 0, which the last server added took.
        assert_eq!(c.check_ack(0, 5), Some([24; 32]));
    }
}
