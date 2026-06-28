//! Per-agent tree views with capability-based access filtering (K5-G3).
//!
//! [`AgentTreeView`] wraps tree-like access and filters paths based on an
//! agent's capabilities. Agents only see authorized subtrees.
//!
//! This is a stand-alone module that does not depend on `TreeManager` directly
//! (which requires `exochain`). Instead it provides path-level authorization
//! that can be composed with any tree backend.

use serde::{Deserialize, Serialize};

use crate::error::KernelError;
use crate::process::Pid;

// ---------------------------------------------------------------------------
// TreeScope
// ---------------------------------------------------------------------------

/// Defines what subtrees an agent is allowed to access.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TreeScope {
    /// Full access to the entire tree.
    Full,
    /// Agent sees only its own subtree and read-only kernel config.
    Restricted {
        /// The agent's own subtree root.
        agent_path: String,
    },
    /// Agent sees its own subtree and specific namespace(s).
    Namespace {
        /// The agent's own subtree root.
        agent_path: String,
        /// Additional namespace paths the agent can access.
        namespaces: Vec<String>,
    },
    /// No tree access.
    None,
}

// ---------------------------------------------------------------------------
// AgentTreeView
// ---------------------------------------------------------------------------

/// A filtered view of the resource tree scoped to an agent's capabilities.
///
/// All path operations pass through the view's authorization filter.
/// Unauthorized access returns [`KernelError::CapabilityDenied`].
pub struct AgentTreeView {
    /// Agent identifier.
    pub agent_id: String,
    /// Agent's PID.
    pub pid: Pid,
    /// Tree scope defining allowed paths.
    pub scope: TreeScope,
}

impl AgentTreeView {
    /// Create a new tree view for an agent.
    pub fn new(agent_id: String, pid: Pid, scope: TreeScope) -> Self {
        Self {
            agent_id,
            pid,
            scope,
        }
    }

    /// Create a full-access tree view.
    pub fn full_access(agent_id: String, pid: Pid) -> Self {
        Self::new(agent_id, pid, TreeScope::Full)
    }

    /// Create a restricted tree view for an agent.
    pub fn restricted(agent_id: String, pid: Pid) -> Self {
        let agent_path = format!("/agents/{agent_id}");
        Self::new(agent_id, pid, TreeScope::Restricted { agent_path })
    }

    /// Create a namespace-scoped tree view.
    pub fn namespace_scoped(agent_id: String, pid: Pid, namespaces: Vec<String>) -> Self {
        let agent_path = format!("/agents/{agent_id}");
        Self::new(
            agent_id,
            pid,
            TreeScope::Namespace {
                agent_path,
                namespaces,
            },
        )
    }

    /// Check whether a path is authorized for reading.
    pub fn can_read(&self, path: &str) -> bool {
        match &self.scope {
            TreeScope::Full => true,
            TreeScope::Restricted { agent_path } => {
                path.starts_with(agent_path) || path.starts_with("/kernel/config/") // read-only config access
            }
            TreeScope::Namespace {
                agent_path,
                namespaces,
            } => path.starts_with(agent_path) || namespaces.iter().any(|ns| path.starts_with(ns)),
            TreeScope::None => false,
        }
    }

    /// Check whether a path is authorized for writing.
    pub fn can_write(&self, path: &str) -> bool {
        match &self.scope {
            TreeScope::Full => true,
            TreeScope::Restricted { agent_path } => {
                // Restricted agents can only write within their own subtree.
                path.starts_with(agent_path)
            }
            TreeScope::Namespace {
                agent_path,
                namespaces,
            } => path.starts_with(agent_path) || namespaces.iter().any(|ns| path.starts_with(ns)),
            TreeScope::None => false,
        }
    }

    /// Assert read access or return a permission error.
    pub fn assert_read(&self, path: &str) -> Result<(), KernelError> {
        if self.can_read(path) {
            Ok(())
        } else {
            Err(KernelError::CapabilityDenied {
                pid: self.pid,
                action: format!("read path '{path}'"),
                reason: format!("agent '{}' not authorized for path '{path}'", self.agent_id),
            })
        }
    }

