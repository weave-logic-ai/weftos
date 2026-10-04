//! Values the supervisor exposes: config, dependencies, errors, status.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use clawft_kernel::gate::{GateBackend, GovernanceSnapshot};
use clawft_kernel::token_authority::TokenAuthority;
use clawft_kernel::workload_runtime::RuntimeError;
use clawft_rpc::Response;
use clawft_types::project::ChildState;
use serde_json::Value;

use super::idle::ActivitySource;
use super::io::ChildIo;
use crate::project_cert_rpc::CertEnv;

/// Longest unix socket path that binds on every supported platform
/// (`sun_path` is 104 bytes on macOS, 108 on Linux, one for the NUL).
pub const MAX_SOCKET_PATH: usize = 103;

/// Chain source of supervisor events.
pub const CHAIN_SOURCE: &str = "project.supervisor";

/// Timings and paths. The defaults are the production values; tests shrink
/// the timings.
#[derive(Debug, Clone)]
pub struct SupervisorConfig {
    /// The user's home (the child's `HOME`).
    pub home: PathBuf,
    /// `~/.weftos/run`: children live in `<run_root>/<id>/`.
    pub run_root: PathBuf,
    /// `~/.weftos/projects`.
    pub manifests_dir: PathBuf,
    /// The kernel executable children are started from.
    pub exe: PathBuf,
    /// The user daemon's socket, written to `spawn.json`.
    pub parent_socket: PathBuf,
    /// First restart delay.
    pub backoff_initial: Duration,
    /// Longest restart delay.
    pub backoff_max: Duration,
    /// Wait after `kernel.shutdown` before `SIGTERM`.
    pub term_grace: Duration,
    /// Wait after `SIGTERM` before `SIGKILL`.
    pub kill_grace: Duration,
    /// How long a start waits for the child's handshake.
    pub ready_timeout: Duration,
    /// Handshake poll interval.
    pub ready_poll: Duration,
    /// Idle check interval.
    pub idle_poll: Duration,
    /// Poll interval for an adopted child's liveness.
    pub exit_poll: Duration,
    /// This daemon's build stamp (`BUILD_GIT_HASH`): a child whose handshake
    /// reports another one runs a stale binary (reported, never restarted).
    pub build_sha: String,
    /// How long a `running` child's registry session may stay expired
    /// (three missed heartbeats and no re-registration) before the
    /// supervisor treats the child as crashed and restarts it.
    pub lost_heartbeat_grace: Duration,
    /// The same for a child whose last heartbeat said it was busy: spared a
    /// stalled heartbeat handler for this much longer, then treated as lost
    /// (10 x [`lost_heartbeat_grace`](Self::lost_heartbeat_grace) by default).
    pub lost_heartbeat_busy_ceiling: Duration,
}

impl SupervisorConfig {
    /// Production defaults for a user daemon at `home` starting children
    /// from `exe`.
    pub fn new(home: &Path, exe: PathBuf) -> Self {
        let run_root = clawft_types::runtime_paths::user_runtime_root(home);
        Self {
            home: home.to_path_buf(),
            parent_socket: crate::user_daemon::child_socket_path(&run_root),
            run_root,
            manifests_dir: crate::user_daemon::manifests_dir(home),
            exe,
            backoff_initial: Duration::from_secs(1),
            backoff_max: Duration::from_secs(30),
            term_grace: Duration::from_secs(10),
            kill_grace: Duration::from_secs(5),
            ready_timeout: Duration::from_secs(30),
            ready_poll: Duration::from_millis(100),
            idle_poll: Duration::from_secs(30),
            exit_poll: Duration::from_millis(500),
            build_sha: env!("BUILD_GIT_HASH").to_owned(),
            lost_heartbeat_grace: Duration::from_secs(2 * crate::mesh_local_registry::HEARTBEAT_SECS),
            lost_heartbeat_busy_ceiling: Duration::from_secs(20 * crate::mesh_local_registry::HEARTBEAT_SECS),
        }
    }
}

/// What the supervisor needs from the daemon.
pub struct Deps {
    /// The user chain, the user key and the manifest store.
    pub cert_env: CertEnv,
    /// Snapshot of the governance engine for `parent-policy.json`.
    pub snapshot: Arc<dyn Fn() -> Option<GovernanceSnapshot> + Send + Sync>,
    /// Token authority (project tokens).
    pub tokens: Option<Arc<TokenAuthority>>,
    /// Where child activity comes from (idle stop).
    pub activity: Arc<dyn ActivitySource>,
    /// Calls to a child.
    pub io: Arc<dyn ChildIo>,
    /// Override of the workload gate (tests).
    pub gate: Option<Arc<dyn GateBackend>>,
}

