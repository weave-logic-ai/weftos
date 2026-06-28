//! Application framework for WeftOS.
//!
//! Applications are packaged units that declare their agents, tools,
//! services, capabilities, and lifecycle hooks via a manifest file
//! (`weftapp.toml` or `weftapp.json`). The kernel manages application
//! installation, startup, shutdown, and removal.
//!
//! # Design
//!
//! All types compile unconditionally. The `AppManager` tracks installed
//! applications and their lifecycle state. Actual filesystem operations
//! (install from disk, hook execution) require the `native` feature
//! and a running async runtime -- those integrations are future work.
//!
//! Agent IDs are namespaced as `app-name/agent-id` to avoid conflicts
//! between apps and with built-in agents.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::capability::{AgentCapabilities, IpcScope};
use crate::container::PortMapping;
#[cfg(feature = "exochain")]
use crate::gate::GateBackend;
use crate::process::Pid;
use crate::supervisor::SpawnRequest;

// ── Manifest Types ──────────────────────────────────────────────────

/// Application manifest, parsed from `weftapp.toml` or `weftapp.json`.
///
/// Declares the agents, tools, services, capabilities, and lifecycle
/// hooks for a WeftOS application.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppManifest {
    /// Application name (unique identifier).
    pub name: String,

    /// Semantic version string.
    pub version: String,

    /// Human-readable description.
    #[serde(default)]
    pub description: String,

    /// Application author.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,

    /// License identifier (SPDX).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,

    /// Agent specifications.
    #[serde(default)]
    pub agents: Vec<AgentSpec>,

    /// Tool specifications.
    #[serde(default)]
    pub tools: Vec<ToolSpec>,

    /// Service specifications (containers, processes).
    #[serde(default)]
    pub services: Vec<ServiceSpec>,

    /// Application-level capability requirements.
    #[serde(default)]
    pub capabilities: AppCapabilities,

    /// Lifecycle hooks.
    #[serde(default)]
    pub hooks: AppHooks,
}

/// Specification for an agent within an application.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSpec {
    /// Agent identifier (scoped to the app: `app-name/id`).
    pub id: String,

    /// Agent role (e.g. "code-review", "report-generator").
    #[serde(default)]
    pub role: String,

    /// Capabilities for this agent.
    #[serde(default)]
    pub capabilities: AgentCapabilities,

    /// Whether to start this agent automatically when the app starts.
    #[serde(default)]
    pub auto_start: bool,
}

/// Specification for a tool provided by an application.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    /// Tool name (scoped to the app: `app-name/name`).
    pub name: String,

    /// Where the tool implementation comes from.
    pub source: ToolSource,

    /// JSON Schema for the tool's input parameters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<serde_json::Value>,
}

/// Source of a tool implementation.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolSource {
    /// WASM module (path relative to app directory).
    Wasm(String),
    /// Built-in native tool (name).
    Native(String),
    /// Skill file (path relative to app directory).
    Skill(String),
}

/// Specification for a sidecar service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceSpec {
    /// Service name.
    pub name: String,

    /// Docker image (for container services).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,

    /// Native command (for process services).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,

    /// Port mappings.
    #[serde(default)]
    pub ports: Vec<PortMapping>,

    /// Environment variables.
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Health check endpoint URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_endpoint: Option<String>,
}

/// Application-level capability requirements.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppCapabilities {
    /// Whether the app needs network access.
    #[serde(default)]
    pub network: bool,

    /// Filesystem paths the app needs access to.
    #[serde(default)]
    pub filesystem: Vec<String>,

    /// Whether the app needs shell access.
    #[serde(default)]
    pub shell: bool,

    /// IPC scope for the app's agents.
    #[serde(default)]
    pub ipc: IpcScope,
}

/// Lifecycle hooks (scripts run at lifecycle transitions).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppHooks {
    /// Script to run after installation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_install: Option<String>,

    /// Script to run before starting agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_start: Option<String>,

    /// Script to run after stopping agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_stop: Option<String>,

    /// Script to run before removal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_remove: Option<String>,
}

// ── Application Lifecycle ───────────────────────────────────────────

/// Application lifecycle state.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AppState {
    /// Installed but not started.
    Installed,
    /// Starting agents and services.
    Starting,
    /// All agents and services running.
    Running,
    /// Shutting down agents and services.
    Stopping,
    /// Stopped (can be restarted).
    Stopped,
    /// Failed with a reason.
    Failed(String),
}

impl std::fmt::Display for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppState::Installed => write!(f, "installed"),
            AppState::Starting => write!(f, "starting"),
            AppState::Running => write!(f, "running"),
            AppState::Stopping => write!(f, "stopping"),
            AppState::Stopped => write!(f, "stopped"),
            AppState::Failed(reason) => write!(f, "failed: {reason}"),
        }
    }
}

/// An installed application with its runtime state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledApp {
    /// Application manifest.
    pub manifest: AppManifest,

    /// Current lifecycle state.
    pub state: AppState,

    /// When the app was installed.
    pub installed_at: DateTime<Utc>,

    /// PIDs of agents spawned by this app (populated at start time).
    #[serde(default)]
    pub agent_pids: Vec<Pid>,

    /// Names of services started by this app (populated at start time).
    #[serde(default)]
    pub service_names: Vec<String>,
}

// ── Errors ──────────────────────────────────────────────────────────

/// Application framework errors.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// Manifest file not found.
    #[error("manifest not found at '{path}'")]
    ManifestNotFound {
        /// Path that was checked.
        path: String,
    },

    /// Manifest parsing failed.
    #[error("invalid manifest: {reason}")]
    ManifestInvalid {
        /// Why parsing failed.
        reason: String,
    },

    /// App with this name already installed.
    #[error("app already installed: '{name}'")]
    AlreadyInstalled {
        /// App name.
        name: String,
    },

    /// App not found.
    #[error("app not found: '{name}'")]
    NotFound {
        /// App name.
        name: String,
    },

    /// Invalid state for the requested operation.
    #[error("invalid state for app '{name}': expected {expected}, got {actual}")]
    InvalidState {
        /// App name.
        name: String,
        /// Expected state description.
        expected: String,
        /// Actual state.
        actual: String,
    },

    /// Agent spawn failed.
    #[error("failed to spawn agent '{agent_id}' for app '{app_name}': {reason}")]
    SpawnFailed {
        /// App name.
        app_name: String,
        /// Agent ID within the app.
        agent_id: String,
        /// Failure reason.
        reason: String,
    },

    /// Governance gate denied the operation.
    #[error("governance denied app operation '{action}': {reason}")]
    GovernanceDenied {
        /// The action that was denied.
        action: String,
        /// Reason for denial.
        reason: String,
    },

    /// Hook execution failed.
    #[error("hook '{hook}' failed for app '{app_name}': {reason}")]
    HookFailed {
        /// App name.
        app_name: String,
        /// Hook name (on_install, on_start, etc.).
        hook: String,
        /// Failure reason.
        reason: String,
    },
}

// ── Manifest Validation ─────────────────────────────────────────────

