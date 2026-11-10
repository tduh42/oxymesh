//! How a server's replies travel (`src/helpers/RoutingPolicy.h`, `MyMesh::sendFloodReply`,
//! MeshCore, MIT): by the route the request came, and for a flood, within the request's scope.

use mc_air::{PacketBuf, PathLen, PayloadType, crypto};
use mc_core::Instant;
use mc_node::Node;
use mc_node::region::RegionMap;

/// The scope a flooded reply goes out with (`chooseReplyScope`): the request's region if it
/// has a key; nothing for a request flooded unscoped; otherwise the default region's.
pub fn scope(node: &Node, region: Option<u16>) -> Option<[u8; 16]> {
    let routing = node.routing.as_ref()?;
    let regions = &routing.regions;
    let request_key = region
        .filter(|&id| id != 0)
        .and_then(|id| regions.get(id))
        .and_then(|r| RegionMap::key(&r));
    if request_key.is_some() {
        return request_key;
    }
    if region == Some(0) {
        return None;
    }
    default_scope(regions)
}

/// The default region's key (this node's own floods).
pub fn default_scope(regions: &RegionMap) -> Option<[u8; 16]> {
    regions.default_region().and_then(|r| RegionMap::key(&r))
}

/// `sendFloodScoped`: within `key` if any, with hashes of `hash_size` bytes.
pub fn flood_scoped(
    node: &mut Node,
    pkt: PacketBuf,
    key: Option<[u8; 16]>,
    delay_ms: u64,
    hash_size: u8,
    now: Instant,
) {
    let transport = key.map(|k| {
        [
            crypto::transport_code(&k, pkt.header.ptype.value(), pkt.payload()),
            0,
        ]
    });
    node.send_flood_sized(pkt, transport, delay_ms, now, hash_size);
}

/// `sendFloodReply`: a flooded reply within the request's scope (`region`), hashes the size of
/// the request's.
pub fn flood_reply(
    node: &mut Node,
    pkt: PacketBuf,
    region: Option<u16>,
    in_path_len: u8,
    delay_ms: u64,
    now: Instant,
) {
    let key = scope(node, region);
    let size = PathLen::from_byte(in_path_len).map_or(1, |p| p.hash_size());
    flood_scoped(node, pkt, key, delay_ms, size, now);
}

/// Where a reply goes (`chooseReplyRoute`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route<'a> {
    /// The request came by flood: a PATH back, flooded, carrying the reply.
    PathReturn,
    /// Direct, along a path the request supplied.
    Supplied(u8, &'a [u8]),
    /// Direct, along the client's stored path.
    OutPath(u8, &'a [u8]),
    /// No path: flooded.
    Flood,
}

/// The request as it arrived.
#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    pub key: &'a [u8; 32],
    pub flood: bool,
    pub in_path_len: u8,
    pub in_path: &'a [u8],
    pub region: Option<u16>,
}

/// Sends `data` (a RESPONSE, or a TXT_MSG) to the requester by `route`, `delay_ms` from now.
#[allow(clippy::too_many_arguments)]
pub fn send(
    node: &mut Node,
    req: &Request<'_>,
    route: Route<'_>,
    ptype: PayloadType,
    data: &[u8],
    delay_ms: u64,
    rnd4: [u8; 4],
    now: Instant,
) {
    let Some(secret) = node.id.shared_secret(req.key) else {
        return;
    };
    let dest = req.key[0];
    let src = node.id.hash();
    match route {
        Route::PathReturn => {
            let Ok(pl) = PathLen::from_byte(req.in_path_len) else {
                return;
            };
            if let Ok(p) = mc_air::payload::path_return(
                dest,
                src,
                &secret,
                pl,
                req.in_path,
                Some((ptype.value(), data)),
                rnd4,
            ) {
                flood_reply(node, p, req.region, req.in_path_len, delay_ms, now);
            }
        }
        Route::Supplied(len, path) | Route::OutPath(len, path) => {
            if let Ok(p) = mc_air::payload::datagram(ptype, dest, src, &secret, data) {
                node.send_direct(p, len, path, delay_ms, now);
            }
        }
        Route::Flood => {
            if let Ok(p) = mc_air::payload::datagram(ptype, dest, src, &secret, data) {
                flood_reply(node, p, req.region, req.in_path_len, delay_ms, now);
            }
        }
    }
}
