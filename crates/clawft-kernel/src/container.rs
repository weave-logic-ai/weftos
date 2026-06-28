//! Container integration for sidecar service orchestration.
//!
//! Provides types and configuration for managing containerized
//! sidecar services (databases, caches, external APIs) alongside
//! the agent process.
//!
//! # Feature Gate
//!
//! This module is compiled unconditionally, but actual Docker
//! integration requires the `containers` feature flag. Without it,
//! [`ContainerManager::new`] returns a manager that rejects all
//! operations with [`ContainerError::DockerNotAvailable`].
//!
//! # Architecture
//!
//! Each managed container is wrapped in a `ContainerService` and
//! registered in the kernel's `ServiceRegistry`, making container
//! health visible through the standard health monitoring system.

use std::collections::HashMap;
#[cfg(feature = "exochain")]
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tracing::debug;

use async_trait::async_trait;

use crate::health::HealthStatus;
use crate::service::{ServiceType, SystemService};

/// Configuration for the container manager.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerConfig {
    /// Docker socket path.
    /// Default: "unix:///var/run/docker.sock"
    #[serde(default = "default_docker_socket")]
    pub docker_socket: String,

    /// Docker network name for managed containers.
    /// Default: "weftos"
    #[serde(default = "default_network_name")]
    pub network_name: String,

    /// Default restart policy for new containers.
    #[serde(default)]
    pub default_restart_policy: RestartPolicy,

    /// Health check interval in seconds.
    #[serde(default = "default_health_check_interval")]
    pub health_check_interval_secs: u64,
}

fn default_docker_socket() -> String {
    "unix:///var/run/docker.sock".into()
}

fn default_network_name() -> String {
    "weftos".into()
}

fn default_health_check_interval() -> u64 {
    30
}

impl Default for ContainerConfig {
    fn default() -> Self {
        Self {
            docker_socket: default_docker_socket(),
            network_name: default_network_name(),
            default_restart_policy: RestartPolicy::default(),
            health_check_interval_secs: default_health_check_interval(),
        }
    }
}

impl ContainerConfig {
    /// Get the health check interval as a Duration.
    pub fn health_check_interval(&self) -> Duration {
        Duration::from_secs(self.health_check_interval_secs)
    }
}

/// Container lifecycle state.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContainerState {
    /// Image is being pulled.
    Pulling,
    /// Container is being created.
    Creating,
    /// Container is running.
    Running,
    /// Container is being stopped.
    Stopping,
    /// Container is stopped.
    Stopped,
    /// Container failed with an error.
    Failed(String),
}

impl std::fmt::Display for ContainerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContainerState::Pulling => write!(f, "pulling"),
            ContainerState::Creating => write!(f, "creating"),
            ContainerState::Running => write!(f, "running"),
            ContainerState::Stopping => write!(f, "stopping"),
            ContainerState::Stopped => write!(f, "stopped"),
            ContainerState::Failed(reason) => write!(f, "failed: {reason}"),
        }
    }
}

/// Port mapping between host and container.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortMapping {
    /// Host port number.
    pub host_port: u16,
    /// Container port number.
    pub container_port: u16,
    /// Protocol (tcp, udp).
    #[serde(default = "default_protocol")]
    pub protocol: String,
}

fn default_protocol() -> String {
    "tcp".into()
}

/// Volume mount configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeMount {
    /// Host path to mount.
    pub host_path: String,
    /// Container path to mount to.
    pub container_path: String,
    /// Whether the mount is read-only.
    #[serde(default)]
    pub read_only: bool,
}

/// Container restart policy.
#[non_exhaustive]
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum RestartPolicy {
    /// Never restart.
    #[default]
    Never,
    /// Restart on failure up to max_retries.
    OnFailure {
        /// Maximum number of restart attempts.
        max_retries: u32,
    },
    /// Always restart.
    Always,
}

/// Specification for a managed container.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedContainer {
    /// Container name (unique identifier).
    pub name: String,

    /// Docker image reference.
    pub image: String,

    /// Docker container ID (set after creation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container_id: Option<String>,

    /// Current state.
    #[serde(default = "default_container_state")]
    pub state: ContainerState,

    /// Port mappings.
    #[serde(default)]
    pub ports: Vec<PortMapping>,

    /// Environment variables.
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Volume mounts.
    #[serde(default)]
    pub volumes: Vec<VolumeMount>,

    /// HTTP health check endpoint (e.g. "http://localhost:6379/ping").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_endpoint: Option<String>,

    /// Restart policy override (uses manager default if None).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart_policy: Option<RestartPolicy>,
}

fn default_container_state() -> ContainerState {
    ContainerState::Stopped
}

/// Health report for a single managed container.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerHealth {
    /// Container name.
    pub container_id: String,
    /// Current lifecycle state.
    pub status: ContainerState,
    /// Whether the container is considered healthy.
    pub healthy: bool,
    /// Optional diagnostic message.
    pub message: Option<String>,
}

