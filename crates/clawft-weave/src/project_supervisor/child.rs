//! Launching, probing and terminating one project kernel (ADR-103 A6,
//! Phase 2 package G).
//!
//! [`Launcher`] implements the kernel's [`ChildLauncher`], so the `logical`
//! adapter reaches the OS only through it. It does not use
//! `workload_runtime::supervise::Supervised`: that captures stdout and
//! stderr through pipes, and a child whose supervisor (the user daemon) is
//! restarted would die of `SIGPIPE` on its next log line. Children log to
//! `<run>/<id>/kernel.log` instead, in their own process group, and survive
//! a daemon restart to be adopted.
//!
//! Spawn contract (hard requirements from the Phase 2 reviews):
//!
//! * the child gets an **allow-list** environment after `env_clear()`;
//!   provider keys and everything else in the daemon's environment never
//!   reach it ([`child_env`]);
//! * the project token in `spawn.json` is Write-only and project-scoped;
//! * `--profile project` is always passed;
//! * before the process starts the run dir holds the user-key pin
//!   (`user.pub`), the signed `parent-policy.json` and `spawn.json`.

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use clawft_kernel::gate::GovernanceSnapshot;
use clawft_kernel::governance_overlay::{load_overlay, merge as merge_overlay};
use clawft_kernel::overlay_runtime::write_user_pin;
use clawft_kernel::parent_policy::{export_rules, export_rules_to};
use clawft_kernel::token_authority::{Issuer, TokenAuthority};
use clawft_kernel::workload_runtime::{
    ChildIdentity, ChildLauncher, ChildProbe, ChildRef, ChildSpec, RuntimeError,
};
use clawft_types::project::cert::key_id;
use clawft_types::project::token_consts::PROJECT_TOKEN_TTL_SECS;
use clawft_types::project::{ContainerTransport, ProjectSandbox, SpawnFile};
use clawft_types::runtime_paths::{
    LOG_FILE_NAME, PARENT_POLICY_FILE, SOCKET_NAME, SPAWN_JSON_FILE,
};
use ed25519_dalek::SigningKey;
use nix::sys::signal::{Signal, kill, killpg};
use nix::unistd::Pid;
use rand::RngCore;
use tokio::sync::watch;

use super::SupervisorConfig;
use super::io::ChildIo;
use super::state;
use crate::mesh_local_registry::{SpawnExpectation, cancel_spawn, expect_spawn, registry};

/// Process groups of every child this daemon started or adopted, until the
/// group is gone (see [`Launcher::supervised_pids`]).
static GROUPS: Mutex<std::collections::BTreeSet<u32>> =
    Mutex::new(std::collections::BTreeSet::new());

pub(crate) fn note_group(pgid: u32) {
    GROUPS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(pgid);
}

/// See [`Launcher::supervised_pids`].
pub(crate) fn supervised_groups() -> Vec<u32> {
    let mut g = GROUPS.lock().unwrap_or_else(|e| e.into_inner());
    g.retain(|pgid| !group_gone(*pgid));
    g.iter().copied().collect()
}

fn group_gone(pgid: u32) -> bool {
    matches!(
        killpg(Pid::from_raw(pgid as i32), None),
        Err(nix::errno::Errno::ESRCH)
    )
}

/// Drop `pgid` from the set now if its group is empty. Called when a waiter
/// sees the leader exit: pruning only at the next accept would leave a window
/// in which an unrelated same-uid group that recycled the number is still
/// classed as a child (fail closed, but wrong). A group that still has
/// members (orphaned grandchildren) stays until it is gone.
pub(crate) fn prune_if_gone(pgid: u32) {
    if group_gone(pgid) {
        GROUPS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&pgid);
    }
}

#[cfg(test)]
pub(crate) fn group_noted(pgid: u32) -> bool {
    GROUPS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(&pgid)
}

static EXE_SHA: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Locale variables passed through when set.
const LOCALE_VARS: &[&str] = &[
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "LC_COLLATE",
    "LC_MESSAGES",
    "LC_NUMERIC",
    "LC_TIME",
    "LC_MONETARY",
];

/// How a child ended.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExitInfo {
    /// Exit code, when it exited normally.
    pub code: Option<i32>,
    /// Terminating signal.
    pub signal: Option<i32>,
    /// An adopted child whose `kernel.pid` was removed: it shut down cleanly.
    pub clean_hint: bool,
}

impl ExitInfo {
    /// A clean exit is never restarted (OTP `Transient`).
    pub fn clean(&self) -> bool {
        self.code == Some(0) || self.clean_hint
    }
}

fn exit_info(st: std::process::ExitStatus) -> ExitInfo {
    use std::os::unix::process::ExitStatusExt as _;
    ExitInfo {
        code: st.code(),
        signal: st.signal(),
        clean_hint: false,
    }
}

enum Proc {
    Owned {
        pid: u32,
        exit: watch::Receiver<Option<ExitInfo>>,
    },
    Adopted {
        pid: u32,
    },
    Container {
        id: String,
        host_pid: u32,
        cfg: super::container::OperatorConfig,
        mounts: super::container::Mounts,
    },
}

struct Entry {
    proc: Proc,
    /// Set once a stop was requested, so the monitor does not restart.
    stop: Arc<AtomicBool>,
}

/// Token bookkeeping per project: the supervisor honours a refresh only
/// for the token it issued last (or the one before, for in-flight calls).
#[derive(Default)]
struct TokenSlot {
    current: Option<String>,
    previous: Option<String>,
}

