//! One resolver for every file a WeftOS kernel keeps in its runtime dir.
//!
//! Socket, PID, log, lock, node key, chain checkpoint (with its RVF, signing
//! key and tree checkpoint), anchor ledger, workload catalog, cluster peers,
//! installed apps and the revoked-host list all hang off a single root, so
//! they can never disagree about which kernel they belong to (ADR-103 D4).
//!
//! # Root resolution (first match wins)
//!
//! 1. `$WEFTOS_RUNTIME_DIR` when set and non-empty (full-isolation override
//!    for tests, probes and nested instances).
//! 2. `<project>/.weftos/runtime`, where `<project>` is the nearest ancestor
//!    of the working directory that is a project root. A directory is one
//!    when it has `.weftos/project.toml`, or `.weftos/weave.toml`, or
//!    `weave.toml` next to a `.weftos/` directory (what `weaver init`
//!    writes), or an existing `.weftos/runtime/` directory (a kernel has run
//!    there), or a `.weftos/` directory in a git top-level (a `.git` file or
//!    directory; covers git worktrees). The walk never returns `$HOME` and
//!    stops when it reaches it, so the `~/.weftos/` that holds apps and
//!    models is not mistaken for a project.
//! 3. `~/.clawft` (legacy).
//!
//! Phase 1 of ADR-103 changes the root; keep that change inside
//! [`resolve_root`].
//!
//! # User profile
//!
//! `weaver kernel start --profile user` runs the per-user daemon, whose
//! root is `~/.weftos/run` ([`RootSource::User`]) and never the project
//! walk-up. The profile is process state ([`set_user_profile`]), set once
//! by the CLI, so every later [`RuntimePaths::resolve`] (socket, pid,
//! kernel boot, chain choice) agrees. `$WEFTOS_RUNTIME_DIR` still wins.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Environment variable that points a daemon at an isolated runtime dir.
pub const RUNTIME_DIR_ENV: &str = "WEFTOS_RUNTIME_DIR";

/// File name of the chain JSON checkpoint inside the runtime root.
pub const CHAIN_CHECKPOINT_FILE: &str = "chain.json";

/// Unix socket (logical pipe name on Windows).
pub const SOCKET_NAME: &str = "kernel.sock";
/// PID file.
pub const PID_FILE_NAME: &str = "kernel.pid";
/// Daemon log file.
pub const LOG_FILE_NAME: &str = "kernel.log";
/// Advisory single-instance lock file.
pub const LOCK_FILE_NAME: &str = "kernel.lock";

/// Where the runtime root came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootSource {
    /// `$WEFTOS_RUNTIME_DIR`.
    Env,
    /// A project directory found by walking up from the working directory.
    Project(PathBuf),
    /// The legacy `~/.clawft` directory.
    LegacyHome,
    /// The per-user daemon root, `~/.weftos/run` (`--profile user`).
    User,
}

static USER_PROFILE: AtomicBool = AtomicBool::new(false);

/// Make [`RuntimePaths::resolve`] return the user root for this process.
pub fn set_user_profile(on: bool) {
    USER_PROFILE.store(on, Ordering::SeqCst);
}

/// True when this process runs (or addresses) the user daemon.
pub fn user_profile_active() -> bool {
    USER_PROFILE.load(Ordering::SeqCst)
}

/// `<home>/.weftos`, the per-user state directory (ADR-103 D4).
pub fn user_weftos_dir(home: &Path) -> PathBuf {
    home.join(".weftos")
}

/// `<home>/.weftos/run`, the user daemon's runtime root.
pub fn user_runtime_root(home: &Path) -> PathBuf {
    user_weftos_dir(home).join("run")
}

/// `<home>/.weftos/chain/chain.json`, where the user chain lives once
/// `weaver migrate user-chain` has moved it (Phase 1 package E).
pub fn user_chain_checkpoint(home: &Path) -> PathBuf {
    user_weftos_dir(home)
        .join("chain")
        .join(CHAIN_CHECKPOINT_FILE)
}

/// Every runtime file location, derived from one root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePaths {
    root: PathBuf,
    source: RootSource,
}

fn same_dir(a: &Path, b: &Path) -> bool {
    a == b
        || match (a.canonicalize(), b.canonicalize()) {
            (Ok(x), Ok(y)) => x == y,
            _ => false,
        }
}

/// True when `dir` is a project root (see module docs).
fn is_project_dir(dir: &Path) -> bool {
    let weftos = dir.join(".weftos");
    if !weftos.is_dir() {
        return false;
    }
    weftos.join("project.toml").is_file()
        || weftos.join("weave.toml").is_file()
        || dir.join("weave.toml").is_file()
        || weftos.join("runtime").is_dir()
        || dir.join(".git").exists()
}