    /// Assert write access or return a permission error.
    pub fn assert_write(&self, path: &str) -> Result<(), KernelError> {
        if self.can_write(path) {
            Ok(())
        } else {
            Err(KernelError::CapabilityDenied {
                pid: self.pid,
                action: format!("write path '{path}'"),
                reason: format!(
                    "agent '{}' not authorized to write path '{path}'",
                    self.agent_id
                ),
            })
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn pid(n: u64) -> Pid {
        n
    }

    #[test]
    fn full_access_reads_everything() {
        let view = AgentTreeView::full_access("admin".into(), pid(1));
        assert!(view.can_read("/kernel/services/health"));
        assert!(view.can_read("/agents/other"));
        assert!(view.can_read("/kernel/secrets/api"));
    }

    #[test]
    fn full_access_writes_everything() {
        let view = AgentTreeView::full_access("admin".into(), pid(1));
        assert!(view.can_write("/kernel/config/app/key"));
        assert!(view.can_write("/agents/admin/state"));
    }

    #[test]
    fn restricted_sees_own_subtree() {
        let view = AgentTreeView::restricted("worker-1".into(), pid(2));
        assert!(view.can_read("/agents/worker-1/state"));
        assert!(view.can_read("/agents/worker-1/logs/entry"));
    }

    #[test]
    fn restricted_sees_config_readonly() {
        let view = AgentTreeView::restricted("worker-1".into(), pid(2));
        assert!(view.can_read("/kernel/config/app/timeout"));
        assert!(!view.can_write("/kernel/config/app/timeout"));
    }

    #[test]
    fn restricted_cannot_see_other_agents() {
        let view = AgentTreeView::restricted("worker-1".into(), pid(2));
        assert!(!view.can_read("/agents/worker-2/state"));
    }

    #[test]
    fn restricted_cannot_see_secrets() {
        let view = AgentTreeView::restricted("worker-1".into(), pid(2));
        assert!(!view.can_read("/kernel/secrets/api_key"));
    }

    #[test]
    fn namespace_scoped_sees_namespaces() {
        let view = AgentTreeView::namespace_scoped(
            "ns-agent".into(),
            pid(3),
            vec!["/namespaces/prod".into()],
        );
        assert!(view.can_read("/agents/ns-agent/state"));
        assert!(view.can_read("/namespaces/prod/config"));
        assert!(view.can_write("/namespaces/prod/data"));
    }

    #[test]
    fn namespace_scoped_cannot_see_other_namespaces() {
        let view = AgentTreeView::namespace_scoped(
            "ns-agent".into(),
            pid(3),
            vec!["/namespaces/prod".into()],
        );
        assert!(!view.can_read("/namespaces/staging/config"));
    }

    #[test]
    fn none_scope_denies_everything() {
        let view = AgentTreeView::new("isolated".into(), pid(4), TreeScope::None);
        assert!(!view.can_read("/anything"));
        assert!(!view.can_write("/anything"));
    }

    #[test]
    fn assert_read_ok() {
        let view = AgentTreeView::full_access("admin".into(), pid(1));
        assert!(view.assert_read("/kernel/services").is_ok());
    }

    #[test]
    fn assert_read_denied() {
        let view = AgentTreeView::restricted("worker".into(), pid(2));
        let result = view.assert_read("/kernel/secrets/key");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("denied"), "got: {err}");
    }

    #[test]
    fn assert_write_ok() {
        let view = AgentTreeView::restricted("worker".into(), pid(2));
        assert!(view.assert_write("/agents/worker/data").is_ok());
    }

    #[test]
    fn assert_write_denied() {
        let view = AgentTreeView::restricted("worker".into(), pid(2));
        let result = view.assert_write("/kernel/config/x");
        assert!(result.is_err());
    }

    #[test]
    fn restricted_write_to_own_subtree() {
        let view = AgentTreeView::restricted("my-agent".into(), pid(5));
        assert!(view.can_write("/agents/my-agent/data"));
        assert!(!view.can_write("/agents/other-agent/data"));
    }
}
