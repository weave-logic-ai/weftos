//! Reconciliation controller (K2b-G1).
//!
//! Compares desired state (from `AppManifest`) against actual state
//! (from `ProcessTable`) and takes corrective action: spawning
//! missing agents, stopping extra agents, and logging state
//! mismatches.
//!
//! Gated behind `cfg(feature = "os-patterns")`.

use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::debug;

use crate::capability::AgentCapabilities;
use crate::process::{Pid, ProcessState, ProcessTable};

/// Desired state for a single agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesiredAgentState {
    /// Application that owns this agent.
    pub app_id: String,
    /// Agent identifier.
    pub agent_id: String,
    /// Agent type label.
    pub agent_type: String,
    /// Number of replicas desired.
    pub replicas: u32,
    /// Capabilities to assign.
    pub capabilities: AgentCapabilities,
}

/// A drift event detected by the reconciler.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DriftEvent {
    /// A desired agent is not running.
    AgentMissing { agent_id: String, app_id: String },
    /// An agent is running but not in the desired state.
    ExtraAgent { pid: Pid, agent_id: String },
    /// An agent is in the wrong state.
    WrongState {
        pid: Pid,
        expected: String,
        actual: String,
    },
    /// An agent was spawned to correct drift.
    AgentSpawned { agent_id: String, app_id: String },
    /// An extra agent was stopped.
    AgentStopped { pid: Pid, agent_id: String },
}

/// Reconciliation controller: desired state vs actual state.
///
/// Registered as a `SystemService` with `ServiceType::Core`.
/// Runs a background tick every `interval` to detect and correct
/// drift between desired and actual process states.
pub struct ReconciliationController {
    interval: Duration,
    desired: DashMap<String, DesiredAgentState>,
    drifts: Arc<RwLock<Vec<DriftEvent>>>,
    process_table: Arc<ProcessTable>,
    max_drift_history: usize,
    /// Optional chain manager for exochain audit logging.
    #[cfg(feature = "exochain")]
    chain_manager: Option<Arc<crate::chain::ChainManager>>,
}

impl ReconciliationController {
    /// Create a new reconciliation controller.
    pub fn new(process_table: Arc<ProcessTable>, interval: Duration) -> Self {
        Self {
            interval,
            desired: DashMap::new(),
            drifts: Arc::new(RwLock::new(Vec::new())),
            process_table,
            max_drift_history: 100,
            #[cfg(feature = "exochain")]
            chain_manager: None,
        }
    }

    /// Attach a chain manager for exochain audit logging.
    #[cfg(feature = "exochain")]
    pub fn set_chain_manager(&mut self, cm: Arc<crate::chain::ChainManager>) {
        self.chain_manager = Some(cm);
    }

    /// Get the tick interval.
    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// Set desired state for an agent.
    pub fn set_desired(&self, key: String, state: DesiredAgentState) {
        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain_manager {
            cm.append(
                "reconciler",
                crate::chain::EVENT_KIND_RECONCILER_DESIRED_SET,
                Some(serde_json::json!({
                    "key": &key,
                    "agent_id": &state.agent_id,
                    "app_id": &state.app_id,
                    "replicas": state.replicas,
                })),
            );
        }
        self.desired.insert(key, state);
    }

    /// Remove desired state for an agent.
    pub fn remove_desired(&self, key: &str) -> Option<DesiredAgentState> {
        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain_manager {
            cm.append(
                "reconciler",
                crate::chain::EVENT_KIND_RECONCILER_DESIRED_REMOVE,
                Some(serde_json::json!({
                    "key": key,
                })),
            );
        }
        self.desired.remove(key).map(|(_, v)| v)
    }

    /// Get all desired states.
    pub fn list_desired(&self) -> Vec<DesiredAgentState> {
        self.desired.iter().map(|e| e.value().clone()).collect()
    }

    /// Get recent drift events.
    pub async fn recent_drifts(&self) -> Vec<DriftEvent> {
        self.drifts.read().await.clone()
    }

