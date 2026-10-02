//! Boot refusals and the SIGHUP re-exec plan (ADR-103 Phase 1, package H).
//!
//! A daemon that is refused at boot (instance lock held, chain lock held,
//! legacy chain adoption refused, bad config) cannot succeed by being
//! retried, so it exits with [`EX_CONFIG`] rather than 1. The generated
//! systemd unit lists that code in `RestartPreventExitStatus`; launchd
//! cannot filter on exit codes and relies on `ThrottleInterval` instead.
//!
//! The re-exec plan keeps the original arguments (notably `--profile user`,
//! which is process state, not environment) minus the one-shot chain flags,
//! and execs a path that survives `weaver update` replacing the binary.

use std::path::{Path, PathBuf};

use crate::instance_lock::LockError;
use crate::service_units::strip_deleted;

/// `sysexits.h` EX_CONFIG: the daemon was refused or misconfigured.
pub const EX_CONFIG: i32 = crate::service_units::REFUSED_EXIT;

/// Flags that apply to one boot only and must not be replayed on re-exec.
const ONE_SHOT_FLAGS: [&str; 2] = ["--new-chain", "--adopt-legacy-chain"];

/// A boot that retrying will not fix.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct BootRefused(pub String);

/// Process exit code for an error that ended `kernel start --foreground`.
///
/// 78 only for refusals that retrying cannot fix: instance lock held, chain
/// lock held, legacy chain adoption refused, unusable boot configuration, a
/// live daemon on the socket (or one we may not probe). Everything else
/// (service start, mesh bind, I/O) exits 1 so a service manager retries
/// within its start limit.
pub fn exit_code(e: &anyhow::Error) -> i32 {
    use clawft_kernel::KernelError;
    let refused = e.downcast_ref::<BootRefused>().is_some()
        || matches!(e.downcast_ref::<KernelError>(), Some(KernelError::BootRefused(_)))
        || matches!(e.downcast_ref::<LockError>(), Some(LockError::Held { .. }));
    if refused { EX_CONFIG } else { 1 }
}

/// Everything a SIGHUP re-exec must reproduce, captured once at startup
/// before the daemon changes directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replay {
    /// Arguments after the program name, `--config` made absolute.
    pub args: Vec<String>,
    /// Absolute `$WEFTOS_RUNTIME_DIR`, when set.
    pub runtime_dir: Option<PathBuf>,
}

static REPLAY: std::sync::OnceLock<Replay> = std::sync::OnceLock::new();

/// Make the value of `--config X`, `-c X` and `--config=X` absolute against `cwd`.
pub fn absolutize_config_args(args: Vec<String>, cwd: &Path) -> Vec<String> {
    let abs = |v: &str| {
        let p = Path::new(v);
        if p.is_absolute() { v.to_owned() } else { cwd.join(p).to_string_lossy().into_owned() }
    };
    let mut out = Vec::with_capacity(args.len());
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        if a == "--config" || a == "-c" {
            out.push(a);
            if let Some(v) = it.next() {
                out.push(abs(&v));
            }
        } else if let Some(v) = a.strip_prefix("--config=") {
            out.push(format!("--config={}", abs(v)));
        } else {
            out.push(a);
        }
    }
    out
}

impl Replay {
    /// Build from explicit inputs (no process state).
    pub fn capture(args: Vec<String>, cwd: &Path, runtime_env: Option<&str>) -> Self {
        Self {
            args: absolutize_config_args(args, cwd),
            runtime_dir: runtime_env
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(|v| if Path::new(v).is_absolute() { PathBuf::from(v) } else { cwd.join(v) }),
        }
    }
}

/// Record the replay inputs from the process. Call first thing in
/// `kernel`, before any `chdir`; later calls are ignored.
pub fn capture_replay() {
    let cwd = std::env::current_dir().unwrap_or_default();
    let env = std::env::var("WEFTOS_RUNTIME_DIR").ok();
    let _ = REPLAY.set(Replay::capture(std::env::args().skip(1).collect(), &cwd, env.as_deref()));
}

/// The captured replay inputs, or the current process state if nothing was
/// captured (tests, unusual entry points).
pub fn replay() -> Replay {
    REPLAY.get().cloned().unwrap_or_else(|| {
        let cwd = std::env::current_dir().unwrap_or_default();
        let env = std::env::var("WEFTOS_RUNTIME_DIR").ok();
        Replay::capture(std::env::args().skip(1).collect(), &cwd, env.as_deref())
    })
}

/// Arguments (after the program name) for a re-exec: the originals without
/// the one-shot flags.
pub fn reexec_args<I: IntoIterator<Item = String>>(original: I) -> Vec<String> {
    let mut args: Vec<String> = original
        .into_iter()
        .filter(|a| !ONE_SHOT_FLAGS.contains(&a.as_str()))
        .collect();
    // A plain `kernel start` that is being re-exec'd is a daemon that was
    // already running: the one-release refusal of project-rooted daemons
    // beside a user daemon (ADR-103 A6) must not take it down on SIGHUP.
    let plain_start = args.iter().any(|a| a == "start")
        && !args.iter().any(|a| a == "--profile" || a.starts_with("--profile="))
        && !args.iter().any(|a| a == "--legacy-project-daemon");
    if plain_start {
        args.push("--legacy-project-daemon".to_owned());
    }
    args
}

