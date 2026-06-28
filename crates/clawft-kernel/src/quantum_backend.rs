//! Quantum backend abstraction for WeftOS.
//!
//! EXPERIMENTAL (0.6.x). Interface only — concrete backends (`quantum_pasqal`,
//! `quantum_braket`) are stubs until full REST + auth land in a later release.
//!
//! The trait is backend-agnostic so that neutral-atom analog processors from
//! different vendors (Pasqal Fresnel, QuEra Aquila on AWS Braket) can be
//! swapped or chained for fallback. Shared graph → atom-position mapping lives
//! in `quantum_register`.
//!
//! See `.planning/development_notes/pasqal-integration.md` §6.1 and §13.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::quantum_state::QuantumCognitiveState;

/// Opaque handle to an in-flight quantum job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobHandle {
    pub backend: &'static str,
    pub job_id: String,
    pub batch_id: Option<String>,
}

/// Measurement results from a quantum backend, normalized across vendors.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantumResults {
    /// One bitstring per shot. Inner `Vec<u8>` is 0/1 per atom.
    pub bitstrings: Vec<Vec<u8>>,
    /// Per-atom Rydberg-excitation probability (length = n_atoms).
    pub rydberg_probs: Vec<f64>,
    /// Number of shots that actually completed.
    pub shots: u32,
}

/// Job lifecycle state, normalized across vendors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobStatus {
    Pending,
    Running,
    Done,
    Canceled,
    Error,
}

/// Backend health snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendStatus {
    pub name: &'static str,
    pub reachable: bool,
    pub queue_depth: Option<u32>,
    pub estimated_wait: Option<Duration>,
    pub max_qubits: usize,
}

/// Evolution parameters — the subset of the Rydberg Hamiltonian controls that
/// both Pasqal and QuEra expose in analog mode.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct EvolutionParams {
    pub duration_ns: u64,
    pub omega_rad_per_us: f64,
    pub detuning_rad_per_us: f64,
    pub phase_rad: f64,
    pub shots: u32,
}

impl Default for EvolutionParams {
    fn default() -> Self {
        Self {
            duration_ns: 1000,
            omega_rad_per_us: 1.0,
            detuning_rad_per_us: 0.0,
            phase_rad: 0.0,
            shots: 100,
        }
    }
}

/// Errors from quantum backends.
#[derive(Debug, thiserror::Error)]
pub enum QuantumError {
    #[error("backend not implemented (experimental 0.6.x interface)")]
    NotImplemented,
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("transport error: {0}")]
    Transport(String),
    #[error("backend rejected request: {0}")]
    Rejected(String),
    #[error("graph too large for backend: {nodes} > {max}")]
    GraphTooLarge { nodes: usize, max: usize },
    #[error("invalid register layout: {0}")]
    InvalidRegister(String),
    #[error("serialization: {0}")]
    Serde(String),
}

/// Abstraction over neutral-atom analog quantum processors.
///
/// All implementations MUST be object-safe so backends can be stored as
/// `Box<dyn QuantumBackend>` for runtime selection / fallback chaining.
#[async_trait]
pub trait QuantumBackend: Send + Sync {
    fn name(&self) -> &'static str;

    fn max_qubits(&self) -> usize;

    async fn health_check(&self) -> Result<BackendStatus, QuantumError>;

    /// Submit an evolution of the given quantum state on the given register.
    /// Register coordinates are in micrometers.
    async fn submit_evolution(
        &self,
        register: &[(String, [f64; 2])],
        state: &QuantumCognitiveState,
        params: EvolutionParams,
    ) -> Result<JobHandle, QuantumError>;

    async fn poll(&self, handle: &JobHandle) -> Result<JobStatus, QuantumError>;

    async fn get_results(&self, handle: &JobHandle)
    -> Result<Option<QuantumResults>, QuantumError>;

    async fn cancel(&self, handle: &JobHandle) -> Result<(), QuantumError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn _assert_object_safe(_: &dyn QuantumBackend) {}

    #[test]
    fn evolution_params_default_sane() {
        let p = EvolutionParams::default();
        assert!(p.shots > 0);
        assert!(p.duration_ns > 0);
    }

    #[test]
    fn not_implemented_error_formats() {
        let e = QuantumError::NotImplemented;
        assert!(e.to_string().contains("not implemented"));
    }

    #[test]
    fn graph_too_large_error_contains_counts() {
        let e = QuantumError::GraphTooLarge {
            nodes: 500,
            max: 100,
        };
        let msg = e.to_string();
        assert!(msg.contains("500"));
        assert!(msg.contains("100"));
    }
}
