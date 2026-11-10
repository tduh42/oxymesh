//! The room server (`examples/simple_room_server/MyMesh.cpp`, MeshCore v1.18, MIT;
//! `docs/spec/server.md` §3): a bulletin board. Logged-in clients post plain text; every
//! post is pushed to every other client in turn, one at a time, until each acknowledges it.
//! Posts live in memory only, as in the C++.

use mc_air::PayloadType;
use mc_air::crypto::{PUB_KEY_LEN, sha256_trunc};
use mc_core::{Duration, Instant};
use mc_node::Node;
use mc_node::cli::Reply;
use mc_node::contacts::PATH_UNKNOWN;
use rand_core::Rng;

use crate::acl;
use crate::reply::{self, Request, Route};
use crate::server::{
    Discard, LOGIN_OK, REPLY_MAX, Server, c_str, ensure_contact, forget_contact, le32,
};

/// `MAX_UNSYNCED_POSTS`.
pub const MAX_POSTS: usize = 32;
/// Characters kept of a post (`StrHelper::strncpy` into `MAX_POST_TEXT_LEN` = 151).
pub const POST_TEXT_MAX: usize = 150;
/// `PUSH_NOTIFY_DELAY_MILLIS`: pushes wait this long after a post or a login.
const PUSH_NOTIFY_DELAY_MS: u64 = 2000;
/// `SYNC_PUSH_INTERVAL`: after a push; an eighth of it when a client had nothing new.
const SYNC_PUSH_INTERVAL_MS: u64 = 1200;
/// `PUSH_ACK_TIMEOUT_FLOOD`, `PUSH_TIMEOUT_BASE`, `PUSH_ACK_TIMEOUT_FACTOR`.
const PUSH_ACK_TIMEOUT_FLOOD_MS: u64 = 12_000;
const PUSH_TIMEOUT_BASE_MS: u64 = 4000;
const PUSH_ACK_TIMEOUT_FACTOR_MS: u64 = 2000;
/// `POST_SYNC_DELAY_SECS`: a post waits this long before it is pushed.
const POST_SYNC_DELAY_S: u32 = 6;
/// `REPLY_DELAY_MILLIS`: a console reply waits this long after the ACK.
const REPLY_DELAY_MS: u64 = 1500;
/// Pushes given up in a row before a client is skipped (until it is heard again).
const PUSH_FAILURES_MAX: u8 = 3;
const REQ_KEEP_ALIVE: u8 = 0x02;
/// `TXT_TYPE_SIGNED_PLAIN`: a pushed post, with its author's key prefix.
const TXT_SIGNED_PLAIN: u8 = 2;
/// `TXT_TYPE_CLI_DATA`, `TXT_TYPE_CLI_COMMAND`.
const TXT_CLI_DATA: u8 = 1;
const TXT_CLI_COMMAND: u8 = 3;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Post {
    pub author: [u8; PUB_KEY_LEN],
    pub text: heapless::String<POST_TEXT_MAX>,
    /// When it was stored (our clock, unique); 0 for an empty slot.
    pub ts: u32,
}

/// Posts (a ring of 32) and the push round.
#[derive(Debug)]
pub struct Room {
    pub posts: [Post; MAX_POSTS],
    next_post: usize,
    next_client: usize,
    pub(crate) next_push: Option<Instant>,
    /// Posts stored, pushes sent (`_num_posted`, `_num_post_pushes`).
    pub posted: u16,
    pub pushes: u16,
}

impl Default for Room {
    fn default() -> Self {
        Self::new()
    }
}

impl Room {
    pub fn new() -> Self {
        Self {
            posts: core::array::from_fn(|_| Post::default()),
            next_post: 0,
            next_client: 0,
            // `next_push = 0`: due at once.
            next_push: Some(Instant::ZERO),
            posted: 0,
            pushes: 0,
        }
    }

