//! WeftOS machine mesh service building blocks (ADR-103, Phase 3).
//!
//! Package J provides the machine journal ([`journal`]) and the bindings
//! folded from it ([`bindings`]). The service itself (package S) builds on
//! these.
//!
//! This crate must not depend on `clawft-kernel`.

pub mod bindings;
pub mod journal;

pub use bindings::{BindError, BindHow, BindMeta, Bindings, Check, ConflictReason};
pub use journal::{Head, Journal, JournalError, JournalOptions, Record};
