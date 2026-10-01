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
pub fn exit_code(e: &anyhow::Error) -> i32 {
    let refused = e.downcast_ref::<BootRefused>().is_some()
        || matches!(e.downcast_ref::<LockError>(), Some(LockError::Held { .. }));
    if refused { EX_CONFIG } else { 1 }
}

/// Arguments (after the program name) for a re-exec: the originals without
/// the one-shot flags.
pub fn reexec_args<I: IntoIterator<Item = String>>(original: I) -> Vec<String> {
    original
        .into_iter()
        .filter(|a| !ONE_SHOT_FLAGS.contains(&a.as_str()))
        .collect()
}

/// `(program, args)` to exec for a restart. The program is `current_exe`
/// without Linux's ` (deleted)` marker, so a binary replaced by an update
/// still starts; with no arguments at all, fall back to the plain foreground
/// start.
pub fn reexec_plan(current_exe: &Path, original: Vec<String>) -> (PathBuf, Vec<String>) {
    let mut args = reexec_args(original);
    if args.is_empty() {
        args = ["kernel", "start", "--foreground"].map(String::from).to_vec();
    }
    (strip_deleted(current_exe), args)
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
    fn plan_strips_deleted_marker_and_defaults_args() {
        let (exe, args) = reexec_plan(Path::new("/opt/bin/weaver (deleted)"), v(&["kernel", "start", "--foreground"]));
        assert_eq!(exe, PathBuf::from("/opt/bin/weaver"));
        assert_eq!(args, v(&["kernel", "start", "--foreground"]));
        let (_, args) = reexec_plan(Path::new("/x/weaver"), Vec::new());
        assert_eq!(args, v(&["kernel", "start", "--foreground"]));
    }

    #[test]
    fn refusals_exit_78_everything_else_1() {
        let held = anyhow::Error::new(LockError::Held { root: "/r".into(), pid: "7".into() });
        assert_eq!(exit_code(&held), EX_CONFIG);
        assert_eq!(exit_code(&anyhow::Error::new(BootRefused("chain in use".into()))), 78);
        assert_eq!(exit_code(&anyhow::anyhow!("socket write failed")), 1);
        let io = LockError::Io { path: "/x".into(), source: std::io::Error::other("e") };
        assert_eq!(exit_code(&anyhow::Error::new(io)), 1);
    }
}