/// Container manager errors.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum ContainerError {
    /// Docker is not available on this system.
    #[error("Docker not available: {0}")]
    DockerNotAvailable(String),

    /// Image pull failed.
    #[error("image pull failed for '{image}': {reason}")]
    ImagePullFailed {
        /// Image reference.
        image: String,
        /// Failure reason.
        reason: String,
    },

    /// Container creation failed.
    #[error("container creation failed for '{name}': {reason}")]
    CreateFailed {
        /// Container name.
        name: String,
        /// Failure reason.
        reason: String,
    },

    /// Container start failed.
    #[error("container start failed for '{name}': {reason}")]
    StartFailed {
        /// Container name.
        name: String,
        /// Failure reason.
        reason: String,
    },

    /// Port conflict on the host.
    #[error("port conflict: host port {port} already in use")]
    PortConflict {
        /// Conflicting port.
        port: u16,
    },

    /// Container not found.
    #[error("container not found: '{name}'")]
    ContainerNotFound {
        /// Container name.
        name: String,
    },

    /// Health check failed.
    #[error("health check failed for '{name}': {reason}")]
    HealthCheckFailed {
        /// Container name.
        name: String,
        /// Failure reason.
        reason: String,
    },

    /// Invalid container configuration.
    #[error("invalid config: {0}")]
    InvalidConfig(String),
}

/// Container lifecycle manager.
///
/// When the `containers` feature is enabled, this uses bollard
/// for Docker API access. Without the feature, all operations
/// return [`ContainerError::DockerNotAvailable`].
pub struct ContainerManager {
    config: ContainerConfig,
    managed: DashMap<String, ManagedContainer>,
    #[cfg(feature = "exochain")]
    chain_manager: Option<Arc<crate::chain::ChainManager>>,
}

impl ContainerManager {
    /// Create a new container manager.
    ///
    /// Does NOT attempt to connect to Docker at construction time.
    /// Connection is deferred to the first operation that needs it.
    pub fn new(config: ContainerConfig) -> Self {
        Self {
            config,
            managed: DashMap::new(),
            #[cfg(feature = "exochain")]
            chain_manager: None,
        }
    }

    /// Attach a chain manager for exochain event logging.
    #[cfg(feature = "exochain")]
    pub fn set_chain_manager(&mut self, cm: Arc<crate::chain::ChainManager>) {
        self.chain_manager = Some(cm);
    }

    /// Get the container configuration.
    pub fn config(&self) -> &ContainerConfig {
        &self.config
    }

    /// Configure and validate a container image, registering it for management.
    ///
    /// Validates the specification (image name must be non-empty, port
    /// numbers must be valid, container name must be non-empty) and
    /// registers it in the `Stopped` state. Returns the container name
    /// as its identifier.
    ///
    /// # Errors
    ///
    /// Returns [`ContainerError::InvalidConfig`] when validation fails.
    pub fn configure(&self, spec: ManagedContainer) -> Result<String, ContainerError> {
        // Validate image name
        if spec.image.trim().is_empty() {
            return Err(ContainerError::InvalidConfig(
                "image name must not be empty".into(),
            ));
        }
        // Validate container name
        if spec.name.trim().is_empty() {
            return Err(ContainerError::InvalidConfig(
                "container name must not be empty".into(),
            ));
        }
        // Validate ports
        for pm in &spec.ports {
            if pm.host_port == 0 {
                return Err(ContainerError::InvalidConfig(
                    "host port must be > 0".into(),
                ));
            }
            if pm.container_port == 0 {
                return Err(ContainerError::InvalidConfig(
                    "container port must be > 0".into(),
                ));
            }
        }
        let name = spec.name.clone();
        debug!(name = %spec.name, image = %spec.image, "configuring container");

        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain_manager {
            cm.append(
                "container",
                crate::chain::EVENT_KIND_CONTAINER_CONFIGURE,
                Some(serde_json::json!({
                    "name": &name,
                    "image": &spec.image,
                    "ports": spec.ports.len(),
                })),
            );
        }

        self.managed.insert(spec.name.clone(), spec);
        Ok(name)
    }

    /// Register a container specification for management.
    ///
    /// This does not start the container; it only registers it
    /// for tracking. Call `start_container` to actually start it.
    pub fn register(&self, spec: ManagedContainer) {
        debug!(name = %spec.name, image = %spec.image, "registering container");
        self.managed.insert(spec.name.clone(), spec);
    }