    /// The posts, oldest first (empty slots left out).
    pub fn iter(&self) -> impl Iterator<Item = &Post> {
        (0..MAX_POSTS)
            .filter_map(|k| self.posts.get((self.next_post + k) % MAX_POSTS))
            .filter(|p| p.ts != 0)
    }

    /// `getUnsyncedCount`: posts newer than the client's sync point, not its own.
    pub fn unsynced(&self, c: &acl::Client) -> u8 {
        self.posts
            .iter()
            .filter(|p| p.ts > c.sync_since && p.author != c.key)
            .count() as u8
    }
}

fn hash4(parts: &[&[u8]]) -> u32 {
    u32::from_le_bytes(sha256_trunc::<4>(parts))
}

impl Server {
    /// `storePost`: into the ring, pushed after `PUSH_NOTIFY_DELAY_MILLIS`.
    pub fn add_post(
        &mut self,
        node: &mut Node,
        author: &[u8; PUB_KEY_LEN],
        text: &str,
        now: Instant,
    ) {
        let mut t = heapless::String::new();
        for ch in text.chars() {
            if t.push(ch).is_err() {
                break;
            }
        }
        let ts = node.rtc.unique(now);
        let r = &mut self.room;
        if let Some(slot) = r.posts.get_mut(r.next_post) {
            *slot = Post {
                author: *author,
                text: t,
                ts,
            };
        }
        r.next_post = (r.next_post + 1) % MAX_POSTS;
        r.next_push = Some(now + Duration::from_millis(PUSH_NOTIFY_DELAY_MS));
        r.posted = r.posted.wrapping_add(1);
    }

    /// No more waiting for this client's ACK.
    fn clear_pending(&mut self, node: &mut Node, i: usize) {
        if let Some(c) = self.acl.clients.get_mut(i) {
            if c.pending_ack != 0
                && let Some(k) = node.server_acks.iter().position(|&a| a == c.pending_ack)
            {
                node.server_acks.swap_remove(k);
            }
            c.pending_ack = 0;
        }
    }

    /// The room's login (`onAnonDataRecv`): `{sync since}{password}` after the timestamp. The
    /// room password gives read-write, the admin password admin; with `allow.read.only`, any
    /// other password logs in as a guest (read only).
    pub(crate) fn room_login(
        &mut self,
        node: &mut Node,
        req: &Request<'_>,
        ts: u32,
        data: &[u8],
        now: Instant,
        rng: &mut impl Rng,
    ) {
        let sync_since = le32(data, 0);
        let password = c_str(data.get(4..).unwrap_or(&[]));
        let clock = node.rtc.now(now);
        let mut idx = if password.is_empty() {
            self.acl.get(req.key)
        } else {
            None
        };
        if idx.is_none() {
            let perms = if password == self.prefs.admin_password.as_bytes() {
                acl::ADMIN
            } else if password == self.prefs.guest_password.as_bytes() {
                acl::READ_WRITE
            } else if self.prefs.allow_read_only {
                acl::GUEST
            } else {
                return;
            };
            let put = self.acl.put(req.key, 0);
            if let acl::Put::Evicted(_, gone) = put {
                forget_contact(node, &gone);
            }
            let i = put.index();
            if self
                .acl
                .clients
                .get(i)
                .is_none_or(|c| ts <= c.last_timestamp)
            {
                return; // replay, or a full table
            }
            self.clear_pending(node, i);
            if let Some(c) = self.acl.clients.get_mut(i) {
                c.last_timestamp = ts;
                c.sync_since = sync_since;
                c.push_failures = 0;
                c.last_activity = clock;
                c.permissions = (c.permissions & !acl::ROLE_MASK) | perms;
            }
            ensure_contact(node, req.key);
            self.save_acl_soon(now);
            idx = Some(i);
        }
        let Some(c) = idx.and_then(|i| self.acl.clients.get(i)).copied() else {
            return;
        };
        if req.flood
            && let Some(ct) = node
                .contacts
                .find(req.key)
                .and_then(|s| node.contacts.get_mut(s))
        {
            ct.out_path_len = PATH_UNKNOWN;
        }
        let mut out: heapless::Vec<u8, REPLY_MAX> = heapless::Vec::new();
        let _ = out.extend_from_slice(&node.rtc.unique(now).to_le_bytes());
        let _ = out.push(LOGIN_OK);
        let _ = out.push(self.legacy_keep_alive);
        let _ = out.push(if c.is_admin() {
            1
        } else if c.permissions == 0 {
            2
        } else {
            0
        });
        let _ = out.push(c.permissions);
        let _ = out.extend_from_slice(&rng.next_u32().to_le_bytes());
        let _ = out.push(self.role.fw_level());
        // The pushes wait: the login reply goes first.
        self.room.next_push = Some(now + Duration::from_millis(PUSH_NOTIFY_DELAY_MS));
        let path = self.client_path(node, req.key);
        let route = match &path {
            _ if req.flood => Route::PathReturn,
            Some((len, p)) => Route::OutPath(*len, p),
            None => Route::Flood,
        };
        let rnd = rng.next_u32().to_le_bytes();
        reply::send(
            node,
            req,
            route,
            PayloadType::RESPONSE,
            &out,
            crate::RESPONSE_DELAY_MS,
            rnd,
            now,
        );
    }

