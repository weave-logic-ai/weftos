//! The project supervisor: per-project child kernels under the user daemon
//! (ADR-103 A6, Phase 2 package G).
//!
//! The user daemon owns one [`Supervisor`]. A project becomes a running
//! child through the `project` workload kind and the `logical` adapter
//! (`clawft-kernel`), driven by a [`WorkloadHost`] so every transition is
//! gated (only the supervisor principal may run the `project` kind) and
//! chained as `workload.*`. The supervisor adds, around that:
//!
//! * [`child`]: spawn contract (allow-list environment, `spawn.json`,
//!   `--profile project`), terminate (graceful `kernel.shutdown`, then
//!   `SIGTERM`, then `SIGKILL`);
//! * [`restart`]: OTP `one_for_one` plus `Transient`: crashes restart with
//!   1 s..30 s backoff inside a budget, then `failed`; clean exits never do;
//! * [`adopt`]: children that outlived a daemon restart are verified and
//!   adopted, never signalled when unverifiable;
//! * [`idle`]: graceful stop of idle projects.
//!
//! Locking: one async mutex per project serialises start, stop and restart,
//! so concurrent `ensure_running` calls start exactly one child. The state
//! mutex is a plain `std` mutex held only for short, non-awaiting sections.

pub mod adopt;
pub mod child;
pub mod idle;
pub mod io;
pub mod restart;
pub mod state;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use clawft_kernel::gate::{GateBackend, GovernanceSnapshot};
use clawft_kernel::token_authority::TokenAuthority;
use clawft_kernel::workload_governance::{
    NodeTrustTier, SUPERVISOR_PRINCIPAL, WorkloadGate, project_supervisor_permit,
};
use clawft_kernel::workload_kind::{ProjectFacts, ProjectPrepareError, prepare_project};
use clawft_kernel::workload_runtime::{
    ChildLauncher, ChildProbe, HostContract, InstanceHandle, LogicalRuntime, RunMode, RuntimeError,
    VerifiedWorkload, WorkloadConfig, WorkloadHost,
};
use clawft_rpc::Response;
use clawft_types::project::{ChildState, ProjectManifest, ServeVia};
use clawft_types::runtime_paths::{LOCK_FILE_NAME, SOCKET_NAME};
use serde_json::{Value, json};

use self::adopt::Found;
use self::child::{ExitInfo, Launcher, LauncherParts};
use self::idle::ActivitySource;
use self::io::ChildIo;
use self::restart::{Decision, RestartTracker};
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
}