/// Everything the launcher needs besides the config.
pub struct LauncherParts {
    /// The user key: signs the parent policy; its public half is pinned.
    pub user_key: SigningKey,
    /// Snapshot of the daemon's governance engine for `parent-policy.json`.
    pub snapshot: Arc<dyn Fn() -> Option<GovernanceSnapshot> + Send + Sync>,
    /// Token authority issuing project tokens (`None`: empty token, the
    /// child's parent link fails closed).
    pub tokens: Option<Arc<TokenAuthority>>,
    /// Handshake and shutdown calls to a child.
    pub io: Arc<dyn ChildIo>,
}

/// The OS side of the `logical` adapter.
pub struct Launcher {
    cfg: Arc<SupervisorConfig>,
    parts: LauncherParts,
    procs: Mutex<HashMap<String, Entry>>,
    token_slots: Mutex<HashMap<String, TokenSlot>>,
    spawns: AtomicU64,
}

/// The environment a child is started with: an allow-list and nothing else.
/// `parent` supplies PATH and the locale variables.
pub fn child_env(
    home: &Path,
    run_dir: &Path,
    project_root: &Path,
    project_id: &str,
    parent: impl Fn(&str) -> Option<String>,
) -> Vec<(String, String)> {
    let mut env = vec![
        ("HOME".to_owned(), home.display().to_string()),
        (
            "PATH".to_owned(),
            parent("PATH").unwrap_or_else(|| "/usr/bin:/bin:/usr/sbin:/sbin".to_owned()),
        ),
        (
            "WEFTOS_RUNTIME_DIR".to_owned(),
            run_dir.display().to_string(),
        ),
        ("WEFTOS_PROJECT_ID".to_owned(), project_id.to_owned()),
        (
            "TMPDIR".to_owned(),
            project_root.join(".weftos/tmp").display().to_string(),
        ),
    ];
    for name in LOCALE_VARS {
        if let Some(v) = parent(name) {
            env.push(((*name).to_owned(), v));
        }
    }
    env
}

