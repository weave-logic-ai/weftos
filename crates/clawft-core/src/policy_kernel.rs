//! POLICY_KERNEL -- persistent routing policy store (H2.5).
//!
//! Stores `IntelligentRouter` routing policies and cost records to disk
//! so they survive agent restarts. The default location is
//! `~/.clawft/agents/<id>/policy_kernel.json`, falling back to
//! `~/.clawft/memory/policy_kernel.json` when no agent ID is set.
//!
//! The store is loaded on startup and saved on every policy change.
//! Format is intentionally JSON for debuggability.
//!
//! This module is gated behind the `vector-memory` feature flag.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

// ── Serializable types ──────────────────────────────────────────────

/// A persisted routing policy entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyEntry {
    /// Unique identifier for this policy.
    pub id: String,
    /// The prompt pattern that triggered this policy.
    pub pattern: String,
    /// The embedding vector for the pattern.
    pub embedding: Vec<f32>,
    /// The tier (1=WASM, 2=Haiku, 3=Sonnet/Opus).
    pub tier: u8,
    /// Tags (first tag is the tier string).
    pub tags: Vec<String>,
    /// Arbitrary metadata.
    pub metadata: HashMap<String, serde_json::Value>,
    /// Unix timestamp (seconds since epoch) when the policy was created.
    pub timestamp: u64,
}

/// A persisted cost record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedCostRecord {
    /// The model that was used.
    pub model: String,
    /// Number of tokens consumed.
    pub tokens: u64,
    /// Monetary cost in USD.
    pub cost: f32,
    /// End-to-end latency in milliseconds.
    pub latency_ms: u64,
    /// Unix timestamp (seconds since epoch).
    pub timestamp: u64,
}

/// The full persisted state of the policy kernel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyKernelState {
    /// Schema version for forward compatibility.
    pub version: u32,
    /// Routing policy entries.
    pub policies: Vec<PolicyEntry>,
    /// Cost record history.
    pub cost_records: Vec<PersistedCostRecord>,
}

impl Default for PolicyKernelState {
    fn default() -> Self {
        Self {
            version: 1,
            policies: Vec::new(),
            cost_records: Vec::new(),
        }
    }
}

// ── Errors ──────────────────────────────────────────────────────────

/// Errors from policy kernel persistence.
#[non_exhaustive]
#[derive(Debug)]
pub enum PolicyKernelError {
    /// An I/O error occurred.
    Io(std::io::Error),
    /// A serialization/deserialization error occurred.
    Serde(serde_json::Error),
}

impl std::fmt::Display for PolicyKernelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PolicyKernelError::Io(e) => write!(f, "policy kernel I/O error: {e}"),
            PolicyKernelError::Serde(e) => {
                write!(f, "policy kernel serde error: {e}")
            }
        }
    }
}

impl std::error::Error for PolicyKernelError {}

impl From<std::io::Error> for PolicyKernelError {
    fn from(e: std::io::Error) -> Self {
        PolicyKernelError::Io(e)
    }
}

impl From<serde_json::Error> for PolicyKernelError {
    fn from(e: serde_json::Error) -> Self {
        PolicyKernelError::Serde(e)
    }
}

// ── PolicyKernel ────────────────────────────────────────────────────

/// Persistent routing policy store.
///
/// Wraps a [`PolicyKernelState`] with load/save operations tied to a
/// file path. Changes are saved eagerly on every mutation.
pub struct PolicyKernel {
    state: PolicyKernelState,
    path: PathBuf,
}

impl PolicyKernel {
    /// Resolve the default policy kernel path for a given agent.
    ///
    /// Returns `~/.clawft/agents/<agent_id>/policy_kernel.json`.
    pub fn agent_path(agent_id: &str) -> Option<PathBuf> {
        #[cfg(feature = "native")]
        {
            dirs::home_dir().map(|h| {
                h.join(".clawft")
                    .join("agents")
                    .join(agent_id)
                    .join("policy_kernel.json")
            })
        }
        #[cfg(not(feature = "native"))]
        {
            Some(
                PathBuf::from(".clawft")
                    .join("agents")
                    .join(agent_id)
                    .join("policy_kernel.json"),
            )
        }
    }

    /// Resolve the default global policy kernel path.
    ///
    /// Returns `~/.clawft/memory/policy_kernel.json`.
    pub fn global_path() -> Option<PathBuf> {
        #[cfg(feature = "native")]
        {
            dirs::home_dir().map(|h| h.join(".clawft").join("memory").join("policy_kernel.json"))
        }
        #[cfg(not(feature = "native"))]
        {
            Some(
                PathBuf::from(".clawft")
                    .join("memory")
                    .join("policy_kernel.json"),
            )
        }
    }

