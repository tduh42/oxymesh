//! The access list (`src/helpers/ClientACL.cpp`, MeshCore, MIT): up to 32 clients with their
//! permissions. Their paths and shared secrets live in the node's contact table.

use mc_air::crypto::PUB_KEY_LEN;
use mc_core::Instant;

/// `MAX_CLIENTS`.
pub const MAX: usize = 32;
/// `PERM_ACL_*`, in the low 2 bits.
pub const ROLE_MASK: u8 = 3;
pub const GUEST: u8 = 0;
pub const READ_ONLY: u8 = 1;
pub const READ_WRITE: u8 = 2;
pub const ADMIN: u8 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Client {
    pub key: [u8; PUB_KEY_LEN],
    pub permissions: u8,
    /// By their clock (replay protection); not kept across restarts.
    pub last_timestamp: u32,
    /// By our clock.
    pub last_activity: u32,
    /// Room server: posts already pushed up to this time (our clock).
    pub sync_since: u32,
    /// Room server: the ACK of the post being pushed (0: none), that post's time, when to
    /// give up on it, and the pushes given up in a row (`extra.room`).
    pub pending_ack: u32,
    pub push_post_timestamp: u32,
    pub ack_timeout: Instant,
    pub push_failures: u8,
}

impl Client {
    pub const fn new(key: [u8; PUB_KEY_LEN], permissions: u8) -> Self {
        Self {
            key,
            permissions,
            last_timestamp: 0,
            last_activity: 0,
            sync_since: 0,
            pending_ack: 0,
            push_post_timestamp: 0,
            ack_timeout: Instant::ZERO,
            push_failures: 0,
        }
    }

    pub const fn is_admin(&self) -> bool {
        self.permissions & ROLE_MASK == ADMIN
    }

    pub const fn role(&self) -> u8 {
        self.permissions & ROLE_MASK
    }
}

#[derive(Debug, Clone, Default)]
pub struct Acl {
    pub clients: heapless::Vec<Client, MAX>,
}

/// What `put` did to the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Put {
    Known(usize),
    Added(usize),
    /// The client at this index replaced an evicted one (its key).
    Evicted(usize, [u8; PUB_KEY_LEN]),
}

impl Put {
    pub fn index(self) -> usize {
        match self {
            Put::Known(i) | Put::Added(i) | Put::Evicted(i, _) => i,
        }
    }
}

impl Acl {
    pub const fn new() -> Self {
        Self {
            clients: heapless::Vec::new(),
        }
    }

    /// `getClient`: by key prefix (an empty one matches the first client, as `memcmp` of 0
    /// bytes does).
    pub fn get(&self, prefix: &[u8]) -> Option<usize> {
        self.clients.iter().position(|c| c.key.starts_with(prefix))
    }

    /// `putClient`: the client's entry, a new one, or, when full, the least recently active
    /// non-admin's (the last slot if all are admins) taken over.
    pub fn put(&mut self, key: &[u8; PUB_KEY_LEN], init_perms: u8) -> Put {
        if let Some(i) = self.clients.iter().position(|c| c.key == *key) {
            return Put::Known(i);
        }
        if !self.clients.is_full() {
            let _ = self.clients.push(Client::new(*key, init_perms));
            return Put::Added(self.clients.len() - 1);
        }
        let mut oldest = MAX - 1;
        let mut min = u32::MAX;
        for (i, c) in self.clients.iter().enumerate() {
            if !c.is_admin() && c.last_activity < min {
                oldest = i;
                min = c.last_activity;
            }
        }
        match self.clients.get_mut(oldest) {
            Some(c) => {
                let gone = c.key;
                *c = Client::new(*key, init_perms);
                Put::Evicted(oldest, gone)
            }
            None => Put::Known(0),
        }
    }

    /// `applyPermissions`: a guest role removes the entry (prefix allowed); any other role
    /// needs the whole key and adds or updates it. `None` if not done.
    pub fn apply(&mut self, prefix: &[u8], perms: u8) -> Option<Put> {
        if perms & ROLE_MASK == GUEST {
            let i = self.get(prefix)?;
            let gone = self.clients.remove(i);
            return Some(Put::Evicted(i, gone.key));
        }
        let key: [u8; PUB_KEY_LEN] = prefix.try_into().ok()?;
        let p = self.put(&key, 0);
        if let Some(c) = self.clients.get_mut(p.index()) {
            c.permissions = perms;
        }
        Some(p)
    }

    pub fn len(&self) -> usize {
        self.clients.len()
    }

    pub fn is_empty(&self) -> bool {
        self.clients.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eviction_spares_admins() {
        let mut a = Acl::new();
        for i in 0..MAX as u8 {
            let p = a.put(&[i; 32], if i == 0 { ADMIN } else { GUEST });
            a.clients[p.index()].last_activity = 100 + u32::from(i);
        }
        // Full: the least recently active non-admin (key 1) goes, not the admin (key 0).
        assert_eq!(a.put(&[200; 32], 0), Put::Evicted(1, [1; 32]));
        assert_eq!(a.put(&[0; 32], 0), Put::Known(0));
        assert_eq!(a.get(&[200, 200]), Some(1));
        // Guest role removes, by prefix.
        assert!(matches!(a.apply(&[200], GUEST), Some(Put::Evicted(1, _))));
        assert_eq!(a.len(), MAX - 1);
        assert!(a.apply(&[5, 5], ADMIN).is_none(), "a whole key is needed");
        assert!(a.apply(&[9; 32], ADMIN).is_some());
        assert!(a.clients[a.get(&[9]).unwrap()].is_admin());
    }
}