    /// Run a single reconciliation tick.
    ///
    /// Compares desired state against process table and returns
    /// detected drifts. Does NOT take corrective action (that
    /// requires a supervisor reference and governance gate).
    pub async fn tick(&self) -> Vec<DriftEvent> {
        let mut drifts = Vec::new();

        // Check desired agents exist in process table
        for entry in self.desired.iter() {
            let desired = entry.value();
            let matching: Vec<_> = self
                .process_table
                .list()
                .into_iter()
                .filter(|p| p.agent_id == desired.agent_id)
                .collect();

            let running_count = matching
                .iter()
                .filter(|p| p.state == ProcessState::Running)
                .count() as u32;

            if running_count < desired.replicas {
                for _ in 0..(desired.replicas - running_count) {
                    drifts.push(DriftEvent::AgentMissing {
                        agent_id: desired.agent_id.clone(),
                        app_id: desired.app_id.clone(),
                    });
                }
            }
        }

        // Check for extra agents (not in desired state)
        for entry in self.process_table.list() {
            if entry.state != ProcessState::Running {
                continue;
            }
            // Skip kernel and system processes
            if entry.agent_id.starts_with("kernel.") || entry.agent_id.starts_with("system.") {
                continue;
            }
            // Skip PID 0 (kernel)
            if entry.pid == 0 {
                continue;
            }

            let is_desired = self
                .desired
                .iter()
                .any(|d| d.value().agent_id == entry.agent_id);

            if !is_desired {
                drifts.push(DriftEvent::ExtraAgent {
                    pid: entry.pid,
                    agent_id: entry.agent_id.clone(),
                });
            }
        }

        // Store drift history (bounded)
        {
            let mut history = self.drifts.write().await;
            history.extend(drifts.clone());
            while history.len() > self.max_drift_history {
                history.remove(0);
            }
        }

        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain_manager {
            cm.append(
                "reconciler",
                crate::chain::EVENT_KIND_RECONCILER_TICK,
                Some(serde_json::json!({
                    "drift_count": drifts.len(),
                    "desired_count": self.desired.len(),
                })),
            );
        }

        debug!(drift_count = drifts.len(), "reconciliation tick completed");
        drifts
    }

    /// Record a corrective action in drift history.
    pub async fn record_action(&self, event: DriftEvent) {
        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain_manager {
            cm.append(
                "reconciler",
                crate::chain::EVENT_KIND_RECONCILER_ACTION,
                Some(serde_json::json!({
                    "event": serde_json::to_value(&event).unwrap_or_default(),
                })),
            );
        }
        let mut history = self.drifts.write().await;
        history.push(event);
        while history.len() > self.max_drift_history {
            history.remove(0);
        }
    }

    /// Clear all desired state (used during shutdown).
    pub fn clear_desired(&self) {
        self.desired.clear();
    }

