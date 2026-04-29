//! # clawft-core
//!
//! Core engine for the clawft AI assistant framework.
//!
//! Contains the agent loop, message bus, session management, tool registry,
//! context builder, memory store, and the 6-stage pipeline system.
//!
//! ## Crate Ecosystem
//!
//! WeftOS is built from these crates:
//!
//! | Crate | Role |
//! |-------|------|
//! | [`weftos`](https://crates.io/crates/weftos) | Product facade -- re-exports kernel, core, types |
//! | [`clawft-kernel`](https://crates.io/crates/clawft-kernel) | Kernel: processes, services, governance, mesh, ExoChain |
//! | [`clawft-core`](https://crates.io/crates/clawft-core) | Agent framework: pipeline, context, tools, skills |
//! | [`clawft-types`](https://crates.io/crates/clawft-types) | Shared type definitions |
//! | [`clawft-platform`](https://crates.io/crates/clawft-platform) | Platform abstraction (native/WASM/browser) |
//! | [`clawft-plugin`](https://crates.io/crates/clawft-plugin) | Plugin SDK for tools, channels, and extensions |
//! | [`clawft-llm`](https://crates.io/crates/clawft-llm) | LLM provider abstraction (11 providers + local) |
//! | [`exo-resource-tree`](https://crates.io/crates/exo-resource-tree) | Hierarchical resource namespace with Merkle integrity |
//!
//! Source: <https://github.com/weave-logic-ai/weftos>

pub mod agent;
#[cfg(feature = "native")]
pub mod agent_bus;
pub mod chain_event;
pub mod agent_routing;
pub mod bootstrap;
pub mod bus;
pub mod clawft_md;
pub mod config_merge;
pub mod json_repair;
pub mod pipeline;
// `planning` uses `tokio::time::{Instant, timeout}` directly. Until those
// callsites get a runtime abstraction, the module only compiles for native
// builds. Browser builds skip it (no consumers cross-crate; see audit
// 2026-04-28 in `.planning/reviews/0.7.0-release-gate/16-browser-wasm.md`).
#[cfg(feature = "native")]
pub mod planning;
pub mod routing_validation;
pub mod runtime;
pub mod security;
pub mod session;
pub mod tools;
pub mod workspace;

#[cfg(feature = "vector-memory")]
pub mod embeddings;
#[cfg(feature = "vector-memory")]
pub mod intelligent_router;
#[cfg(feature = "vector-memory")]
pub mod policy_kernel;
#[cfg(feature = "vector-memory")]
pub mod session_indexer;
#[cfg(feature = "vector-memory")]
pub mod vector_store;

#[cfg(feature = "rvf")]
pub mod complexity;
#[cfg(feature = "rvf")]
pub mod memory_bootstrap;
#[cfg(feature = "rvf")]
pub mod scoring;
