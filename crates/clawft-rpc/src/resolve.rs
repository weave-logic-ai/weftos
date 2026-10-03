//! Endpoint resolver (ADR-103 D14): which kernel should this command talk to?
//!
//! Precedence, first match wins for the runtime root:
//!
//! 1. flag: `--runtime <dir>` (and `--project <id>` for the expected project)
//! 2. env: `WEFTOS_RUNTIME_DIR`, `WEFTOS_PROJECT`
//! 3. manifest: `project.toml` found walking up from the cwd (never `$HOME`),
//!    then `~/.weftos/projects/<id>.toml` and its `[serve] runtime_dir`
//!    Without a `runtime_dir`, a manifest with `[serve] via = "user-daemon"`
//!    selects the user daemon's root (`~/.weftos/run`).
//! 4. user default: with no project known, `~/.weftos/run` when its
//!    `kernel.sock` or `kernel.lock` exists
//! 5. default: [`RuntimePaths::resolve_with`] (the Phase 0 answer)
//!
//! The expected project id is chosen independently with the same order
//! (flag, env, `project.toml`), so a flag or env project still gets its
//! manifest's runtime override. Everything is pure over [`ResolveInputs`];
//! only [`resolve`] reads the process environment.

use std::fmt;
use std::path::{Path, PathBuf};

use clawft_types::project::{
    ServeVia, find_project_toml, read_manifest, read_project_toml, validate_id,
};
use clawft_types::runtime_paths::{
    LOCK_FILE_NAME, RUNTIME_DIR_ENV, RuntimePaths, SOCKET_NAME, home_dir, user_runtime_root,
};

use crate::probe::{SocketState, describe_state};

/// Environment variable naming the expected project ULID.
pub const PROJECT_ENV: &str = "WEFTOS_PROJECT";

/// Which precedence level decided something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveSource {
    Flag,
    Env,
    Manifest,
    Default,
}

impl fmt::Display for ResolveSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Flag => "flag",
            Self::Env => "env",
            Self::Manifest => "manifest",
            Self::Default => "default",
        })
    }
}

/// Command-line overrides (`--runtime`, `--project`).
#[derive(Debug, Clone, Default)]
pub struct ResolveFlags {
    pub runtime: Option<PathBuf>,
    pub project: Option<String>,
}

/// Everything the resolver reads, injected so tests never touch real state.
#[derive(Debug, Clone, Default)]
pub struct ResolveInputs {
    pub flags: ResolveFlags,
    pub env_runtime: Option<String>,
    pub env_project: Option<String>,
    pub cwd: Option<PathBuf>,
    pub home: Option<PathBuf>,
}

impl ResolveInputs {
    /// Read env, cwd and home from the process.
    pub fn from_process(flags: ResolveFlags) -> Self {
        Self {
            flags,
            env_runtime: std::env::var(RUNTIME_DIR_ENV).ok(),
            env_project: std::env::var(PROJECT_ENV).ok(),
            cwd: std::env::current_dir().ok(),
            home: home_dir(),
        }
    }
}

/// One precedence level consulted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    pub level: ResolveSource,
    pub detail: String,
    /// True for the level that decided the runtime root.
    pub used: bool,
}

/// A `via = "child-kernel"` project: its kernel is a child of the user
/// daemon, started on demand (ADR-103 A6, Phase 2 package G).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnsureRunning {
    /// Project to start.
    pub project_id: String,
    /// The user daemon's socket (`~/.weftos/run/kernel.sock`).
    pub user_socket: PathBuf,
}

/// The resolved endpoint and what was expected of the daemon behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub socket: PathBuf,
    pub runtime_root: PathBuf,
    /// Level that decided the runtime root.
    pub source: ResolveSource,
    /// Project ULID the daemon must serve, when one was named or discovered.
    pub project_id: Option<String>,
    /// Node id the daemon must have, when the caller knows one.
    pub expected_node: Option<String>,
    /// Set when the endpoint is a supervised child kernel: connecting
    /// calls `project.ensure_running` on the user daemon once when the
    /// socket is absent. `None` for an explicit endpoint (flag, env,
    /// `runtime_dir`).
    pub ensure: Option<EnsureRunning>,
    pub tried: Vec<Attempt>,
}

impl Resolution {
    /// Require the daemon behind this endpoint to have node id `node`.
    pub fn expect_node(mut self, node: impl Into<String>) -> Self {
        self.expected_node = Some(node.into());
        self
    }

    /// Operator-facing message for an unreachable endpoint: probe wording
    /// plus everything the resolver tried.
    pub fn unreachable_message(&self, state: &SocketState) -> String {
        format!(
            "no kernel reachable: {}\n{self}\n  start one with `weaver kernel start`, \
             or point at another with --runtime / WEFTOS_RUNTIME_DIR",
            describe_state(&self.socket, state)
        )
    }
}