    /// Get the number of desired agent entries.
    pub fn desired_count(&self) -> usize {
        self.desired.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::AgentCapabilities;
    use crate::process::{ProcessEntry, ResourceUsage};
    use tokio_util::sync::CancellationToken;

    fn make_process_table() -> Arc<ProcessTable> {
        Arc::new(ProcessTable::new(64))
    }

    fn make_controller(pt: Arc<ProcessTable>) -> ReconciliationController {
        ReconciliationController::new(pt, Duration::from_secs(5))
    }

    fn insert_running_agent(pt: &ProcessTable, agent_id: &str) -> Pid {
        let entry = ProcessEntry {
            pid: 0,
            agent_id: agent_id.to_owned(),
            state: ProcessState::Starting,
            capabilities: AgentCapabilities::default(),
            resource_usage: ResourceUsage::default(),
            cancel_token: CancellationToken::new(),
            parent_pid: None,
        };
        let pid = pt.insert(entry).unwrap();
        pt.update_state(pid, ProcessState::Running).unwrap();
        pid
    }

    fn make_desired(app_id: &str, agent_id: &str) -> DesiredAgentState {
        DesiredAgentState {
            app_id: app_id.to_owned(),
            agent_id: agent_id.to_owned(),
            agent_type: "worker".to_owned(),
            replicas: 1,
            capabilities: AgentCapabilities::default(),
        }
    }

    #[tokio::test]
    async fn detects_missing_agent() {
        let pt = make_process_table();
        let ctrl = make_controller(pt);

        ctrl.set_desired("app/worker-1".into(), make_desired("app", "worker-1"));

        let drifts = ctrl.tick().await;
        assert_eq!(drifts.len(), 1);
        assert!(
            matches!(&drifts[0], DriftEvent::AgentMissing { agent_id, .. } if agent_id == "worker-1")
        );
    }

    #[tokio::test]
    async fn no_drift_when_agent_running() {
        let pt = make_process_table();
        insert_running_agent(&pt, "worker-1");
        let ctrl = make_controller(pt);

        ctrl.set_desired("app/worker-1".into(), make_desired("app", "worker-1"));

        let drifts = ctrl.tick().await;
        // No missing agents, but worker-1 is desired so no extra either
        assert!(drifts.is_empty());
    }

    #[tokio::test]
    async fn detects_extra_agent() {
        let pt = make_process_table();
        insert_running_agent(&pt, "rogue-agent");
        let ctrl = make_controller(pt);

        // No desired state set -- rogue-agent is extra
        let drifts = ctrl.tick().await;
        assert_eq!(drifts.len(), 1);
        assert!(
            matches!(&drifts[0], DriftEvent::ExtraAgent { agent_id, .. } if agent_id == "rogue-agent")
        );
    }

    #[tokio::test]
    async fn skips_kernel_and_system_agents() {
        let pt = make_process_table();
        insert_running_agent(&pt, "kernel.boot");
        insert_running_agent(&pt, "system.health");
        let ctrl = make_controller(pt);

        let drifts = ctrl.tick().await;
        assert!(drifts.is_empty());
    }

    #[tokio::test]
    async fn detects_multiple_missing_replicas() {
        let pt = make_process_table();
        let ctrl = make_controller(pt);

        let mut desired = make_desired("app", "worker-1");
        desired.replicas = 3;
        ctrl.set_desired("app/worker-1".into(), desired);

        let drifts = ctrl.tick().await;
        assert_eq!(drifts.len(), 3);
    }

    #[tokio::test]
    async fn partial_missing_replicas() {
        let pt = make_process_table();
        insert_running_agent(&pt, "worker-1");
        let ctrl = make_controller(pt);

        let mut desired = make_desired("app", "worker-1");
        desired.replicas = 3;
        ctrl.set_desired("app/worker-1".into(), desired);

        let drifts = ctrl.tick().await;
        // 3 desired, 1 running = 2 missing
        let missing: Vec<_> = drifts
            .iter()
            .filter(|d| matches!(d, DriftEvent::AgentMissing { .. }))
            .collect();
        assert_eq!(missing.len(), 2);
    }

    #[tokio::test]
    async fn drift_history_bounded() {
        let pt = make_process_table();
        let ctrl = make_controller(pt);

        // Insert 150 drifts manually via record_action
        for i in 0..150 {
            ctrl.record_action(DriftEvent::AgentMissing {
                agent_id: format!("agent-{i}"),
                app_id: "app".into(),
            })
            .await;
        }

        let history = ctrl.recent_drifts().await;
        assert!(history.len() <= 100);
    }

    #[tokio::test]
    async fn set_and_remove_desired() {
        let pt = make_process_table();
        let ctrl = make_controller(pt);

        ctrl.set_desired("app/w1".into(), make_desired("app", "w1"));
        assert_eq!(ctrl.desired_count(), 1);

        let removed = ctrl.remove_desired("app/w1");
        assert!(removed.is_some());
        assert_eq!(ctrl.desired_count(), 0);
    }

    #[tokio::test]
    async fn clear_desired() {
        let pt = make_process_table();
        let ctrl = make_controller(pt);

        ctrl.set_desired("app/w1".into(), make_desired("app", "w1"));
        ctrl.set_desired("app/w2".into(), make_desired("app", "w2"));
        assert_eq!(ctrl.desired_count(), 2);

        ctrl.clear_desired();
        assert_eq!(ctrl.desired_count(), 0);
    }

    #[tokio::test]
    async fn interval_accessor() {
        let pt = make_process_table();
        let ctrl = ReconciliationController::new(pt, Duration::from_secs(10));
        assert_eq!(ctrl.interval(), Duration::from_secs(10));
    }

    #[test]
    fn drift_event_serde_roundtrip() {
        let events = vec![
            DriftEvent::AgentMissing {
                agent_id: "w1".into(),
                app_id: "app".into(),
            },
            DriftEvent::ExtraAgent {
                pid: 42,
                agent_id: "rogue".into(),
            },
            DriftEvent::WrongState {
                pid: 5,
                expected: "running".into(),
                actual: "stopped".into(),
            },
        ];
        for event in events {
            let json = serde_json::to_string(&event).unwrap();
            let _: DriftEvent = serde_json::from_str(&json).unwrap();
        }
    }

    #[test]
    fn desired_agent_state_serde() {
        let state = make_desired("app", "worker-1");
        let json = serde_json::to_string(&state).unwrap();
        let restored: DesiredAgentState = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.agent_id, "worker-1");
        assert_eq!(restored.app_id, "app");
    }
}