    /// The room's `onPeerDataRecv`: a post or a console command (TXT_MSG), a keep-alive or a
    /// request (REQ).
    pub(crate) fn room_peer(
        &mut self,
        node: &mut Node,
        req: &Request<'_>,
        ptype: PayloadType,
        data: &[u8],
        now: Instant,
        rng: &mut impl Rng,
    ) {
        let Some(i) = self.acl.get(req.key) else {
            return;
        };
        let Some(client) = self.acl.clients.get(i).copied() else {
            return;
        };
        let ts = le32(data, 0);
        let path = self.client_path(node, req.key);
        if ptype == PayloadType::TXT_MSG && data.len() > 5 {
            let flags = data.get(4).copied().unwrap_or(0) >> 2;
            if !matches!(flags, 0 | TXT_CLI_DATA | TXT_CLI_COMMAND) || ts < client.last_timestamp {
                return;
            }
            let retry = ts == client.last_timestamp;
            let unique = node.rtc.unique(now);
            if let Some(c) = self.acl.clients.get_mut(i) {
                c.last_timestamp = ts;
                c.last_activity = unique;
                c.push_failures = 0;
            }
            let text = c_str(data.get(5..).unwrap_or(&[]));
            let head = data.get(..5).unwrap_or(&[]);
            let ack = sha256_trunc::<4>(&[head, text, req.key]);
            let mut answer = Reply::new();
            let mut send_ack = false;
            if flags == TXT_CLI_DATA || flags == TXT_CLI_COMMAND {
                if client.is_admin() && !retry {
                    self.command(node, ts, text, &mut answer, &mut Discard, now, rng);
                }
            } else if client.role() != acl::GUEST {
                if !retry {
                    let s = core::str::from_utf8(text).unwrap_or("");
                    self.add_post(node, req.key, s, now);
                }
                send_ack = true;
            }
            let mut delay = 0;
            if send_ack {
                match &path {
                    None => {
                        if let Ok(p) = mc_air::payload::ack(&ack) {
                            reply::flood_reply(
                                node,
                                p,
                                req.region,
                                req.in_path_len,
                                crate::TXT_ACK_DELAY_MS,
                                now,
                            );
                        }
                        delay = crate::TXT_ACK_DELAY_MS + REPLY_DELAY_MS;
                    }
                    Some((len, pp)) => {
                        let mut d = crate::TXT_ACK_DELAY_MS;
                        if node.prefs.multi_acks > 0 {
                            if let Ok(p) = mc_air::payload::multi_ack(&ack, 1) {
                                node.send_direct(p, *len, pp, d, now);
                            }
                            d += 300;
                        }
                        if let Ok(p) = mc_air::payload::ack(&ack) {
                            node.send_direct(p, *len, pp, d, now);
                        }
                        delay = d + REPLY_DELAY_MS;
                    }
                }
            }
            if answer.is_empty() {
                return;
            }
            let stamp = if unique == ts {
                unique.wrapping_add(1)
            } else {
                unique
            };
            let mut out: heapless::Vec<u8, REPLY_MAX> = heapless::Vec::new();
            let _ = out.extend_from_slice(&stamp.to_le_bytes());
            let _ = out.push(TXT_CLI_DATA << 2);
            let _ = out.extend_from_slice(answer.as_bytes());
            let route = crate::server::direct_or_flood(&path);
            reply::send(
                node,
                req,
                route,
                PayloadType::TXT_MSG,
                &out,
                delay + crate::RESPONSE_DELAY_MS,
                [0; 4],
                now,
            );
        } else if ptype == PayloadType::REQ && data.len() >= 5 {
            if ts < client.last_timestamp {
                return; // replay
            }
            let clock = node.rtc.now(now);
            if let Some(c) = self.acl.clients.get_mut(i) {
                c.last_timestamp = ts;
                c.last_activity = clock;
                c.push_failures = 0;
            }
            if data.get(4) == Some(&REQ_KEEP_ALIVE) && !req.flood {
                // `{ts}{type}{last post time received}`: zeros where the request is shorter.
                let mut b = [0u8; 9];
                for (o, v) in b.iter_mut().zip(data) {
                    *o = *v;
                }
                let force_since = le32(&b, 5);
                self.clear_pending(node, i);
                if force_since > 0
                    && let Some(c) = self.acl.clients.get_mut(i)
                {
                    c.sync_since = force_since;
                }
                // Answered direct only: an ACK carrying how many posts wait.
                if let Some((len, pp)) = &path {
                    let crc = sha256_trunc::<4>(&[&b, req.key]);
                    let n = self.acl.clients.get(i).map_or(0, |c| self.room.unsynced(c));
                    let payload = [crc[0], crc[1], crc[2], crc[3], n];
                    if let Ok(p) = mc_air::payload::ack(&payload) {
                        node.send_direct(p, *len, pp, crate::RESPONSE_DELAY_MS, now);
                    }
                }
                return;
            }
            let mut out: heapless::Vec<u8, REPLY_MAX> = heapless::Vec::new();
            let _ = out.extend_from_slice(&ts.to_le_bytes());
            if !self.request(node, &client, data.get(4..).unwrap_or(&[]), now, &mut out) {
                return;
            }
            let route = match &path {
                _ if req.flood => Route::PathReturn,
                Some((len, p)) => Route::OutPath(*len, p),
                None => Route::Flood,
            };
            let rnd = rng.next_u32().to_le_bytes();
            reply::send(
                node,
                req,
                route,
                PayloadType::RESPONSE,
                &out,
                crate::RESPONSE_DELAY_MS,
                rnd,
                now,
            );
        }
    }

