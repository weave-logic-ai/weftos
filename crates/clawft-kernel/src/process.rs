//! Process table for PID-based agent tracking.
//!
//! The [`ProcessTable`] uses [`DashMap`] for lock-free concurrent
//! access, allowing multiple kernel subsystems to query and update
//! process state without contention.

use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(feature = "exochain")]
use std::sync::Arc;

use dashmap::DashMap;
use serde::{Deserialize, Serialize};

use crate::capability::AgentCapabilities;
use crate::error::KernelError;

// Re-export CancellationToken: use tokio_util when available (native),
// otherwise provide a minimal shim for non-async targets (WASI).
#[cfg(feature = "native")]
pub use tokio_util::sync::CancellationToken;

#[cfg(not(feature = "native"))]
mod cancel_shim {
    /// Minimal CancellationToken shim for non-async (WASI) builds.
    ///
    /// The real token lives in `tokio_util`. This provides the same
    /// construction + Clone API so that `ProcessEntry` compiles on all
    /// targets. The async `cancelled()` future is intentionally absent
    /// because WASI builds do not run an async runtime.
    #[derive(Debug, Clone)]
    pub struct CancellationToken {
        _priv: (),
    }

    impl CancellationToken {
        pub fn new() -> Self {
            Self { _priv: () }
        }

        /// Request cancellation (no-op in shim -- no waiters).
        pub fn cancel(&self) {}
    }

    impl Default for CancellationToken {
        fn default() -> Self {
            Self::new()
        }
    }
}

#[cfg(not(feature = "native"))]
pub use cancel_shim::CancellationToken;

/// Process identifier. Monotonically increasing, never reused.
pub type Pid = u64;

/// State of a kernel-managed process.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProcessState {
    /// Process is initializing.
    Starting,
    /// Process is actively running.
    Running,
    /// Process has been suspended (paused).
    Suspended,
    /// Process is in the process of stopping.
    Stopping,
    /// Process has exited with a status code.
    Exited(i32),
}

impl ProcessState {
    /// Check whether a state transition is valid.
    ///
    /// Valid transitions:
    /// - Starting -> Running | Exited
    /// - Running -> Suspended | Stopping | Exited
    /// - Suspended -> Running | Stopping | Exited
    /// - Stopping -> Exited
    /// - Exited -> (none)
    pub fn can_transition_to(&self, next: &ProcessState) -> bool {
        matches!(
            (self, next),
            (ProcessState::Starting, ProcessState::Running)
                | (ProcessState::Starting, ProcessState::Exited(_))
                | (ProcessState::Running, ProcessState::Suspended)
                | (ProcessState::Running, ProcessState::Stopping)
                | (ProcessState::Running, ProcessState::Exited(_))
                | (ProcessState::Suspended, ProcessState::Running)
                | (ProcessState::Suspended, ProcessState::Stopping)
                | (ProcessState::Suspended, ProcessState::Exited(_))
                | (ProcessState::Stopping, ProcessState::Exited(_))
        )
    }
}

impl std::fmt::Display for ProcessState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProcessState::Starting => write!(f, "starting"),
            ProcessState::Running => write!(f, "running"),
            ProcessState::Suspended => write!(f, "suspended"),
            ProcessState::Stopping => write!(f, "stopping"),
            ProcessState::Exited(code) => write!(f, "exited({code})"),
        }
    }
}

/// Resource usage counters for a process.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceUsage {
    /// Approximate memory usage in bytes.
    pub memory_bytes: u64,
    /// Accumulated CPU time in milliseconds.
    pub cpu_time_ms: u64,
    /// Total number of tool calls made.
    pub tool_calls: u64,
    /// Total number of IPC messages sent.
    pub messages_sent: u64,
}

/// A single entry in the process table.
#[derive(Debug, Clone)]
pub struct ProcessEntry {
    /// Unique process identifier.
    pub pid: Pid,
    /// Agent identifier string.
    pub agent_id: String,
    /// Current state.
    pub state: ProcessState,
    /// Capabilities granted to this process.
    pub capabilities: AgentCapabilities,
    /// Resource usage counters.
    pub resource_usage: ResourceUsage,
    /// Cancellation token for cooperative shutdown.
    pub cancel_token: CancellationToken,
    /// PID of the parent process (None for the root process).
    pub parent_pid: Option<Pid>,
}