/// Validate an application manifest for structural correctness.
///
/// Checks:
/// - Name is non-empty and contains only valid characters
/// - Version is non-empty
/// - Agent IDs are unique within the app
/// - Tool names are unique within the app
/// - Service names are unique within the app
/// - Tool sources are valid variants
pub fn validate_manifest(manifest: &AppManifest) -> Result<(), AppError> {
    // Name validation
    if manifest.name.is_empty() {
        return Err(AppError::ManifestInvalid {
            reason: "app name must not be empty".into(),
        });
    }

    if !manifest
        .name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Err(AppError::ManifestInvalid {
            reason: format!(
                "app name '{}' contains invalid characters (use alphanumeric, - or _)",
                manifest.name
            ),
        });
    }

    // Version validation
    if manifest.version.is_empty() {
        return Err(AppError::ManifestInvalid {
            reason: "version must not be empty".into(),
        });
    }

    // Unique agent IDs
    let mut agent_ids = std::collections::HashSet::new();
    for agent in &manifest.agents {
        if agent.id.is_empty() {
            return Err(AppError::ManifestInvalid {
                reason: "agent id must not be empty".into(),
            });
        }
        if !agent_ids.insert(&agent.id) {
            return Err(AppError::ManifestInvalid {
                reason: format!("duplicate agent id: '{}'", agent.id),
            });
        }
    }

    // Unique tool names
    let mut tool_names = std::collections::HashSet::new();
    for tool in &manifest.tools {
        if tool.name.is_empty() {
            return Err(AppError::ManifestInvalid {
                reason: "tool name must not be empty".into(),
            });
        }
        if !tool_names.insert(&tool.name) {
            return Err(AppError::ManifestInvalid {
                reason: format!("duplicate tool name: '{}'", tool.name),
            });
        }
    }

    // Unique service names
    let mut service_names = std::collections::HashSet::new();
    for service in &manifest.services {
        if service.name.is_empty() {
            return Err(AppError::ManifestInvalid {
                reason: "service name must not be empty".into(),
            });
        }
        if !service_names.insert(&service.name) {
            return Err(AppError::ManifestInvalid {
                reason: format!("duplicate service name: '{}'", service.name),
            });
        }
    }

    Ok(())
}

// ── AppManager ──────────────────────────────────────────────────────

/// Application lifecycle manager.
///
/// Tracks installed applications and their lifecycle state. Agent
/// spawning and service starting are delegated to the supervisor and
/// container manager respectively -- those integrations are wired in
/// the kernel boot sequence.
pub struct AppManager {
    apps: DashMap<String, InstalledApp>,
    #[cfg(feature = "exochain")]
    chain_manager: Option<std::sync::Arc<crate::chain::ChainManager>>,
    #[cfg(feature = "exochain")]
    governance_gate: Option<std::sync::Arc<crate::gate::GovernanceGate>>,
}

impl AppManager {
    /// Create a new application manager.
    pub fn new() -> Self {
        Self {
            apps: DashMap::new(),
            #[cfg(feature = "exochain")]
            chain_manager: None,
            #[cfg(feature = "exochain")]
            governance_gate: None,
        }
    }

    /// Attach a chain manager for audit logging.
    #[cfg(feature = "exochain")]
    pub fn set_chain_manager(
        &mut self,
        chain_manager: Option<std::sync::Arc<crate::chain::ChainManager>>,
    ) {
        self.chain_manager = chain_manager;
    }

    /// Attach a governance gate for policy enforcement.
    #[cfg(feature = "exochain")]
    pub fn set_governance_gate(
        &mut self,
        gate: Option<std::sync::Arc<crate::gate::GovernanceGate>>,
    ) {
        self.governance_gate = gate;
    }

    /// Register an application from a parsed manifest.
    ///
    /// The app is placed in the `Installed` state. Call `transition_to`
    /// to advance the state (e.g., to `Starting` or `Running`).
    ///
    /// # Errors
    ///
    /// Returns `AppError::AlreadyInstalled` if an app with the same
    /// name is already registered.
    pub fn install(&self, manifest: AppManifest) -> Result<String, AppError> {
        validate_manifest(&manifest)?;

        let name = manifest.name.clone();

        if self.apps.contains_key(&name) {
            return Err(AppError::AlreadyInstalled { name: name.clone() });
        }

        // Governance gate — block install if policy denies it.
        #[cfg(feature = "exochain")]
        if let Some(ref gate) = self.governance_gate {
            let context = serde_json::json!({
                "app_name": &name,
                "version": &manifest.version,
                "effect": { "risk": 0.3, "security": 0.3 },
            });
            let decision = gate.check("kernel", "app.install", &context);
            if decision.is_deny() {
                return Err(AppError::GovernanceDenied {
                    action: "app.install".into(),
                    reason: format!("governance denied installing app '{name}'"),
                });
            }
        }

        debug!(app = %name, version = %manifest.version, "installing application");

        self.apps.insert(
            name.clone(),
            InstalledApp {
                manifest: manifest.clone(),
                state: AppState::Installed,
                installed_at: Utc::now(),
                agent_pids: Vec::new(),
                service_names: Vec::new(),
            },
        );

        // Chain logging — record the install event.
        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain_manager {
            cm.append(
                "app",
                crate::chain::EVENT_KIND_APP_INSTALL,
                Some(serde_json::json!({
                    "app_name": &name,
                    "version": &manifest.version,
                    "agents": manifest.agents.len(),
                    "tools": manifest.tools.len(),
                    "services": manifest.services.len(),
                })),
            );
        }

        Ok(name)
    }

    /// Transition an app to a new state.
    ///
    /// Validates that the transition is legal per the state machine:
    /// - Installed -> Starting
    /// - Starting -> Running | Failed
    /// - Running -> Stopping
    /// - Stopping -> Stopped | Failed
    /// - Stopped -> Starting
    ///
    /// # Errors
    ///
    /// Returns `AppError::NotFound` or `AppError::InvalidState`.
    pub fn transition_to(&self, name: &str, new_state: AppState) -> Result<(), AppError> {
        let mut entry = self.apps.get_mut(name).ok_or_else(|| AppError::NotFound {
            name: name.to_owned(),
        })?;

        let valid = matches!(
            (&entry.state, &new_state),
            (AppState::Installed, AppState::Starting)
                | (AppState::Starting, AppState::Running)
                | (AppState::Starting, AppState::Failed(_))
                | (AppState::Running, AppState::Stopping)
                | (AppState::Stopping, AppState::Stopped)
                | (AppState::Stopping, AppState::Failed(_))
                | (AppState::Stopped, AppState::Starting)
        );

        if !valid {
            return Err(AppError::InvalidState {
                name: name.to_owned(),
                expected: format!("valid transition from {}", entry.state),
                actual: format!("{} -> {new_state}", entry.state),
            });
        }

        let from_state = entry.state.to_string();
        let to_state = new_state.to_string();
        debug!(app = name, from = %from_state, to = %to_state, "state transition");
        entry.state = new_state;

        // Chain logging — record state transition.
        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain_manager {
            cm.append(
                "app",
                crate::chain::EVENT_KIND_APP_TRANSITION,
                Some(serde_json::json!({
                    "app_name": name,
                    "from": from_state,
                    "to": to_state,
                })),
            );
        }

        Ok(())
    }