/// Nearest project directory at or above `cwd`, never `home` or above it.
pub fn find_project_dir(cwd: &Path, home: Option<&Path>) -> Option<PathBuf> {
    for dir in cwd.ancestors() {
        if home.is_some_and(|h| same_dir(dir, h)) {
            return None;
        }
        if is_project_dir(dir) {
            return Some(dir.to_path_buf());
        }
    }
    None
}

/// Resolve the runtime root from explicit inputs (see module docs).
///
/// `env` is the raw value of [`RUNTIME_DIR_ENV`]; empty or whitespace-only
/// counts as unset.
pub fn resolve_root(
    env: Option<&str>,
    cwd: Option<&Path>,
    home: Option<&Path>,
) -> (PathBuf, RootSource) {
    if let Some(dir) = env.map(str::trim).filter(|d| !d.is_empty()) {
        return (PathBuf::from(dir), RootSource::Env);
    }
    if let Some(project) = cwd.and_then(|c| find_project_dir(c, home)) {
        return (
            project.join(".weftos").join("runtime"),
            RootSource::Project(project),
        );
    }
    let legacy = home
        .map(Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir)
        .join(".clawft");
    (legacy, RootSource::LegacyHome)
}

impl RuntimePaths {
    /// Paths rooted at an explicit directory (tests, isolated runs).
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            source: RootSource::Env,
        }
    }

    /// Resolve from explicit inputs; no process state is read.
    pub fn resolve_with(env: Option<&str>, cwd: Option<&Path>, home: Option<&Path>) -> Self {
        let (root, source) = resolve_root(env, cwd, home);
        Self { root, source }
    }

    /// The user daemon's paths: `$WEFTOS_RUNTIME_DIR` when set, else
    /// `<home>/.weftos/run`. Never walks up to a project. Without a home
    /// directory the root falls back to a temp dir, as the legacy one does.
    pub fn user_with(env: Option<&str>, home: Option<&Path>) -> Self {
        if let Some(dir) = env.map(str::trim).filter(|d| !d.is_empty()) {
            return Self {
                root: PathBuf::from(dir),
                source: RootSource::User,
            };
        }
        let home = home
            .map(Path::to_path_buf)
            .unwrap_or_else(std::env::temp_dir);
        Self {
            root: user_runtime_root(&home),
            source: RootSource::User,
        }
    }

    /// Resolve from the process environment, working directory and home.
    /// Honours [`set_user_profile`].
    pub fn resolve() -> Self {
        let env = std::env::var(RUNTIME_DIR_ENV).ok();
        if user_profile_active() {
            return Self::user_with(env.as_deref(), home_dir().as_deref());
        }
        let cwd = std::env::current_dir().ok();
        let home = home_dir();
        Self::resolve_with(env.as_deref(), cwd.as_deref(), home.as_deref())
    }

    /// The runtime root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// How the root was chosen.
    pub fn source(&self) -> &RootSource {
        &self.source
    }

    fn file(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    /// Daemon socket (logical pipe name on Windows).
    pub fn socket(&self) -> PathBuf {
        self.file(SOCKET_NAME)
    }
    /// PID file.
    pub fn pid(&self) -> PathBuf {
        self.file(PID_FILE_NAME)
    }
    /// Daemon log.
    pub fn log(&self) -> PathBuf {
        self.file(LOG_FILE_NAME)
    }
    /// Single-instance advisory lock.
    pub fn lock(&self) -> PathBuf {
        self.file(LOCK_FILE_NAME)
    }
    /// Daemon Ed25519 node key.
    pub fn node_key(&self) -> PathBuf {
        self.file("node.key")
    }
    /// Chain JSON checkpoint; the RVF, key and tree files derive from it.
    pub fn chain_checkpoint(&self) -> PathBuf {
        self.file(CHAIN_CHECKPOINT_FILE)
    }
    /// Chain RVF store (checkpoint path with `.rvf`).
    pub fn chain_rvf(&self) -> PathBuf {
        self.chain_checkpoint().with_extension("rvf")
    }
    /// Chain signing key (checkpoint path with `.key`).
    pub fn chain_key(&self) -> PathBuf {
        self.chain_checkpoint().with_extension("key")
    }
    /// Resource-tree checkpoint (checkpoint path with `.tree.json`).
    pub fn chain_tree(&self) -> PathBuf {
        self.chain_checkpoint().with_extension("tree.json")
    }
    /// Chain export/scratch directory.
    pub fn chain_dir(&self) -> PathBuf {
        self.file("chain")
    }
    /// External-anchor ledger.
    pub fn anchors_ledger(&self) -> PathBuf {
        self.chain_dir().join("anchors.jsonl")
    }
    /// Node-local workload catalog.
    pub fn workloads(&self) -> PathBuf {
        self.file("workloads.json")
    }
    /// Persisted cluster peers.
    pub fn cluster_peers(&self) -> PathBuf {
        self.file("cluster_peers.json")
    }
    /// Installed-apps manifest store.
    pub fn apps(&self) -> PathBuf {
        self.file("apps.json")
    }
    /// Persistent revoked-host ban list.
    pub fn revoked_hosts(&self) -> PathBuf {
        self.file("revoked_hosts.json")
    }
    /// Paired-host list.
    pub fn paired_hosts(&self) -> PathBuf {
        self.file("paired_hosts.json")
    }
}