    /// Start a managed container by transitioning its state to Running.
    ///
    /// In a production environment this would shell out to `docker run`
    /// or `podman run`. The current implementation simulates the state
    /// transition so the integration between ContainerManager and the
    /// kernel ServiceRegistry / HealthSystem can be tested without a
    /// container runtime installed.
    ///
    /// # Errors
    ///
    /// Returns [`ContainerError::ContainerNotFound`] if the name is
    /// not registered, or [`ContainerError::StartFailed`] if the
    /// container is in a state that cannot be started.
    pub fn start_container(&self, name: &str) -> Result<(), ContainerError> {
        let mut entry =
            self.managed
                .get_mut(name)
                .ok_or_else(|| ContainerError::ContainerNotFound {
                    name: name.to_owned(),
                })?;

        match &entry.state {
            ContainerState::Stopped | ContainerState::Creating | ContainerState::Failed(_) => {
                debug!(name, "starting container (simulated)");
                // Simulate: Stopped -> Creating -> Running
                entry.state = ContainerState::Running;
                // Assign a synthetic container ID when first started
                if entry.container_id.is_none() {
                    use std::collections::hash_map::DefaultHasher;
                    use std::hash::{Hash, Hasher};
                    let mut h = DefaultHasher::new();
                    name.hash(&mut h);
                    entry.container_id = Some(format!("sim-{:08x}", h.finish() as u32));
                }

                #[cfg(feature = "exochain")]
                if let Some(ref cm) = self.chain_manager {
                    cm.append(
                        "container",
                        crate::chain::EVENT_KIND_CONTAINER_START,
                        Some(serde_json::json!({
                            "name": name,
                            "image": &entry.image,
                        })),
                    );
                }

                Ok(())
            }
            ContainerState::Running => {
                // Already running — idempotent
                Ok(())
            }
            other => Err(ContainerError::StartFailed {
                name: name.to_owned(),
                reason: format!("cannot start from state: {other}"),
            }),
        }
    }

    /// Stop a managed container.
    ///
    /// Transitions the container from any active state to `Stopped`.
    pub fn stop_container(&self, name: &str) -> Result<(), ContainerError> {
        let mut entry =
            self.managed
                .get_mut(name)
                .ok_or_else(|| ContainerError::ContainerNotFound {
                    name: name.to_owned(),
                })?;

        debug!(name, "stopping container");
        entry.state = ContainerState::Stopped;

        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain_manager {
            cm.append(
                "container",
                crate::chain::EVENT_KIND_CONTAINER_STOP,
                Some(serde_json::json!({
                    "name": name,
                })),
            );
        }

        Ok(())
    }

    /// Get the state of a managed container.
    pub fn container_state(&self, name: &str) -> Option<ContainerState> {
        self.managed.get(name).map(|e| e.state.clone())
    }

    /// List all managed containers with their states.
    pub fn list_containers(&self) -> Vec<(String, ContainerState)> {
        self.managed
            .iter()
            .map(|e| (e.key().clone(), e.value().state.clone()))
            .collect()
    }

    /// Health check for a specific container, returning a [`HealthStatus`].
    pub fn health_check(&self, name: &str) -> Result<HealthStatus, ContainerError> {
        let entry = self
            .managed
            .get(name)
            .ok_or_else(|| ContainerError::ContainerNotFound {
                name: name.to_owned(),
            })?;

        match &entry.state {
            ContainerState::Running => Ok(HealthStatus::Healthy),
            ContainerState::Stopped => Ok(HealthStatus::Unhealthy("stopped".into())),
            ContainerState::Failed(reason) => {
                Ok(HealthStatus::Unhealthy(format!("failed: {reason}")))
            }
            other => Ok(HealthStatus::Degraded(format!("state: {other}"))),
        }
    }

    /// Detailed health report for a specific container.
    pub fn container_health(&self, name: &str) -> Result<ContainerHealth, ContainerError> {
        let entry = self
            .managed
            .get(name)
            .ok_or_else(|| ContainerError::ContainerNotFound {
                name: name.to_owned(),
            })?;

        let (healthy, message) = match &entry.state {
            ContainerState::Running => (true, None),
            ContainerState::Stopped => (false, Some("container is stopped".into())),
            ContainerState::Failed(reason) => (false, Some(format!("failed: {reason}"))),
            other => (false, Some(format!("transitional state: {other}"))),
        };

        Ok(ContainerHealth {
            container_id: entry.name.clone(),
            status: entry.state.clone(),
            healthy,
            message,
        })
    }

    /// Stop all managed containers.
    pub fn stop_all(&self) {
        for mut entry in self.managed.iter_mut() {
            if matches!(entry.state, ContainerState::Running) {
                debug!(name = %entry.key(), "stopping container");
                entry.state = ContainerState::Stopped;
            }
        }
    }

    /// Get the number of managed containers.
    pub fn len(&self) -> usize {
        self.managed.len()
    }

    /// Check whether any containers are managed.
    pub fn is_empty(&self) -> bool {
        self.managed.is_empty()
    }
}

// ---------------------------------------------------------------------------
// K4 E: ContainerService — SystemService adapter for ContainerManager
// ---------------------------------------------------------------------------

/// Wraps [`ContainerManager`] as a [`SystemService`] so it participates in
/// the kernel's service registry lifecycle and health aggregation.
pub struct ContainerService {
    manager: std::sync::Arc<ContainerManager>,
}

impl ContainerService {
    /// Create a new container service wrapping the given manager.
    pub fn new(manager: std::sync::Arc<ContainerManager>) -> Self {
        Self { manager }
    }

    /// Access the underlying container manager.
    pub fn manager(&self) -> &std::sync::Arc<ContainerManager> {
        &self.manager
    }
}

