//! # clawft-types
//!
//! Core type definitions for the clawft AI assistant framework.
//!
//! This crate is the foundation of the dependency graph -- all other
//! clawft crates depend on it. It contains:
//!
//! - **[`error`]** -- [`ClawftError`] and [`ChannelError`] error types
//! - **[`config`]** -- Configuration schema (ported from Python `schema.py`)
//! - **[`event`]** -- Inbound/outbound message events
//! - **[`provider`]** -- LLM response types and the 15-provider registry
//! - **[`session`]** -- Conversation session state
//! - **[`cron`]** -- Scheduled job types
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

pub mod agent_bus;
pub mod agent_chat;
pub mod agent_routing;
pub mod canvas;
pub mod company;
pub mod config;
pub mod cron;
pub mod delegation;
pub mod error;
pub mod event;
pub mod goal;
pub mod provider;
pub mod registry;
pub mod routing;
pub mod secret;
pub mod security;
pub mod session;
pub mod skill;
pub mod workspace;

pub use error::{ChannelError, ClawftError, Result};
pub use registry::Registry;