/// The user's home directory (the `$HOME` the project walk refuses to return).
///
/// Uses `dirs` when the `native` feature is on; otherwise `$HOME` /
/// `%USERPROFILE%`.
pub fn home_dir() -> Option<PathBuf> {
    #[cfg(feature = "native")]
    {
        dirs::home_dir()
    }
    #[cfg(not(feature = "native"))]
    {
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .filter(|h| !h.is_empty())
            .map(PathBuf::from)
    }
}

/// The pre-ADR-103 chain checkpoint (`~/.clawft/chain.json`) when `paths`
/// resolved to a project root and that legacy chain exists on disk while the
/// resolved one does not. `None` for isolated (`WEFTOS_RUNTIME_DIR`) runs,
/// legacy-rooted runs, or when nothing is left behind.
pub fn legacy_chain_left_behind(paths: &RuntimePaths, home: Option<&Path>) -> Option<PathBuf> {
    if !matches!(paths.source(), RootSource::Project(_)) {
        return None;
    }
    let legacy = home?.join(".clawft").join(CHAIN_CHECKPOINT_FILE);
    let resolved = paths.chain_checkpoint();
    let has_chain = |p: &Path| p.exists() || p.with_extension("rvf").exists();
    (!has_chain(&resolved) && has_chain(&legacy)).then_some(legacy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn mk(dir: &Path, rel: &str) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, "").unwrap();
    }

    #[test]
    fn user_root_is_weftos_run_and_ignores_projects() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().join("home");
        let p = RuntimePaths::user_with(None, Some(&home));
        assert_eq!(p.root(), home.join(".weftos/run"));
        assert_eq!(p.source(), &RootSource::User);
        assert_eq!(p.socket(), home.join(".weftos/run/kernel.sock"));
        assert_eq!(p.lock(), home.join(".weftos/run/kernel.lock"));
        assert_eq!(
            user_chain_checkpoint(&home),
            home.join(".weftos/chain/chain.json")
        );
    }

    #[test]
    fn user_root_env_override_and_blank_env() {
        let home = Path::new("/h");
        let p = RuntimePaths::user_with(Some("/run/probe"), Some(home));
        assert_eq!(p.root(), Path::new("/run/probe"));
        assert_eq!(p.source(), &RootSource::User);
        let p = RuntimePaths::user_with(Some("  "), Some(home));
        assert_eq!(p.root(), Path::new("/h/.weftos/run"));
    }

    #[test]
    fn env_wins_and_empty_env_is_unset() {
        let t = tempfile::tempdir().unwrap();
        mk(t.path(), ".weftos/project.toml");
        let p = RuntimePaths::resolve_with(Some("/run/probe"), Some(t.path()), None);
        assert_eq!(p.root(), Path::new("/run/probe"));
        assert_eq!(p.chain_checkpoint(), PathBuf::from("/run/probe/chain.json"));
        assert_eq!(p.chain_rvf(), PathBuf::from("/run/probe/chain.rvf"));
        assert_eq!(p.chain_key(), PathBuf::from("/run/probe/chain.key"));
        assert_eq!(p.chain_tree(), PathBuf::from("/run/probe/chain.tree.json"));
        assert_eq!(
            p.anchors_ledger(),
            PathBuf::from("/run/probe/chain/anchors.jsonl")
        );
        let p = RuntimePaths::resolve_with(Some("  "), Some(t.path()), None);
        assert!(matches!(p.source(), RootSource::Project(_)));
    }

    #[test]
    fn walks_up_to_project_marker() {
        let t = tempfile::tempdir().unwrap();
        mk(t.path(), "proj/.weftos/weave.toml");
        fs::create_dir_all(t.path().join("proj/a/b")).unwrap();
        let cwd = t.path().join("proj/a/b");
        let p = RuntimePaths::resolve_with(None, Some(&cwd), None);
        assert_eq!(p.root(), t.path().join("proj/.weftos/runtime"));
        assert_eq!(
            p.socket(),
            t.path().join("proj/.weftos/runtime/kernel.sock")
        );
        assert_eq!(p.node_key(), t.path().join("proj/.weftos/runtime/node.key"));
    }

    #[test]
    fn accepts_weaver_init_layout() {
        let t = tempfile::tempdir().unwrap();
        mk(t.path(), "proj/weave.toml");
        fs::create_dir_all(t.path().join("proj/.weftos/runtime")).unwrap();
        let p = RuntimePaths::resolve_with(None, Some(&t.path().join("proj")), None);
        assert_eq!(p.root(), t.path().join("proj/.weftos/runtime"));
    }

    #[test]
    fn existing_runtime_dir_marks_a_project() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir_all(t.path().join("proj/.weftos/runtime")).unwrap();
        fs::create_dir_all(t.path().join("proj/src")).unwrap();
        let home = t.path().join("home");
        let p = RuntimePaths::resolve_with(None, Some(&t.path().join("proj/src")), Some(&home));
        assert_eq!(p.root(), t.path().join("proj/.weftos/runtime"));
    }

    #[test]
    fn git_worktree_with_tracked_weftos_is_its_own_project() {
        let t = tempfile::tempdir().unwrap();
        // Main checkout: .git directory + .weftos.
        fs::create_dir_all(t.path().join("main/.git")).unwrap();
        mk(t.path(), "main/.weftos/SESSION_HANDOFF.md");
        // Worktree inside it: `.git` is a file, .weftos is tracked content.
        mk(t.path(), "main/.claude/worktrees/w1/.git");
        mk(
            t.path(),
            "main/.claude/worktrees/w1/.weftos/SESSION_HANDOFF.md",
        );
        let cwd = t.path().join("main/.claude/worktrees/w1");
        let p = RuntimePaths::resolve_with(None, Some(&cwd), None);
        assert_eq!(p.root(), cwd.join(".weftos/runtime"));
        let main = RuntimePaths::resolve_with(None, Some(&t.path().join("main")), None);
        assert_eq!(main.root(), t.path().join("main/.weftos/runtime"));
        assert_ne!(p.root(), main.root());
    }

    #[test]
    fn bare_weftos_dir_without_any_marker_is_not_a_project() {
        let t = tempfile::tempdir().unwrap();
        mk(t.path(), "proj/.weftos/notes.md");
        let home = t.path().join("home");
        let p = RuntimePaths::resolve_with(None, Some(&t.path().join("proj")), Some(&home));
        assert_eq!(p.source(), &RootSource::LegacyHome);
        // A .git without .weftos is not one either.
        fs::create_dir_all(t.path().join("repo/.git")).unwrap();
        let p = RuntimePaths::resolve_with(None, Some(&t.path().join("repo")), Some(&home));
        assert_eq!(p.source(), &RootSource::LegacyHome);
    }

    #[test]
    fn never_resolves_to_home_even_with_markers() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().join("home");
        mk(&home, ".weftos/weave.toml");
        fs::create_dir_all(home.join("sub/deep")).unwrap();
        for cwd in [home.clone(), home.join("sub/deep")] {
            let p = RuntimePaths::resolve_with(None, Some(&cwd), Some(&home));
            assert_eq!(p.source(), &RootSource::LegacyHome);
            assert_eq!(p.root(), home.join(".clawft"));
        }
    }

    #[test]
    fn project_below_home_still_found() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().join("home");
        mk(&home, ".weftos/weave.toml");
        mk(&home, "work/p/.weftos/project.toml");
        let p = RuntimePaths::resolve_with(None, Some(&home.join("work/p")), Some(&home));
        assert_eq!(p.root(), home.join("work/p/.weftos/runtime"));
    }

    #[test]
    fn legacy_chain_notice_only_for_project_roots() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().join("home");
        mk(&home, ".clawft/chain.json");
        mk(t.path(), "proj/.weftos/project.toml");
        let cwd = t.path().join("proj");
        let project = RuntimePaths::resolve_with(None, Some(&cwd), Some(&home));
        assert_eq!(
            legacy_chain_left_behind(&project, Some(&home)),
            Some(home.join(".clawft/chain.json"))
        );
        // Resolved chain exists: nothing left behind.
        mk(&cwd, ".weftos/runtime/chain.json");
        assert_eq!(legacy_chain_left_behind(&project, Some(&home)), None);
        // Isolated and legacy-rooted runs never warn.
        let env = RuntimePaths::resolve_with(Some("/x"), Some(&cwd), Some(&home));
        assert_eq!(legacy_chain_left_behind(&env, Some(&home)), None);
        let leg = RuntimePaths::resolve_with(None, None, Some(&home));
        assert_eq!(legacy_chain_left_behind(&leg, Some(&home)), None);
    }

    #[test]
    fn no_home_falls_back_to_temp_legacy() {
        let p = RuntimePaths::resolve_with(None, None, None);
        assert_eq!(p.source(), &RootSource::LegacyHome);
        assert!(p.root().ends_with(".clawft"));
    }
}