    /// Record an agent PID for a running app.
    pub fn add_agent_pid(&self, name: &str, pid: Pid) -> Result<(), AppError> {
        let mut entry = self.apps.get_mut(name).ok_or_else(|| AppError::NotFound {
            name: name.to_owned(),
        })?;
        entry.agent_pids.push(pid);
        Ok(())
    }

    /// Record a service name for a running app.
    pub fn add_service_name(&self, name: &str, service_name: String) -> Result<(), AppError> {
        let mut entry = self.apps.get_mut(name).ok_or_else(|| AppError::NotFound {
            name: name.to_owned(),
        })?;
        entry.service_names.push(service_name);
        Ok(())
    }

    /// Remove an installed application.
    ///
    /// The app must be in `Installed`, `Stopped`, or `Failed` state.
    pub fn remove(&self, name: &str) -> Result<AppManifest, AppError> {
        let entry = self.apps.get(name).ok_or_else(|| AppError::NotFound {
            name: name.to_owned(),
        })?;

        let removable = matches!(
            entry.state,
            AppState::Installed | AppState::Stopped | AppState::Failed(_)
        );

        if !removable {
            return Err(AppError::InvalidState {
                name: name.to_owned(),
                expected: "Installed, Stopped, or Failed".into(),
                actual: entry.state.to_string(),
            });
        }

        // Governance gate — block removal if policy denies it.
        #[cfg(feature = "exochain")]
        if let Some(ref gate) = self.governance_gate {
            let context = serde_json::json!({
                "app_name": name,
                "state": entry.state.to_string(),
                "effect": { "risk": 0.4, "security": 0.2 },
            });
            let decision = gate.check("kernel", "app.remove", &context);
            if decision.is_deny() {
                return Err(AppError::GovernanceDenied {
                    action: "app.remove".into(),
                    reason: format!("governance denied removing app '{name}'"),
                });
            }
        }

        drop(entry); // release the read lock before remove
        let (_, app) = self.apps.remove(name).ok_or_else(|| AppError::NotFound {
            name: name.to_owned(),
        })?;

        debug!(app = name, "removed application");

        // Chain logging — record the removal event.
        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain_manager {
            cm.append(
                "app",
                crate::chain::EVENT_KIND_APP_REMOVE,
                Some(serde_json::json!({
                    "app_name": name,
                    "version": &app.manifest.version,
                })),
            );
        }

        Ok(app.manifest)
    }

    /// List all installed applications.
    pub fn list(&self) -> Vec<(String, AppState, String)> {
        self.apps
            .iter()
            .map(|entry| {
                (
                    entry.key().clone(),
                    entry.state.clone(),
                    entry.manifest.version.clone(),
                )
            })
            .collect()
    }

    /// Get details for an installed application.
    pub fn inspect(&self, name: &str) -> Result<InstalledApp, AppError> {
        self.apps
            .get(name)
            .map(|e| e.value().clone())
            .ok_or_else(|| AppError::NotFound {
                name: name.to_owned(),
            })
    }

    /// Get the number of installed apps.
    pub fn len(&self) -> usize {
        self.apps.len()
    }

    /// Check whether any apps are installed.
    pub fn is_empty(&self) -> bool {
        self.apps.is_empty()
    }

    /// Get namespaced agent IDs for an app's manifest.
    ///
    /// Returns IDs in the form `app-name/agent-id`.
    pub fn namespaced_agent_ids(manifest: &AppManifest) -> Vec<String> {
        manifest
            .agents
            .iter()
            .map(|a| format!("{}/{}", manifest.name, a.id))
            .collect()
    }

    /// Get namespaced tool names for an app's manifest.
    ///
    /// Returns names in the form `app-name/tool-name`.
    pub fn namespaced_tool_names(manifest: &AppManifest) -> Vec<String> {
        manifest
            .tools
            .iter()
            .map(|t| format!("{}/{}", manifest.name, t.name))
            .collect()
    }

    /// Start an installed or stopped application.
    ///
    /// Transitions the app through `Starting` to `Running` and builds
    /// [`SpawnRequest`]s for each agent declared in the manifest. The
    /// caller (kernel boot / CLI) is responsible for executing the spawn
    /// requests via the [`AgentSupervisor`].
    ///
    /// Returns the list of spawn requests so the caller can hand them to
    /// the supervisor.
    ///
    /// # Errors
    ///
    /// Returns `AppError::NotFound` if the app is not installed, or
    /// `AppError::InvalidState` if the app is not in `Installed` or
    /// `Stopped` state.
    pub fn start(&self, name: &str) -> Result<Vec<SpawnRequest>, AppError> {
        // Validate current state allows starting.
        {
            let entry = self.apps.get(name).ok_or_else(|| AppError::NotFound {
                name: name.to_owned(),
            })?;
            let startable = matches!(entry.state, AppState::Installed | AppState::Stopped);
            if !startable {
                return Err(AppError::InvalidState {
                    name: name.to_owned(),
                    expected: "Installed or Stopped".into(),
                    actual: entry.state.to_string(),
                });
            }
        }

        // Transition: current -> Starting
        self.transition_to(name, AppState::Starting)?;

        // Build spawn requests from manifest agent specs.
        let spawn_requests = {
            let entry = self.apps.get(name).ok_or_else(|| AppError::NotFound {
                name: name.to_owned(),
            })?;
            Self::build_spawn_requests(&entry.manifest)
        };

        // Transition: Starting -> Running
        self.transition_to(name, AppState::Running)?;

        debug!(
            app = name,
            agents = spawn_requests.len(),
            "application started"
        );

        // Chain logging — record the start event.
        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain_manager {
            cm.append(
                "app",
                crate::chain::EVENT_KIND_APP_START,
                Some(serde_json::json!({
                    "app_name": name,
                    "agents_spawned": spawn_requests.len(),
                })),
            );
        }

        Ok(spawn_requests)
    }

    /// Stop a running application.
    ///
    /// Transitions `Running` -> `Stopping` -> `Stopped` and clears the
    /// recorded agent PIDs and service names.
    ///
    /// # Errors
    ///
    /// Returns `AppError::NotFound` or `AppError::InvalidState`.
    pub fn stop(&self, name: &str) -> Result<(), AppError> {
        {
            let entry = self.apps.get(name).ok_or_else(|| AppError::NotFound {
                name: name.to_owned(),
            })?;
            if entry.state != AppState::Running {
                return Err(AppError::InvalidState {
                    name: name.to_owned(),
                    expected: "Running".into(),
                    actual: entry.state.to_string(),
                });
            }
        }

        self.transition_to(name, AppState::Stopping)?;

        // Clear runtime bookkeeping.
        {
            let mut entry = self.apps.get_mut(name).ok_or_else(|| AppError::NotFound {
                name: name.to_owned(),
            })?;
            entry.agent_pids.clear();
            entry.service_names.clear();
        }

        self.transition_to(name, AppState::Stopped)?;

        debug!(app = name, "application stopped");

        // Chain logging — record the stop event.
        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain_manager {
            cm.append(
                "app",
                crate::chain::EVENT_KIND_APP_STOP,
                Some(serde_json::json!({
                    "app_name": name,
                })),
            );
        }

        Ok(())
    }