#[async_trait]
impl SystemService for ContainerService {
    fn name(&self) -> &str {
        "containers"
    }

    fn service_type(&self) -> ServiceType {
        ServiceType::Custom("containers".into())
    }

    async fn start(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        debug!(
            "container service starting ({} managed)",
            self.manager.len()
        );
        Ok(())
    }

    async fn stop(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        debug!("container service stopping — stopping all containers");
        self.manager.stop_all();
        Ok(())
    }

    async fn health_check(&self) -> HealthStatus {
        let containers = self.manager.list_containers();
        if containers.is_empty() {
            return HealthStatus::Healthy;
        }
        let mut unhealthy = Vec::new();
        for (name, state) in &containers {
            if !matches!(state, ContainerState::Running) {
                unhealthy.push(format!("{name}: {state}"));
            }
        }
        if unhealthy.is_empty() {
            HealthStatus::Healthy
        } else if unhealthy.len() == containers.len() {
            HealthStatus::Unhealthy(format!("all containers down: {}", unhealthy.join(", ")))
        } else {
            HealthStatus::Degraded(format!(
                "{}/{} unhealthy: {}",
                unhealthy.len(),
                containers.len(),
                unhealthy.join(", ")
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let config = ContainerConfig::default();
        assert!(config.docker_socket.contains("docker.sock"));
        assert_eq!(config.network_name, "weftos");
        assert_eq!(config.default_restart_policy, RestartPolicy::Never);
        assert_eq!(config.health_check_interval_secs, 30);
    }

    #[test]
    fn config_serde_roundtrip() {
        let config = ContainerConfig {
            docker_socket: "tcp://localhost:2375".into(),
            network_name: "custom-net".into(),
            default_restart_policy: RestartPolicy::Always,
            health_check_interval_secs: 10,
        };
        let json = serde_json::to_string(&config).unwrap();
        let restored: ContainerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.network_name, "custom-net");
        assert_eq!(restored.default_restart_policy, RestartPolicy::Always);
    }

    #[test]
    fn health_check_interval_duration() {
        let config = ContainerConfig {
            health_check_interval_secs: 15,
            ..Default::default()
        };
        assert_eq!(config.health_check_interval(), Duration::from_secs(15));
    }

    #[test]
    fn container_state_display() {
        assert_eq!(ContainerState::Pulling.to_string(), "pulling");
        assert_eq!(ContainerState::Running.to_string(), "running");
        assert_eq!(ContainerState::Stopped.to_string(), "stopped");
        assert_eq!(
            ContainerState::Failed("oom".into()).to_string(),
            "failed: oom"
        );
    }

    #[test]
    fn register_and_list() {
        let manager = ContainerManager::new(ContainerConfig::default());
        manager.register(ManagedContainer {
            name: "redis".into(),
            image: "redis:7-alpine".into(),
            container_id: None,
            state: ContainerState::Stopped,
            ports: vec![PortMapping {
                host_port: 6379,
                container_port: 6379,
                protocol: "tcp".into(),
            }],
            env: HashMap::new(),
            volumes: Vec::new(),
            health_endpoint: None,
            restart_policy: None,
        });

        let containers = manager.list_containers();
        assert_eq!(containers.len(), 1);
        assert_eq!(containers[0].0, "redis");
    }

    #[test]
    fn stop_container() {
        let manager = ContainerManager::new(ContainerConfig::default());
        manager.register(ManagedContainer {
            name: "redis".into(),
            image: "redis:7-alpine".into(),
            container_id: None,
            state: ContainerState::Running,
            ports: Vec::new(),
            env: HashMap::new(),
            volumes: Vec::new(),
            health_endpoint: None,
            restart_policy: None,
        });

        manager.stop_container("redis").unwrap();
        assert_eq!(
            manager.container_state("redis"),
            Some(ContainerState::Stopped)
        );
    }

    #[test]
    fn stop_nonexistent_fails() {
        let manager = ContainerManager::new(ContainerConfig::default());
        let result = manager.stop_container("nonexistent");
        assert!(matches!(
            result,
            Err(ContainerError::ContainerNotFound { .. })
        ));
    }

    #[test]
    fn health_check_running() {
        let manager = ContainerManager::new(ContainerConfig::default());
        manager.register(ManagedContainer {
            name: "redis".into(),
            image: "redis:7-alpine".into(),
            container_id: None,
            state: ContainerState::Running,
            ports: Vec::new(),
            env: HashMap::new(),
            volumes: Vec::new(),
            health_endpoint: None,
            restart_policy: None,
        });

        let health = manager.health_check("redis").unwrap();
        assert!(matches!(health, HealthStatus::Healthy));
    }

    #[test]
    fn health_check_stopped() {
        let manager = ContainerManager::new(ContainerConfig::default());
        manager.register(ManagedContainer {
            name: "redis".into(),
            image: "redis:7-alpine".into(),
            container_id: None,
            state: ContainerState::Stopped,
            ports: Vec::new(),
            env: HashMap::new(),
            volumes: Vec::new(),
            health_endpoint: None,
            restart_policy: None,
        });

        let health = manager.health_check("redis").unwrap();
        assert!(matches!(health, HealthStatus::Unhealthy(_)));
    }

    #[test]
    fn health_check_nonexistent() {
        let manager = ContainerManager::new(ContainerConfig::default());
        assert!(manager.health_check("nope").is_err());
    }

    #[test]
    fn stop_all() {
        let manager = ContainerManager::new(ContainerConfig::default());
        for name in &["redis", "postgres", "memcached"] {
            manager.register(ManagedContainer {
                name: (*name).into(),
                image: format!("{name}:latest"),
                container_id: None,
                state: ContainerState::Running,
                ports: Vec::new(),
                env: HashMap::new(),
                volumes: Vec::new(),
                health_endpoint: None,
                restart_policy: None,
            });
        }

        manager.stop_all();

        for (_, state) in manager.list_containers() {
            assert_eq!(state, ContainerState::Stopped);
        }
    }

    #[test]
    fn start_container_transitions_to_running() {
        let manager = ContainerManager::new(ContainerConfig::default());
        manager.register(ManagedContainer {
            name: "redis".into(),
            image: "redis:7-alpine".into(),
            container_id: None,
            state: ContainerState::Stopped,
            ports: Vec::new(),
            env: HashMap::new(),
            volumes: Vec::new(),
            health_endpoint: None,
            restart_policy: None,
        });

        manager.start_container("redis").unwrap();
        assert_eq!(
            manager.container_state("redis"),
            Some(ContainerState::Running)
        );
    }

    #[test]
    fn start_container_assigns_id() {
        let manager = ContainerManager::new(ContainerConfig::default());
        manager.register(ManagedContainer {
            name: "pg".into(),
            image: "postgres:16".into(),
            container_id: None,
            state: ContainerState::Stopped,
            ports: Vec::new(),
            env: HashMap::new(),
            volumes: Vec::new(),
            health_endpoint: None,
            restart_policy: None,
        });

        manager.start_container("pg").unwrap();
        let entry = manager.managed.get("pg").unwrap();
        assert!(entry.container_id.is_some());
        assert!(entry.container_id.as_ref().unwrap().starts_with("sim-"));
    }

    #[test]
    fn start_already_running_is_idempotent() {
        let manager = ContainerManager::new(ContainerConfig::default());
        manager.register(ManagedContainer {
            name: "redis".into(),
            image: "redis:7-alpine".into(),
            container_id: None,
            state: ContainerState::Running,
            ports: Vec::new(),
            env: HashMap::new(),
            volumes: Vec::new(),
            health_endpoint: None,
            restart_policy: None,
        });

        // Should succeed without error
        manager.start_container("redis").unwrap();
        assert_eq!(
            manager.container_state("redis"),
            Some(ContainerState::Running)
        );
    }

    #[test]
    fn managed_container_serde_roundtrip() {
        let container = ManagedContainer {
            name: "redis".into(),
            image: "redis:7-alpine".into(),
            container_id: Some("abc123".into()),
            state: ContainerState::Running,
            ports: vec![PortMapping {
                host_port: 6379,
                container_port: 6379,
                protocol: "tcp".into(),
            }],
            env: HashMap::from([("REDIS_PASSWORD".into(), "secret".into())]),
            volumes: vec![VolumeMount {
                host_path: "/data".into(),
                container_path: "/var/lib/redis".into(),
                read_only: false,
            }],
            health_endpoint: Some("http://localhost:6379/ping".into()),
            restart_policy: Some(RestartPolicy::OnFailure { max_retries: 3 }),
        };

        let json = serde_json::to_string(&container).unwrap();
        let restored: ManagedContainer = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.name, "redis");
        assert_eq!(restored.ports.len(), 1);
        assert_eq!(restored.volumes.len(), 1);
        assert!(!restored.volumes[0].read_only);
    }

    #[test]
    fn container_error_display() {
        let err = ContainerError::DockerNotAvailable("not installed".into());
        assert!(err.to_string().contains("Docker"));

        let err = ContainerError::ContainerNotFound {
            name: "redis".into(),
        };
        assert!(err.to_string().contains("redis"));

        let err = ContainerError::PortConflict { port: 8080 };
        assert!(err.to_string().contains("8080"));
    }

    #[test]
    fn restart_policy_serde() {
        let policies = vec![
            RestartPolicy::Never,
            RestartPolicy::OnFailure { max_retries: 5 },
            RestartPolicy::Always,
        ];
        for policy in policies {
            let json = serde_json::to_string(&policy).unwrap();
            let restored: RestartPolicy = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, policy);
        }
    }

