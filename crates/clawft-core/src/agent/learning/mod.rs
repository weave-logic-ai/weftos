//! Agent learning subsystem (Phase 4).
//!
//! Homes periodic distillation and other self-improvement loops that bridge
//! short-lived conversation state with durable agent memory.
//!
//! ## Boundary
//!
//! - [`super::sink::ConversationSink`] owns per-turn logs (substrate /
//!   local JSONL / in-memory).
//! - [`super::memory::MemoryStore`] owns cross-conversation distilled facts
//!   (`MEMORY.md` / `HISTORY.md`).
//! - [`consolidator::MemoryConsolidator`] is the only component that reads
//!   the former and writes the latter. The two stores never share paths.

pub mod consolidator;
pub mod topics;

pub use consolidator::{
    ConsolidationConfig, ConsolidationOutcome, ConsolidationReport, DistilledMemory,
    MemoryConsolidator, distill_turns,
};
