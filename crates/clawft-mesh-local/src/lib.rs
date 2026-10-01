//! mesh-local/1: the protocol between a per-user WeftOS daemon and the
//! machine mesh service (ADR-103, Phase 3 package L).
//!
//! Contents: wire messages and version negotiation ([`proto`]), the user
//! certificate ([`cert`]), `weft://` addresses ([`addr`]), peer credentials
//! ([`peer`]), bounded line framing ([`framing`]), a client ([`client`]) and a
//! loopback test server ([`testing`]).
//!
//! This crate must not depend on `clawft-kernel`.

pub mod addr;
pub mod cert;
pub mod client;
pub mod framing;
pub mod hexser;
pub mod peer;
pub mod proto;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use addr::{AddrError, Node, WeftAddr};
pub use cert::{node_id_from_pubkey, CertError, UserCert};
pub use client::{ClientConfig, ClientError, MeshLocalClient};
#[cfg(any(test, feature = "testing"))]
pub use peer::InjectedPeer;
pub use peer::{PeerCreds, PeerError, PeerIdentity, Principal};
pub use proto::{Frame, Message, PROTO_MAX, PROTO_MIN};