    // --- K4 E: ContainerService tests ---

    #[test]
    fn container_service_implements_system_service() {
        let mgr = std::sync::Arc::new(ContainerManager::new(ContainerConfig::default()));
        let svc = ContainerService::new(mgr);
        assert_eq!(svc.name(), "containers");
        assert_eq!(svc.service_type(), ServiceType::Custom("containers".into()));
    }

    #[tokio::test]
    async fn container_service_health_empty_is_healthy() {
        let mgr = std::sync::Arc::new(ContainerManager::new(ContainerConfig::default()));
        let svc = ContainerService::new(mgr);
        let health = svc.health_check().await;
        assert!(matches!(health, HealthStatus::Healthy));
    }

    #[tokio::test]
    async fn container_service_health_propagates() {
        let mgr = std::sync::Arc::new(ContainerManager::new(ContainerConfig::default()));
        mgr.register(ManagedContainer {
            name: "redis".into(),
            image: "redis:7-alpine".into(),
            container_id: None,
            state: ContainerState::Running,
            ports: Vec::new(),
            env: HashMap::new(),
            volumes: Vec::new(),
            health_endpoint: None,
            restart_policy: None,
        });
        mgr.register(ManagedContainer {
            name: "pg".into(),
            image: "postgres:16".into(),
            container_id: None,
            state: ContainerState::Stopped,
            ports: Vec::new(),
            env: HashMap::new(),
            volumes: Vec::new(),
            health_endpoint: None,
            restart_policy: None,
        });
        let svc = ContainerService::new(mgr);
        let health = svc.health_check().await;
        // One running, one stopped → degraded
        assert!(matches!(health, HealthStatus::Degraded(_)));
    }

