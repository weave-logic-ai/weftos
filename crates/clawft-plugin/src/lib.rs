//! Plugin trait definitions for clawft.
//!
//! This crate defines the unified plugin architecture for the clawft AI
//! assistant framework. It provides trait definitions for tools, channels,
//! pipeline stages, skills, memory backends, and voice handlers -- the
//! six core extension points that all downstream feature work depends on.
//!
//! # Trait Overview
//!
//! | Trait | Purpose |
//! |-------|---------|
//! | [`Tool`] | Tool execution interface for agent capabilities |
//! | [`ChannelAdapter`] | Channel message handling for external platforms |
//! | [`PipelineStage`] | Processing stage in the agent pipeline |
//! | [`Skill`] | High-level agent capability with tools and instructions |
//! | [`MemoryBackend`] | Pluggable memory storage backend |
//! | [`VoiceHandler`] | Voice/audio processing (placeholder for Workstream G) |
//!
//! # Supporting Traits
//!
//! | Trait | Purpose |
//! |-------|---------|
//! | [`KeyValueStore`] | Key-value storage exposed to plugins via `ToolContext` |
//! | [`ToolContext`] | Execution context passed to tool/skill invocations |
//! | [`ChannelAdapterHost`] | Host services for channel adapters |
//!
//! # Plugin Manifest
//!
//! Plugins declare their capabilities, permissions, and resource limits
//! through a [`PluginManifest`], typically parsed from a JSON file
//! (`clawft.plugin.json`).
//!
//! # Feature Flags
//!
//! - `voice` -- Voice umbrella. Pulls in `voice-vad`, `voice-wake`,
//!   `voice-stt`, and `voice-tts` so `cargo build --features voice`
//!   compiles the full in-process pipeline scaffold (WEFT-212).
//! - `voice-vad` -- Voice Activity Detection (Silero VAD stub).
//! - `voice-stt` -- Speech-to-Text (sherpa-rs stub).
//! - `voice-tts` -- Text-to-Speech (sherpa-rs stub).
//! - `voice-wake` -- Wake-word detection (reserved).
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

pub mod error;
pub mod manifest;
pub mod message;
pub mod sandbox;
pub mod skill_grants;
pub mod traits;

#[cfg(feature = "voice")]
pub mod voice;

// Re-export core types at crate root for convenience.
pub use error::{PluginError, SkillLoadError, WasmHostError};
pub use manifest::{
    validate_voice_capability, PermissionDiff, PluginCapability, PluginManifest,
    PluginPermissions, PluginResourceConfig, VoiceCapability, VoiceGrants,
};
pub use skill_grants::validate_allowed_tools;
pub use message::MessagePayload;
pub use sandbox::{
    SandboxAuditEntry, SandboxPolicy, SandboxType,
    NetworkPolicy, FilesystemPolicy, ProcessPolicy, EnvPolicy,
};
pub use traits::{
    CancellationToken, ChannelAdapter, ChannelAdapterHost, KeyValueStore, MemoryBackend,
    PipelineStage, PipelineStageType, Skill, Tool, ToolContext, VoiceHandler,
};
