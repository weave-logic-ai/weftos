//! Model manifest, adopt-in-place and weights locality (card
//! mesh-placement-17; ADR-101 section 3, ADR-099 section 6).
//!
//! - [`body`]: the `model` manifest body and the operator attestation over
//!   its hash manifest, reusing the workload package envelope, signing
//!   domain and [`crate::workload_pkg::TrustAnchors`].
//! - [`adopt`]: hash existing HF cache / MLX quant / GGUF / Ollama files
//!   where they lie. Nothing is copied.
//! - [`registry`]: adopted models, lazy re-hash, refusal on hash mismatch,
//!   `Degraded` on a detached store, and [`ModelRegistry::resolve`], the API
//!   inference-server adapters consume.
//! - [`advertise`]: `model.present` and `store.tier.external` capabilities
//!   for node facts, from existence and size checks only.
//! - [`locality`]: the pure fetch-versus-relocate decision with a transfer
//!   policy, and placer preferences / requirements.
//! - `sharing` (mesh feature): the only door from model weights into the
//!   artifact exchange; redistribution fails closed.
//!
//! Inference-server adapters (card mesh-placement-18) consume
//! [`ModelRegistry::resolve`]; none of that lives here.

pub mod adopt;
pub mod advertise;
pub mod body;
pub mod check;
pub mod locality;
pub mod registry;
#[cfg(feature = "mesh")]
pub mod sharing;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_locality;

pub use adopt::{AdoptInput, FileRole, ScannedFile, ScannedModel, hash_file, scan_dir, scan_file, scan_ollama};
pub use advertise::{StoreTier, TierResolver, model_capabilities};
pub use body::{
    KIND_MODEL, MAX_SHARDS, MODEL_MARKER_PREFIX, ModelError, ModelFormat, ModelPackageBody,
    ModelSource, VerifiedModel, attest, verify_model,
};
pub use locality::{
    LocalityDecision, LocalityPlan, NodeHolding, Sharing, TransferPolicy, Unplaceable, decide,
    locality_preference, model_present_requirement,
};
pub use check::{CheckMode, FileOutcome, ModelCheck, ModelState};
pub use registry::{AdoptedModel, ModelEntry, ModelRegistry, ResolvedModel};
