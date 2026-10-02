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
mod boot;
pub mod child;
pub mod idle;
pub mod io;
pub mod restart;
pub mod state;
mod types;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use clawft_kernel::gate::GateBackend;
use clawft_kernel::workload_governance::{
    NodeTrustTier, SUPERVISOR_PRINCIPAL, WorkloadGate, project_supervisor_permit,
};
use clawft_kernel::workload_kind::{ProjectFacts, ProjectPrepareError, prepare_project};
use clawft_kernel::workload_runtime::{
    ChildLauncher, ChildProbe, ChildRef, HostContract, InstanceHandle, LogicalRuntime, RunMode, RuntimeError,
    VerifiedWorkload, WorkloadConfig, WorkloadHost,
};
use clawft_types::project::{ChildState, ProjectManifest, ServeVia};
use clawft_types::runtime_paths::{LOCK_FILE_NAME, SOCKET_NAME};
use serde_json::{Value, json};

use self::adopt::Found;
use self::child::{ExitInfo, Launcher, LauncherParts};
use self::restart::{Decision, RestartTracker};
use crate::project_cert_rpc::CertEnv;

#[derive(Default)]
struct SlotState {
    state: ChildState,
    handle: Option<InstanceHandle>,
    tracker: Option<RestartTracker>,
    generation: u64,
    last_exit: Option<ExitInfo>,
    failed: Option<String>,
    restarts: u32,
    /// When the liveness pass first saw this `running` child's session
    /// expired (and it has not registered again since).
    expired_since: Option<Instant>,
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