    /// Load the policy kernel from disk, or create a new empty one.
    ///
    /// If the file does not exist, returns an empty kernel that will
    /// create the file on the first save.
    pub fn load(path: &Path) -> Result<Self, PolicyKernelError> {
        if path.exists() {
            debug!(path = %path.display(), "loading policy kernel");
            let data = std::fs::read_to_string(path)?;
            let state: PolicyKernelState = serde_json::from_str(&data)?;
            debug!(
                policies = state.policies.len(),
                cost_records = state.cost_records.len(),
                "policy kernel loaded"
            );
            Ok(Self {
                state,
                path: path.to_path_buf(),
            })
        } else {
            debug!(path = %path.display(), "no existing policy kernel, creating new");
            Ok(Self {
                state: PolicyKernelState::default(),
                path: path.to_path_buf(),
            })
        }
    }

    /// Save the current state to disk.
    pub fn save(&self) -> Result<(), PolicyKernelError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let json = serde_json::to_string_pretty(&self.state)?;
        std::fs::write(&self.path, json)?;

        debug!(
            path = %self.path.display(),
            policies = self.state.policies.len(),
            "policy kernel saved"
        );
        Ok(())
    }

    /// Add a routing policy and persist to disk.
    pub fn add_policy(
        &mut self,
        id: String,
        pattern: String,
        embedding: Vec<f32>,
        tier: u8,
    ) -> Result<(), PolicyKernelError> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        self.state.policies.push(PolicyEntry {
            id,
            pattern,
            embedding,
            tier,
            tags: vec![tier.to_string()],
            metadata: HashMap::new(),
            timestamp,
        });

        self.save()
    }

    /// Record a cost observation and persist to disk.
    pub fn record_cost(
        &mut self,
        model: &str,
        tokens: u64,
        cost: f32,
        latency_ms: u64,
    ) -> Result<(), PolicyKernelError> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        self.state.cost_records.push(PersistedCostRecord {
            model: model.to_owned(),
            tokens,
            cost,
            latency_ms,
            timestamp,
        });

        self.save()
    }

    /// Return the policy entries (read-only).
    pub fn policies(&self) -> &[PolicyEntry] {
        &self.state.policies
    }

    /// Return the cost records (read-only).
    pub fn cost_records(&self) -> &[PersistedCostRecord] {
        &self.state.cost_records
    }

    /// Return the number of policy entries.
    pub fn policy_count(&self) -> usize {
        self.state.policies.len()
    }

    /// Return the full state (for export).
    pub fn state(&self) -> &PolicyKernelState {
        &self.state
    }

    /// Replace the full state (for import). Saves immediately.
    pub fn set_state(&mut self, state: PolicyKernelState) -> Result<(), PolicyKernelError> {
        self.state = state;
        self.save()
    }

    /// Return the file path of this kernel.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Clear all policies and cost records. Saves immediately.
    pub fn clear(&mut self) -> Result<(), PolicyKernelError> {
        self.state.policies.clear();
        self.state.cost_records.clear();
        self.save()
    }
}