    /// `processAck`: the client's post arrived; its sync point moves on.
    pub(crate) fn room_ack(&mut self, crc: u32) {
        if crc == 0 {
            return;
        }
        if let Some(c) = self.acl.clients.iter_mut().find(|c| c.pending_ack == crc) {
            c.pending_ack = 0;
            c.push_failures = 0;
            c.sync_since = c.push_post_timestamp;
        }
    }

    /// The push round (`MyMesh::loop`): give up on ACKs that timed out, then the next client
    /// in turn gets its oldest new post, if any.
    pub(crate) fn room_tick(&mut self, node: &mut Node, now: Instant, rng: &mut impl Rng) {
        if self.room.next_push.is_none_or(|t| now < t) || self.acl.is_empty() {
            return;
        }
        for i in 0..self.acl.len() {
            let timed_out = self
                .acl
                .clients
                .get(i)
                .is_some_and(|c| c.pending_ack != 0 && now >= c.ack_timeout);
            if timed_out {
                self.clear_pending(node, i);
                if let Some(c) = self.acl.clients.get_mut(i) {
                    c.push_failures = c.push_failures.saturating_add(1);
                }
            }
        }
        let n = self.acl.len();
        let ci = self.room.next_client % n;
        let mut pushed = false;
        if let Some(c) = self.acl.clients.get(ci).copied()
            && c.pending_ack == 0
            && c.last_activity != 0
            && c.push_failures < PUSH_FAILURES_MAX
        {
            let clock = node.rtc.now(now);
            let pick = (0..MAX_POSTS)
                .map(|k| (self.room.next_post + k) % MAX_POSTS)
                .find(|&pi| {
                    self.room.posts.get(pi).is_some_and(|p| {
                        clock >= p.ts.saturating_add(POST_SYNC_DELAY_S)
                            && p.ts > c.sync_since
                            && p.author != c.key
                    })
                });
            if let Some(pi) = pick {
                self.push_post(node, ci, pi, now, rng);
                pushed = true;
            }
        }
        self.room.next_client = (ci + 1) % n;
        let wait = if pushed {
            SYNC_PUSH_INTERVAL_MS
        } else {
            SYNC_PUSH_INTERVAL_MS / 8
        };
        self.room.next_push = Some(now + Duration::from_millis(wait));
    }