/// The arguments a child kernel is started with.
pub fn child_args(project_id: &str) -> Vec<String> {
    [
        "kernel",
        "start",
        "--foreground",
        "--profile",
        "project",
        "--project",
        project_id,
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect()
}

/// True when an ADOPTED `pid` is alive. After a SIGHUP re-exec of the user
/// daemon (`weaver update`) the new image is still the parent of the
/// children but has no waiter for them, so a child that exits stays a zombie
/// and `kill(pid, 0)` would call it alive forever; reaping it here (only for
/// adopted pids: an owned child has a waiter thread that must not lose its
/// status) tells the truth. `ECHILD` means it is not our child: fall back to
/// the signal-0 probe.
pub fn adopted_alive(pid: u32) -> bool {
    use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
    match waitpid(Pid::from_raw(pid as i32), Some(WaitPidFlag::WNOHANG)) {
        Ok(WaitStatus::StillAlive) => true,
        Ok(_) => false,
        Err(_) => pid_alive(pid),
    }
}

/// True when `pid` names a live process (EPERM counts: it exists).
pub fn pid_alive(pid: u32) -> bool {
    match kill(Pid::from_raw(pid as i32), None) {
        Ok(()) => true,
        Err(nix::errno::Errno::EPERM) => true,
        Err(_) => false,
    }
}

fn backend(msg: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::Backend(msg.to_string())
}

fn validate_child_endpoint(socket: &Path) -> Result<(), String> {
    if socket.file_name().and_then(|s| s.to_str()) != Some("child.sock")
        || socket
            .parent()
            .and_then(Path::file_name)
            .and_then(|s| s.to_str())
            != Some("child-ipc")
    {
        return Err("container parent must be the dedicated child.sock endpoint".into());
    }
    Ok(())
}

fn inspected_container_probe(
    result: Result<super::container::Inspected, String>,
    engine: String,
    cid: String,
) -> ChildProbe {
    match result {
        Ok(i) if i.running => match i.host_pid {
            Some(host_pid) => ChildProbe::Running {
                identity: ChildIdentity::Container {
                    engine,
                    immutable_container_id: cid,
                    host_pid,
                },
            },
            None => ChildProbe::Unverifiable {
                reason: "running container has no inspected host PID".into(),
            },
        },
        Ok(i) => ChildProbe::Exited {
            code: i.exit_code,
            signal: None,
        },
        Err(reason) => ChildProbe::Unverifiable {
            reason: format!("container {cid}: {reason}"),
        },
    }
}

impl Launcher {
    /// A launcher over `cfg`.
    pub fn new(cfg: Arc<SupervisorConfig>, parts: LauncherParts) -> Self {
        Self {
            cfg,
            parts,
            procs: Mutex::new(HashMap::new()),
            token_slots: Mutex::new(HashMap::new()),
            spawns: AtomicU64::new(0),
        }
    }

    /// How many children this launcher has started (tests).
    pub fn spawn_count(&self) -> u64 {
        self.spawns.load(Ordering::SeqCst)
    }

    /// `<run_root>/<id>` (`runtime_paths::child_run_dir`; ids are validated
    /// before they get here). This is the child's `$WEFTOS_RUNTIME_DIR`.
    pub fn run_dir(&self, id: &str) -> PathBuf {
        clawft_types::runtime_paths::child_run_dir(&self.cfg.run_root, id)
            .unwrap_or_else(|| self.cfg.run_root.join(id))
    }

    fn procs(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.procs.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn token_slots(&self) -> std::sync::MutexGuard<'_, HashMap<String, TokenSlot>> {
        self.token_slots.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Register a verified, already-running child (adoption).
    pub fn adopt(&self, id: &str, pid: u32) {
        note_group(pid);
        self.procs().insert(
            id.to_owned(),
            Entry {
                proc: Proc::Adopted { pid },
                stop: Arc::new(AtomicBool::new(false)),
            },
        );
    }

    pub async fn adopt_container(&self, id: &str, host_pid: u32) -> Result<(), String> {
        self.require_child_endpoint()?;
        let st = state::read(&self.run_dir(id)).ok_or("missing state")?;
        let saved = st.container.ok_or("missing container state")?;
        let manifest = clawft_types::project::find_by_id(&self.cfg.manifests_dir, id)
            .map_err(|e| e.to_string())?
            .ok_or("missing manifest")?;
        let cfg = super::container::OperatorConfig::load(&self.cfg.home)?;
        if cfg.engine != saved.engine {
            return Err("operator engine changed".into());
        }
        let mounts = self.container_mounts(id, &manifest.root);
        if saved.host_socket != mounts.runtime.join(SOCKET_NAME) {
            return Err("persisted host socket differs from the supervised runtime".into());
        }
        let inspected = self
            .container_client(cfg.clone())
            .inspect(&saved.id, id, &mounts)
            .await?;
        if !inspected.running || inspected.host_pid != Some(host_pid) {
            return Err("container changed during adoption".into());
        }
        self.procs().insert(
            id.into(),
            Entry {
                proc: Proc::Container {
                    id: saved.id,
                    host_pid,
                    cfg,
                    mounts,
                },
                stop: Arc::new(AtomicBool::new(false)),
            },
        );
        Ok(())
    }

    /// Forget a project's process entry.
    pub fn forget(&self, id: &str) {
        self.procs().remove(id);
    }

    /// True when a stop was requested for the current process of `id`.
    pub fn stop_requested(&self, id: &str) -> bool {
        self.procs()
            .get(id)
            .is_some_and(|e| e.stop.load(Ordering::SeqCst))
    }

    /// The pid of the live child of `id`.
    pub fn pid_of(&self, id: &str) -> Option<u32> {
        match self.procs().get(id).map(|e| &e.proc) {
            Some(Proc::Owned { pid, exit }) if exit.borrow().is_none() => Some(*pid),
            Some(Proc::Adopted { pid }) if adopted_alive(*pid) => Some(*pid),
            Some(Proc::Container { host_pid, .. }) => Some(*host_pid),
            _ => None,
        }
    }

    /// Process-group ids of every supervised child whose group still exists.
    /// Each child leads its own group (`process_group(0)`, so the group id is
    /// its pid). A group stays in the set until `killpg(pgid, 0)` reports
    /// `ESRCH`, so grandchildren orphaned by the leader's exit are still
    /// recognised (`child_peer`, review S9). Never reaps: unlike
    /// [`adopted_alive`] it does not `waitpid`, because this runs on the
    /// accept path.
    pub fn supervised_pids(&self) -> Vec<u32> {
        supervised_groups()
    }

    /// Wait until the child of `id` is gone and say how it ended.
    pub async fn wait_exit(&self, id: &str) -> ExitInfo {
        enum W {
            Owned(watch::Receiver<Option<ExitInfo>>),
            Adopted(u32),
            Container(
                String,
                super::container::OperatorConfig,
                super::container::Mounts,
            ),
            None,
        }
        let w = match self.procs().get(id).map(|e| &e.proc) {
            Some(Proc::Owned { exit, .. }) => W::Owned(exit.clone()),
            Some(Proc::Adopted { pid }) => W::Adopted(*pid),
            Some(Proc::Container {
                id, cfg, mounts, ..
            }) => W::Container(id.clone(), cfg.clone(), mounts.clone()),
            None => W::None,
        };
        match w {
            W::None => ExitInfo::default(),
            W::Owned(mut rx) => loop {
                if let Some(info) = *rx.borrow() {
                    return info;
                }
                if rx.changed().await.is_err() {
                    return rx.borrow().unwrap_or_default();
                }
            },
            W::Adopted(pid) => {
                while adopted_alive(pid) {
                    tokio::time::sleep(self.cfg.exit_poll).await;
                }
                prune_if_gone(pid);
                // A clean shutdown removes kernel.pid; a crash leaves it.
                let clean = !self.run_dir(id).join("kernel.pid").exists();
                ExitInfo {
                    code: None,
                    signal: None,
                    clean_hint: clean,
                }
            }
            W::Container(cid, cfg, mounts) => {
                let client = self.container_client(cfg);
                loop {
                    match client.inspect(&cid, id, &mounts).await {
                        Ok(i) if !i.running => {
                            return ExitInfo {
                                code: i.exit_code,
                                signal: None,
                                clean_hint: i.exit_code == Some(0),
                            };
                        }
                        // Inspection uncertainty cannot be treated as a crash:
                        // that would permit an ungoverned duplicate launch.
                        _ => tokio::time::sleep(self.cfg.exit_poll).await,
                    }
                }
            }
        }
    }

    /// SHA-256 of the kernel executable (hex), computed once.
    fn exe_sha(&self) -> String {
        EXE_SHA
            .get_or_init(|| {
                use sha2::{Digest, Sha256};
                std::fs::read(&self.cfg.exe)
                    .map(|b| hex::encode(Sha256::digest(b)))
                    .unwrap_or_default()
            })
            .clone()
    }

    /// Write the run dir files and file the spawn expectation in the
    /// registry. Returns the spawn nonce.
    fn write_run_files(&self, spec: &ChildSpec, token: &str) -> Result<(), RuntimeError> {
        let run_dir = self.run_dir(&spec.project_id);
        std::fs::create_dir_all(&run_dir)
            .map_err(|e| backend(format!("{}: {e}", run_dir.display())))?;
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&run_dir, std::fs::Permissions::from_mode(0o700));
        }
        let pubkey = self.parts.user_key.verifying_key().to_bytes();
        write_user_pin(&run_dir, &pubkey).map_err(|e| backend(format!("user.pub: {e}")))?;
        let mut snap = (self.parts.snapshot)().ok_or_else(|| {
            RuntimeError::AdmissionRefused(
                "this daemon has no governance engine to export a parent policy from".into(),
            )
        })?;
        if let Some(parent_id) = clawft_types::project::read_project_toml(&spec.root)
            .map_err(|e| RuntimeError::AdmissionRefused(format!("nested project.toml: {e}")))?
            .and_then(|pt| pt.parent)
        {
            let master = clawft_types::project::find_by_id(&self.cfg.manifests_dir, &parent_id)
                .map_err(|e| RuntimeError::AdmissionRefused(format!("master manifest: {e}")))?
                .ok_or_else(|| RuntimeError::AdmissionRefused("master is not registered".into()))?;
            if master.state != clawft_types::project::ProjectState::Active {
                return Err(RuntimeError::AdmissionRefused(
                    "master is not active".into(),
                ));
            }
            let master_root = master
                .root
                .canonicalize()
                .map_err(|e| RuntimeError::AdmissionRefused(format!("master root: {e}")))?;
            let child_root = spec
                .root
                .canonicalize()
                .map_err(|e| RuntimeError::AdmissionRefused(format!("nested child root: {e}")))?;
            if child_root == master_root || !child_root.starts_with(&master_root) {
                return Err(RuntimeError::AdmissionRefused(
                    "child escaped its master root".into(),
                ));
            }
            let master_pt = clawft_types::project::read_project_toml(&master.root)
                .map_err(|e| RuntimeError::AdmissionRefused(format!("master project.toml: {e}")))?
                .ok_or_else(|| {
                    RuntimeError::AdmissionRefused("master has no project.toml".into())
                })?;
            if master_pt.id != parent_id || !master_pt.is_weave_master() {
                return Err(RuntimeError::AdmissionRefused(
                    "master identity or weave.master changed".into(),
                ));
            }
            let signed_parent = export_rules(
                snap.rules.clone(),
                snap.risk_threshold,
                snap.human_approval_required,
                &snap.limits,
                &self.parts.user_key,
                1,
                Utc::now(),
            )
            .map_err(|e| RuntimeError::AdmissionRefused(format!("master policy base: {e}")))?;
            let overlay = load_overlay(&master.root.join(".weftos/overlay.toml"))
                .map_err(|e| RuntimeError::AdmissionRefused(format!("master overlay: {e}")))?;
            let effective = merge_overlay(&signed_parent, &overlay)
                .map_err(|e| RuntimeError::AdmissionRefused(format!("master overlay: {e}")))?;
            snap.risk_threshold = effective.risk_threshold(snap.risk_threshold);
            snap.human_approval_required = effective.human_approval(snap.human_approval_required);
            snap.limits = effective.limits;
            snap.rules = effective.rules;
        }
        export_rules_to(
            &run_dir.join(PARENT_POLICY_FILE),
            snap.rules,
            snap.risk_threshold,
            snap.human_approval_required,
            &snap.limits,
            &self.parts.user_key,
        )
        .map_err(|e| backend(format!("parent-policy.json: {e}")))?;
        let mut nonce = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let now = state::now_unix();
        let nonce = hex::encode(nonce);
        let spawn = SpawnFile::new(
            nonce.clone(),
            self.cfg.parent_socket.clone(),
            hex::encode(pubkey),
            key_id(&pubkey),
            spec.project_id.clone(),
            spec.root.clone(),
            (!token.is_empty()).then(|| token.to_owned()),
            now,
        );
        // A live registry session blocks any new registration (even with a
        // fresh nonce), so a respawn evicts the dead child's first.
        registry().evict(&spec.project_id);
        // The expectation first: if it cannot be filed (ledger full) nothing
        // is written and nothing starts.
        expect_spawn(SpawnExpectation {
            project_id: spec.project_id.clone(),
            nonce,
            pid: None,
            container: None,
            exe_sha: self.exe_sha(),
            root: spec.root.clone(),
            expires_unix: spawn.expires_unix,
        })
        .map_err(|e| RuntimeError::Backend(format!("spawn ledger: {e}")))?;
        spawn.write(&run_dir.join(SPAWN_JSON_FILE)).map_err(|e| {
            cancel_spawn(&spec.project_id);
            backend(format!("spawn.json: {e}"))
        })
    }

    /// Issue the child's project token (Write only, project-scoped).
    fn issue_token(&self, id: &str) -> Result<String, RuntimeError> {
        let Some(auth) = &self.parts.tokens else {
            return Ok(String::new());
        };
        let ttl = chrono::Duration::seconds(PROJECT_TOKEN_TTL_SECS as i64);
        let (secret, info) = auth
            .issue_project(
                id,
                ttl,
                &Issuer {
                    uid: Some(nix::unistd::getuid().as_raw()),
                },
            )
            .map_err(|e| backend(format!("project token: {e}")))?;
        let mut slots = self.token_slots();
        let slot = slots.entry(id.to_owned()).or_default();
        if let Some(old) = slot.previous.take() {
            let _ = auth.revoke(&old);
        }
        slot.previous = slot.current.take();
        slot.current = Some(info.id);
        Ok(secret)
    }

    /// Renew a child's token (`project.token.refresh`). The presented token
    /// must be a live project-scoped token for `id` that this supervisor
    /// issued last (or the one before); the older one is revoked.
    pub async fn refresh_token(
        &self,
        id: &str,
        presented: &str,
    ) -> Result<(String, chrono::DateTime<Utc>), String> {
        use clawft_kernel::token_authority::TokenScope;
        let auth = self.parts.tokens.as_ref().ok_or("no token authority")?;
        let info = auth
            .validate(presented)
            .ok_or("token is unknown, expired or revoked")?;
        if info.scope != TokenScope::Project || info.project.as_deref() != Some(id) {
            return Err("token is not a project token for this project".into());
        }
        {
            let slots = self.token_slots();
            if let Some(slot) = slots.get(id)
                && slot.current.as_deref() != Some(&info.id)
                && slot.previous.as_deref() != Some(&info.id)
            {
                return Err("token is not the one this supervisor issued last".into());
            }
        }
        if !matches!(self.probe_inner(id).await, ChildProbe::Running { .. }) {
            return Err("no verified live child for this project".into());
        }
        let ttl = chrono::Duration::seconds(PROJECT_TOKEN_TTL_SECS as i64);
        let (secret, new) = auth
            .issue_project(
                id,
                ttl,
                &Issuer {
                    uid: Some(nix::unistd::getuid().as_raw()),
                },
            )
            .map_err(|e| e.to_string())?;
        let mut slots = self.token_slots();
        let slot = slots.entry(id.to_owned()).or_default();
        if let Some(old) = slot.previous.take() {
            let _ = auth.revoke(&old);
        }
        slot.previous = slot.current.replace(new.id.clone());
        if slot.previous.is_none() {
            slot.previous = Some(info.id);
        }
        Ok((secret, new.expires_at))
    }

    /// Revoke every token issued for `id`.
    pub fn revoke_tokens(&self, id: &str) {
        let Some(auth) = &self.parts.tokens else {
            return;
        };
        if let Some(slot) = self.token_slots().remove(id) {
            for t in [slot.current, slot.previous].into_iter().flatten() {
                let _ = auth.revoke(&t);
            }
        }
    }
}