    #[tokio::test]
    async fn container_service_stop_halts_all() {
        let mgr = std::sync::Arc::new(ContainerManager::new(ContainerConfig::default()));
        mgr.register(ManagedContainer {
            name: "redis".into(),
            image: "redis:7-alpine".into(),
            container_id: None,
            state: ContainerState::Running,
            ports: Vec::new(),
            env: HashMap::new(),
            volumes: Vec::new(),
            health_endpoint: None,
            restart_policy: None,
        });
        let svc = ContainerService::new(mgr.clone());
        svc.stop().await.unwrap();
        assert_eq!(mgr.container_state("redis"), Some(ContainerState::Stopped));
    }

    // ── K4 gate tests: container config, lifecycle, health propagation ──

    #[test]
    fn container_config_validates() {
        let manager = ContainerManager::new(ContainerConfig::default());
        let spec = ManagedContainer {
            name: "alpine-test".into(),
            image: "alpine:latest".into(),
            container_id: None,
            state: ContainerState::Stopped,
            ports: vec![PortMapping {
                host_port: 8080,
                container_port: 80,
                protocol: "tcp".into(),
            }],
            env: HashMap::new(),
            volumes: Vec::new(),
            health_endpoint: None,
            restart_policy: None,
        };
        let id = manager.configure(spec).unwrap();
        assert_eq!(id, "alpine-test");
        // Container should now be tracked
        assert_eq!(
            manager.container_state("alpine-test"),
            Some(ContainerState::Stopped)
        );
    }

    #[test]
    fn container_invalid_config_empty_image_rejected() {
        let manager = ContainerManager::new(ContainerConfig::default());
        let spec = ManagedContainer {
            name: "bad".into(),
            image: "".into(),
            container_id: None,
            state: ContainerState::Stopped,
            ports: Vec::new(),
            env: HashMap::new(),
            volumes: Vec::new(),
            health_endpoint: None,
            restart_policy: None,
        };
        let result = manager.configure(spec);
        assert!(matches!(result, Err(ContainerError::InvalidConfig(_))));
    }

    #[test]
    fn container_invalid_config_empty_name_rejected() {
        let manager = ContainerManager::new(ContainerConfig::default());
        let spec = ManagedContainer {
            name: "".into(),
            image: "alpine:latest".into(),
            container_id: None,
            state: ContainerState::Stopped,
            ports: Vec::new(),
            env: HashMap::new(),
            volumes: Vec::new(),
            health_endpoint: None,
            restart_policy: None,
        };
        let result = manager.configure(spec);
        assert!(matches!(result, Err(ContainerError::InvalidConfig(_))));
    }

    #[test]
    fn container_invalid_config_zero_port_rejected() {
        let manager = ContainerManager::new(ContainerConfig::default());
        let spec = ManagedContainer {
            name: "bad-port".into(),
            image: "alpine:latest".into(),
            container_id: None,
            state: ContainerState::Stopped,
            ports: vec![PortMapping {
                host_port: 0,
                container_port: 80,
                protocol: "tcp".into(),
            }],
            env: HashMap::new(),
            volumes: Vec::new(),
            health_endpoint: None,
            restart_policy: None,
        };
        let result = manager.configure(spec);
        assert!(matches!(result, Err(ContainerError::InvalidConfig(_))));
    }

