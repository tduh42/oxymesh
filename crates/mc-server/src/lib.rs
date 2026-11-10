//! MeshCore's server roles on top of `mc-node` (`docs/spec/server.md`, ADR-0010): the repeater
//! (`examples/simple_repeater`) and, later, the room server, ported from MeshCore v1.18 (MIT).
//!
//! A server owns a [`mc_node::Node`] in server mode (`Node::routing` set): the node forwards by
//! the server's rules and hands over what is addressed to it ([`mc_node::Event::AnonRequest`],
//! `ServerPeer`, `ServerPath`, `AdvertHeard`, `ControlData`); the server answers. Clients that
//! logged in live in the node's contact table, so that their requests decrypt and their paths
//! are kept; the access list holds their permissions.
#![no_std]
#![deny(clippy::float_arithmetic)]
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )
)]

pub mod acl;
pub mod limiter;
pub mod neighbours;
pub mod repeater;
pub mod reply;

pub use repeater::Repeater;

/// `SERVER_RESPONSE_DELAY`, ms.
pub const RESPONSE_DELAY_MS: u64 = 300;
/// `TXT_ACK_DELAY`, ms.
pub const TXT_ACK_DELAY_MS: u64 = 200;
/// `CLI_REPLY_DELAY_MILLIS`, ms.
pub const CLI_REPLY_DELAY_MS: u64 = 600;