#[async_trait]
impl ChildLauncher for Launcher {
    async fn spawn(&self, spec: &ChildSpec) -> Result<ChildRef, RuntimeError> {
        let id = spec.project_id.as_str();
        if matches!(self.probe_inner(id).await, ChildProbe::Running { .. }) {
            return Err(RuntimeError::InvalidState(format!(
                "{id} already has a live kernel"
            )));
        }
        if let Some(saved) = state::read(&self.run_dir(id)).and_then(|s| s.container) {
            let cfg = super::container::OperatorConfig::load(&self.cfg.home).map_err(backend)?;
            if cfg.engine != saved.engine {
                return Err(backend(
                    "operator engine differs from the persisted container",
                ));
            }
            let inspected = self
                .container_client(cfg)
                .inspect(&saved.id, id, &self.container_mounts(id, &spec.root))
                .await
                .map_err(backend)?;
            if inspected.running {
                return Err(RuntimeError::InvalidState(format!(
                    "{id} already has a running container"
                )));
            }
        }
        self.forget(id);
        let token = self.issue_token(id)?;
        let run_dir = self.run_dir(id);
        let sandbox = clawft_types::project::find_by_id(&self.cfg.manifests_dir, id)
            .map_err(backend)?
            .and_then(|m| m.serve)
            .map_or(ProjectSandbox::Logical, |s| s.sandbox);
        let started = match sandbox {
            ProjectSandbox::Logical | ProjectSandbox::Seatbelt => {
                match self.write_run_files(spec, &token) {
                    Ok(()) => self.launch(spec, &run_dir).await,
                    Err(e) => Err(e),
                }
            }
            ProjectSandbox::LinuxContainer => self.launch_container(spec, &token).await,
        };
        if started.is_err() {
            // Nothing is left behind by a failed spawn: no live token, no
            // spawn file with a nonce in it, no outstanding expectation.
            cancel_spawn(id);
            self.revoke_tokens(id);
            self.clean_spawn_file(id);
        }
        started
    }