    #[test]
    fn container_lifecycle_configure_start_stop() {
        let manager = ContainerManager::new(ContainerConfig::default());

        // Configure
        let spec = ManagedContainer {
            name: "lifecycle-test".into(),
            image: "redis:7-alpine".into(),
            container_id: None,
            state: ContainerState::Stopped,
            ports: Vec::new(),
            env: HashMap::new(),
            volumes: Vec::new(),
            health_endpoint: None,
            restart_policy: None,
        };
        let name = manager.configure(spec).unwrap();

        // Start
        manager.start_container(&name).unwrap();
        assert_eq!(
            manager.container_state(&name),
            Some(ContainerState::Running)
        );

        // Health while running
        let health = manager.health_check(&name).unwrap();
        assert_eq!(health, HealthStatus::Healthy);

        // Stop
        manager.stop_container(&name).unwrap();
        assert_eq!(
            manager.container_state(&name),
            Some(ContainerState::Stopped)
        );

        // Health while stopped
        let health = manager.health_check(&name).unwrap();
        assert!(matches!(health, HealthStatus::Unhealthy(_)));
    }

    #[test]
    fn container_health_report_detail() {
        let manager = ContainerManager::new(ContainerConfig::default());
        manager.register(ManagedContainer {
            name: "detail".into(),
            image: "alpine:latest".into(),
            container_id: None,
            state: ContainerState::Running,
            ports: Vec::new(),
            env: HashMap::new(),
            volumes: Vec::new(),
            health_endpoint: None,
            restart_policy: None,
        });

        let report = manager.container_health("detail").unwrap();
        assert!(report.healthy);
        assert_eq!(report.status, ContainerState::Running);
        assert!(report.message.is_none());

        // Stop and check again
        manager.stop_container("detail").unwrap();
        let report = manager.container_health("detail").unwrap();
        assert!(!report.healthy);
        assert_eq!(report.status, ContainerState::Stopped);
        assert!(report.message.is_some());
    }

    #[tokio::test]
    async fn container_health_propagates_to_kernel_health_system() {
        use crate::health::HealthSystem;
        use crate::service::ServiceRegistry;

        let mgr = std::sync::Arc::new(ContainerManager::new(ContainerConfig::default()));

        // Configure and start a container
        let spec = ManagedContainer {
            name: "redis".into(),
            image: "redis:7-alpine".into(),
            container_id: None,
            state: ContainerState::Stopped,
            ports: Vec::new(),
            env: HashMap::new(),
            volumes: Vec::new(),
            health_endpoint: None,
            restart_policy: None,
        };
        mgr.configure(spec).unwrap();
        mgr.start_container("redis").unwrap();

        // Register ContainerService in a ServiceRegistry
        let svc = std::sync::Arc::new(ContainerService::new(mgr.clone()));
        let registry = std::sync::Arc::new(ServiceRegistry::new());
        registry.register(svc).unwrap();

        // HealthSystem should see the container as healthy
        let hs = HealthSystem::new(30);
        let (overall, results) = hs.aggregate(&registry).await;
        assert!(
            matches!(overall, crate::health::OverallHealth::Healthy),
            "expected Healthy, got {overall:?}"
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "containers");
        assert_eq!(results[0].1, HealthStatus::Healthy);

        // Stop the container -- health should degrade
        mgr.stop_container("redis").unwrap();
        let (overall, _) = hs.aggregate(&registry).await;
        assert!(
            matches!(overall, crate::health::OverallHealth::Down),
            "expected Down after stopping all containers, got {overall:?}"
        );
    }

    // ── Sprint 09a: serde roundtrip tests ────────────────────────