/// What to exec for a restart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReexecPlan {
    /// `current_exe` without Linux's ` (deleted)` marker, so a binary
    /// replaced by an update still starts.
    pub exe: PathBuf,
    pub args: Vec<String>,
    /// Absolute runtime dir to pass as `WEFTOS_RUNTIME_DIR` (via
    /// `Command::env`, never `set_var`).
    pub runtime_dir: Option<PathBuf>,
}

/// Plan the re-exec from `current_exe` and the captured [`Replay`]; with no
/// arguments at all, fall back to the plain foreground start.
pub fn reexec_plan(current_exe: &Path, replay: Replay) -> ReexecPlan {
    let mut args = reexec_args(replay.args);
    if args.is_empty() {
        args = ["kernel", "start", "--foreground"].map(String::from).to_vec();
    }
    ReexecPlan { exe: strip_deleted(current_exe), args, runtime_dir: replay.runtime_dir }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn profile_survives_and_one_shot_flags_do_not() {
        let got = reexec_args(v(&[
            "kernel", "start", "--foreground", "--profile", "user", "--new-chain", "--adopt-legacy-chain",
        ]));
        assert_eq!(got, v(&["kernel", "start", "--foreground", "--profile", "user"]));
    }

    #[test]
    fn a_re_exec_of_a_plain_daemon_is_marked_legacy_so_the_refusal_cannot_kill_it() {
        let got = reexec_args(v(&["kernel", "start", "--foreground"]));
        assert_eq!(got, v(&["kernel", "start", "--foreground", "--legacy-project-daemon"]));
        // Already marked: not doubled. Profiles carry their own meaning.
        let marked = v(&["kernel", "start", "--legacy-project-daemon"]);
        assert_eq!(reexec_args(marked.clone()), marked);
        let project = v(&["kernel", "start", "--foreground", "--profile", "project", "--project", "X"]);
        assert_eq!(reexec_args(project.clone()), project);
        let user = v(&["kernel", "start", "--foreground", "--profile=user"]);
        assert_eq!(reexec_args(user.clone()), user);
    }

    #[test]
    fn plan_strips_deleted_marker_and_defaults_args() {
        let r = Replay { args: v(&["kernel", "start", "--foreground"]), runtime_dir: None };
        let p = reexec_plan(Path::new("/opt/bin/weaver (deleted)"), r);
        assert_eq!(p.exe, PathBuf::from("/opt/bin/weaver"));
        assert_eq!(p.args, v(&["kernel", "start", "--foreground", "--legacy-project-daemon"]));
        let p = reexec_plan(Path::new("/x/weaver"), Replay { args: Vec::new(), runtime_dir: None });
        assert_eq!(p.args, v(&["kernel", "start", "--foreground"]));
    }

    #[test]
    fn relative_config_is_absolute_in_every_spelling_after_a_chdir() {
        // Captured with cwd=/work, replayed after the daemon moved to ~/.weftos.
        let cwd = Path::new("/work");
        for (given, want) in [
            (v(&["kernel", "start", "--config", "w.toml"]), v(&["kernel", "start", "--config", "/work/w.toml"])),
            (v(&["kernel", "-c", "sub/w.toml", "start"]), v(&["kernel", "-c", "/work/sub/w.toml", "start"])),
            (v(&["kernel", "start", "--config=w.toml"]), v(&["kernel", "start", "--config=/work/w.toml"])),
            (v(&["--config", "/abs/w.toml"]), v(&["--config", "/abs/w.toml"])),
        ] {
            let r = Replay::capture(given, cwd, None);
            assert_eq!(r.args, want);
            let moved = tempfile::tempdir().unwrap();
            let _ = moved; // the plan never consults the current directory
            // (a plain `start` also gains the legacy marker, tested below)
            assert_eq!(reexec_plan(Path::new("/x/weaver"), r).args, reexec_args(want));
        }
    }

    #[test]
    fn runtime_dir_is_captured_absolute_and_blank_is_unset() {
        let cwd = Path::new("/work");
        assert_eq!(Replay::capture(vec![], cwd, Some("rt")).runtime_dir, Some(PathBuf::from("/work/rt")));
        assert_eq!(Replay::capture(vec![], cwd, Some("/rt")).runtime_dir, Some(PathBuf::from("/rt")));
        assert_eq!(Replay::capture(vec![], cwd, Some("  ")).runtime_dir, None);
        assert_eq!(Replay::capture(vec![], cwd, None).runtime_dir, None);
    }

    #[test]
    fn only_irreparable_refusals_exit_78() {
        use clawft_kernel::KernelError;
        let held = anyhow::Error::new(LockError::Held { root: "/r".into(), pid: "7".into() });
        assert_eq!(exit_code(&held), EX_CONFIG);
        assert_eq!(exit_code(&anyhow::Error::new(BootRefused("socket live".into()))), 78);
        assert_eq!(exit_code(&anyhow::Error::new(KernelError::BootRefused("chain in use".into()))), 78);
        // Retryable: everything else.
        for e in [
            anyhow::Error::new(KernelError::Boot("service start failed: x".into())),
            anyhow::Error::new(KernelError::Boot("mesh enabled but the listener could not bind".into())),
            anyhow::anyhow!("socket write failed"),
            anyhow::Error::new(LockError::Io { path: "/x".into(), source: std::io::Error::other("e") }),
        ] {
            assert_eq!(exit_code(&e), 1, "{e}");
        }
    }
}
