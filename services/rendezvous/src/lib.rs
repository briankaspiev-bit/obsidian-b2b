//! Obsidian B2B rendezvous: how two DJ laptops find and reach each other.
//!
//! - [`server`]: room codes, public-address discovery, path arbitration and a
//!   blind UDP relay, all on one UDP port.
//! - [`client`]: create/join a room, hole-punch, and hand the engine a socket
//!   plus the address to send media to.
//! - [`proto`]: the control packet format shared by both.

pub mod client;
pub mod proto;
pub mod server;