/// Concurrent process table with PID allocation.
///
/// Uses [`DashMap`] for lock-free concurrent reads and writes.
/// PIDs are allocated monotonically from an [`AtomicU64`] and are
/// never reused within a kernel session.
pub struct ProcessTable {
    next_pid: AtomicU64,
    entries: DashMap<Pid, ProcessEntry>,
    max_processes: u32,
    #[cfg(feature = "exochain")]
    chain_manager: Option<Arc<crate::chain::ChainManager>>,
}

impl ProcessTable {
    /// Create a new process table with the given maximum capacity.
    pub fn new(max_processes: u32) -> Self {
        Self {
            next_pid: AtomicU64::new(1), // PID 0 reserved for kernel
            entries: DashMap::new(),
            max_processes,
            #[cfg(feature = "exochain")]
            chain_manager: None,
        }
    }

    /// Attach a chain manager for exochain event logging.
    #[cfg(feature = "exochain")]
    pub fn set_chain_manager(&mut self, cm: Arc<crate::chain::ChainManager>) {
        self.chain_manager = Some(cm);
    }

    /// Allocate the next PID without inserting an entry.
    pub fn allocate_pid(&self) -> Pid {
        self.next_pid.fetch_add(1, Ordering::Relaxed)
    }

    /// Insert a process entry into the table.
    ///
    /// The entry's `pid` field is set to the next available PID.
    /// Returns the assigned PID, or an error if the process table
    /// is at capacity.
    pub fn insert(&self, mut entry: ProcessEntry) -> Result<Pid, KernelError> {
        if self.entries.len() >= self.max_processes as usize {
            return Err(KernelError::ProcessTableFull {
                max: self.max_processes,
            });
        }
        let pid = self.allocate_pid();
        entry.pid = pid;

        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain_manager {
            cm.append(
                "process",
                crate::chain::EVENT_KIND_PROCESS_REGISTER,
                Some(serde_json::json!({
                    "pid": pid,
                    "agent_id": &entry.agent_id,
                    "parent_pid": entry.parent_pid,
                })),
            );
        }

        self.entries.insert(pid, entry);
        Ok(pid)
    }

    /// Insert a process entry with a specific PID (for kernel PID 0).
    pub fn insert_with_pid(&self, entry: ProcessEntry) -> Result<(), KernelError> {
        if self.entries.len() >= self.max_processes as usize {
            return Err(KernelError::ProcessTableFull {
                max: self.max_processes,
            });
        }
        self.entries.insert(entry.pid, entry);
        Ok(())
    }

    /// Get a clone of a process entry by PID.
    pub fn get(&self, pid: Pid) -> Option<ProcessEntry> {
        self.entries.get(&pid).map(|e| e.value().clone())
    }

    /// Remove a process entry by PID.
    pub fn remove(&self, pid: Pid) -> Option<ProcessEntry> {
        let removed = self.entries.remove(&pid).map(|(_, e)| e);

        #[cfg(feature = "exochain")]
        if let Some(ref entry) = removed
            && let Some(ref cm) = self.chain_manager {
                cm.append(
                    "process",
                    crate::chain::EVENT_KIND_PROCESS_DEREGISTER,
                    Some(serde_json::json!({
                        "pid": pid,
                        "agent_id": &entry.agent_id,
                    })),
                );
            }

        removed
    }

    /// List all process entries (cloned).
    pub fn list(&self) -> Vec<ProcessEntry> {
        self.entries.iter().map(|e| e.value().clone()).collect()
    }

    /// Update the state of a process.
    ///
    /// Validates the state transition before applying.
    pub fn update_state(&self, pid: Pid, new_state: ProcessState) -> Result<(), KernelError> {
        let mut entry = self
            .entries
            .get_mut(&pid)
            .ok_or(KernelError::ProcessNotFound { pid })?;

        if !entry.state.can_transition_to(&new_state) {
            return Err(KernelError::InvalidStateTransition {
                pid,
                from: entry.state.clone(),
                to: new_state,
            });
        }

        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain_manager {
            cm.append(
                "process",
                crate::chain::EVENT_KIND_PROCESS_STATE,
                Some(serde_json::json!({
                    "pid": pid,
                    "from": entry.state.to_string(),
                    "to": new_state.to_string(),
                })),
            );
        }

        entry.state = new_state;
        Ok(())
    }