impl fmt::Display for Resolution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "  endpoint: {} (from {})",
            self.socket.display(),
            self.source
        )?;
        match &self.project_id {
            Some(id) => writeln!(f, "  expected project: {id}")?,
            None => writeln!(f, "  expected project: none")?,
        }
        write!(f, "  tried, in order:")?;
        for a in &self.tried {
            write!(
                f,
                "\n    {} {}: {}",
                if a.used { "*" } else { "-" },
                a.level,
                a.detail
            )?;
        }
        Ok(())
    }
}

/// A flag or env value that cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// `--project` / `WEFTOS_PROJECT` is not a ULID.
    InvalidProject { from: ResolveSource, value: String },
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidProject { from, value } => write!(
                f,
                "invalid project id {value:?} from {from}: expected a 26-character ULID \
                 (see `weft project list`)"
            ),
        }
    }
}

impl std::error::Error for ResolveError {}

/// `<home>/.weftos/projects`.
pub fn manifests_dir(home: &Path) -> PathBuf {
    home.join(".weftos").join("projects")
}

fn canon(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

fn nonempty(v: &Option<String>) -> Option<&str> {
    v.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// Resolve from the process environment.
pub fn resolve(flags: &ResolveFlags) -> Result<Resolution, ResolveError> {
    let inputs = ResolveInputs::from_process(flags.clone());
    let r = resolve_with(&inputs)?;
    // A test must not resolve the real home's runtime root implicitly.
    let pinned = matches!(r.source, ResolveSource::Flag | ResolveSource::Env);
    clawft_types::runtime_paths::refuse_real_runtime_in_tests(&r.runtime_root, pinned);
    Ok(r)
}

/// The socket the resolver picks for `i`, or the legacy default when
/// resolution itself fails (an invalid `--project`, say). For callers that
/// only need a path to name or probe; anything that talks to the daemon uses
/// [`resolve`] and `connect_resolved`.
pub fn socket_for(i: &ResolveInputs) -> PathBuf {
    resolve_with(i)
        .map(|r| r.socket)
        .unwrap_or_else(|_| crate::protocol::socket_path())
}

/// [`socket_for`] from the process environment and working directory, so a
/// manifest `runtime_dir` is honoured where `WEFTOS_RUNTIME_DIR` is unset
/// (Phase 1 review S8).
pub fn current_socket_path() -> PathBuf {
    socket_for(&ResolveInputs::from_process(ResolveFlags::default()))
}

/// Resolve from explicit inputs.
pub fn resolve_with(i: &ResolveInputs) -> Result<Resolution, ResolveError> {
    let mut tried = Vec::new();
    let home = i.home.as_deref();

    // Expected project id: flag > env > project.toml above the cwd.
    let mut project: Option<(String, ResolveSource)> = None;
    for (level, raw) in [
        (ResolveSource::Flag, nonempty(&i.flags.project)),
        (ResolveSource::Env, nonempty(&i.env_project)),
    ] {
        if let Some(v) = raw {
            validate_id(v).map_err(|_| ResolveError::InvalidProject {
                from: level,
                value: v.to_owned(),
            })?;
            project = Some((v.to_owned(), level));
            break;
        }
    }
    let mut manifest_note = String::from("no project.toml above the working directory");
    // The project directory when the id came from `project.toml`; the
    // manifest's recorded root must match it before its override applies.
    let mut found_root: Option<PathBuf> = None;
    if project.is_none()
        && let Some(root) = i.cwd.as_deref().and_then(|c| find_project_toml(c, home))
    {
        match read_project_toml(&root) {
            Ok(Some(pt)) => {
                manifest_note = format!("project {} at {}", pt.id, root.display());
                project = Some((pt.id, ResolveSource::Manifest));
                found_root = Some(root);
            }
            Ok(None) => {}
            Err(e) => manifest_note = format!("unreadable project.toml: {e}"),
        }
    }

    // The child kernel's run dir when the manifest says `via = child-kernel`
    // and names no explicit `runtime_dir`.
    let mut child_root: Option<PathBuf> = None;
    // Runtime override from the project's user-level manifest.
    let manifest_root = match (&project, home) {
        (Some((id, _)), Some(h)) => match read_manifest(&manifests_dir(h), id) {
            Ok(Some(m)) => {
                let over = m.runtime_dir_override().map(Path::to_path_buf);
                let is_child = m.serve.as_ref().is_some_and(|s| s.via == ServeVia::ChildKernel);
                match (&over, &found_root) {
                    (None, Some(root)) if is_child && canon(&m.root) != canon(root) => {
                        manifest_note = format!(
                            "{manifest_note}; ignored via = child-kernel: manifest root {} \
                             is not this project's root {} (a copy must not reuse the \
                             original's kernel)",
                            m.root.display(),
                            root.display()
                        );
                        None
                    }
                    (None, _) if is_child => {
                        let run = user_runtime_root(h).join(&m.id);
                        manifest_note = format!(
                            "{manifest_note}; via = child-kernel, run dir {}",
                            run.display()
                        );
                        child_root = Some(run);
                        None
                    }
                    (None, _) if m.serve.as_ref().is_some_and(|s| s.via == ServeVia::UserDaemon) => {
                        // D14: the project is served by the user daemon.
                        manifest_note =
                            format!("{manifest_note}; manifest serves via user-daemon");
                        Some(user_runtime_root(h))
                    }
                    (None, _) => {
                        manifest_note =
                            format!("{manifest_note}; manifest has no [serve] runtime_dir");
                        None
                    }
                    (Some(rt), _) if rt.is_relative() => {
                        manifest_note = format!(
                            "{manifest_note}; ignored relative [serve] runtime_dir {}",
                            rt.display()
                        );
                        None
                    }
                    (Some(_), Some(root)) if canon(&m.root) != canon(root) => {
                        manifest_note = format!(
                            "{manifest_note}; ignored runtime_dir override: manifest root {} \
                             is not this project's root {} (a copy must not reuse the \
                             original's kernel)",
                            m.root.display(),
                            root.display()
                        );
                        None
                    }
                    _ => over,
                }
            }
            Ok(None) => {
                manifest_note = format!("{manifest_note}; no manifest in {}", manifests_dir(h).display());
                None
            }
            Err(e) => {
                manifest_note = format!("{manifest_note}; unreadable manifest: {e}");
                None
            }
        },
        _ => None,
    };

    let flag_rt = i.flags.runtime.clone().filter(|p| !p.as_os_str().is_empty());
    let env_rt = nonempty(&i.env_runtime).map(PathBuf::from);
    let default = RuntimePaths::resolve_with(None, i.cwd.as_deref(), home);
    // User default (D14): no project, but a user daemon has been there.
    let user_default = home
        .map(user_runtime_root)
        .filter(|r| r.join(SOCKET_NAME).exists() || r.join(LOCK_FILE_NAME).exists());
    let user_default = if project.is_none() { user_default } else { None };
    let user_note = match (&user_default, home) {
        (Some(r), _) => format!("user daemon root {}", r.display()),
        (None, Some(h)) => format!(
            "no user daemon at {} (no kernel.sock or kernel.lock)",
            user_runtime_root(h).display()
        ),
        (None, None) => "no home directory".into(),
    };

    let (root, source) = if let Some(p) = flag_rt.clone() {
        (p, ResolveSource::Flag)
    } else if let Some(p) = env_rt.clone() {
        (p, ResolveSource::Env)
    } else if let Some(p) = manifest_root.clone() {
        (p, ResolveSource::Manifest)
    } else if let Some(p) = child_root.clone() {
        (p, ResolveSource::Manifest)
    } else if let Some(p) = user_default.clone() {
        (p, ResolveSource::Default)
    } else {
        (default.root().to_path_buf(), ResolveSource::Default)
    };

    tried.push(Attempt {
        level: ResolveSource::Flag,
        detail: match (&flag_rt, nonempty(&i.flags.project)) {
            (Some(p), _) => format!("--runtime {}", p.display()),
            (None, Some(id)) => format!("--project {id} (expected project only)"),
            _ => "no --runtime / --project given".into(),
        },
        used: source == ResolveSource::Flag,
    });
    tried.push(Attempt {
        level: ResolveSource::Env,
        detail: match (&env_rt, nonempty(&i.env_project)) {
            (Some(p), _) => format!("{RUNTIME_DIR_ENV}={}", p.display()),
            (None, Some(id)) => format!("{PROJECT_ENV}={id} (expected project only)"),
            _ => format!("{RUNTIME_DIR_ENV} and {PROJECT_ENV} not set"),
        },
        used: source == ResolveSource::Env,
    });
    tried.push(Attempt {
        level: ResolveSource::Manifest,
        detail: match &manifest_root {
            Some(p) => format!("{manifest_note}; runtime_dir {}", p.display()),
            None => manifest_note,
        },
        used: source == ResolveSource::Manifest,
    });
    tried.push(Attempt {
        level: ResolveSource::Default,
        detail: format!("{user_note}; else runtime root {}", default.root().display()),
        used: source == ResolveSource::Default,
    });

    let ensure = match (&child_root, &project, home) {
        (Some(_), Some((id, _)), Some(h)) if source == ResolveSource::Manifest => Some(EnsureRunning {
            project_id: id.clone(),
            user_socket: user_runtime_root(h).join(SOCKET_NAME),
        }),
        _ => None,
    };
    Ok(Resolution {
        ensure,
        socket: root.join(SOCKET_NAME),
        runtime_root: root,
        source,
        project_id: project.map(|(id, _)| id),
        expected_node: None,
        tried,
    })
}

#[cfg(test)]
#[path = "resolve_tests.rs"]
mod tests;
