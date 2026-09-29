//! Open capability vocabulary and requirement matching for governed
//! workload placement (ADR-099 section 2; card mesh-placement-02).
//!
//! - [`capability`]: [`Capability`] records a node advertises: a dotted,
//!   open id, typed attributes, provenance (`claimed` < `probed` <
//!   `measured`), state and an `exclusive` flag.
//! - [`requirement`]: [`Requirement`]s (exact id or prefix, attribute
//!   predicates `eq`/`gte`/`lte`/`in`/`has`, count, exclusive, minimum
//!   provenance) and matching. Unknown and `x.` ids match like any other.
//! - [`assign`]: whole-workload matching ([`match_all`]); exclusive
//!   requirements never share, independent of request order.
//! - [`engine`]: `WorkloadSpec` and the pure `place()` engine (card
//!   mesh-placement-04): hard constraints, scoring, pins, explain.
//! - [`memory`]: the `mem.unified` shared-pool accounting rule.
//! - [`perf`]: measured-performance capabilities (`perf.cog.cycle_ms`,
//!   `perf.infer.tok_s`).
//! - [`vocabulary`]: the advisory well-known vocabulary file
//!   (`config/capabilities.toml`): warnings and docs.
//! - [`vocabulary_pin`]: the governance pin (`config/capabilities.pin.toml`)
//!   that refuses free edits, and the change record a governed change
//!   carries (gate + chain event: `clawft_kernel::placement_vocabulary`).
//!
//! Pure data and logic: no probing, no hardware access, no I/O.

pub mod assign;
pub mod capability;
pub mod engine;
pub mod memory;
pub mod perf;
pub mod requirement;
pub mod vocabulary;
pub mod vocabulary_pin;

#[cfg(test)]
mod tests_assign;
#[cfg(test)]
mod tests_matching;
#[cfg(test)]
mod tests_vocab;

pub use assign::{Assignment, match_all};
pub use capability::{AttrValue, Capability, CapabilityId, CapabilityState, Provenance};
pub use memory::{MemoryDemand, MemoryLedger, MemoryPool, MemoryShortfall};
pub use requirement::{AttrPredicate, IdSelector, MatchFailure, PredicateOp, Requirement};
pub use vocabulary::{VocabWarning, Vocabulary, VocabularyError};
pub use vocabulary_pin::{
    VOCABULARY_CONFIG_KEY, VOCABULARY_CONFIG_NAMESPACE, VocabularyChange, VocabularyPin,
};

/// A capability, predicate or requirement failed boundary validation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlacementTypeError {
    /// Malformed id or prefix.
    #[error("invalid capability id {id:?}: {reason}")]
    InvalidId {
        /// Offending id (truncated).
        id: String,
        /// Why.
        reason: String,
    },
    /// Malformed attribute.
    #[error("invalid attribute {attr:?} on {id}: {reason}")]
    InvalidAttr {
        /// Capability id.
        id: String,
        /// Attribute name.
        attr: String,
        /// Why.
        reason: String,
    },
    /// Operand does not fit the operator.
    #[error("invalid predicate on {attr:?}: {reason}")]
    InvalidPredicate {
        /// Attribute name.
        attr: String,
        /// Why.
        reason: String,
    },
    /// Requirement-level problem (selector, count, size).
    #[error("invalid requirement: {0}")]
    InvalidRequirement(String),
}