    /// Update resource usage for a process.
    pub fn update_resources(&self, pid: Pid, usage: ResourceUsage) -> Result<(), KernelError> {
        let mut entry = self
            .entries
            .get_mut(&pid)
            .ok_or(KernelError::ProcessNotFound { pid })?;
        entry.resource_usage = usage;
        Ok(())
    }

    /// Get the number of processes in the table.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check whether the process table is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get the maximum process capacity.
    pub fn max_processes(&self) -> u32 {
        self.max_processes
    }

    /// Update the capabilities of a process.
    ///
    /// Replaces the existing capabilities with the given ones.
    /// This is used by the supervisor when hot-updating an agent's
    /// permissions (future work) or during restart.
    pub fn set_capabilities(
        &self,
        pid: Pid,
        capabilities: AgentCapabilities,
    ) -> Result<(), KernelError> {
        let mut entry = self
            .entries
            .get_mut(&pid)
            .ok_or(KernelError::ProcessNotFound { pid })?;
        entry.capabilities = capabilities;
        Ok(())
    }

    /// Count processes in the given state.
    pub fn count_by_state(&self, state: &ProcessState) -> usize {
        self.entries.iter().filter(|e| &e.state == state).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(agent_id: &str) -> ProcessEntry {
        ProcessEntry {
            pid: 0, // Will be set by insert()
            agent_id: agent_id.to_owned(),
            state: ProcessState::Starting,
            capabilities: AgentCapabilities::default(),
            resource_usage: ResourceUsage::default(),
            cancel_token: CancellationToken::new(),
            parent_pid: None,
        }
    }

    #[test]
    fn insert_and_get() {
        let table = ProcessTable::new(64);
        let pid = table.insert(make_entry("agent-1")).unwrap();
        assert_eq!(pid, 1);

        let entry = table.get(pid).unwrap();
        assert_eq!(entry.agent_id, "agent-1");
        assert_eq!(entry.pid, 1);
    }

    #[test]
    fn insert_multiple() {
        let table = ProcessTable::new(64);
        let p1 = table.insert(make_entry("a1")).unwrap();
        let p2 = table.insert(make_entry("a2")).unwrap();
        let p3 = table.insert(make_entry("a3")).unwrap();

        assert_eq!(p1, 1);
        assert_eq!(p2, 2);
        assert_eq!(p3, 3);
        assert_eq!(table.len(), 3);
    }

    #[test]
    fn remove() {
        let table = ProcessTable::new(64);
        let pid = table.insert(make_entry("agent-1")).unwrap();
        let removed = table.remove(pid);
        assert!(removed.is_some());
        assert!(table.get(pid).is_none());
        assert!(table.is_empty());
    }

    #[test]
    fn remove_nonexistent() {
        let table = ProcessTable::new(64);
        assert!(table.remove(999).is_none());
    }

    #[test]
    fn list() {
        let table = ProcessTable::new(64);
        table.insert(make_entry("a1")).unwrap();
        table.insert(make_entry("a2")).unwrap();

        let entries = table.list();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn update_state_valid() {
        let table = ProcessTable::new(64);
        let pid = table.insert(make_entry("agent-1")).unwrap();

        // Starting -> Running
        table.update_state(pid, ProcessState::Running).unwrap();
        assert_eq!(table.get(pid).unwrap().state, ProcessState::Running);

        // Running -> Stopping
        table.update_state(pid, ProcessState::Stopping).unwrap();
        assert_eq!(table.get(pid).unwrap().state, ProcessState::Stopping);

        // Stopping -> Exited
        table.update_state(pid, ProcessState::Exited(0)).unwrap();
        assert_eq!(table.get(pid).unwrap().state, ProcessState::Exited(0));
    }

    #[test]
    fn update_state_invalid_transition() {
        let table = ProcessTable::new(64);
        let pid = table.insert(make_entry("agent-1")).unwrap();

        // Starting -> Suspended is not valid
        let result = table.update_state(pid, ProcessState::Suspended);
        assert!(result.is_err());
    }

    #[test]
    fn update_state_nonexistent_pid() {
        let table = ProcessTable::new(64);
        let result = table.update_state(999, ProcessState::Running);
        assert!(result.is_err());
    }

    #[test]
    fn process_table_full() {
        let table = ProcessTable::new(2);
        table.insert(make_entry("a1")).unwrap();
        table.insert(make_entry("a2")).unwrap();
        let result = table.insert(make_entry("a3"));
        assert!(result.is_err());
    }

    #[test]
    fn update_resources() {
        let table = ProcessTable::new(64);
        let pid = table.insert(make_entry("agent-1")).unwrap();

        let usage = ResourceUsage {
            memory_bytes: 1024,
            cpu_time_ms: 500,
            tool_calls: 10,
            messages_sent: 5,
        };
        table.update_resources(pid, usage).unwrap();

        let entry = table.get(pid).unwrap();
        assert_eq!(entry.resource_usage.memory_bytes, 1024);
        assert_eq!(entry.resource_usage.cpu_time_ms, 500);
    }

    #[test]
    fn state_display() {
        assert_eq!(ProcessState::Starting.to_string(), "starting");
        assert_eq!(ProcessState::Running.to_string(), "running");
        assert_eq!(ProcessState::Suspended.to_string(), "suspended");
        assert_eq!(ProcessState::Stopping.to_string(), "stopping");
        assert_eq!(ProcessState::Exited(0).to_string(), "exited(0)");
        assert_eq!(ProcessState::Exited(-1).to_string(), "exited(-1)");
    }

    #[test]
    fn state_transitions() {
        // Valid transitions
        assert!(ProcessState::Starting.can_transition_to(&ProcessState::Running));
        assert!(ProcessState::Starting.can_transition_to(&ProcessState::Exited(1)));
        assert!(ProcessState::Running.can_transition_to(&ProcessState::Suspended));
        assert!(ProcessState::Running.can_transition_to(&ProcessState::Stopping));
        assert!(ProcessState::Running.can_transition_to(&ProcessState::Exited(0)));
        assert!(ProcessState::Suspended.can_transition_to(&ProcessState::Running));
        assert!(ProcessState::Suspended.can_transition_to(&ProcessState::Stopping));
        assert!(ProcessState::Stopping.can_transition_to(&ProcessState::Exited(0)));

        // Invalid transitions
        assert!(!ProcessState::Starting.can_transition_to(&ProcessState::Suspended));
        assert!(!ProcessState::Starting.can_transition_to(&ProcessState::Stopping));
        assert!(!ProcessState::Stopping.can_transition_to(&ProcessState::Running));
        assert!(!ProcessState::Exited(0).can_transition_to(&ProcessState::Running));
        assert!(!ProcessState::Exited(0).can_transition_to(&ProcessState::Starting));
    }

    #[test]
    fn insert_with_pid_kernel() {
        let table = ProcessTable::new(64);
        let entry = ProcessEntry {
            pid: 0,
            agent_id: "kernel".to_owned(),
            state: ProcessState::Running,
            capabilities: AgentCapabilities::default(),
            resource_usage: ResourceUsage::default(),
            cancel_token: CancellationToken::new(),
            parent_pid: None,
        };
        table.insert_with_pid(entry).unwrap();
        assert_eq!(table.get(0).unwrap().agent_id, "kernel");
    }

    #[test]
    fn set_capabilities() {
        let table = ProcessTable::new(64);
        let pid = table.insert(make_entry("agent-1")).unwrap();

        let new_caps = AgentCapabilities {
            can_spawn: false,
            can_network: true,
            ..Default::default()
        };
        table.set_capabilities(pid, new_caps).unwrap();

        let entry = table.get(pid).unwrap();
        assert!(!entry.capabilities.can_spawn);
        assert!(entry.capabilities.can_network);
    }

    #[test]
    fn set_capabilities_nonexistent_pid() {
        let table = ProcessTable::new(64);
        let result = table.set_capabilities(999, AgentCapabilities::default());
        assert!(result.is_err());
    }

    #[test]
    fn count_by_state() {
        let table = ProcessTable::new(64);
        let p1 = table.insert(make_entry("a1")).unwrap();
        let p2 = table.insert(make_entry("a2")).unwrap();
        table.insert(make_entry("a3")).unwrap();

        // All start as Starting
        assert_eq!(table.count_by_state(&ProcessState::Starting), 3);
        assert_eq!(table.count_by_state(&ProcessState::Running), 0);

        table.update_state(p1, ProcessState::Running).unwrap();
        table.update_state(p2, ProcessState::Running).unwrap();

        assert_eq!(table.count_by_state(&ProcessState::Starting), 1);
        assert_eq!(table.count_by_state(&ProcessState::Running), 2);
    }
}