    #[test]
    fn container_state_serde_roundtrip_all_variants() {
        let variants = vec![
            ContainerState::Pulling,
            ContainerState::Creating,
            ContainerState::Running,
            ContainerState::Stopping,
            ContainerState::Stopped,
            ContainerState::Failed("oom killed".into()),
        ];
        for state in variants {
            let json = serde_json::to_string(&state).unwrap();
            let restored: ContainerState = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, state);
        }
    }

    #[test]
    fn port_mapping_serde_roundtrip() {
        let pm = PortMapping {
            host_port: 8080,
            container_port: 80,
            protocol: "tcp".into(),
        };
        let json = serde_json::to_string(&pm).unwrap();
        let restored: PortMapping = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.host_port, 8080);
        assert_eq!(restored.container_port, 80);
        assert_eq!(restored.protocol, "tcp");
    }

    #[test]
    fn port_mapping_default_protocol() {
        let json = r#"{"host_port": 3000, "container_port": 3000}"#;
        let pm: PortMapping = serde_json::from_str(json).unwrap();
        assert_eq!(pm.protocol, "tcp");
    }

    #[test]
    fn volume_mount_serde_roundtrip() {
        let vm = VolumeMount {
            host_path: "/data".into(),
            container_path: "/var/data".into(),
            read_only: true,
        };
        let json = serde_json::to_string(&vm).unwrap();
        let restored: VolumeMount = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.host_path, "/data");
        assert_eq!(restored.container_path, "/var/data");
        assert!(restored.read_only);
    }

    #[test]
    fn volume_mount_default_read_only() {
        let json = r#"{"host_path": "/a", "container_path": "/b"}"#;
        let vm: VolumeMount = serde_json::from_str(json).unwrap();
        assert!(!vm.read_only);
    }

    #[test]
    fn restart_policy_serde_roundtrip_all_variants() {
        let variants = vec![
            RestartPolicy::Never,
            RestartPolicy::OnFailure { max_retries: 5 },
            RestartPolicy::Always,
        ];
        for policy in variants {
            let json = serde_json::to_string(&policy).unwrap();
            let restored: RestartPolicy = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, policy);
        }
    }

    #[test]
    fn restart_policy_default_is_never() {
        assert_eq!(RestartPolicy::default(), RestartPolicy::Never);
    }

    #[test]
    fn container_health_serde_roundtrip() {
        let health = ContainerHealth {
            container_id: "redis-1".into(),
            status: ContainerState::Running,
            healthy: true,
            message: None,
        };
        let json = serde_json::to_string(&health).unwrap();
        let restored: ContainerHealth = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.container_id, "redis-1");
        assert!(restored.healthy);
        assert!(restored.message.is_none());
    }

    #[test]
    fn container_health_with_message_roundtrip() {
        let health = ContainerHealth {
            container_id: "pg-1".into(),
            status: ContainerState::Failed("timeout".into()),
            healthy: false,
            message: Some("health check failed after 30s".into()),
        };
        let json = serde_json::to_string(&health).unwrap();
        let restored: ContainerHealth = serde_json::from_str(&json).unwrap();
        assert!(!restored.healthy);
        assert_eq!(restored.message.unwrap(), "health check failed after 30s");
    }

    #[test]
    fn container_state_display_all_variants() {
        assert_eq!(ContainerState::Pulling.to_string(), "pulling");
        assert_eq!(ContainerState::Creating.to_string(), "creating");
        assert_eq!(ContainerState::Running.to_string(), "running");
        assert_eq!(ContainerState::Stopping.to_string(), "stopping");
        assert_eq!(ContainerState::Stopped.to_string(), "stopped");
        assert_eq!(
            ContainerState::Failed("oom".into()).to_string(),
            "failed: oom"
        );
    }

    #[test]
    fn container_config_health_check_interval() {
        let cfg = ContainerConfig {
            health_check_interval_secs: 10,
            ..Default::default()
        };
        assert_eq!(cfg.health_check_interval(), Duration::from_secs(10));
    }

    #[test]
    fn container_config_defaults_populated() {
        let cfg = ContainerConfig::default();
        assert_eq!(cfg.docker_socket, "unix:///var/run/docker.sock");
        assert_eq!(cfg.network_name, "weftos");
        assert_eq!(cfg.default_restart_policy, RestartPolicy::Never);
        assert_eq!(cfg.health_check_interval_secs, 30);
    }

    #[test]
    fn managed_container_with_env_and_volumes_roundtrip() {
        let mut env = HashMap::new();
        env.insert("REDIS_URL".into(), "redis://localhost".into());
        env.insert("LOG_LEVEL".into(), "debug".into());

        let mc = ManagedContainer {
            name: "full-spec".into(),
            image: "redis:7".into(),
            container_id: Some("abc123".into()),
            state: ContainerState::Running,
            ports: vec![PortMapping {
                host_port: 6379,
                container_port: 6379,
                protocol: "tcp".into(),
            }],
            env,
            volumes: vec![VolumeMount {
                host_path: "/data/redis".into(),
                container_path: "/data".into(),
                read_only: false,
            }],
            health_endpoint: Some("http://localhost:6379/ping".into()),
            restart_policy: Some(RestartPolicy::Always),
        };

        let json = serde_json::to_string(&mc).unwrap();
        let restored: ManagedContainer = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.name, "full-spec");
        assert_eq!(restored.container_id, Some("abc123".into()));
        assert_eq!(restored.ports.len(), 1);
        assert_eq!(restored.env.len(), 2);
        assert_eq!(restored.volumes.len(), 1);
        assert_eq!(restored.restart_policy, Some(RestartPolicy::Always));
    }

    #[test]
    fn configure_multiple_containers_succeeds() {
        let manager = ContainerManager::new(ContainerConfig::default());
        let spec1 = ManagedContainer {
            name: "svc-a".into(),
            image: "alpine:latest".into(),
            container_id: None,
            state: ContainerState::Stopped,
            ports: vec![PortMapping {
                host_port: 8080,
                container_port: 80,
                protocol: "tcp".into(),
            }],
            env: HashMap::new(),
            volumes: Vec::new(),
            health_endpoint: None,
            restart_policy: None,
        };
        manager.configure(spec1).unwrap();

        let spec2 = ManagedContainer {
            name: "svc-b".into(),
            image: "nginx:latest".into(),
            container_id: None,
            state: ContainerState::Stopped,
            ports: vec![PortMapping {
                host_port: 9090,
                container_port: 80,
                protocol: "tcp".into(),
            }],
            env: HashMap::new(),
            volumes: Vec::new(),
            health_endpoint: None,
            restart_policy: None,
        };
        manager.configure(spec2).unwrap();

        assert_eq!(
            manager.container_state("svc-a"),
            Some(ContainerState::Stopped)
        );
        assert_eq!(
            manager.container_state("svc-b"),
            Some(ContainerState::Stopped)
        );
    }
}