/// Why a supervisor operation was refused or failed.
#[derive(Debug)]
pub enum SupError {
    /// `id` is not a project id.
    InvalidId(String),
    /// Not in the manifest store.
    NotRegistered(String),
    /// The root is `$HOME` or `/`: refusing to run a kernel over it.
    RootIsHome(PathBuf),
    /// The root is gone.
    RootMissing(PathBuf),
    /// `<root>/.weftos/project.toml` is missing or names another project.
    IdMismatch {
        /// Id the manifest registered.
        manifest: String,
        /// What project.toml says (`None`: missing).
        found: Option<String>,
    },
    /// A legacy project-rooted daemon holds the project's own `kernel.lock`.
    LegacyDaemonRunning(PathBuf),
    /// `<run>/<id>/kernel.sock` would not fit in a unix socket address.
    SocketPathTooLong(PathBuf),
    /// The project key was revoked and no certificate is in force.
    Revoked(String),
    /// Certificate or identity check failed.
    Identity(String),
    /// A nested project is outside its declared master or its master is not registered.
    Nested(String),
    /// The restart budget is spent; `project.restart` clears it.
    Failed(String),
    /// The child did not become ready.
    NotReady(String),
    /// A live kernel for the project (verified as ours, or holding its
    /// `kernel.lock`) cannot be taken over, and a second one cannot start.
    LiveLeftover(String),
    /// The adapter or gate refused or failed.
    Runtime(RuntimeError),
}

impl SupError {
    /// snake_case discriminator for clients.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::InvalidId(_) => "invalid_params",
            Self::NotRegistered(_) => "project_not_found",
            Self::RootIsHome(_) => "root_is_home",
            Self::RootMissing(_) => "root_missing",
            Self::IdMismatch { .. } => "project_id_mismatch",
            Self::LegacyDaemonRunning(_) => "legacy_daemon_running",
            Self::SocketPathTooLong(_) => "socket_path_too_long",
            Self::Revoked(_) => "project_revoked",
            Self::Identity(_) => "project_identity_error",
            Self::Nested(_) => "nested_project_refused",
            Self::Failed(_) => "project_failed",
            Self::NotReady(_) => "project_not_ready",
            Self::LiveLeftover(_) => "leftover_kernel",
            Self::Runtime(RuntimeError::Governance(_)) => "governance_denied",
            Self::Runtime(e) => match e.code() {
                "admission-refused" => "admission_refused",
                _ => "project_start_failed",
            },
        }
    }

    /// The RPC error response.
    pub fn response(&self) -> Response {
        Response::error_with_kind(self.kind(), self.to_string())
    }
}

impl std::fmt::Display for SupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidId(m) => write!(f, "{m}"),
            Self::NotRegistered(id) => write!(f, "project {id} is not registered with this daemon"),
            Self::RootIsHome(p) => write!(f, "refusing to run a project kernel over {}", p.display()),
            Self::RootMissing(p) => write!(f, "project root {} does not exist", p.display()),
            Self::IdMismatch { manifest, found } => write!(
                f,
                "project.toml in the root names {} but the manifest registered {manifest}",
                found.as_deref().unwrap_or("no project")
            ),
            Self::LegacyDaemonRunning(p) => write!(
                f,
                "a project-rooted daemon holds {}; stop it first (`weaver project migrate-kernel` explains)",
                p.display()
            ),
            Self::SocketPathTooLong(p) => write!(
                f,
                "{} is too long for a unix socket address ({} bytes, the limit is {MAX_SOCKET_PATH}); \
                 move the user daemon's run root (`WEFTOS_RUNTIME_DIR`) to a shorter path",
                p.display(),
                p.as_os_str().len()
            ),
            Self::Revoked(id) => write!(
                f,
                "project {id} was revoked (`project.revoke` is terminal for its id) and is never started again; \
                 to run the tree again give it a new identity (`weft project init --fork --force`); \
                 the marker is <run_root>/{id}/revoked"
            ),
            Self::Identity(m) => write!(f, "{m}"),
            Self::Nested(m) => write!(f, "nested project refused: {m}"),
            Self::Failed(m) => write!(
                f,
                "project kernel failed ({m}); fix the cause, then `weaver kernel restart --project <id>` \
                 (`project.restart`) clears it"
            ),
            Self::NotReady(m) => write!(f, "project kernel did not become ready: {m}"),
            Self::LiveLeftover(m) => write!(f, "a live project kernel is already there and cannot be adopted: {m}"),
            Self::Runtime(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SupError {}

impl From<RuntimeError> for SupError {
    fn from(e: RuntimeError) -> Self {
        Self::Runtime(e)
    }
}

/// Result of [`Supervisor::ensure_running`] and friends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
    /// The child's socket.
    pub socket: PathBuf,
    /// The child's pid.
    pub pid: u32,
    /// True when this call started it.
    pub started: bool,
}

/// A project's supervision status.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Status {
    /// Project id.
    pub project_id: String,
    /// State machine position.
    pub state: ChildState,
    /// Live pid.
    pub pid: Option<u32>,
    /// Immutable engine identity for a Linux container, when selected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<super::state::ContainerState>,
    /// Child socket.
    pub socket: PathBuf,
    /// Automatic restarts so far.
    pub restarts: u32,
    /// Exit code of the last exit.
    pub last_exit_code: Option<i32>,
    /// Why the project is `failed`.
    pub failed_reason: Option<String>,
    /// Build stamp the running kernel reported when it became ready.
    pub kernel_sha: Option<String>,
    /// Crate version the running kernel reported.
    pub kernel_version: Option<String>,
    /// The kernel runs another build than this daemon (typically after
    /// `weaver update`, which keeps children running).
    pub stale_build: bool,
    /// Seconds an adopted child has gone without registering with this
    /// daemon, as of the last liveness pass (`None`: it registered, or the
    /// project was not adopted). Such a child is never restarted for a lost
    /// heartbeat, so this is how it is reported.
    pub unregistered_secs: Option<u64>,
}

impl Status {
    /// JSON for RPC replies.
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}