impl SupervisorConfig {
    /// Production defaults for a user daemon at `home` starting children
    /// from `exe`.
    pub fn new(home: &Path, exe: PathBuf) -> Self {
        let run_root = clawft_types::runtime_paths::user_runtime_root(home);
        Self {
            home: home.to_path_buf(),
            parent_socket: run_root.join(SOCKET_NAME),
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
    /// The restart budget is spent; `project.restart` clears it.
    Failed(String),
    /// The child did not become ready.
    NotReady(String),
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
            Self::Failed(_) => "project_failed",
            Self::NotReady(_) => "project_not_ready",
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
            Self::Revoked(id) => write!(f, "project {id}'s key was revoked; re-register or rekey it"),
            Self::Identity(m) => write!(f, "{m}"),
            Self::Failed(m) => write!(
                f,
                "project kernel failed ({m}); fix the cause, then `weaver kernel restart --project <id>` \
                 (`project.restart`) clears it"
            ),
            Self::NotReady(m) => write!(f, "project kernel did not become ready: {m}"),
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
    /// Child socket.
    pub socket: PathBuf,
    /// Automatic restarts so far.
    pub restarts: u32,
    /// Exit code of the last exit.
    pub last_exit_code: Option<i32>,
    /// Why the project is `failed`.
    pub failed_reason: Option<String>,
}

impl Status {
    /// JSON for RPC replies.
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

#[derive(Default)]
struct SlotState {
    state: ChildState,
    handle: Option<InstanceHandle>,
    tracker: Option<RestartTracker>,
    generation: u64,
    last_exit: Option<ExitInfo>,
    failed: Option<String>,
    restarts: u32,
}

#[derive(Default)]
struct Slot {
    gate: tokio::sync::Mutex<()>,
    st: Mutex<SlotState>,
}

impl Slot {
    fn st(&self) -> std::sync::MutexGuard<'_, SlotState> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The supervisor.
pub struct Supervisor {
    cfg: Arc<SupervisorConfig>,
    deps: Deps,
    launcher: Arc<Launcher>,
    host: WorkloadHost,
    slots: Mutex<HashMap<String, Arc<Slot>>>,
    leftovers: Mutex<Vec<Found>>,
}

static GLOBAL: OnceLock<Arc<Supervisor>> = OnceLock::new();

/// Install `sup` as the process supervisor. `false` when one is installed
/// already (the first wins).
pub fn install_global(sup: Arc<Supervisor>) -> bool {
    GLOBAL.set(sup).is_ok()
}

/// The user daemon's supervisor, once installed ([`post_boot`]).
pub fn global() -> Option<Arc<Supervisor>> {
    GLOBAL.get().cloned()
}

impl Supervisor {
    /// A supervisor over `cfg` and `deps`.
    pub fn new(cfg: SupervisorConfig, deps: Deps) -> Arc<Self> {
        let cfg = Arc::new(cfg);
        let launcher = Arc::new(Launcher::new(
            Arc::clone(&cfg),
            LauncherParts {
                user_key: deps.cert_env.user_key.clone(),
                snapshot: Arc::clone(&deps.snapshot),
                tokens: deps.tokens.clone(),
                io: Arc::clone(&deps.io),
            },
        ));
        let gate: Arc<dyn GateBackend> = deps.gate.clone().unwrap_or_else(|| {
            Arc::new(
                WorkloadGate::new(0.95, false)
                    .with_permit(project_supervisor_permit())
                    .unwrap_or_else(|_| WorkloadGate::new(0.95, false))
                    .with_chain(Arc::clone(&deps.cert_env.chain)),
            )
        });
        let runtime = Arc::new(LogicalRuntime::new(Arc::clone(&launcher) as Arc<dyn ChildLauncher>));
        let host = WorkloadHost::new(runtime, gate, SUPERVISOR_PRINCIPAL, NodeTrustTier::Paired)
            .with_chain(Arc::clone(&deps.cert_env.chain));
        Arc::new(Self {
            cfg,
            deps,
            launcher,
            host,
            slots: Mutex::new(HashMap::new()),
            leftovers: Mutex::new(Vec::new()),
        })
    }

    /// The config.
    pub fn config(&self) -> &SupervisorConfig {
        &self.cfg
    }

    /// The launcher (token refresh, tests).
    pub fn launcher(&self) -> &Arc<Launcher> {
        &self.launcher
    }

    /// `<run_root>/<id>`.
    pub fn run_dir(&self, id: &str) -> PathBuf {
        self.cfg.run_root.join(id)
    }

    fn socket(&self, id: &str) -> PathBuf {
        self.run_dir(id).join(SOCKET_NAME)
    }

    fn slot(&self, id: &str) -> Arc<Slot> {
        let mut m = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        Arc::clone(m.entry(id.to_owned()).or_default())
    }

    fn chain(&self, kind: &str, payload: Value) {
        self.deps.cert_env.chain.append(CHAIN_SOURCE, kind, Some(payload));
    }

    /// The spawn refusals plus the workload the `logical` adapter runs.
    fn prepare(&self, id: &str) -> Result<(VerifiedWorkload, ProjectManifest), SupError> {
        clawft_types::project::validate_id(id).map_err(|e| SupError::InvalidId(e.to_string()))?;
        let manifest = clawft_types::project::find_by_id(&self.cfg.manifests_dir, id)
            .map_err(|e| SupError::Identity(e.to_string()))?
            .ok_or_else(|| SupError::NotRegistered(id.to_owned()))?;
        let root = manifest.root.clone();
        let home = self.cfg.home.canonicalize().unwrap_or_else(|_| self.cfg.home.clone());
        let canon = root.canonicalize().map_err(|_| SupError::RootMissing(root.clone()))?;
        if canon == home || canon == Path::new("/") {
            return Err(SupError::RootIsHome(canon));
        }
        if !canon.is_dir() {
            return Err(SupError::RootMissing(root));
        }
        let found = clawft_types::project::read_project_toml(&canon)
            .ok()
            .flatten()
            .map(|p| p.id);
        if found.as_deref() != Some(id) {
            return Err(SupError::IdMismatch { manifest: id.to_owned(), found });
        }
        let sock = self.socket(id);
        if sock.as_os_str().len() > MAX_SOCKET_PATH {
            return Err(SupError::SocketPathTooLong(sock));
        }
        let legacy = canon.join(".weftos").join("runtime").join(LOCK_FILE_NAME);
        if adopt::lock_held(&legacy) {
            return Err(SupError::LegacyDaemonRunning(legacy));
        }
        let view = crate::project_cert_rpc::current_view(&self.deps.cert_env)
            .map_err(|e| SupError::Identity(e.kind().to_owned() + ": " + &e.to_string()))?;
        let cert = view.current_cert(id).cloned();
        let run_dir = self.run_dir(id);
        if cert.is_none() && state::is_marked_revoked(&run_dir) {
            return Err(SupError::Revoked(id.to_owned()));
        }
        let upub = self.deps.cert_env.user_key.verifying_key().to_bytes();
        let facts = ProjectFacts {
            cert: cert.as_ref(),
            user_pubkey: &upub,
            revocations: &view,
            manifest_id: id,
            root: &canon,
            policy_hash: "",
        };
        let w = prepare_project(&facts).map_err(|e| match e {
            ProjectPrepareError::Identity(m) => SupError::Identity(m),
            other => SupError::Identity(other.to_string()),
        })?;
        // A valid certificate in force again after a rekey lifts the marker.
        if cert.is_some() {
            state::clear_revoked(&run_dir);
        }
        Ok((w, manifest))
    }

    fn tracker_for(&self, m: &ProjectManifest) -> RestartTracker {
        let serve = m.serve.clone().unwrap_or_default();
        RestartTracker::new(
            serve.restart_max(),
            Duration::from_secs(serve.restart_window_secs()),
            self.cfg.backoff_initial,
            self.cfg.backoff_max,
        )
    }

    fn host_cfg(id: &str) -> WorkloadConfig {
        WorkloadConfig {
            mode: RunMode::Listener,
            args: Vec::new(),
            host: HostContract::default_feed(),
            node_id: id.to_owned(),
        }
    }

    fn set_state(&self, id: &str, slot: &Slot, state: ChildState) {
        let (restarts, exit, failed) = {
            let mut st = slot.st();
            st.state = state;
            (st.restarts, st.last_exit, st.failed.clone())
        };
        state::update(&self.run_dir(id), |s| {
            s.state = state;
            s.restarts = restarts;
            s.last_exit_code = exit.and_then(|e| e.code);
            s.failed_reason = failed;
            if matches!(state, ChildState::Stopped | ChildState::Failed) {
                s.pid = None;
            }
        });
    }

    async fn probe_running(&self, id: &str) -> Option<u32> {
        match self.launcher.probe(id).await {
            ChildProbe::Running { pid } => Some(pid),
            _ => None,
        }
    }

    /// Start (or find) the project's child and wait for its handshake.
    /// Idempotent and safe to call concurrently: exactly one child starts.
    pub async fn ensure_running(self: &Arc<Self>, id: &str) -> Result<Running, SupError> {
        clawft_types::project::validate_id(id).map_err(|e| SupError::InvalidId(e.to_string()))?;
        let slot = self.slot(id);
        let _g = slot.gate.lock().await;
        let current = slot.st().state;
        if let Some(pid) = self.probe_running(id).await
            && matches!(current, ChildState::Running | ChildState::Starting)
        {
            return Ok(Running { socket: self.socket(id), pid, started: false });
        }
        let failed = {
            let st = slot.st();
            (st.state == ChildState::Failed).then(|| st.failed.clone().unwrap_or_default())
        };
        if let Some(why) = failed {
            return Err(SupError::Failed(why));
        }
        self.start_locked(id, &slot).await
    }

    /// `project.start`: same as [`ensure_running`](Self::ensure_running).
    pub async fn start(self: &Arc<Self>, id: &str) -> Result<Running, SupError> {
        self.ensure_running(id).await
    }

    async fn start_locked(self: &Arc<Self>, id: &str, slot: &Arc<Slot>) -> Result<Running, SupError> {
        let (w, manifest) = self.prepare(id)?;
        let existing = {
            let mut st = slot.st();
            match st.tracker.as_mut() {
                Some(t) => {
                    let s = manifest.serve.clone().unwrap_or_default();
                    t.reconfigure(s.restart_max(), Duration::from_secs(s.restart_window_secs()));
                }
                None => st.tracker = Some(self.tracker_for(&manifest)),
            }
            st.handle.clone()
        };
        let handle = match existing {
            Some(h) => h,
            None => {
                let h = self.host.load(&w, &Self::host_cfg(id)).await?;
                slot.st().handle = Some(h.clone());
                h
            }
        };
        let bumped = {
            let mut st = slot.st();
            st.generation += 1;
            st.failed = None;
            st.state = ChildState::Starting;
            st.generation
        };
        if let Err(e) = self.host.start(&handle).await {
            self.set_state(id, slot, ChildState::Stopped);
            return Err(e.into());
        }
        if let Some(t) = slot.st().tracker.as_mut() {
            t.on_started(Instant::now());
        }
        let pid = self.launcher.pid_of(id).unwrap_or(0);
        self.chain("project.kernel.started", json!({"project_id": id, "pid": pid}));
        self.set_state(id, slot, ChildState::Starting);
        self.spawn_monitor(id.to_owned(), Arc::clone(slot), bumped);
        match self.wait_ready(id).await {
            Ok(pid) => {
                self.set_state(id, slot, ChildState::Running);
                self.record_kernel_build(id).await;
                Ok(Running { socket: self.socket(id), pid, started: true })
            }
            Err(why) => Err(SupError::NotReady(why)),
        }
    }

    /// Write the version and build of the kernel just started into the
    /// manifest's `[serve]` (`kernel_version`, `kernel_sha`; supervisor-written,
    /// never by the owner), only when they changed.
    async fn record_kernel_build(&self, id: &str) {
        let (dir, id) = (self.cfg.manifests_dir.clone(), id.to_owned());
        let r = tokio::task::spawn_blocking(move || {
            clawft_types::project::update_manifest(&dir, &id, |m| {
                let s = m.serve.get_or_insert_with(Default::default);
                s.kernel_version = Some(env!("CARGO_PKG_VERSION").to_owned());
                s.kernel_sha = Some(env!("BUILD_GIT_HASH").to_owned());
            })
        })
        .await;
        if !matches!(r, Ok(Ok(Some(_)))) {
            tracing::warn!("could not record the project kernel build in the manifest");
        }
    }

    /// Wait for the child's handshake to name this project.
    async fn wait_ready(&self, id: &str) -> Result<u32, String> {
        let deadline = tokio::time::Instant::now() + self.cfg.ready_timeout;
        let sock = self.socket(id);
        loop {
            let Some(pid) = self.launcher.pid_of(id) else {
                let info = self.launcher.wait_exit(id).await;
                return Err(format!(
                    "the child exited (code {:?}, signal {:?}) before answering; see {}",
                    info.code,
                    info.signal,
                    self.run_dir(id).join("kernel.log").display()
                ));
            };
            if let Some(h) = self.deps.io.handshake(&sock).await
                && h.project_id.as_deref() == Some(id)
            {
                return Ok(pid);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(format!("no handshake from {} within {:?}", sock.display(), self.cfg.ready_timeout));
            }
            tokio::time::sleep(self.cfg.ready_poll).await;
        }
    }

    fn spawn_monitor(self: &Arc<Self>, id: String, slot: Arc<Slot>, generation: u64) {
        let this = Arc::clone(self);
        tokio::spawn(async move { this.monitor(id, slot, generation).await });
    }

    /// Watch one child through its crashes: restart per policy, mark
    /// `failed` when the budget is spent, stop watching on a clean exit or
    /// when a stop or newer start superseded this generation.
    async fn monitor(self: Arc<Self>, id: String, slot: Arc<Slot>, mut generation: u64) {
        let mut synthetic: Option<ExitInfo> = None;
        loop {
            let info = match synthetic.take() {
                Some(i) => i,
                None => self.launcher.wait_exit(&id).await,
            };
            let stop_requested = self.launcher.stop_requested(&id);
            let outcome = {
                let mut st = slot.st();
                if st.generation != generation {
                    return;
                }
                st.last_exit = Some(info);
                if stop_requested || info.clean() {
                    None
                } else {
                    let now = Instant::now();
                    match st.tracker.as_mut().map(|t| t.on_crash(now)) {
                        Some(Decision::Restart { after }) => {
                            st.restarts += 1;
                            Some(Ok(after))
                        }
                        Some(Decision::GiveUp { restarts_in_window }) => {
                            let why = format!(
                                "{restarts_in_window} restarts inside the window, last exit code {:?} signal {:?}",
                                info.code, info.signal
                            );
                            st.failed = Some(why.clone());
                            Some(Err(why))
                        }
                        None => Some(Err("no restart policy".to_owned())),
                    }
                }
            };
            match outcome {
                None => {
                    self.set_state(&id, &slot, ChildState::Stopped);
                    self.chain(
                        "project.kernel.exited",
                        json!({"project_id": id, "code": info.code, "signal": info.signal, "clean": true}),
                    );
                    return;
                }
                Some(Err(why)) => {
                    self.set_state(&id, &slot, ChildState::Failed);
                    self.launcher.revoke_tokens(&id);
                    self.chain("project.kernel.failed", json!({"project_id": id, "reason": why}));
                    return;
                }
                Some(Ok(after)) => {
                    self.set_state(&id, &slot, ChildState::Starting);
                    self.chain(
                        "project.kernel.exited",
                        json!({"project_id": id, "code": info.code, "signal": info.signal,
                               "clean": false, "restart_in_ms": after.as_millis() as u64}),
                    );
                    tokio::time::sleep(after).await;
                    let _g = slot.gate.lock().await;
                    if slot.st().generation != generation {
                        return;
                    }
                    match self.restart_once(&id, &slot).await {
                        Ok(g) => {
                            generation = g;
                            self.chain("project.kernel.restarted", json!({"project_id": id}));
                        }
                        Err(e) => {
                            tracing::warn!(project = %id, error = %e, "project kernel restart failed");
                            synthetic = Some(ExitInfo { code: Some(-1), signal: None, clean_hint: false });
                        }
                    }
                }
            }
        }
    }

    async fn restart_once(self: &Arc<Self>, id: &str, slot: &Arc<Slot>) -> Result<u64, SupError> {
        self.prepare(id)?;
        let handle = slot
            .st()
            .handle
            .clone()
            .ok_or_else(|| SupError::Identity("project kernel was unloaded".into()))?;
        self.host.start(&handle).await?;
        let g = {
            let mut st = slot.st();
            st.generation += 1;
            if let Some(t) = st.tracker.as_mut() {
                t.on_started(Instant::now());
            }
            st.generation
        };
        // The new child becomes `running` when it answers; the monitor does
        // not wait for that.
        let this = Arc::clone(self);
        let (id2, slot2) = (id.to_owned(), Arc::clone(slot));
        tokio::spawn(async move {
            if this.wait_ready(&id2).await.is_ok() && slot2.st().generation == g {
                this.set_state(&id2, &slot2, ChildState::Running);
            }
        });
        Ok(g)
    }

    /// Stop the child gracefully (final anchor, then signals). Stopped
    /// children are not restarted. Returns whether one was running.
    pub async fn stop(self: &Arc<Self>, id: &str) -> Result<bool, SupError> {
        clawft_types::project::validate_id(id).map_err(|e| SupError::InvalidId(e.to_string()))?;
        let slot = self.slot(id);
        let _g = slot.gate.lock().await;
        self.stop_locked(id, &slot, "stop").await
    }

    async fn stop_locked(self: &Arc<Self>, id: &str, slot: &Arc<Slot>, why: &str) -> Result<bool, SupError> {
        let running = self.probe_running(id).await.is_some();
        let handle = {
            let mut st = slot.st();
            st.generation += 1; // retire the monitor of the old child
            st.handle.clone()
        };
        if running {
            if why == "idle" {
                self.set_state(id, slot, ChildState::IdleStopping);
            }
            if let Some(h) = handle {
                self.host.stop(&h, self.cfg.term_grace).await?;
            }
            self.chain(
                if why == "idle" { "project.kernel.idle_stop" } else { "project.kernel.stopped" },
                json!({"project_id": id}),
            );
        }
        self.launcher.revoke_tokens(id);
        // A failed project stays failed until `restart` clears it.
        let cur = {
            let mut st = slot.st();
            if st.state != ChildState::Failed {
                st.state = ChildState::Stopped;
            }
            st.state
        };
        self.set_state(id, slot, cur);
        Ok(running)
    }

    /// `project.restart`: stop, forget the failure and start again.
    pub async fn restart(self: &Arc<Self>, id: &str) -> Result<Running, SupError> {
        clawft_types::project::validate_id(id).map_err(|e| SupError::InvalidId(e.to_string()))?;
        let slot = self.slot(id);
        let _g = slot.gate.lock().await;
        self.stop_locked(id, &slot, "restart").await?;
        {
            let mut st = slot.st();
            st.failed = None;
            st.restarts = 0;
            st.state = ChildState::Stopped;
            if let Some(t) = st.tracker.as_mut() {
                t.reset();
            }
        }
        self.start_locked(id, &slot).await
    }

    /// Stop every running child (user-daemon stop cascade).
    pub async fn stop_all(self: &Arc<Self>) -> Vec<String> {
        let ids: Vec<String> = self.slots.lock().unwrap_or_else(|e| e.into_inner()).keys().cloned().collect();
        let mut stopped = Vec::new();
        for id in ids {
            if matches!(self.stop(&id).await, Ok(true)) {
                stopped.push(id);
            }
        }
        stopped
    }

    /// Mark a project revoked or rekeyed: drop the marker its child checks
    /// and stop the child (its key is no longer certified).
    pub async fn revoked(self: &Arc<Self>, id: &str, reason: &str) {
        if let Err(e) = state::mark_revoked(&self.run_dir(id), reason) {
            tracing::warn!(project = id, error = %e, "could not write the revoked marker");
        }
        if let Err(e) = self.stop(id).await {
            tracing::warn!(project = id, error = %e, "could not stop a revoked project's kernel");
        }
    }

    /// Status of one project.
    pub async fn status(&self, id: &str) -> Status {
        let slot = self.slot(id);
        let (state_, restarts, exit, failed) = {
            let st = slot.st();
            (st.state, st.restarts, st.last_exit, st.failed.clone())
        };
        Status {
            project_id: id.to_owned(),
            state: state_,
            pid: self.probe_running(id).await,
            socket: self.socket(id),
            restarts,
            last_exit_code: exit.and_then(|e| e.code),
            failed_reason: failed,
        }
    }

    /// Status of every project the supervisor knows.
    pub async fn status_all(&self) -> Vec<Status> {
        let ids: Vec<String> = self.slots.lock().unwrap_or_else(|e| e.into_inner()).keys().cloned().collect();
        let mut v = Vec::new();
        for id in ids {
            v.push(self.status(&id).await);
        }
        v.sort_by(|a, b| a.project_id.cmp(&b.project_id));
        v
    }

    /// Leftover run dirs the last adoption scan could not verify.
    pub fn unverifiable(&self) -> Vec<Found> {
        self.leftovers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|f| matches!(f, Found::Unverifiable { reason, .. } if *reason != adopt::Skip::Dead))
            .cloned()
            .collect()
    }

    /// Verify and adopt children left by an earlier daemon (see [`adopt`]).
    pub async fn adopt_on_boot(self: &Arc<Self>) -> Vec<Found> {
        let found = adopt::scan(&self.cfg.run_root, &self.cfg.exe, self.deps.io.as_ref()).await;
        for f in &found {
            match f {
                Found::Adopted { id, pid } => {
                    let slot = self.slot(id);
                    let _g = slot.gate.lock().await;
                    let (w, manifest) = match self.prepare(id) {
                        Ok(x) => x,
                        Err(e) => {
                            tracing::warn!(project = %id, error = %e, "not adopting: project no longer prepares");
                            continue;
                        }
                    };
                    self.launcher.adopt(id, *pid);
                    let handle = match self.host.load(&w, &Self::host_cfg(id)).await {
                        Ok(h) => h,
                        Err(e) => {
                            tracing::warn!(project = %id, error = %e, "not adopting: load refused");
                            self.launcher.forget(id);
                            continue;
                        }
                    };
                    let g = {
                        let mut st = slot.st();
                        st.handle = Some(handle);
                        st.tracker = Some(self.tracker_for(&manifest));
                        st.generation += 1;
                        st.generation
                    };
                    self.set_state(id, &slot, ChildState::Running);
                    self.chain("project.kernel.adopted", json!({"project_id": id, "pid": pid}));
                    self.spawn_monitor(id.clone(), Arc::clone(&slot), g);
                }
                Found::Unverifiable { id, pid, reason } if *reason != adopt::Skip::Dead => {
                    tracing::warn!(project = %id, pid = ?pid, %reason, "leftover project kernel not adopted and not signalled");
                }
                Found::Unverifiable { .. } => {}
            }
        }
        *self.leftovers.lock().unwrap_or_else(|e| e.into_inner()) = found.clone();
        found
    }

    /// One idle pass at `now_unix`: stop every running project that has been
    /// quiet for its `idle_stop_secs`. Returns the ids stopped.
    pub async fn idle_pass(self: &Arc<Self>, now_unix: u64) -> Vec<String> {
        let running: Vec<String> = self
            .status_all()
            .await
            .into_iter()
            .filter(|s| s.state == ChildState::Running)
            .map(|s| s.project_id)
            .collect();
        let mut stopped = Vec::new();
        for id in running {
            let Ok(Some(m)) = clawft_types::project::find_by_id(&self.cfg.manifests_dir, &id) else {
                continue;
            };
            let secs = m.serve.as_ref().map_or(0, |s| s.idle_stop_secs());
            let activity = self.deps.activity.activity(&id);
            if idle::should_stop(now_unix, secs, activity.as_ref()) {
                let slot = self.slot(&id);
                let _g = slot.gate.lock().await;
                if slot.st().state == ChildState::Running
                    && matches!(self.stop_locked(&id, &slot, "idle").await, Ok(true))
                {
                    stopped.push(id);
                }
            }
        }
        stopped
    }

    /// Run [`idle_pass`](Self::idle_pass) every `idle_poll` until the
    /// supervisor is dropped.
    pub fn spawn_idle_loop(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let every = self.cfg.idle_poll;
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(every).await;
                let Some(this) = weak.upgrade() else { return };
                this.idle_pass(state::now_unix()).await;
            }
        });
    }

