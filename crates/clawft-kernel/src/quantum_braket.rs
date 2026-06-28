//! AWS Braket backend stub targeting QuEra Aquila.
//!
//! EXPERIMENTAL (0.6.x). Interface-only — AWS SigV4 auth and the Braket AHS
//! (Analog Hamiltonian Simulation) JSON body are deferred to a later release.
//! See `.planning/development_notes/pasqal-integration.md` §13.
//!
//! Targets:
//! - QuEra Aquila (256 atoms, Rydberg) via
//!   `arn:aws:braket:us-east-1::device/qpu/quera/Aquila`
//! - Braket local simulator (`braket_sv`) for dev
//!
//! Feature: `quantum-braket` (off by default).

use async_trait::async_trait;

use crate::quantum_backend::{
    BackendStatus, EvolutionParams, JobHandle, JobStatus, QuantumBackend, QuantumError,
    QuantumResults,
};
use crate::quantum_state::QuantumCognitiveState;

/// Braket device selector (analog neutral-atom only — the trait doesn't model
/// gate-based devices).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BraketDevice {
    QueraAquila,
    LocalSimulator,
}

impl BraketDevice {
    pub fn arn(self) -> &'static str {
        match self {
            BraketDevice::QueraAquila => "arn:aws:braket:us-east-1::device/qpu/quera/Aquila",
            BraketDevice::LocalSimulator => "local:braket/braket_ahs_sim",
        }
    }

    pub fn max_qubits(self) -> usize {
        match self {
            BraketDevice::QueraAquila => 256,
            BraketDevice::LocalSimulator => 40,
        }
    }
}

/// Configuration for the Braket backend.
#[derive(Debug, Clone)]
pub struct BraketConfig {
    pub region: String,
    pub s3_results_bucket: String,
    pub s3_results_prefix: String,
    pub device: BraketDevice,
    /// Access key is read from the AWS credential chain in production; only
    /// stored on the config for tests/mocks.
    pub access_key_id: Option<String>,
    pub secret_access_key: Option<String>,
}

impl Default for BraketConfig {
    fn default() -> Self {
        Self {
            region: "us-east-1".into(),
            s3_results_bucket: String::new(),
            s3_results_prefix: "weftos-quantum/".into(),
            device: BraketDevice::LocalSimulator,
            access_key_id: None,
            secret_access_key: None,
        }
    }
}

/// Stub Braket backend. Compiles and returns `NotImplemented` on submission.
pub struct BraketBackend {
    cfg: BraketConfig,
}

impl BraketBackend {
    pub fn new(cfg: BraketConfig) -> Self {
        Self { cfg }
    }

    pub fn config(&self) -> &BraketConfig {
        &self.cfg
    }
}

#[async_trait]
impl QuantumBackend for BraketBackend {
    fn name(&self) -> &'static str {
        "braket"
    }

    fn max_qubits(&self) -> usize {
        self.cfg.device.max_qubits()
    }

    async fn health_check(&self) -> Result<BackendStatus, QuantumError> {
        Ok(BackendStatus {
            name: "braket",
            reachable: false,
            queue_depth: None,
            estimated_wait: None,
            max_qubits: self.max_qubits(),
        })
    }

    async fn submit_evolution(
        &self,
        _register: &[(String, [f64; 2])],
        _state: &QuantumCognitiveState,
        _params: EvolutionParams,
    ) -> Result<JobHandle, QuantumError> {
        Err(QuantumError::NotImplemented)
    }

    async fn poll(&self, _handle: &JobHandle) -> Result<JobStatus, QuantumError> {
        Err(QuantumError::NotImplemented)
    }

    async fn get_results(
        &self,
        _handle: &JobHandle,
    ) -> Result<Option<QuantumResults>, QuantumError> {
        Err(QuantumError::NotImplemented)
    }

    async fn cancel(&self, _handle: &JobHandle) -> Result<(), QuantumError> {
        Err(QuantumError::NotImplemented)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aquila_arn_is_correct_region() {
        assert!(BraketDevice::QueraAquila.arn().contains("us-east-1"));
        assert!(BraketDevice::QueraAquila.arn().contains("Aquila"));
    }

    #[test]
    fn aquila_supports_256_qubits() {
        assert_eq!(BraketDevice::QueraAquila.max_qubits(), 256);
    }

    #[test]
    fn stub_submit_returns_not_implemented() {
        let backend = BraketBackend::new(BraketConfig::default());
        let state = QuantumCognitiveState::uniform(1, &[0]);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let err = rt
            .block_on(backend.submit_evolution(&[], &state, EvolutionParams::default()))
            .unwrap_err();
        assert!(matches!(err, QuantumError::NotImplemented));
    }
}