    async fn terminate(
        &self,
        child: &ChildRef,
        grace: Duration,
    ) -> Result<Option<i32>, RuntimeError> {
        self.terminate_inner(child, grace).await
    }

    async fn probe(&self, project_id: &str) -> ChildProbe {
        self.probe_inner(project_id).await
    }
}

impl Launcher {
    fn container_client(
        &self,
        cfg: super::container::OperatorConfig,
    ) -> super::container::EngineClient {
        super::container::EngineClient {
            cfg,
            runner: Arc::new(clawft_kernel::workload_runtime::SystemRunner),
        }
    }

    fn container_mounts(&self, id: &str, root: &Path) -> super::container::Mounts {
        let run = self.run_dir(id);
        super::container::Mounts {
            project: root.to_path_buf(),
            runtime: run.join("guest"),
            trust: run.clone(),
            link: self
                .cfg
                .parent_socket
                .parent()
                .unwrap_or(Path::new("/"))
                .to_path_buf(),
            guest_runtime: format!("/weftos/run/{id}"),
            supervisor_id: key_id(&self.parts.user_key.verifying_key().to_bytes()),
        }
    }

    fn require_child_endpoint(&self) -> Result<(), String> {
        validate_child_endpoint(&self.cfg.parent_socket)
    }

    async fn launch_container(
        &self,
        spec: &ChildSpec,
        token: &str,
    ) -> Result<ChildRef, RuntimeError> {
        if !cfg!(target_os = "linux") {
            return Err(RuntimeError::AdmissionRefused(
                "Linux container driver is available only on Linux".into(),
            ));
        }
        let id = spec.project_id.as_str();
        self.require_child_endpoint().map_err(backend)?;
        let cfg = super::container::OperatorConfig::load(&self.cfg.home).map_err(backend)?;
        let mounts = self.container_mounts(id, &spec.root);
        std::fs::create_dir_all(&mounts.runtime).map_err(backend)?;
        let client = self.container_client(cfg.clone());
        if let Some(old) = state::read(&self.run_dir(id)).and_then(|s| s.container) {
            if old.engine != cfg.engine {
                return Err(backend(
                    "operator engine changed while a container is persisted",
                ));
            }
            client
                .remove_exited(&old.id, id, &mounts)
                .await
                .map_err(backend)?;
        }
        self.write_run_files(spec, token)?;
        let uid = nix::unistd::geteuid().as_raw();
        let gid = nix::unistd::getegid().as_raw();
        let cid = match client.create_unverified(id, &mounts, uid, gid).await {
            Ok(cid) => cid,
            Err(first) => {
                // A previous create may have returned its ID just before a
                // daemon crash. Resolve the occupied name to a verified ID;
                // no operation targets the name after discovery.
                let old = client
                    .inspect_named(id, &mounts)
                    .await
                    .map_err(|_| backend(first))?;
                if old.running {
                    return Err(backend(
                        "a verified container already occupies the project name",
                    ));
                }
                client
                    .remove_exited(&old.id, id, &mounts)
                    .await
                    .map_err(backend)?;
                client
                    .create_unverified(id, &mounts, uid, gid)
                    .await
                    .map_err(backend)?
            }
        };
        let host_socket = mounts.runtime.join(SOCKET_NAME);
        state::update(&self.run_dir(id), |st| {
            st.state = clawft_types::project::ChildState::Starting;
            st.container = Some(super::state::ContainerState {
                engine: cfg.engine.clone(),
                id: cid.clone(),
                host_socket: host_socket.clone(),
            });
            st.pid = None;
            st.started_unix = Some(state::now_unix());
        });
        if state::read(&self.run_dir(id))
            .and_then(|s| s.container)
            .is_none_or(|s| s.id != cid)
        {
            let cleanup = client.remove_exited(&cid, id, &mounts).await;
            return Err(backend(format!(
                "container {cid} identity could not be persisted; verified cleanup: {cleanup:?}"
            )));
        }
        client.inspect(&cid, id, &mounts).await.map_err(backend)?;
        // `create` returns the immutable ID. The guest cannot run until the
        // protected spawn file and nonce ledger both bind that exact ID.
        let spawn_path = self.run_dir(id).join(SPAWN_JSON_FILE);
        let mut spawn: SpawnFile =
            serde_json::from_slice(&std::fs::read(&spawn_path).map_err(backend)?)
                .map_err(backend)?;
        spawn.container = Some(ContainerTransport {
            engine: cfg.engine.clone(),
            container_id: cid.clone(),
            guest_parent_socket: format!("{}/child.sock", super::container::GUEST_LINK).into(),
            guest_runtime_root: mounts.guest_runtime.clone().into(),
            guest_trust_root: super::container::GUEST_TRUST.into(),
            guest_project_root: super::container::GUEST_PROJECT.into(),
            host_child_socket: host_socket.clone(),
        });
        spawn.write(&spawn_path).map_err(backend)?;
        expect_spawn(SpawnExpectation {
            project_id: id.into(),
            nonce: spawn.nonce.clone(),
            pid: None,
            container: Some(clawft_rpc::mesh_local::ContainerRegistration {
                engine: cfg.engine.clone(),
                container_id: cid.clone(),
                host_socket: host_socket.to_string_lossy().into_owned(),
            }),
            exe_sha: self.exe_sha(),
            root: spec.root.clone(),
            expires_unix: spawn.expires_unix,
        })
        .map_err(backend)?;
        let inspected = client.start(&cid, id, &mounts).await.map_err(backend)?;
        let host_pid = inspected
            .host_pid
            .ok_or_else(|| backend("engine did not inspect a host PID"))?;
        let engine = cfg.engine.clone();
        self.procs().insert(
            id.into(),
            Entry {
                proc: Proc::Container {
                    id: cid.clone(),
                    host_pid,
                    cfg,
                    mounts,
                },
                stop: Arc::new(AtomicBool::new(false)),
            },
        );
        self.spawns.fetch_add(1, Ordering::SeqCst);
        Ok(ChildRef {
            project_id: id.into(),
            identity: ChildIdentity::Container {
                engine,
                immutable_container_id: cid,
                host_pid,
            },
        })
    }