    /// `<run_root>/<id>` (`runtime_paths::child_run_dir`; ids are validated
    /// before they get here).
    pub fn run_dir(&self, id: &str) -> PathBuf {
        clawft_types::runtime_paths::child_run_dir(&self.cfg.run_root, id)
            .unwrap_or_else(|| self.cfg.run_root.join(id))
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
        // Revoke is terminal: the marker is never lifted by the supervisor.
        // The journal says the same: a project whose key was revoked and
        // that has no certificate in force is refused even when the marker
        // is missing (a full disk, a hand-removed file), instead of being
        // spawned just to die on `project_revoked` and burn its restart
        // budget.
        if state::is_marked_revoked(&self.cfg.run_root, id) || (cert.is_none() && view.was_revoked(id)) {
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
        if let Some(pid) = self.probe_running(id).await {
            match current {
                ChildState::Running => return Ok(Running { socket: self.socket(id), pid, started: false }),
                // An automatic restart in flight: its child has a pid but
                // has not bound its socket yet. Never hand that socket out;
                // wait (bounded) for the handshake like a fresh start does.
                ChildState::Starting => {
                    return match self.wait_ready(id).await {
                        Ok(pid) => {
                            self.note_build(id).await;
                            if slot.st().state == ChildState::Starting {
                                self.set_state(id, &slot, ChildState::Running);
                            }
                            Ok(Running { socket: self.socket(id), pid, started: false })
                        }
                        Err(why) => Err(SupError::NotReady(why)),
                    };
                }
                _ => {}
            }
        }
        if state::is_marked_revoked(&self.cfg.run_root, id) {
            return Err(SupError::Revoked(id.to_owned()));
        }
        let failed = {
            let st = slot.st();
            (st.state == ChildState::Failed).then(|| st.failed.clone().unwrap_or_default())
        };
        if let Some(why) = failed {
            return Err(SupError::Failed(why));
        }
        // A live verified kernel that nobody supervises (an adoption that
        // was skipped, a restarted daemon) is taken over, never duplicated.
        if let Some(found) =
            adopt::scan_one(&self.run_dir(id), id, &self.cfg.exe, self.deps.io.as_ref()).await
        {
            match found {
                Found::Adopted { pid, .. } => {
                    return match self.adopt_one(id, pid, &slot).await {
                        Ok(()) => Ok(Running { socket: self.socket(id), pid, started: false }),
                        Err(reason) => Err(SupError::LiveLeftover(reason.to_string())),
                    };
                }
                Found::Unverifiable { pid: Some(pid), reason: adopt::Skip::HandshakeFailed(m), .. } => {
                    return Err(SupError::LiveLeftover(format!("pid {pid} holds the lock but {m}")));
                }
                Found::Unverifiable { .. } => {}
            }
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
                self.note_build(id).await;
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

    /// Record the build the running child reports (its handshake `sha` and
    /// `version`) in `state.json`, where [`status`](Self::status) and the
    /// doctor compare it with this daemon's build (a child outlives
    /// `weaver update`; adoption never replaces it).
    pub(super) async fn note_build(&self, id: &str) {
        let Some(h) = self.deps.io.handshake(&self.socket(id)).await else { return };
        if h.project_id.as_deref() != Some(id) {
            return;
        }
        state::update(&self.run_dir(id), |s| {
            s.kernel_sha = (!h.sha.is_empty()).then(|| h.sha.clone());
            s.kernel_version = (!h.version.is_empty()).then(|| h.version.clone());
        });
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
            // The answer must come from the process we launched: a stale
            // socket or a squatter answering for the project is not it.
            if let Some(h) = self.deps.io.handshake(&sock).await
                && h.project_id.as_deref() == Some(id)
                && h.pid == pid
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
                    self.chain(
                        "project.kernel.exited",
                        json!({"project_id": id, "code": info.code, "signal": info.signal, "clean": true}),
                    );
                    self.set_state(&id, &slot, ChildState::Stopped);
                    return;
                }
                Some(Err(why)) => {
                    // Chained before the state flips: whoever sees `failed`
                    // can already find the event.
                    self.chain("project.kernel.failed", json!({"project_id": id, "reason": why}));
                    self.launcher.revoke_tokens(&id);
                    self.launcher.clean_spawn_file(&id);
                    self.set_state(&id, &slot, ChildState::Failed);
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
                this.note_build(&id2).await;
                this.record_kernel_build(&id2).await;
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
        // Credentials first: whatever happens to the process, its token is
        // dead before we try to stop it.
        self.launcher.revoke_tokens(id);
        let mut stop_err: Option<String> = None;
        if running {
            if why == "idle" {
                self.set_state(id, slot, ChildState::IdleStopping);
            }
            if let Some(h) = handle
                && let Err(e) = self.host.stop(&h, self.cfg.term_grace).await
            {
                stop_err = Some(e.to_string());
            }
            // The gated stop failed or did not finish: fall through to the
            // launcher's graceful-then-signal path. A project we were asked
            // to stop must not keep running because governance said no.
            if let Some(pid) = self.probe_running(id).await {
                let child = ChildRef { project_id: id.to_owned(), pid };
                if let Err(e) = self.launcher.terminate(&child, Duration::ZERO).await {
                    stop_err.get_or_insert(e.to_string());
                }
            }
            if self.probe_running(id).await.is_none() {
                self.chain(
                    if why == "idle" { "project.kernel.idle_stop" } else { "project.kernel.stopped" },
                    json!({"project_id": id, "via_fallback": stop_err.is_some()}),
                );
                stop_err = None;
            }
        }
        self.launcher.clean_spawn_file(id);
        // A failed project stays failed until `restart` clears it; a child
        // we could not stop is failed too.
        let cur = {
            let mut st = slot.st();
            if let Some(e) = &stop_err {
                st.failed = Some(format!("could not stop the project kernel: {e}"));
                st.state = ChildState::Failed;
            } else if st.state != ChildState::Failed {
                st.state = ChildState::Stopped;
            }
            st.state
        };
        self.set_state(id, slot, cur);
        match stop_err {
            Some(e) => Err(SupError::Runtime(RuntimeError::Backend(e))),
            None => Ok(running),
        }
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
        // A child that was still booting when the adoption scan ran (its
        // socket not yet bound) is a leftover, not a slot: give it the
        // chance to answer now, so the cascade does not skip it.
        self.reconcile_leftovers().await;
        let ids: Vec<String> = self.slots.lock().unwrap_or_else(|e| e.into_inner()).keys().cloned().collect();
        let mut stopped = Vec::new();
        for id in ids {
            if matches!(self.stop(&id).await, Ok(true)) {
                stopped.push(id);
            }
        }
        stopped
    }

    /// A project's key was replaced (`project.rekey`): stop the old child;
    /// the next start runs the rekeyed project normally. No marker.
    pub async fn rekeyed(self: &Arc<Self>, id: &str) {
        if let Err(e) = self.stop(id).await {
            tracing::warn!(project = id, error = %e, "could not stop a rekeyed project's kernel");
        }
    }

    /// `project.revoke` happened (`on_identity_change` wrote the terminal
    /// `<run>/<id>/revoked` marker first): kill the child's credentials first, then
    /// stop it (signals if the gated stop fails) and mark the project failed.
    /// The project is never respawned: `prepare` refuses while the marker
    /// exists.
    pub async fn revoked(self: &Arc<Self>, id: &str, reason: &str) {
        self.launcher.revoke_tokens(id);
        let slot = self.slot(id);
        let _g = slot.gate.lock().await;
        if let Err(e) = self.stop_locked(id, &slot, "revoke").await {
            tracing::warn!(project = id, error = %e, "could not stop a revoked project's kernel");
        }
        {
            let mut st = slot.st();
            st.failed = Some(format!("revoked ({reason})"));
            st.state = ChildState::Failed;
        }
        self.set_state(id, &slot, ChildState::Failed);
    }

    /// Status of one project.
    pub async fn status(&self, id: &str) -> Status {
        let slot = self.slot(id);
        let (state_, restarts, exit, failed) = {
            let st = slot.st();
            (st.state, st.restarts, st.last_exit, st.failed.clone())
        };
        let file = state::read(&self.run_dir(id)).unwrap_or_default();
        let pid = self.probe_running(id).await;
        let stale_build = pid.is_some() && file.kernel_sha.as_deref().is_some_and(|sha| sha != self.cfg.build_sha);
        Status {
            project_id: id.to_owned(),
            state: state_,
            pid,
            socket: self.socket(id),
            restarts,
            last_exit_code: exit.and_then(|e| e.code),
            failed_reason: failed,
            kernel_sha: file.kernel_sha,
            kernel_version: file.kernel_version,
            stale_build,
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

    /// A live kernel for `id` that the supervisor does not manage (an
    /// adopted-but-refused leftover, or one that failed verification): its
    /// pid and why. `project.stop` names it instead of saying "was not
    /// running". It is never signalled.
    pub fn unmanaged(&self, id: &str) -> Option<(u32, String)> {
        self.leftovers.lock().unwrap_or_else(|e| e.into_inner()).iter().find_map(|f| match f {
            Found::Unverifiable { id: i, pid: Some(pid), reason }
                if i == id && *reason != adopt::Skip::Dead && child::pid_alive(*pid) =>
            {
                Some((*pid, reason.to_string()))
            }
            _ => None,
        })
    }

    /// `via = child-kernel` for the project (the owner's opt-in).
    pub fn is_child_kernel(&self, id: &str) -> bool {
        matches!(
            clawft_types::project::find_by_id(&self.cfg.manifests_dir, id),
            Ok(Some(m)) if m.serve.as_ref().is_some_and(|s| s.via == ServeVia::ChildKernel)
        )
    }
}

pub use boot::post_boot;
pub use types::{CHAIN_SOURCE, Deps, MAX_SOCKET_PATH, Running, Status, SupError, SupervisorConfig};
