//! Mesh coordination for cross-project assessment exchange (K6.6).
//!
//! Defines the protocol messages exchanged between assessment services
//! running on different WeftOS kernel instances, and the coordinator
//! that tracks peer assessment state via gossip.

use std::collections::HashMap;
use std::sync::Mutex;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use super::AssessmentReport;

// ── Protocol messages ─────────────────────────────────────────────

/// Messages exchanged between assessment services across the mesh.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AssessmentMessage {
    /// Broadcast: a new assessment completed on this node.
    ReportAvailable {
        node_id: String,
        project_name: String,
        timestamp: String,
        files_scanned: usize,
        finding_count: usize,
        coherence_score: f64,
    },
    /// Request: fetch the full latest report from a peer.
    RequestReport { requesting_node: String },
    /// Response: the full assessment report.
    FullReport { report: AssessmentReport },
    /// Gossip: lightweight status exchange (sent periodically).
    Gossip {
        node_id: String,
        project_name: String,
        last_assessment: Option<String>,
        finding_count: usize,
        analyzer_count: usize,
    },
}

// ── Peer state ────────────────────────────────────────────────────

/// Snapshot of a peer's assessment state, learned via gossip.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerAssessmentState {
    pub node_id: String,
    pub project_name: String,
    pub last_assessment: Option<String>,
    pub finding_count: usize,
    pub analyzer_count: usize,
    pub last_gossip: String,
}

// ── Coordinator ───────────────────────────────────────────────────

/// Tracks peer assessment states and handles mesh protocol messages.
///
/// Lives inside `AssessmentService` (behind `Option`) and is only
/// active when `[mesh] enabled = true` in the project config.
pub struct MeshCoordinator {
    /// Known peer assessment states (from gossip).
    peer_states: Mutex<HashMap<String, PeerAssessmentState>>,
    /// This node's ID.
    node_id: String,
    /// Project name from weave.toml.
    project_name: String,
    /// Pending outbound message produced after an assessment run.
    pending_broadcast: Mutex<Option<AssessmentMessage>>,
}

impl MeshCoordinator {
    /// Create a new coordinator for the given node and project.
    pub fn new(node_id: String, project_name: String) -> Self {
        Self {
            peer_states: Mutex::new(HashMap::new()),
            node_id,
            project_name,
            pending_broadcast: Mutex::new(None),
        }
    }

    /// Process an incoming mesh message, optionally returning a response.
    ///
    /// - `Gossip` updates peer state, no response.
    /// - `ReportAvailable` updates peer state, no response.
    /// - `RequestReport` returns nothing here (caller should check
    ///   `AssessmentService::get_latest()` and build a `FullReport`).
    /// - `FullReport` is handled by the caller (store the report).
    pub fn handle_message(&self, msg: AssessmentMessage) -> Option<AssessmentMessage> {
        match msg {
            AssessmentMessage::Gossip {
                ref node_id,
                ref project_name,
                ref last_assessment,
                finding_count,
                analyzer_count,
            } => {
                self.update_peer(PeerAssessmentState {
                    node_id: node_id.clone(),
                    project_name: project_name.clone(),
                    last_assessment: last_assessment.clone(),
                    finding_count,
                    analyzer_count,
                    last_gossip: Utc::now().to_rfc3339(),
                });
                None
            }
            AssessmentMessage::ReportAvailable {
                ref node_id,
                ref project_name,
                finding_count,
                ..
            } => {
                // Update peer state with the broadcast info.
                let mut peers = self.peer_states.lock().unwrap();
                let entry = peers
                    .entry(node_id.clone())
                    .or_insert_with(|| PeerAssessmentState {
                        node_id: node_id.clone(),
                        project_name: project_name.clone(),
                        last_assessment: None,
                        finding_count: 0,
                        analyzer_count: 0,
                        last_gossip: Utc::now().to_rfc3339(),
                    });
                entry.finding_count = finding_count;
                entry.last_gossip = Utc::now().to_rfc3339();
                None
            }
            AssessmentMessage::RequestReport { .. } => {
                // Caller should check AssessmentService::get_latest()
                // and wrap it in FullReport if available.
                None
            }
            AssessmentMessage::FullReport { .. } => {
                // Caller handles storing the received report.
                None
            }
        }
    }

