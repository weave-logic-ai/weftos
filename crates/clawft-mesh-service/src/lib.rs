//! WeftOS machine mesh service building blocks (ADR-103, Phase 3).
//!
//! Package J provides the machine journal ([`journal`]) and the bindings
//! folded from it ([`bindings`]). The service itself (package S) builds on
//! these.
//!
//! This crate must not depend on `clawft-kernel`.

mod bind_events;
pub mod bindings;
mod fsutil;
mod lost;
pub mod journal;

pub use bindings::{BindError, BindHow, BindMeta, Bindings, Check, ConflictReason};
pub use journal::{AdminAck, Head, Journal, JournalError, JournalOptions, Record};
pub use lost::LostInfo;

#[cfg(test)]
mod tests_internal;
