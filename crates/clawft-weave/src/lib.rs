//! `clawft-weave` — shared library surface for the `weaver` CLI.
//!
//! The CLI binary lives in [`main.rs`](../../../src/main.rs). This lib
//! target re-exports the modules used by integration tests (notably
//! [`daemon::handle_connection`] for driving a kernel listener from a
//! temp socket). Keeping these modules public under `pub` rather than
//! `pub(crate)` is the minimum change required to make integration
//! tests link against them.

/// `app.*` daemon RPC handlers (mesh-placement-06).
pub mod app_rpc;
pub mod capability;
/// Tracing → pending-buffer bridge for ExoChain (WEFT-597).
pub mod chain_bridge;
pub mod client;
pub mod commands;
pub mod control;
pub mod conv_postmortem;
// Local RPC daemon: Unix UDS + Windows named pipes (WEFT-559).
#[cfg(any(unix, windows))]
pub mod daemon;
#[cfg(any(unix, windows))]
pub mod llm_service;
/// Live MCP registry RPC handlers (WEFT-494 / ADR-070). Available on all
/// platforms so unit tests can exercise add/list/remove without UDS.
pub mod mcp_rpc;
pub mod node_identity;
/// Node facts probe, signing, cache and `cluster.facts` (mesh-placement-03).
#[cfg(any(feature = "mesh", feature = "exochain"))]
pub mod node_facts_rpc;
pub mod protocol;
/// Governance-gate helper for daemon RPC families (mesh-placement-06).
pub mod rpc_gate;
// WEFT-720 residual: `spatial_rpc` dropped when BvhStore/CLI helpers
// diverged; reattach via SpatialService before re-enabling spatial_cli_e2e.
#[cfg(feature = "rvf-rpc")]
pub mod rvf_codec;
#[cfg(feature = "rvf-rpc")]
pub mod rvf_rpc;
#[cfg(feature = "exochain")]
pub mod turn_ledger;
pub mod voice_loop;
pub mod voice_router;
pub mod voice_trace;
/// Node-local workload catalog (ADR-099, mesh-placement-06).
pub mod workload_registry;
/// `workload.*` daemon RPC family (ADR-099, mesh-placement-06).
#[cfg(feature = "exochain")]
pub mod workload_rpc;
/// Placement control plane RPCs (ADR-099, mesh-placement-12).
#[cfg(all(feature = "placement", unix))]
pub mod workload_place_rpc;