    /// `via = child-kernel` for the project (the owner's opt-in).
    pub fn is_child_kernel(&self, id: &str) -> bool {
        matches!(
            clawft_types::project::find_by_id(&self.cfg.manifests_dir, id),
            Ok(Some(m)) if m.serve.as_ref().is_some_and(|s| s.via == ServeVia::ChildKernel)
        )
    }
}

/// `post_boot` body for the user daemon: build the supervisor from the
/// booted kernel, install it, adopt leftovers and start the idle loop. A
/// no-op for any other profile.
pub fn post_boot(kernel: &clawft_kernel::Kernel<clawft_platform::NativePlatform>) {
    if !crate::user_daemon::is_active() {
        return;
    }
    let Ok(rt) = tokio::runtime::Handle::try_current() else { return };
    let Some(chain) = kernel.chain_manager().cloned() else { return };
    let Some(user_key) = chain.signing_key_clone() else { return };
    let (Some(home), Some(manifests_dir)) = (
        clawft_types::runtime_paths::home_dir(),
        crate::project_rpc::configured_dir(),
    ) else {
        return;
    };
    let Ok(exe) = std::env::current_exe() else { return };
    let paths = clawft_types::runtime_paths::RuntimePaths::resolve();
    let mut cfg = SupervisorConfig::new(&home, exe);
    cfg.run_root = paths.root().to_path_buf();
    cfg.parent_socket = paths.socket();
    cfg.manifests_dir = manifests_dir.clone();
    let gate = kernel.governance_gate().cloned();
    let deps = Deps {
        cert_env: CertEnv { chain, user_key: user_key.clone(), manifests_dir: manifests_dir.clone() },
        snapshot: Arc::new(move || gate.as_ref().and_then(|g| g.governance_snapshot())),
        tokens: crate::token_rpc::authority_for_kernel(kernel),
        activity: Arc::new(idle::NoActivity),
        io: Arc::new(io::RpcChildIo::new(user_key, manifests_dir)),
        gate: None,
    };
    let sup = Supervisor::new(cfg, deps);
    if !install_global(Arc::clone(&sup)) {
        return;
    }
    rt.spawn(async move {
        let found = sup.adopt_on_boot().await;
        tracing::info!(children = found.len(), "project supervisor adoption scan done");
        sup.spawn_idle_loop();
    });
}
