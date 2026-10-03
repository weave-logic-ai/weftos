//! Inference-server adapters (card mesh-placement-18; ADR-101 section 4).
//!
//! - [`spec`]: [`InferenceSpec`] and `VerifiedWorkload::inference`.
//! - [`config`]: adopted or managed construction, restart backoff.
//! - [`probe`]: loopback health, model listing, tiny completion.
//! - [`capabilities`]: the probes behind `provides()`.
//! - [`runtime`] / [`lifecycle`]: [`InferRuntime`], the
//!   [`super::WorkloadRuntime`] for `infer.llamacpp`, `infer.mlx-lm` and
//!   `infer.ollama`, plus the API the stable-address proxy builds on
//!   ([`InferRuntime::endpoint`], [`InferRuntime::health`],
//!   [`InferRuntime::reconcile`]).
//! - [`launch`] / [`ollama`]: how a managed server is started or driven.
//!
//! Adopted mode never controls the server. Managed mode launches only a
//! configured launcher script, only on a port nothing else holds, with
//! weights resolved from the model registry, and stops only the process
//! it spawned.

pub mod capabilities;
pub mod config;
pub mod exposure;
pub mod launch;
pub mod lifecycle;
pub mod ollama;
pub mod probe;
pub mod residency;
pub mod roster;
pub mod runtime;
pub mod spec;

#[cfg(test)]
pub(crate) mod fakes;
#[cfg(test)]
mod tests_adopted;
#[cfg(test)]
mod tests_ledger;
#[cfg(test)]
mod tests_managed;
#[cfg(test)]
mod tests_ollama;
#[cfg(test)]
mod tests_roster;
#[cfg(test)]
mod tests_spec;

pub use config::{InferConfig, InferMode, ManagedConfig, RestartPolicy, lab_serve_program};
pub use lifecycle::Reconcile;
pub use probe::{Health, ServerClient, ServerReport};
pub use residency::{CoResidency, GB, ResidencyLedger};
pub use roster::{ImportedRoster, ImportedSpec, RosterOverlay, import_roster};
pub use runtime::InferRuntime;
pub use spec::{InferFlavor, InferenceSpec, KIND_INFERENCE, LatencyClass, MemoryBudget, ServeArgs};