    /// Build [`SpawnRequest`]s for every agent declared in a manifest.
    ///
    /// Each request carries the agent's capabilities from the manifest
    /// and a namespaced agent ID (`app-name/agent-id`).
    pub fn build_spawn_requests(manifest: &AppManifest) -> Vec<SpawnRequest> {
        manifest
            .agents
            .iter()
            .map(|agent| SpawnRequest {
                agent_id: format!("{}/{}", manifest.name, agent.id),
                capabilities: Some(agent.capabilities.clone()),
                parent_pid: None,
                env: HashMap::new(),
                backend: None,
            })
            .collect()
    }
}

impl Default for AppManager {
    fn default() -> Self {
        Self::new()
    }
}

// ── Manifest Parsing ────────────────────────────────────────────────

impl AppManifest {
    /// Parse an [`AppManifest`] from a JSON string.
    ///
    /// The manifest is validated after parsing; structural errors
    /// (empty name, duplicate IDs, etc.) are returned as
    /// [`AppError::ManifestInvalid`].
    pub fn from_json_str(json: &str) -> Result<Self, AppError> {
        let manifest: AppManifest =
            serde_json::from_str(json).map_err(|e| AppError::ManifestInvalid {
                reason: format!("JSON parse error: {e}"),
            })?;
        validate_manifest(&manifest)?;
        Ok(manifest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_manifest() -> AppManifest {
        AppManifest {
            name: "code-reviewer".into(),
            version: "1.0.0".into(),
            description: "Automated code review app".into(),
            author: Some("WeftOS Team".into()),
            license: Some("MIT".into()),
            agents: vec![
                AgentSpec {
                    id: "reviewer".into(),
                    role: "code-review".into(),
                    capabilities: AgentCapabilities::default(),
                    auto_start: true,
                },
                AgentSpec {
                    id: "reporter".into(),
                    role: "report-generator".into(),
                    capabilities: AgentCapabilities {
                        can_network: false,
                        ..Default::default()
                    },
                    auto_start: true,
                },
            ],
            tools: vec![ToolSpec {
                name: "diff-analyzer".into(),
                source: ToolSource::Wasm("tools/diff-analyzer.wasm".into()),
                schema: None,
            }],
            services: vec![ServiceSpec {
                name: "review-db".into(),
                image: Some("redis:7-alpine".into()),
                command: None,
                ports: vec![PortMapping {
                    host_port: 6380,
                    container_port: 6379,
                    protocol: "tcp".into(),
                }],
                env: HashMap::new(),
                health_endpoint: Some("redis://localhost:6380".into()),
            }],
            capabilities: AppCapabilities {
                network: true,
                filesystem: vec!["/workspace".into()],
                shell: false,
                ipc: IpcScope::All,
            },
            hooks: AppHooks {
                on_install: Some("scripts/setup.sh".into()),
                on_start: Some("scripts/migrate.sh".into()),
                on_stop: None,
                on_remove: None,
            },
        }
    }

    #[test]
    fn manifest_serde_roundtrip() {
        let manifest = sample_manifest();
        let json = serde_json::to_string_pretty(&manifest).unwrap();
        let restored: AppManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.name, "code-reviewer");
        assert_eq!(restored.version, "1.0.0");
        assert_eq!(restored.agents.len(), 2);
        assert_eq!(restored.tools.len(), 1);
        assert_eq!(restored.services.len(), 1);
    }

    #[test]
    fn manifest_minimal_serde() {
        let json = r#"{"name":"my-app","version":"0.1.0"}"#;
        let manifest: AppManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.name, "my-app");
        assert!(manifest.agents.is_empty());
        assert!(manifest.tools.is_empty());
        assert!(manifest.services.is_empty());
        assert!(!manifest.capabilities.network);
    }

    #[test]
    fn validate_manifest_ok() {
        let manifest = sample_manifest();
        assert!(validate_manifest(&manifest).is_ok());
    }