/// Try to load a policy kernel for a specific agent, falling back to
/// the global kernel, and finally to a fresh empty kernel at the
/// global path.
pub fn load_policy_kernel(agent_id: Option<&str>) -> PolicyKernel {
    // Try agent-specific path first.
    if let Some(id) = agent_id
        && let Some(path) = PolicyKernel::agent_path(id)
    {
        match PolicyKernel::load(&path) {
            Ok(k) => return k,
            Err(e) => warn!(
                agent_id = id,
                error = %e,
                "failed to load agent policy kernel, trying global"
            ),
        }
    }

    // Try global path.
    if let Some(path) = PolicyKernel::global_path() {
        match PolicyKernel::load(&path) {
            Ok(k) => return k,
            Err(e) => warn!(
                error = %e,
                "failed to load global policy kernel, using in-memory"
            ),
        }
    }

    // Last resort: in-memory only (will fail on save without a valid path).
    PolicyKernel {
        state: PolicyKernelState::default(),
        path: PathBuf::from("policy_kernel.json"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_path(label: &str) -> PathBuf {
        let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        std::env::temp_dir().join(format!("clawft_policy_kernel_test_{label}_{pid}_{n}.json"))
    }

    #[test]
    fn load_nonexistent_creates_empty() {
        let path = temp_path("nonexist");
        let _ = std::fs::remove_file(&path);

        let kernel = PolicyKernel::load(&path).unwrap();
        assert_eq!(kernel.policy_count(), 0);
        assert!(kernel.cost_records().is_empty());
    }

    #[test]
    fn add_policy_persists() {
        let path = temp_path("add_policy");
        let _ = std::fs::remove_file(&path);

        {
            let mut kernel = PolicyKernel::load(&path).unwrap();
            kernel
                .add_policy("p1".into(), "convert temperature".into(), vec![1.0, 0.0], 2)
                .unwrap();
            assert_eq!(kernel.policy_count(), 1);
        }

        // Reload and verify persistence.
        let kernel = PolicyKernel::load(&path).unwrap();
        assert_eq!(kernel.policy_count(), 1);
        assert_eq!(kernel.policies()[0].id, "p1");
        assert_eq!(kernel.policies()[0].tier, 2);
        assert_eq!(kernel.policies()[0].pattern, "convert temperature");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn record_cost_persists() {
        let path = temp_path("record_cost");
        let _ = std::fs::remove_file(&path);

        {
            let mut kernel = PolicyKernel::load(&path).unwrap();
            kernel
                .record_cost("claude-haiku-3.5", 100, 0.001, 500)
                .unwrap();
            kernel
                .record_cost("claude-sonnet-4.5", 500, 0.01, 2000)
                .unwrap();
        }

        let kernel = PolicyKernel::load(&path).unwrap();
        assert_eq!(kernel.cost_records().len(), 2);
        assert_eq!(kernel.cost_records()[0].model, "claude-haiku-3.5");
        assert_eq!(kernel.cost_records()[1].model, "claude-sonnet-4.5");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn clear_removes_all() {
        let path = temp_path("clear");
        let _ = std::fs::remove_file(&path);

        let mut kernel = PolicyKernel::load(&path).unwrap();
        kernel
            .add_policy("p1".into(), "test".into(), vec![1.0], 2)
            .unwrap();
        kernel.record_cost("test-model", 10, 0.0, 100).unwrap();
        kernel.clear().unwrap();

        assert_eq!(kernel.policy_count(), 0);
        assert!(kernel.cost_records().is_empty());

        // Verify on disk.
        let reloaded = PolicyKernel::load(&path).unwrap();
        assert_eq!(reloaded.policy_count(), 0);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn set_state_replaces_and_persists() {
        let path = temp_path("set_state");
        let _ = std::fs::remove_file(&path);

        let mut kernel = PolicyKernel::load(&path).unwrap();
        kernel
            .add_policy("old".into(), "old pattern".into(), vec![0.0], 1)
            .unwrap();

        let new_state = PolicyKernelState {
            version: 1,
            policies: vec![PolicyEntry {
                id: "imported".into(),
                pattern: "imported pattern".into(),
                embedding: vec![1.0, 2.0],
                tier: 3,
                tags: vec!["3".into()],
                metadata: HashMap::new(),
                timestamp: 12345,
            }],
            cost_records: Vec::new(),
        };
        kernel.set_state(new_state).unwrap();

        assert_eq!(kernel.policy_count(), 1);
        assert_eq!(kernel.policies()[0].id, "imported");

        let reloaded = PolicyKernel::load(&path).unwrap();
        assert_eq!(reloaded.policy_count(), 1);
        assert_eq!(reloaded.policies()[0].id, "imported");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn state_version_is_1() {
        let kernel = PolicyKernel::load(&temp_path("version")).unwrap();
        assert_eq!(kernel.state().version, 1);
    }

    #[test]
    fn path_accessor() {
        let path = temp_path("path_acc");
        let kernel = PolicyKernel::load(&path).unwrap();
        assert_eq!(kernel.path(), path.as_path());
    }

    #[test]
    fn agent_path_contains_agent_id() {
        let path = PolicyKernel::agent_path("test-agent").unwrap();
        assert!(path.to_string_lossy().contains("test-agent"));
        assert!(path.to_string_lossy().contains("policy_kernel.json"));
    }

    #[test]
    fn global_path_contains_memory() {
        let path = PolicyKernel::global_path().unwrap();
        assert!(path.to_string_lossy().contains("memory"));
        assert!(path.to_string_lossy().contains("policy_kernel.json"));
    }

    #[test]
    fn default_state_is_empty() {
        let state = PolicyKernelState::default();
        assert_eq!(state.version, 1);
        assert!(state.policies.is_empty());
        assert!(state.cost_records.is_empty());
    }

    #[test]
    fn policy_entry_fields() {
        let entry = PolicyEntry {
            id: "test".into(),
            pattern: "pattern".into(),
            embedding: vec![1.0],
            tier: 2,
            tags: vec!["2".into()],
            metadata: HashMap::new(),
            timestamp: 999,
        };
        assert_eq!(entry.tier, 2);
        assert_eq!(entry.timestamp, 999);
    }

    #[test]
    fn error_display() {
        let io_err =
            PolicyKernelError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "test"));
        assert!(format!("{io_err}").contains("test"));

        let json_str = "invalid json";
        let serde_err: Result<PolicyKernelState, _> = serde_json::from_str(json_str);
        if let Err(e) = serde_err {
            let pke = PolicyKernelError::Serde(e);
            assert!(format!("{pke}").contains("serde"));
        }
    }
}