    /// Build a gossip message from the latest assessment report.
    pub fn build_gossip(&self, report: &AssessmentReport) -> AssessmentMessage {
        AssessmentMessage::Gossip {
            node_id: self.node_id.clone(),
            project_name: self.project_name.clone(),
            last_assessment: Some(report.timestamp.to_rfc3339()),
            finding_count: report.findings.len(),
            analyzer_count: report.analyzers_run.len(),
        }
    }

    /// Build a `ReportAvailable` broadcast from the latest report.
    pub fn build_broadcast(&self, report: &AssessmentReport) -> AssessmentMessage {
        AssessmentMessage::ReportAvailable {
            node_id: self.node_id.clone(),
            project_name: self.project_name.clone(),
            timestamp: report.timestamp.to_rfc3339(),
            files_scanned: report.files_scanned,
            finding_count: report.findings.len(),
            coherence_score: report.summary.coherence_score,
        }
    }

    /// Return a snapshot of all known peer assessment states.
    pub fn peer_states(&self) -> Vec<PeerAssessmentState> {
        self.peer_states.lock().unwrap().values().cloned().collect()
    }

    /// Store or update a peer's assessment state.
    pub fn update_peer(&self, state: PeerAssessmentState) {
        self.peer_states
            .lock()
            .unwrap()
            .insert(state.node_id.clone(), state);
    }

    /// Store a pending broadcast message for the daemon to pick up.
    pub fn set_pending_broadcast(&self, msg: AssessmentMessage) {
        *self.pending_broadcast.lock().unwrap() = Some(msg);
    }

    /// Take the pending broadcast (returns `None` if already consumed).
    pub fn take_pending_broadcast(&self) -> Option<AssessmentMessage> {
        self.pending_broadcast.lock().unwrap().take()
    }

    /// This node's ID.
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    /// This node's project name.
    pub fn project_name(&self) -> &str {
        &self.project_name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assessment::{AssessmentReport, AssessmentSummary};
    use chrono::Utc;

    fn make_report() -> AssessmentReport {
        AssessmentReport {
            timestamp: Utc::now(),
            scope: "full".into(),
            project: "/tmp/test".into(),
            files_scanned: 42,
            summary: AssessmentSummary {
                total_files: 42,
                coherence_score: 0.85,
                ..Default::default()
            },
            findings: vec![],
            analyzers_run: vec!["complexity".into(), "security".into()],
        }
    }

    #[test]
    fn gossip_roundtrip() {
        let coord = MeshCoordinator::new("node-1".into(), "my-project".into());
        let report = make_report();
        let gossip = coord.build_gossip(&report);

        // Simulate receiving our own gossip on another node
        let coord2 = MeshCoordinator::new("node-2".into(), "other-project".into());
        let response = coord2.handle_message(gossip);
        assert!(response.is_none());

        let peers = coord2.peer_states();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].node_id, "node-1");
        assert_eq!(peers[0].project_name, "my-project");
        assert_eq!(peers[0].analyzer_count, 2);
    }

    #[test]
    fn broadcast_updates_peer_state() {
        let coord = MeshCoordinator::new("node-1".into(), "proj-a".into());
        let report = make_report();
        let broadcast = coord.build_broadcast(&report);

        let coord2 = MeshCoordinator::new("node-2".into(), "proj-b".into());
        coord2.handle_message(broadcast);

        let peers = coord2.peer_states();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].finding_count, 0);
    }

    #[test]
    fn pending_broadcast_lifecycle() {
        let coord = MeshCoordinator::new("node-1".into(), "proj".into());
        assert!(coord.take_pending_broadcast().is_none());

        let report = make_report();
        let gossip = coord.build_gossip(&report);
        coord.set_pending_broadcast(gossip);

        assert!(coord.take_pending_broadcast().is_some());
        assert!(coord.take_pending_broadcast().is_none());
    }

    #[test]
    fn update_peer_overwrites() {
        let coord = MeshCoordinator::new("node-1".into(), "proj".into());
        coord.update_peer(PeerAssessmentState {
            node_id: "peer-1".into(),
            project_name: "proj-x".into(),
            last_assessment: None,
            finding_count: 5,
            analyzer_count: 2,
            last_gossip: Utc::now().to_rfc3339(),
        });
        coord.update_peer(PeerAssessmentState {
            node_id: "peer-1".into(),
            project_name: "proj-x".into(),
            last_assessment: Some("2026-01-01T00:00:00Z".into()),
            finding_count: 10,
            analyzer_count: 3,
            last_gossip: Utc::now().to_rfc3339(),
        });

        let peers = coord.peer_states();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].finding_count, 10);
    }
}