    /// Registration is admitted only for a currently inspected, running ID
    /// with the persisted mount contract and parent-selected host socket.
    pub async fn verify_container(
        &self,
        id: &str,
        c: &clawft_rpc::mesh_local::ContainerRegistration,
    ) -> Result<u32, String> {
        self.require_child_endpoint()?;
        let st = state::read(&self.run_dir(id)).ok_or("missing supervisor state")?;
        let saved = st.container.ok_or("no supervised container")?;
        if saved.engine != c.engine
            || saved.id != c.container_id
            || saved.host_socket != Path::new(&c.host_socket)
        {
            return Err("container differs from persisted supervisor identity".into());
        }
        let manifest = clawft_types::project::find_by_id(&self.cfg.manifests_dir, id)
            .map_err(|e| e.to_string())?
            .ok_or("project manifest missing")?;
        let cfg = super::container::OperatorConfig::load(&self.cfg.home)?;
        if cfg.engine != c.engine {
            return Err("operator engine changed".into());
        }
        let mounts = self.container_mounts(id, &manifest.root);
        if saved.host_socket != mounts.runtime.join(SOCKET_NAME) {
            return Err("persisted host socket differs from the supervised runtime".into());
        }
        let inspected = self
            .container_client(cfg)
            .inspect(&c.container_id, id, &mounts)
            .await?;
        if !inspected.running {
            return Err("container is not running".into());
        }
        inspected
            .host_pid
            .ok_or_else(|| "engine supplied no host PID".into())
    }

