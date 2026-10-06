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
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use clawft_kernel::gate::GovernanceSnapshot;
use clawft_kernel::overlay_runtime::write_user_pin;
use clawft_kernel::parent_policy::export_rules_to;
use clawft_kernel::token_authority::{Issuer, TokenAuthority};
use clawft_kernel::workload_runtime::{ChildLauncher, ChildProbe, ChildRef, ChildSpec, RuntimeError};
use clawft_types::project::SpawnFile;
use clawft_types::project::cert::key_id;
use clawft_types::project::token_consts::PROJECT_TOKEN_TTL_SECS;
use clawft_types::runtime_paths::{LOG_FILE_NAME, PARENT_POLICY_FILE, SOCKET_NAME, SPAWN_JSON_FILE};
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
static GROUPS: Mutex<std::collections::BTreeSet<u32>> = Mutex::new(std::collections::BTreeSet::new());

pub(crate) fn note_group(pgid: u32) {
    GROUPS.lock().unwrap_or_else(|e| e.into_inner()).insert(pgid);
}

/// See [`Launcher::supervised_pids`].
pub(crate) fn supervised_groups() -> Vec<u32> {
    let mut g = GROUPS.lock().unwrap_or_else(|e| e.into_inner());
    g.retain(|pgid| !group_gone(*pgid));
    g.iter().copied().collect()
}

fn group_gone(pgid: u32) -> bool {
    matches!(killpg(Pid::from_raw(pgid as i32), None), Err(nix::errno::Errno::ESRCH))
}

/// Drop `pgid` from the set now if its group is empty. Called when a waiter
/// sees the leader exit: pruning only at the next accept would leave a window
/// in which an unrelated same-uid group that recycled the number is still
/// classed as a child (fail closed, but wrong). A group that still has
/// members (orphaned grandchildren) stays until it is gone.
pub(crate) fn prune_if_gone(pgid: u32) {
    if group_gone(pgid) {
        GROUPS.lock().unwrap_or_else(|e| e.into_inner()).remove(&pgid);
    }
}

#[cfg(test)]
pub(crate) fn group_noted(pgid: u32) -> bool {
    GROUPS.lock().unwrap_or_else(|e| e.into_inner()).contains(&pgid)
}