    #[test]
    fn validate_manifest_empty_name() {
        let mut manifest = sample_manifest();
        manifest.name = String::new();
        let err = validate_manifest(&manifest).unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn validate_manifest_invalid_name_chars() {
        let mut manifest = sample_manifest();
        manifest.name = "my app!".into();
        let err = validate_manifest(&manifest).unwrap_err();
        assert!(err.to_string().contains("invalid characters"));
    }

    #[test]
    fn validate_manifest_empty_version() {
        let mut manifest = sample_manifest();
        manifest.version = String::new();
        let err = validate_manifest(&manifest).unwrap_err();
        assert!(err.to_string().contains("version"));
    }

    #[test]
    fn validate_manifest_duplicate_agent_ids() {
        let mut manifest = sample_manifest();
        manifest.agents.push(AgentSpec {
            id: "reviewer".into(), // duplicate
            role: "other".into(),
            capabilities: AgentCapabilities::default(),
            auto_start: false,
        });
        let err = validate_manifest(&manifest).unwrap_err();
        assert!(err.to_string().contains("duplicate agent"));
    }

    #[test]
    fn validate_manifest_duplicate_tool_names() {
        let mut manifest = sample_manifest();
        manifest.tools.push(ToolSpec {
            name: "diff-analyzer".into(), // duplicate
            source: ToolSource::Native("builtin".into()),
            schema: None,
        });
        let err = validate_manifest(&manifest).unwrap_err();
        assert!(err.to_string().contains("duplicate tool"));
    }

    #[test]
    fn validate_manifest_duplicate_service_names() {
        let mut manifest = sample_manifest();
        manifest.services.push(ServiceSpec {
            name: "review-db".into(), // duplicate
            image: None,
            command: Some("redis-server".into()),
            ports: Vec::new(),
            env: HashMap::new(),
            health_endpoint: None,
        });
        let err = validate_manifest(&manifest).unwrap_err();
        assert!(err.to_string().contains("duplicate service"));
    }

    #[test]
    fn app_state_display() {
        assert_eq!(AppState::Installed.to_string(), "installed");
        assert_eq!(AppState::Running.to_string(), "running");
        assert_eq!(AppState::Stopped.to_string(), "stopped");
        assert_eq!(
            AppState::Failed("timeout".into()).to_string(),
            "failed: timeout"
        );
    }

    #[test]
    fn install_and_list() {
        let manager = AppManager::new();
        let name = manager.install(sample_manifest()).unwrap();
        assert_eq!(name, "code-reviewer");

        let list = manager.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].0, "code-reviewer");
        assert_eq!(list[0].1, AppState::Installed);
    }

    #[test]
    fn install_duplicate_fails() {
        let manager = AppManager::new();
        manager.install(sample_manifest()).unwrap();
        let err = manager.install(sample_manifest()).unwrap_err();
        assert!(matches!(err, AppError::AlreadyInstalled { .. }));
    }

    #[test]
    fn inspect_installed_app() {
        let manager = AppManager::new();
        manager.install(sample_manifest()).unwrap();
        let app = manager.inspect("code-reviewer").unwrap();
        assert_eq!(app.state, AppState::Installed);
        assert_eq!(app.manifest.agents.len(), 2);
    }

    #[test]
    fn inspect_not_found() {
        let manager = AppManager::new();
        assert!(matches!(
            manager.inspect("nope"),
            Err(AppError::NotFound { .. })
        ));
    }

    #[test]
    fn state_transitions() {
        let manager = AppManager::new();
        manager.install(sample_manifest()).unwrap();

        // Installed -> Starting -> Running -> Stopping -> Stopped
        manager
            .transition_to("code-reviewer", AppState::Starting)
            .unwrap();
        manager
            .transition_to("code-reviewer", AppState::Running)
            .unwrap();
        manager
            .transition_to("code-reviewer", AppState::Stopping)
            .unwrap();
        manager
            .transition_to("code-reviewer", AppState::Stopped)
            .unwrap();

        let app = manager.inspect("code-reviewer").unwrap();
        assert_eq!(app.state, AppState::Stopped);
    }

    #[test]
    fn state_transition_restart() {
        let manager = AppManager::new();
        manager.install(sample_manifest()).unwrap();

        manager
            .transition_to("code-reviewer", AppState::Starting)
            .unwrap();
        manager
            .transition_to("code-reviewer", AppState::Running)
            .unwrap();
        manager
            .transition_to("code-reviewer", AppState::Stopping)
            .unwrap();
        manager
            .transition_to("code-reviewer", AppState::Stopped)
            .unwrap();
        // Restart: Stopped -> Starting
        manager
            .transition_to("code-reviewer", AppState::Starting)
            .unwrap();
    }

    #[test]
    fn invalid_state_transition() {
        let manager = AppManager::new();
        manager.install(sample_manifest()).unwrap();

        // Installed -> Running (should fail, must go through Starting)
        let err = manager
            .transition_to("code-reviewer", AppState::Running)
            .unwrap_err();
        assert!(matches!(err, AppError::InvalidState { .. }));
    }

    #[test]
    fn state_transition_to_failed() {
        let manager = AppManager::new();
        manager.install(sample_manifest()).unwrap();

        manager
            .transition_to("code-reviewer", AppState::Starting)
            .unwrap();
        manager
            .transition_to("code-reviewer", AppState::Failed("agent crash".into()))
            .unwrap();

        let app = manager.inspect("code-reviewer").unwrap();
        assert_eq!(app.state, AppState::Failed("agent crash".into()));
    }

    #[test]
    fn remove_installed_app() {
        let manager = AppManager::new();
        manager.install(sample_manifest()).unwrap();
        let manifest = manager.remove("code-reviewer").unwrap();
        assert_eq!(manifest.name, "code-reviewer");
        assert!(manager.is_empty());
    }

    #[test]
    fn remove_running_app_fails() {
        let manager = AppManager::new();
        manager.install(sample_manifest()).unwrap();
        manager
            .transition_to("code-reviewer", AppState::Starting)
            .unwrap();
        manager
            .transition_to("code-reviewer", AppState::Running)
            .unwrap();

        let err = manager.remove("code-reviewer").unwrap_err();
        assert!(matches!(err, AppError::InvalidState { .. }));
    }

    #[test]
    fn remove_stopped_app() {
        let manager = AppManager::new();
        manager.install(sample_manifest()).unwrap();
        manager
            .transition_to("code-reviewer", AppState::Starting)
            .unwrap();
        manager
            .transition_to("code-reviewer", AppState::Running)
            .unwrap();
        manager
            .transition_to("code-reviewer", AppState::Stopping)
            .unwrap();
        manager
            .transition_to("code-reviewer", AppState::Stopped)
            .unwrap();

        assert!(manager.remove("code-reviewer").is_ok());
    }

    #[test]
    fn add_agent_pid() {
        let manager = AppManager::new();
        manager.install(sample_manifest()).unwrap();
        manager.add_agent_pid("code-reviewer", 42).unwrap();

        let app = manager.inspect("code-reviewer").unwrap();
        assert_eq!(app.agent_pids, vec![42]);
    }

    #[test]
    fn add_service_name() {
        let manager = AppManager::new();
        manager.install(sample_manifest()).unwrap();
        manager
            .add_service_name("code-reviewer", "review-db".into())
            .unwrap();

        let app = manager.inspect("code-reviewer").unwrap();
        assert_eq!(app.service_names, vec!["review-db"]);
    }

    #[test]
    fn namespaced_ids() {
        let manifest = sample_manifest();
        let agent_ids = AppManager::namespaced_agent_ids(&manifest);
        assert_eq!(
            agent_ids,
            vec!["code-reviewer/reviewer", "code-reviewer/reporter"]
        );

        let tool_names = AppManager::namespaced_tool_names(&manifest);
        assert_eq!(tool_names, vec!["code-reviewer/diff-analyzer"]);
    }

    #[test]
    fn tool_source_variants() {
        let wasm = ToolSource::Wasm("tools/my.wasm".into());
        let native = ToolSource::Native("read_file".into());
        let skill = ToolSource::Skill("skills/REVIEW.md".into());

        // Serde roundtrip
        for source in &[wasm, native, skill] {
            let json = serde_json::to_string(source).unwrap();
            let _restored: ToolSource = serde_json::from_str(&json).unwrap();
        }
    }

    #[test]
    fn app_error_display() {
        let err = AppError::ManifestNotFound {
            path: "/tmp/weftapp.toml".into(),
        };
        assert!(err.to_string().contains("manifest not found"));

        let err = AppError::AlreadyInstalled {
            name: "my-app".into(),
        };
        assert!(err.to_string().contains("my-app"));

        let err = AppError::HookFailed {
            app_name: "my-app".into(),
            hook: "on_start".into(),
            reason: "exit code 1".into(),
        };
        assert!(err.to_string().contains("on_start"));
    }

    #[test]
    fn app_capabilities_serde_roundtrip() {
        let caps = AppCapabilities {
            network: true,
            filesystem: vec!["/workspace".into(), "/data".into()],
            shell: false,
            ipc: IpcScope::All,
        };
        let json = serde_json::to_string(&caps).unwrap();
        let restored: AppCapabilities = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, caps);
    }

    #[test]
    fn app_hooks_serde_roundtrip() {
        let hooks = AppHooks {
            on_install: Some("setup.sh".into()),
            on_start: None,
            on_stop: Some("cleanup.sh".into()),
            on_remove: None,
        };
        let json = serde_json::to_string(&hooks).unwrap();
        let restored: AppHooks = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.on_install.as_deref(), Some("setup.sh"));
        assert!(restored.on_start.is_none());
    }

    #[test]
    fn parse_manifest_from_json() {
        let json = serde_json::json!({
            "name": "test-app",
            "version": "1.0.0",
            "description": "A test app",
            "agents": [],
            "tools": [],
            "services": [],
            "capabilities": {
                "network": false,
                "filesystem": [],
                "shell": false,
                "ipc": "None"
            },
            "hooks": {}
        });
        let manifest = AppManifest::from_json_str(&json.to_string()).unwrap();
        assert_eq!(manifest.name, "test-app");
        assert_eq!(manifest.version, "1.0.0");
        assert!(manifest.agents.is_empty());
    }

    #[test]
    fn parse_manifest_from_json_invalid() {
        let result = AppManifest::from_json_str("not valid json");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("JSON parse error"));
    }

    #[test]
    fn parse_manifest_from_json_empty_name_fails() {
        let json = serde_json::json!({
            "name": "",
            "version": "1.0.0"
        });
        let result = AppManifest::from_json_str(&json.to_string());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty"));
    }

    // ── K5 Integration Tests ────────────────────────────────────────

    #[test]
    fn integration_app_full_lifecycle() {
        // Build a realistic app manifest
        let manifest = AppManifest {
            name: "data-pipeline".into(),
            version: "2.1.0".into(),
            description: "Real-time data ingestion and analysis pipeline".into(),
            author: Some("WeftOS Team".into()),
            license: Some("MIT".into()),
            agents: vec![
                AgentSpec {
                    id: "ingester".into(),
                    role: "data-ingestion".into(),
                    capabilities: AgentCapabilities::default(),
                    auto_start: true,
                },
                AgentSpec {
                    id: "analyzer".into(),
                    role: "data-analysis".into(),
                    capabilities: AgentCapabilities::default(),
                    auto_start: true,
                },
            ],
            tools: vec![ToolSpec {
                name: "transform".into(),
                source: ToolSource::Wasm("tools/transform.wasm".into()),
                schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "input": {"type": "string"},
                        "format": {"type": "string"}
                    }
                })),
            }],
            services: vec![ServiceSpec {
                name: "cache".into(),
                image: Some("redis:7-alpine".into()),
                command: None,
                ports: vec![PortMapping {
                    host_port: 6380,
                    container_port: 6379,
                    protocol: "tcp".into(),
                }],
                env: HashMap::from([("REDIS_MAX_MEMORY".into(), "256mb".into())]),
                health_endpoint: Some("redis://localhost:6380".into()),
            }],
            capabilities: AppCapabilities {
                network: true,
                filesystem: vec!["/data".into(), "/workspace".into()],
                shell: false,
                ipc: IpcScope::All,
            },
            hooks: AppHooks {
                on_install: Some("scripts/setup.sh".into()),
                on_start: Some("scripts/migrate.sh".into()),
                on_stop: Some("scripts/cleanup.sh".into()),
                on_remove: None,
            },
        };

        // 1. Validate
        validate_manifest(&manifest).unwrap();

        // 2. Verify namespacing
        let agent_ids = AppManager::namespaced_agent_ids(&manifest);
        assert_eq!(
            agent_ids,
            vec!["data-pipeline/ingester", "data-pipeline/analyzer"]
        );
        let tool_names = AppManager::namespaced_tool_names(&manifest);
        assert_eq!(tool_names, vec!["data-pipeline/transform"]);

        // 3. Install
        let manager = AppManager::new();
        let app_name = manager.install(manifest.clone()).unwrap();
        assert_eq!(app_name, "data-pipeline");

        // 4. List shows the app
        let list = manager.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].0, "data-pipeline");

        // 5. Inspect preserves all details
        let inspected = manager.inspect("data-pipeline").unwrap();
        assert_eq!(inspected.manifest.agents.len(), 2);
        assert_eq!(inspected.manifest.services.len(), 1);
        assert_eq!(inspected.manifest.tools.len(), 1);
        assert_eq!(inspected.manifest.services[0].name, "cache");
        assert_eq!(
            inspected.manifest.services[0].image,
            Some("redis:7-alpine".into())
        );
        assert!(inspected.manifest.capabilities.network);
        assert_eq!(
            inspected.manifest.capabilities.filesystem,
            vec!["/data", "/workspace"]
        );
        assert!(!inspected.manifest.capabilities.shell);
        assert_eq!(inspected.manifest.capabilities.ipc, IpcScope::All);
        assert_eq!(
            inspected.manifest.hooks.on_install,
            Some("scripts/setup.sh".into())
        );
        assert_eq!(
            inspected.manifest.hooks.on_start,
            Some("scripts/migrate.sh".into())
        );
        assert_eq!(
            inspected.manifest.hooks.on_stop,
            Some("scripts/cleanup.sh".into())
        );
        assert!(inspected.manifest.hooks.on_remove.is_none());
        assert_eq!(inspected.manifest.author, Some("WeftOS Team".into()));
        assert_eq!(inspected.manifest.license, Some("MIT".into()));

        // 6. Lifecycle transitions: Installed -> Starting -> Running
        manager
            .transition_to("data-pipeline", AppState::Starting)
            .unwrap();
        manager
            .transition_to("data-pipeline", AppState::Running)
            .unwrap();

        // 7. Simulate agent spawning (supervisor assigns PIDs)
        manager.add_agent_pid("data-pipeline", 10).unwrap(); // ingester
        manager.add_agent_pid("data-pipeline", 11).unwrap(); // analyzer

        // 8. Simulate service registration
        manager
            .add_service_name("data-pipeline", "cache".into())
            .unwrap();

        // 9. Verify running state with agents and services
        let running = manager.inspect("data-pipeline").unwrap();
        assert!(matches!(running.state, AppState::Running));
        assert_eq!(running.agent_pids.len(), 2);
        assert!(running.agent_pids.contains(&10));
        assert!(running.agent_pids.contains(&11));
        assert_eq!(running.service_names.len(), 1);
        assert!(running.service_names.contains(&"cache".to_string()));

        // 10. Stop: Running -> Stopping -> Stopped
        manager
            .transition_to("data-pipeline", AppState::Stopping)
            .unwrap();
        manager
            .transition_to("data-pipeline", AppState::Stopped)
            .unwrap();

        let stopped = manager.inspect("data-pipeline").unwrap();
        assert!(matches!(stopped.state, AppState::Stopped));

        // 11. Remove
        let removed = manager.remove("data-pipeline").unwrap();
        assert_eq!(removed.name, "data-pipeline");
        assert!(manager.is_empty());
    }

    #[test]
    fn integration_multi_app_isolation() {
        let manager = AppManager::new();

        // Install two apps with overlapping agent/tool names
        let app1 = AppManifest {
            name: "frontend".into(),
            version: "1.0.0".into(),
            description: "Web frontend".into(),
            author: None,
            license: None,
            agents: vec![AgentSpec {
                id: "worker".into(),
                role: "serve".into(),
                capabilities: AgentCapabilities::default(),
                auto_start: true,
            }],
            tools: vec![ToolSpec {
                name: "render".into(),
                source: ToolSource::Native("fs.read_file".into()),
                schema: None,
            }],
            services: vec![],
            capabilities: AppCapabilities {
                network: true,
                filesystem: vec![],
                shell: false,
                ipc: IpcScope::None,
            },
            hooks: AppHooks::default(),
        };

        let app2 = AppManifest {
            name: "backend".into(),
            version: "2.0.0".into(),
            description: "API backend".into(),
            author: None,
            license: None,
            agents: vec![AgentSpec {
                id: "worker".into(), // same agent ID as frontend
                role: "api".into(),
                capabilities: AgentCapabilities::default(),
                auto_start: true,
            }],
            tools: vec![ToolSpec {
                name: "render".into(), // same tool name as frontend
                source: ToolSource::Wasm("tools/render.wasm".into()),
                schema: None,
            }],
            services: vec![ServiceSpec {
                name: "db".into(),
                image: Some("postgres:16-alpine".into()),
                command: None,
                ports: vec![PortMapping {
                    host_port: 5432,
                    container_port: 5432,
                    protocol: "tcp".into(),
                }],
                env: HashMap::from([("POSTGRES_PASSWORD".into(), "dev".into())]),
                health_endpoint: None,
            }],
            capabilities: AppCapabilities {
                network: true,
                filesystem: vec!["/data".into()],
                shell: false,
                ipc: IpcScope::All,
            },
            hooks: AppHooks::default(),
        };

        manager.install(app1).unwrap();
        manager.install(app2).unwrap();

        // Both apps installed
        assert_eq!(manager.list().len(), 2);

        // Namespaces prevent conflicts despite identical agent/tool names
        let fe_agents =
            AppManager::namespaced_agent_ids(&manager.inspect("frontend").unwrap().manifest);
        let be_agents =
            AppManager::namespaced_agent_ids(&manager.inspect("backend").unwrap().manifest);
        assert_eq!(fe_agents, vec!["frontend/worker"]);
        assert_eq!(be_agents, vec!["backend/worker"]);

        let fe_tools =
            AppManager::namespaced_tool_names(&manager.inspect("frontend").unwrap().manifest);
        let be_tools =
            AppManager::namespaced_tool_names(&manager.inspect("backend").unwrap().manifest);
        assert_eq!(fe_tools, vec!["frontend/render"]);
        assert_eq!(be_tools, vec!["backend/render"]);

        // Each app transitions independently
        manager
            .transition_to("frontend", AppState::Starting)
            .unwrap();
        manager
            .transition_to("frontend", AppState::Running)
            .unwrap();
        // backend stays Installed

        let fe = manager.inspect("frontend").unwrap();
        let be = manager.inspect("backend").unwrap();
        assert!(matches!(fe.state, AppState::Running));
        assert!(matches!(be.state, AppState::Installed));

        // Backend can also transition without affecting frontend
        manager
            .transition_to("backend", AppState::Starting)
            .unwrap();
        manager.transition_to("backend", AppState::Running).unwrap();

        // Add PIDs to each app independently
        manager.add_agent_pid("frontend", 100).unwrap();
        manager.add_agent_pid("backend", 200).unwrap();

        let fe = manager.inspect("frontend").unwrap();
        let be = manager.inspect("backend").unwrap();
        assert_eq!(fe.agent_pids, vec![100]);
        assert_eq!(be.agent_pids, vec![200]);

        // Stop frontend, backend stays running
        manager
            .transition_to("frontend", AppState::Stopping)
            .unwrap();
        manager
            .transition_to("frontend", AppState::Stopped)
            .unwrap();

        let fe = manager.inspect("frontend").unwrap();
        let be = manager.inspect("backend").unwrap();
        assert!(matches!(fe.state, AppState::Stopped));
        assert!(matches!(be.state, AppState::Running));
    }

    #[test]
    fn app_hooks_lifecycle() {
        let manifest = AppManifest {
            name: "hooks-test".into(),
            version: "0.1.0".into(),
            description: String::new(),
            author: None,
            license: None,
            agents: Vec::new(),
            tools: Vec::new(),
            services: Vec::new(),
            capabilities: AppCapabilities::default(),
            hooks: AppHooks {
                on_install: Some("scripts/setup.sh".into()),
                on_start: Some("scripts/migrate.sh".into()),
                on_stop: Some("scripts/cleanup.sh".into()),
                on_remove: None,
            },
        };
        assert_eq!(manifest.hooks.on_install, Some("scripts/setup.sh".into()));
        assert_eq!(manifest.hooks.on_start, Some("scripts/migrate.sh".into()));
        assert_eq!(manifest.hooks.on_stop, Some("scripts/cleanup.sh".into()));
        assert!(manifest.hooks.on_remove.is_none());
    }

    // ── K5 Gate Tests ──────────────────────────────────────────────

    #[test]
    fn k5_manifest_parsed_and_validated() {
        // Programmatic manifest creation and validation.
        let manifest = AppManifest {
            name: "test-app".into(),
            version: "1.0.0".into(),
            description: "A test application".into(),
            author: None,
            license: None,
            agents: vec![AgentSpec {
                id: "worker".into(),
                role: "coder".into(),
                capabilities: AgentCapabilities::default(),
                auto_start: true,
            }],
            tools: Vec::new(),
            services: vec![ServiceSpec {
                name: "api".into(),
                image: None,
                command: Some("serve".into()),
                ports: vec![PortMapping {
                    host_port: 8080,
                    container_port: 8080,
                    protocol: "tcp".into(),
                }],
                env: HashMap::new(),
                health_endpoint: None,
            }],
            capabilities: AppCapabilities::default(),
            hooks: AppHooks::default(),
        };
        assert!(validate_manifest(&manifest).is_ok());
        assert!(!manifest.name.is_empty());
        assert!(!manifest.version.is_empty());

        // Also verify JSON parsing path.
        let json = serde_json::to_string(&manifest).unwrap();
        let parsed = AppManifest::from_json_str(&json).unwrap();
        assert_eq!(parsed.name, "test-app");
        assert_eq!(parsed.agents.len(), 1);
        assert_eq!(parsed.services.len(), 1);
    }

    #[test]
    fn k5_app_install_start_stop_lifecycle() {
        let mgr = AppManager::new();
        let manifest = AppManifest {
            name: "lifecycle-app".into(),
            version: "2.0.0".into(),
            description: "Lifecycle test".into(),
            author: None,
            license: None,
            agents: vec![
                AgentSpec {
                    id: "alpha".into(),
                    role: "coder".into(),
                    capabilities: AgentCapabilities::default(),
                    auto_start: true,
                },
                AgentSpec {
                    id: "beta".into(),
                    role: "reviewer".into(),
                    capabilities: AgentCapabilities {
                        can_network: true,
                        ..Default::default()
                    },
                    auto_start: false,
                },
            ],
            tools: Vec::new(),
            services: Vec::new(),
            capabilities: AppCapabilities::default(),
            hooks: AppHooks::default(),
        };

        // Install
        let app_id = mgr.install(manifest).unwrap();
        assert_eq!(app_id, "lifecycle-app");
        let app = mgr.inspect(&app_id).unwrap();
        assert_eq!(app.state, AppState::Installed);

        // Start -- returns spawn requests for both agents
        let spawn_reqs = mgr.start(&app_id).unwrap();
        assert_eq!(spawn_reqs.len(), 2);
        assert_eq!(spawn_reqs[0].agent_id, "lifecycle-app/alpha");
        assert_eq!(spawn_reqs[1].agent_id, "lifecycle-app/beta");

        let app = mgr.inspect(&app_id).unwrap();
        assert_eq!(app.state, AppState::Running);

        // Stop
        mgr.stop(&app_id).unwrap();
        let app = mgr.inspect(&app_id).unwrap();
        assert_eq!(app.state, AppState::Stopped);

        // Restart after stop
        let spawn_reqs = mgr.start(&app_id).unwrap();
        assert_eq!(spawn_reqs.len(), 2);
        let app = mgr.inspect(&app_id).unwrap();
        assert_eq!(app.state, AppState::Running);
    }

    #[test]
    fn k5_app_agents_spawn_with_correct_capabilities() {
        let manifest = AppManifest {
            name: "cap-app".into(),
            version: "1.0.0".into(),
            description: String::new(),
            author: None,
            license: None,
            agents: vec![
                AgentSpec {
                    id: "networker".into(),
                    role: "fetcher".into(),
                    capabilities: AgentCapabilities {
                        can_network: true,
                        can_spawn: false,
                        can_ipc: true,
                        can_exec_tools: true,
                        ipc_scope: IpcScope::All,
                        ..Default::default()
                    },
                    auto_start: true,
                },
                AgentSpec {
                    id: "sandboxed".into(),
                    role: "compute".into(),
                    capabilities: AgentCapabilities {
                        can_network: false,
                        can_spawn: false,
                        can_ipc: false,
                        can_exec_tools: false,
                        ipc_scope: IpcScope::None,
                        ..Default::default()
                    },
                    auto_start: true,
                },
            ],
            tools: Vec::new(),
            services: Vec::new(),
            capabilities: AppCapabilities::default(),
            hooks: AppHooks::default(),
        };

        let spawn_reqs = AppManager::build_spawn_requests(&manifest);
        assert_eq!(spawn_reqs.len(), 2);

        // First agent: networker with network access
        assert_eq!(spawn_reqs[0].agent_id, "cap-app/networker");
        let caps0 = spawn_reqs[0].capabilities.as_ref().unwrap();
        assert!(caps0.can_network);
        assert!(!caps0.can_spawn);
        assert!(caps0.can_ipc);
        assert!(caps0.can_exec_tools);

        // Second agent: sandboxed with no capabilities
        assert_eq!(spawn_reqs[1].agent_id, "cap-app/sandboxed");
        let caps1 = spawn_reqs[1].capabilities.as_ref().unwrap();
        assert!(!caps1.can_network);
        assert!(!caps1.can_spawn);
        assert!(!caps1.can_ipc);
        assert!(!caps1.can_exec_tools);
        assert_eq!(caps1.ipc_scope, IpcScope::None);
    }

    #[test]
    fn k5_app_list_shows_installed() {
        let mgr = AppManager::new();

        let app1 = AppManifest {
            name: "app-one".into(),
            version: "1.0.0".into(),
            description: String::new(),
            author: None,
            license: None,
            agents: Vec::new(),
            tools: Vec::new(),
            services: Vec::new(),
            capabilities: AppCapabilities::default(),
            hooks: AppHooks::default(),
        };
        let app2 = AppManifest {
            name: "app-two".into(),
            version: "2.0.0".into(),
            description: String::new(),
            author: None,
            license: None,
            agents: Vec::new(),
            tools: Vec::new(),
            services: Vec::new(),
            capabilities: AppCapabilities::default(),
            hooks: AppHooks::default(),
        };

        mgr.install(app1).unwrap();
        mgr.install(app2).unwrap();

        let list = mgr.list();
        assert_eq!(list.len(), 2);
        let names: Vec<&str> = list.iter().map(|(n, _, _)| n.as_str()).collect();
        assert!(names.contains(&"app-one"));
        assert!(names.contains(&"app-two"));
    }

    #[test]
    fn k5_invalid_manifest_rejected() {
        let mgr = AppManager::new();

        // Empty name
        let bad = AppManifest {
            name: String::new(),
            version: "1.0.0".into(),
            description: String::new(),
            author: None,
            license: None,
            agents: Vec::new(),
            tools: Vec::new(),
            services: Vec::new(),
            capabilities: AppCapabilities::default(),
            hooks: AppHooks::default(),
        };
        assert!(mgr.install(bad).is_err());

        // Empty version
        let bad = AppManifest {
            name: "ok-name".into(),
            version: String::new(),
            description: String::new(),
            author: None,
            license: None,
            agents: Vec::new(),
            tools: Vec::new(),
            services: Vec::new(),
            capabilities: AppCapabilities::default(),
            hooks: AppHooks::default(),
        };
        assert!(mgr.install(bad).is_err());

        // Invalid name chars
        let bad = AppManifest {
            name: "bad name!".into(),
            version: "1.0.0".into(),
            description: String::new(),
            author: None,
            license: None,
            agents: Vec::new(),
            tools: Vec::new(),
            services: Vec::new(),
            capabilities: AppCapabilities::default(),
            hooks: AppHooks::default(),
        };
        assert!(mgr.install(bad).is_err());

        // No valid apps were installed
        assert!(mgr.is_empty());
    }

    #[test]
    fn k5_start_wrong_state_fails() {
        let mgr = AppManager::new();
        let manifest = AppManifest {
            name: "state-test".into(),
            version: "1.0.0".into(),
            description: String::new(),
            author: None,
            license: None,
            agents: Vec::new(),
            tools: Vec::new(),
            services: Vec::new(),
            capabilities: AppCapabilities::default(),
            hooks: AppHooks::default(),
        };
        mgr.install(manifest).unwrap();
        mgr.start("state-test").unwrap();

        // Already running -- start again should fail
        let err = mgr.start("state-test").unwrap_err();
        assert!(matches!(err, AppError::InvalidState { .. }));
    }

    #[test]
    fn k5_stop_wrong_state_fails() {
        let mgr = AppManager::new();
        let manifest = AppManifest {
            name: "stop-test".into(),
            version: "1.0.0".into(),
            description: String::new(),
            author: None,
            license: None,
            agents: Vec::new(),
            tools: Vec::new(),
            services: Vec::new(),
            capabilities: AppCapabilities::default(),
            hooks: AppHooks::default(),
        };
        mgr.install(manifest).unwrap();

        // Not running -- stop should fail
        let err = mgr.stop("stop-test").unwrap_err();
        assert!(matches!(err, AppError::InvalidState { .. }));
    }

    #[test]
    fn k5_stop_clears_agent_pids() {
        let mgr = AppManager::new();
        let manifest = AppManifest {
            name: "pid-test".into(),
            version: "1.0.0".into(),
            description: String::new(),
            author: None,
            license: None,
            agents: vec![AgentSpec {
                id: "w".into(),
                role: "worker".into(),
                capabilities: AgentCapabilities::default(),
                auto_start: true,
            }],
            tools: Vec::new(),
            services: Vec::new(),
            capabilities: AppCapabilities::default(),
            hooks: AppHooks::default(),
        };
        mgr.install(manifest).unwrap();
        mgr.start("pid-test").unwrap();

        // Simulate supervisor assigning PIDs after start.
        mgr.add_agent_pid("pid-test", 42).unwrap();
        mgr.add_service_name("pid-test", "svc".into()).unwrap();
        let app = mgr.inspect("pid-test").unwrap();
        assert_eq!(app.agent_pids.len(), 1);
        assert_eq!(app.service_names.len(), 1);

        // Stop clears runtime bookkeeping.
        mgr.stop("pid-test").unwrap();
        let app = mgr.inspect("pid-test").unwrap();
        assert!(app.agent_pids.is_empty());
        assert!(app.service_names.is_empty());
    }
}