    async fn launch(&self, spec: &ChildSpec, run_dir: &Path) -> Result<ChildRef, RuntimeError> {
        let id = spec.project_id.as_str();
        let sandbox = clawft_types::project::find_by_id(&self.cfg.manifests_dir, id)
            .map_err(|e| backend(format!("sandbox manifest: {e}")))?
            .ok_or_else(|| RuntimeError::AdmissionRefused(format!("project {id} has no manifest")))?
            .serve
            .unwrap_or_default()
            .sandbox;
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode_0600()
            .open(run_dir.join(LOG_FILE_NAME))
            .map_err(|e| backend(format!("kernel.log: {e}")))?;
        let log2 = log.try_clone().map_err(backend)?;
        let tmp = spec.root.join(".weftos/tmp");
        std::fs::create_dir_all(&tmp).map_err(|e| backend(format!("project tmp: {e}")))?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| backend(format!("project tmp permissions: {e}")))?;
        // A bounded child must never resolve an absent runtime/config path
        // through the daemon owner's HOME. Give it an in-tree home instead.
        let nested = clawft_types::project::read_project_toml(&spec.root)
            .map_err(|e| RuntimeError::AdmissionRefused(format!("project.toml: {e}")))?
            .is_some_and(|p| p.parent.is_some());
        let home = if sandbox == clawft_types::project::ProjectSandbox::Logical && !nested {
            self.cfg.home.clone()
        } else {
            let home = spec.root.join(".weftos/sandbox-home");
            std::fs::create_dir_all(&home).map_err(|e| backend(format!("sandbox HOME: {e}")))?;
            std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| backend(format!("sandbox HOME permissions: {e}")))?;
            home
        };
        let env = child_env(&home, run_dir, &spec.root, id, |k| std::env::var(k).ok());
        let bounded = sandbox != clawft_types::project::ProjectSandbox::Logical;
        let launcher = if bounded {
            std::env::current_exe()
                .map_err(|e| backend(format!("sandbox helper executable: {e}")))?
        } else {
            self.cfg.exe.clone()
        };
        let mut cmd = std::process::Command::new(&launcher);
        if bounded {
            cmd.arg(super::sandbox::HELPER_ARG).arg(&self.cfg.exe);
        }
        cmd.args(child_args(id))
            .env_clear()
            .envs(env)
            .current_dir(&spec.root)
            .stdin(std::process::Stdio::null())
            .stdout(log)
            .stderr(log2)
            .process_group(0);
        super::sandbox::configure(
            &mut cmd,
            sandbox,
            &spec.root,
            run_dir,
            &self.cfg.parent_socket,
            &self.cfg.exe,
        )
        .map_err(|e| RuntimeError::AdmissionRefused(format!("project sandbox: {e}")))?;
        let mut child = cmd
            .spawn()
            .map_err(|e| backend(format!("cannot start {}: {e}", self.cfg.exe.display())))?;
        let pid = child.id();
        registry().note_pid(id, pid);
        self.spawns.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = watch::channel(None);
        // Before the waiter exists: if the child dies at once, the waiter's
        // prune must find the group already noted (else it would be noted
        // after its own prune, and stay until the next accept).
        note_group(pid);
        std::thread::spawn(move || {
            let info = child.wait().map(exit_info).unwrap_or_default();
            prune_if_gone(pid);
            let _ = tx.send(Some(info));
        });
        self.procs().insert(
            id.to_owned(),
            Entry {
                proc: Proc::Owned { pid, exit: rx },
                stop: Arc::new(AtomicBool::new(false)),
            },
        );
        state::update(run_dir, |st| {
            st.state = clawft_types::project::ChildState::Starting;
            st.pid = Some(pid);
            st.container = None;
            st.exe = Some(self.cfg.exe.display().to_string());
            st.started_unix = Some(state::now_unix());
            // The new process has not said which build it is yet.
            st.kernel_sha = None;
            st.kernel_version = None;
        });
        Ok(ChildRef {
            project_id: id.to_owned(),
            identity: ChildIdentity::Native { host_pid: pid },
        })
    }

    async fn terminate_inner(
        &self,
        child: &ChildRef,
        grace: Duration,
    ) -> Result<Option<i32>, RuntimeError> {
        let id = child.project_id.as_str();
        let container = match self.procs().get(id) {
            Some(e) => match &e.proc {
                Proc::Container {
                    id: cid,
                    cfg,
                    mounts,
                    ..
                } => {
                    if !matches!(&child.identity, ChildIdentity::Container { immutable_container_id, .. } if immutable_container_id == cid)
                    {
                        return Err(backend("stale container reference"));
                    }
                    e.stop.store(true, Ordering::SeqCst);
                    Some((cid.clone(), cfg.clone(), mounts.clone()))
                }
                _ => None,
            },
            None => None,
        };
        if let Some((cid, cfg, mounts)) = container {
            let client = self.container_client(cfg);
            // Signed graceful shutdown through the certified child socket.
            self.parts
                .io
                .shutdown(&mounts.runtime.join(SOCKET_NAME), id)
                .await;
            let deadline = tokio::time::Instant::now() + grace;
            loop {
                let i = client.inspect(&cid, id, &mounts).await.map_err(backend)?;
                if !i.running {
                    return Ok(i.exit_code);
                }
                if tokio::time::Instant::now() >= deadline {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            client
                .stop(&cid, id, &mounts, self.cfg.kill_grace)
                .await
                .map_err(backend)?;
            return Ok(None);
        }
        let (owned, stop) = match self.procs().get(id) {
            Some(e) => (matches!(e.proc, Proc::Owned { .. }), Arc::clone(&e.stop)),
            None => return Ok(None),
        };
        stop.store(true, Ordering::SeqCst);
        let socket = self.run_dir(id).join(SOCKET_NAME);
        // Graceful first: kernel.shutdown (final anchor), then wait.
        self.parts.io.shutdown(&socket, id).await;
        if !self.wait_gone(id, grace).await {
            if !owned && !self.adopted_still_ours(id) {
                return Ok(None);
            }
            self.signal(id, owned, Signal::SIGTERM);
            if !self.wait_gone(id, self.cfg.kill_grace).await {
                // Identity is re-checked inside `signal` for an adopted pid:
                // between SIGTERM and SIGKILL the pid may have been recycled.
                self.signal(id, owned, Signal::SIGKILL);
                self.wait_gone(id, self.cfg.kill_grace).await;
            }
        }
        if self.pid_of(id).is_some() {
            return Err(RuntimeError::Backend(format!(
                "{id}: the kernel is still running after SIGKILL"
            )));
        }
        let info = self.wait_exit(id).await;
        Ok(info.code)
    }

    async fn probe_inner(&self, project_id: &str) -> ChildProbe {
        enum P {
            Owned(Option<ExitInfo>, u32),
            Adopted(u32),
            Container(
                String,
                super::container::OperatorConfig,
                super::container::Mounts,
            ),
            None,
        }
        let p = match self.procs().get(project_id).map(|e| &e.proc) {
            Some(Proc::Owned { pid, exit }) => P::Owned(*exit.borrow(), *pid),
            Some(Proc::Adopted { pid }) => P::Adopted(*pid),
            Some(Proc::Container {
                id, cfg, mounts, ..
            }) => P::Container(id.clone(), cfg.clone(), mounts.clone()),
            None => P::None,
        };
        match p {
            P::None => ChildProbe::NotStarted,
            P::Owned(None, pid) => ChildProbe::Running {
                identity: ChildIdentity::Native { host_pid: pid },
            },
            P::Owned(Some(i), _) => ChildProbe::Exited {
                code: i.code,
                signal: i.signal,
            },
            P::Adopted(pid) if adopted_alive(pid) => ChildProbe::Running {
                identity: ChildIdentity::Native { host_pid: pid },
            },
            P::Adopted(_) => ChildProbe::Exited {
                code: None,
                signal: None,
            },
            P::Container(cid, cfg, mounts) => inspected_container_probe(
                self.container_client(cfg.clone())
                    .inspect(&cid, project_id, &mounts)
                    .await,
                cfg.engine,
                cid,
            ),
        }
    }
}

impl Launcher {
    async fn wait_gone(&self, id: &str, within: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            if self.pid_of(id).is_none() {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// An adopted pid is signalled only if it still looks like our child
    /// (pid reuse since adoption must never kill a stranger).
    fn adopted_still_ours(&self, id: &str) -> bool {
        let Some(pid) = self.pid_of(id) else {
            return false;
        };
        super::adopt::identity_ok(&self.run_dir(id), pid, &self.cfg.exe)
    }

    fn signal(&self, id: &str, owned: bool, sig: Signal) {
        let Some(pid) = self.pid_of(id) else { return };
        let target = Pid::from_raw(pid as i32);
        if owned {
            // A child we started leads its own process group.
            let _ = killpg(target, sig);
            return;
        }
        // An adopted pid is signalled only while it still verifies as ours,
        // and as a group only when it really leads one (a recycled pid that
        // leads nothing is never group-killed).
        if !super::adopt::identity_ok(&self.run_dir(id), pid, &self.cfg.exe) {
            return;
        }
        let leads_group = nix::unistd::getpgid(Some(target)).is_ok_and(|g| g == target);
        let _ = if leads_group {
            killpg(target, sig)
        } else {
            kill(target, sig)
        };
    }

    /// Delete `<run>/<id>/spawn.json` (the child consumes it at boot; this
    /// covers a stop, a failure or a spawn that never booted).
    pub fn clean_spawn_file(&self, id: &str) {
        let _ = std::fs::remove_file(self.run_dir(id).join(SPAWN_JSON_FILE));
    }
}

trait OpenOptionsExt0600 {
    fn mode_0600(&mut self) -> &mut Self;
}

impl OpenOptionsExt0600 for std::fs::OpenOptions {
    fn mode_0600(&mut self) -> &mut Self {
        use std::os::unix::fs::OpenOptionsExt as _;
        self.mode(0o600)
    }
}

#[cfg(test)]
mod container_endpoint_tests {
    use super::{inspected_container_probe, validate_child_endpoint};
    use clawft_kernel::workload_runtime::ChildProbe;
    use std::path::Path;
    #[test]
    fn container_endpoint_cannot_be_owner_socket() {
        assert!(validate_child_endpoint(Path::new("/run/child-ipc/child.sock")).is_ok());
        assert!(validate_child_endpoint(Path::new("/run/kernel.sock")).is_err());
        assert!(validate_child_endpoint(Path::new("/run/child-ipc/kernel.sock")).is_err());
    }
    #[test]
    fn engine_failure_never_proves_liveness() {
        let probe = inspected_container_probe(
            Err("engine unavailable".into()),
            "docker".into(),
            "a".repeat(64),
        );
        assert!(matches!(probe, ChildProbe::Unverifiable { .. }));
    }
}