static EXE_SHA: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Locale variables passed through when set.
const LOCALE_VARS: &[&str] = &[
    "LANG", "LC_ALL", "LC_CTYPE", "LC_COLLATE", "LC_MESSAGES", "LC_NUMERIC", "LC_TIME", "LC_MONETARY",
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
    project_id: &str,
    parent: impl Fn(&str) -> Option<String>,
) -> Vec<(String, String)> {
    let mut env = vec![
        ("HOME".to_owned(), home.display().to_string()),
        (
            "PATH".to_owned(),
            parent("PATH").unwrap_or_else(|| "/usr/bin:/bin:/usr/sbin:/sbin".to_owned()),
        ),
        ("WEFTOS_RUNTIME_DIR".to_owned(), run_dir.display().to_string()),
        ("WEFTOS_PROJECT_ID".to_owned(), project_id.to_owned()),
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

    /// Forget a project's process entry.
    pub fn forget(&self, id: &str) {
        self.procs().remove(id);
    }

    /// True when a stop was requested for the current process of `id`.
    pub fn stop_requested(&self, id: &str) -> bool {
        self.procs().get(id).is_some_and(|e| e.stop.load(Ordering::SeqCst))
    }

    /// The pid of the live child of `id`.
    pub fn pid_of(&self, id: &str) -> Option<u32> {
        match self.procs().get(id).map(|e| &e.proc) {
            Some(Proc::Owned { pid, exit }) if exit.borrow().is_none() => Some(*pid),
            Some(Proc::Adopted { pid }) if adopted_alive(*pid) => Some(*pid),
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
            None,
        }
        let w = match self.procs().get(id).map(|e| &e.proc) {
            Some(Proc::Owned { exit, .. }) => W::Owned(exit.clone()),
            Some(Proc::Adopted { pid }) => W::Adopted(*pid),
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
        std::fs::create_dir_all(&run_dir).map_err(|e| backend(format!("{}: {e}", run_dir.display())))?;
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&run_dir, std::fs::Permissions::from_mode(0o700));
        }
        let pubkey = self.parts.user_key.verifying_key().to_bytes();
        write_user_pin(&run_dir, &pubkey).map_err(|e| backend(format!("user.pub: {e}")))?;
        let snap = (self.parts.snapshot)().ok_or_else(|| {
            RuntimeError::AdmissionRefused("this daemon has no governance engine to export a parent policy from".into())
        })?;
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
            pid: 0,
            exe_sha: if spec.adapter == super::wasmtime::ADAPTER {
                super::wasmtime::OperatorConfig::load(&self.cfg, &spec.root)
                    .map_err(backend)?
                    .runner_sha256
            } else {
                self.exe_sha()
            },
            root: spec.root.clone(),
            expires_unix: spawn.expires_unix,
        })
        .map_err(|e| RuntimeError::Backend(format!("spawn ledger: {e}")))?;
        spawn
            .write(&run_dir.join(SPAWN_JSON_FILE))
            .map_err(|e| {
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
    pub fn refresh_token(&self, id: &str, presented: &str) -> Result<(String, chrono::DateTime<Utc>), String> {
        use clawft_kernel::token_authority::TokenScope;
        let auth = self.parts.tokens.as_ref().ok_or("no token authority")?;
        let info = auth.validate(presented).ok_or("token is unknown, expired or revoked")?;
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
        if self.pid_of(id).is_none() {
            return Err("no live child for this project".into());
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
        let Some(auth) = &self.parts.tokens else { return };
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
        if self.pid_of(id).is_some() {
            return Err(RuntimeError::InvalidState(format!("{id} already has a live kernel")));
        }
        let manifest = clawft_types::project::find_by_id(&self.cfg.manifests_dir, id)
            .map_err(backend)?
            .ok_or_else(|| backend("missing project manifest"))?;
        let adapter = super::wasmtime::selected(&manifest, &self.run_dir(id)).map_err(backend)?;
        if adapter != spec.adapter {
            return Err(backend("loaded adapter differs from manifest"));
        }
        if adapter == super::wasmtime::ADAPTER {
            super::wasmtime::preflight(&self.cfg, &spec.root, &self.run_dir(id)).map_err(backend)?;
        }
        let token = self.issue_token(id)?;
        let run_dir = self.run_dir(id);
        let started = match self.write_run_files(spec, &token) {
            Ok(()) => self.launch(spec, &run_dir).await,
            Err(e) => Err(e),
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

    async fn terminate(&self, child: &ChildRef, grace: Duration) -> Result<Option<i32>, RuntimeError> {
        self.terminate_inner(child, grace).await
    }

    async fn probe(&self, project_id: &str) -> ChildProbe {
        self.probe_inner(project_id).await
    }
}

impl Launcher {
    async fn launch(&self, spec: &ChildSpec, run_dir: &Path) -> Result<ChildRef, RuntimeError> {
        let id = spec.project_id.as_str();
        if spec.adapter == super::wasmtime::ADAPTER {
            super::wasmtime::validate_log(run_dir).map_err(backend)?;
        }
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode_0600()
            .open(run_dir.join(LOG_FILE_NAME))
            .map_err(|e| backend(format!("kernel.log: {e}")))?;
        let log2 = log.try_clone().map_err(backend)?;
        let (exe, mut cmd) = if spec.adapter == super::wasmtime::ADAPTER {
            let (exe, config) = super::wasmtime::launch(
                &self.cfg,
                spec,
                run_dir,
                &clawft_types::project::canon::hex_encode(&self.parts.user_key.verifying_key().to_bytes()),
            )
            .map_err(backend)?;
            let mut cmd = std::process::Command::new(&exe);
            cmd.arg(config).env_clear().current_dir(run_dir);
            (exe, cmd)
        } else if spec.adapter == "logical" {
            let mut cmd = std::process::Command::new(&self.cfg.exe);
            cmd.args(child_args(id))
                .env_clear()
                .envs(child_env(&self.cfg.home, run_dir, id, |k| std::env::var(k).ok()))
                .current_dir(&spec.root);
            (self.cfg.exe.clone(), cmd)
        } else {
            return Err(backend("unsupported project adapter"));
        };
        cmd.stdin(std::process::Stdio::null())
            .stdout(log)
            .stderr(log2)
            .process_group(0);
        let mut child = cmd
            .spawn()
            .map_err(|e| backend(format!("cannot start {}: {e}", exe.display())))?;
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
            st.exe = Some(exe.display().to_string());
            st.started_unix = Some(state::now_unix());
            // The new process has not said which build it is yet.
            st.kernel_sha = None;
            st.kernel_version = None;
        });
        Ok(ChildRef {
            project_id: id.to_owned(),
            pid,
        })
    }

    async fn terminate_inner(&self, child: &ChildRef, grace: Duration) -> Result<Option<i32>, RuntimeError> {
        let id = child.project_id.as_str();
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
            return Err(RuntimeError::Backend(format!("{id}: the kernel is still running after SIGKILL")));
        }
        let info = self.wait_exit(id).await;
        Ok(info.code)
    }

    async fn probe_inner(&self, project_id: &str) -> ChildProbe {
        enum P {
            Owned(Option<ExitInfo>, u32),
            Adopted(u32),
            None,
        }
        let p = match self.procs().get(project_id).map(|e| &e.proc) {
            Some(Proc::Owned { pid, exit }) => P::Owned(*exit.borrow(), *pid),
            Some(Proc::Adopted { pid }) => P::Adopted(*pid),
            None => P::None,
        };
        match p {
            P::None => ChildProbe::NotStarted,
            P::Owned(None, pid) => ChildProbe::Running { pid },
            P::Owned(Some(i), _) => ChildProbe::Exited {
                code: i.code,
                signal: i.signal,
            },
            P::Adopted(pid) if adopted_alive(pid) => ChildProbe::Running { pid },
            P::Adopted(_) => ChildProbe::Exited {
                code: None,
                signal: None,
            },
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
        let Some(pid) = self.pid_of(id) else { return false };
        if super::wasmtime::is_wasm_run(&self.run_dir(id)) {
            return super::wasmtime::identity(&self.run_dir(id), pid);
        }
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
        if !self.adopted_still_ours(id) {
            return;
        }
        let leads_group = nix::unistd::getpgid(Some(target)).is_ok_and(|g| g == target);
        let _ = if leads_group { killpg(target, sig) } else { kill(target, sig) };
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
