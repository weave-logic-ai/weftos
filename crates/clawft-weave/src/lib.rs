//! `clawft-weave` — shared library surface for the `weaver` CLI.
//!
//! The CLI binary lives in [`main.rs`](../../../src/main.rs). This lib
//! target re-exports the modules used by integration tests (notably
//! [`daemon::handle_connection`] for driving a kernel listener from a
//! temp socket). Keeping these modules public under `pub` rather than
//! `pub(crate)` is the minimum change required to make integration
//! tests link against them.

pub mod client;
pub mod commands;
pub mod control;
#[cfg(unix)]
pub mod daemon;
pub mod node_identity;
pub mod protocol;
pub mod voice_router;
#[cfg(feature = "rvf-rpc")]
pub mod rvf_codec;
#[cfg(feature = "rvf-rpc")]
pub mod rvf_rpc;
