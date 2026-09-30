//! Path resolution for doctor: the ONE place that decides where things are.
//!
//! When the new runtime-paths API lands, only [`DoctorEnv::detect`] (and
//! [`DoctorEnv::runtime_dir_candidates`]) need to change; every check reads
//! from this struct so tests can point it at temp directories.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Where the runtime dir came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeSource {
    /// `WEFTOS_RUNTIME_DIR` override.
    EnvOverride,
    /// Nearest ancestor `.weftos/runtime`.
    Project,
    /// `~/.clawft` global fallback.
    Global,
}

impl RuntimeSource {
    /// Short label.
    pub fn label(self) -> &'static str {
        match self {
            RuntimeSource::EnvOverride => "WEFTOS_RUNTIME_DIR",
            RuntimeSource::Project => "project .weftos/runtime",
            RuntimeSource::Global => "global ~/.clawft",
        }
    }
}

/// Everything doctor reads from the environment.
#[derive(Debug, Clone)]
pub struct DoctorEnv {
    /// Home directory.
    pub home: PathBuf,
    /// Current directory.
    pub cwd: PathBuf,
    /// `PATH` entries in precedence order, de-duplicated.
    pub path_dirs: Vec<PathBuf>,
    /// Standard install dirs searched even when not on `PATH`.
    pub extra_bin_dirs: Vec<PathBuf>,
    /// `~/.config` (cargo-dist receipts, dev-install marker).
    pub config_dir: PathBuf,
    /// Cargo home (`~/.cargo`), for the install ledger.
    pub cargo_home: PathBuf,
    /// Resolved runtime dir for `cwd`.
    pub runtime_dir: PathBuf,
    /// How `runtime_dir` was resolved.
    pub runtime_source: RuntimeSource,
    /// Canned `ps -axo pid=,command=` output (tests); `None` runs `ps`.
    pub ps_override: Option<String>,
    /// Timeout for each `--version` probe.
    pub probe_timeout: Duration,
    /// Run `--version` on non-native (script) files. Tests only.
    pub probe_scripts: bool,
}

impl DoctorEnv {
    /// Resolve from the real process environment.
    pub fn detect() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        let cwd = std::env::current_dir().unwrap_or_else(|_| home.clone());
        let mut path_dirs: Vec<PathBuf> = Vec::new();
        if let Some(p) = std::env::var_os("PATH") {
            for d in std::env::split_paths(&p) {
                if !d.as_os_str().is_empty() && !path_dirs.contains(&d) {
                    path_dirs.push(d);
                }
            }
        }
        let cargo_home = std::env::var_os("CARGO_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".cargo"));
        let extra_bin_dirs = vec![
            cargo_home.join("bin"),
            PathBuf::from("/usr/local/bin"),
            PathBuf::from("/opt/homebrew/bin"),
            home.join(".local/bin"),
        ];
        let runtime_dir = crate::runtime_dir();
        let runtime_source = if std::env::var_os("WEFTOS_RUNTIME_DIR").is_some() {
            RuntimeSource::EnvOverride
        } else if runtime_dir == home.join(".clawft") {
            RuntimeSource::Global
        } else {
            RuntimeSource::Project
        };
        Self {
            config_dir: home.join(".config"),
            home,
            cwd,
            path_dirs,
            extra_bin_dirs,
            cargo_home,
            runtime_dir,
            runtime_source,
            ps_override: None,
            probe_timeout: Duration::from_secs(5),
            probe_scripts: false,
        }
    }

    /// Every runtime directory doctor should inspect: the resolved one, then
    /// `~/.clawft`, `~/.weftos/runtime`, and each ancestor `.weftos/runtime`.
    /// When `WEFTOS_RUNTIME_DIR` is set it is the ONLY candidate, so tests and
    /// sandboxes can isolate doctor from the real machine.
    pub fn runtime_dir_candidates(&self) -> Vec<PathBuf> {
        let mut out = vec![self.runtime_dir.clone()];
        if self.runtime_source == RuntimeSource::EnvOverride {
            return out;
        }
        let mut push = |p: PathBuf| {
            if !out.contains(&p) {
                out.push(p);
            }
        };
        push(self.home.join(".clawft"));
        push(self.home.join(".weftos/runtime"));
        for p in project_runtime_dirs(&self.cwd) {
            push(p);
        }
        out
    }

    /// All bin dirs to scan: `PATH` order first, then the extras.
    pub fn scan_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = self.path_dirs.clone();
        for d in &self.extra_bin_dirs {
            if !dirs.contains(d) {
                dirs.push(d.clone());
            }
        }
        dirs
    }
}

/// `.weftos/runtime` of every ancestor of `start` that has a `.weftos/` dir.
pub fn project_runtime_dirs(start: &Path) -> Vec<PathBuf> {
    start
        .ancestors()
        .map(|a| a.join(".weftos"))
        .filter(|w| w.is_dir())
        .map(|w| w.join("runtime"))
        .collect()
}

/// Test-only process-wide lock: serializes tests that create unix sockets
/// against any code path that forks (see `probe::run_capture`).
#[cfg(test)]
pub fn serial() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Test helper: an env rooted entirely in `root` with an empty `PATH`.
#[cfg(test)]
pub fn test_env(root: &Path) -> DoctorEnv {
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    DoctorEnv {
        cwd: root.join("proj"),
        path_dirs: Vec::new(),
        extra_bin_dirs: Vec::new(),
        config_dir: home.join(".config"),
        cargo_home: home.join(".cargo"),
        runtime_dir: home.join(".clawft"),
        runtime_source: RuntimeSource::Global,
        home,
        ps_override: Some(String::new()),
        probe_timeout: Duration::from_secs(5),
        probe_scripts: true,
    }
}