    /// `pushPostToClient`: the post as signed plain text (`{time}{flags}{author[0..4]}{text}`),
    /// direct if the client's path is known, otherwise flooded in the default scope; its ACK
    /// is then awaited.
    fn push_post(
        &mut self,
        node: &mut Node,
        ci: usize,
        pi: usize,
        now: Instant,
        rng: &mut impl Rng,
    ) {
        let (Some(c), Some(post)) = (
            self.acl.clients.get(ci).copied(),
            self.room.posts.get(pi).cloned(),
        ) else {
            return;
        };
        let attempt = (rng.next_u32() & 3) as u8;
        let mut out: heapless::Vec<u8, REPLY_MAX> = heapless::Vec::new();
        let _ = out.extend_from_slice(&post.ts.to_le_bytes());
        let _ = out.push((TXT_SIGNED_PLAIN << 2) | attempt);
        let _ = out.extend_from_slice(post.author.get(..4).unwrap_or(&[]));
        let _ = out.extend_from_slice(post.text.as_bytes());
        let ack = hash4(&[&out, &c.key]);
        let Some(secret) = node.id.shared_secret(&c.key) else {
            return;
        };
        let Ok(p) = mc_air::payload::datagram(
            PayloadType::TXT_MSG,
            c.key[0],
            node.id.hash(),
            &secret,
            &out,
        ) else {
            return;
        };
        let timeout = match self.client_path(node, &c.key) {
            Some((len, path)) => {
                node.send_direct(p, len, &path, 0, now);
                let hops = u64::from(len & 63);
                PUSH_TIMEOUT_BASE_MS + PUSH_ACK_TIMEOUT_FACTOR_MS * (hops + 1)
            }
            None => {
                let key = node
                    .routing
                    .as_ref()
                    .and_then(|r| reply::default_scope(&r.regions));
                let size = node.prefs.path_hash_size();
                reply::flood_scoped(node, p, key, 0, size, now);
                PUSH_ACK_TIMEOUT_FLOOD_MS
            }
        };
        if node.server_acks.is_full() {
            node.server_acks.remove(0);
        }
        let _ = node.server_acks.push(ack);
        if let Some(cl) = self.acl.clients.get_mut(ci) {
            cl.pending_ack = ack;
            cl.push_post_timestamp = post.ts;
            cl.ack_timeout = now + Duration::from_millis(timeout);
        }
        self.room.pushes = self.room.pushes.wrapping_add(1);
    }
}
