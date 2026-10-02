//! `weaver project migrate-kernel <id> [--dry-run|--revert]` (ADR-103 A6,
//! Phase 2 package G, plan section 4): the owner's per-project opt-in to a
//! supervised child kernel.
//!
//! Never touched: `~/.clawft`, the user chain, anything under
//! `<root>/.weftos/runtime/` except reading `kernel.pid` and the lock state.
//! Old daemons are **never signalled**: the owner stops them from their own
//! directory with their own binary, and this command refuses while one runs.
//! The migration copies (never moves) `workloads.json` and `apps.json` into
//! `<root>/.weftos/state/`, flips `[serve] via` in the manifest, and prints
//! the rollback line. `--revert` flips `via` back and leaves the copies.

use std::path::{Path, PathBuf};

use clawft_types::project::{
    ProjectManifest, ServeSection, ServeVia, find_by_id, list_manifests, update_manifest, validate_id,
};
use clawft_types::runtime_paths::{LOCK_FILE_NAME, PID_FILE_NAME, user_runtime_root};

/// State files carried over (names are the child layout's, see
/// `RuntimePaths::workloads` and `apps`).
pub const STATE_FILES: [&str; 2] = ["workloads.json", "apps.json"];

/// Why a migration or revert was refused.
#[derive(Debug, thiserror::Error)]
pub enum MigrateError {
    /// Not an id and no unique registered name.
    #[error("no registered project {0:?}: run `weft project list`")]
    NotFound(String),
    /// A name that more than one project has.
    #[error("{0:?} names more than one project; use its id")]
    Ambiguous(String),
    /// The project's own daemon still runs.
    #[error(
        "the project-rooted daemon is still running ({0}); stop it from {1} with its own binary \
         (`weaver kernel stop`), confirm `kernel.pid` is gone, then run this again. Nothing was changed \
         and the old daemon was not signalled."
    )]
    LegacyDaemonRunning(String, String),
    /// A child kernel runs for the project (revert).
    #[error(
        "the project kernel is running (pid {0}); stop it first: `weaver kernel stop --project {1}`"
    )]
    ChildRunning(u32, String),
    /// Already in the requested state.
    #[error("project {0} is already {1}")]
    Already(String, &'static str),
    /// A source, or a directory on the way to it, is a symlink or not a
    /// regular file: never followed.
    #[error("{0} is a symlink or not a regular file; refusing to follow it")]
    Unsafe(String),
    /// Filesystem or manifest failure.
    #[error("{0}")]
    Io(String),
}

/// One thing the migration does (or would do).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Copy a state file (never overwriting a different existing one).
    Copy {
        /// Source under `<root>/.weftos/runtime/`.
        from: PathBuf,
        /// Destination under `<root>/.weftos/state/`.
        to: PathBuf,
    },
    /// Keep an existing, different destination file as is.
    KeepExisting {
        /// The destination that already exists.
        to: PathBuf,
    },
    /// Set `[serve] via`.
    SetVia(ServeVia),
}

/// The migration plan for one project.
#[derive(Debug, Clone)]
pub struct Plan {
    /// The manifest as read.
    pub manifest: ProjectManifest,
    /// Ordered actions.
    pub actions: Vec<Action>,
}

fn pid_alive(pid: u32) -> bool {
    matches!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None),
        Ok(()) | Err(nix::errno::Errno::EPERM)
    )
}

/// The live pid of a daemon whose runtime dir is `dir`, by the lock or the
/// pid file. `None` when nothing runs there.
pub fn live_daemon_in(dir: &Path) -> Option<String> {
    #[cfg(unix)]
    if crate::project_supervisor::adopt::lock_held(&dir.join(LOCK_FILE_NAME)) {
        let pid = crate::project_supervisor::adopt::lock_holder_pid(&dir.join(LOCK_FILE_NAME));
        return Some(match pid {
            Some(p) => format!("pid {p} holds kernel.lock"),
            None => "kernel.lock is held".to_owned(),
        });
    }
    let pid: u32 = std::fs::read_to_string(dir.join(PID_FILE_NAME)).ok()?.trim().parse().ok()?;
    pid_alive(pid).then(|| format!("kernel.pid names live pid {pid}"))
}

/// Resolve `id_or_name` to a registered manifest.
pub fn resolve(manifests_dir: &Path, id_or_name: &str) -> Result<ProjectManifest, MigrateError> {
    if validate_id(id_or_name).is_ok() {
        return find_by_id(manifests_dir, id_or_name)
            .map_err(|e| MigrateError::Io(e.to_string()))?
            .ok_or_else(|| MigrateError::NotFound(id_or_name.to_owned()));
    }
    let listing = list_manifests(manifests_dir).map_err(|e| MigrateError::Io(e.to_string()))?;
    let mut hits: Vec<_> = listing.manifests.into_iter().filter(|m| m.name == id_or_name).collect();
    match hits.len() {
        1 => Ok(hits.remove(0)),
        0 => Err(MigrateError::NotFound(id_or_name.to_owned())),
        _ => Err(MigrateError::Ambiguous(id_or_name.to_owned())),
    }
}

/// Plan the migration; refuses while the project's own daemon runs.
/// `Ok(true)` for a regular file, `Ok(false)` when absent; anything that is
/// a symlink (or not a regular file) is [`MigrateError::Unsafe`].
fn lstat_regular(p: &Path) -> Result<bool, MigrateError> {
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.file_type().is_file() => Ok(true),
        Ok(_) => Err(MigrateError::Unsafe(p.display().to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(MigrateError::Io(format!("{}: {e}", p.display()))),
    }
}

/// A directory on the way (`.weftos`, `runtime`, `state`) must be a real
/// directory when it exists: a symlink there would redirect the copy.
fn lstat_dir(p: &Path) -> Result<(), MigrateError> {
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.file_type().is_dir() => Ok(()),
        Ok(_) => Err(MigrateError::Unsafe(p.display().to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(MigrateError::Io(format!("{}: {e}", p.display()))),
    }
}

/// The checks both planning and applying run: every directory on the way is
/// real and every source is a regular file.
fn verify_paths(root: &Path) -> Result<(), MigrateError> {
    let w = root.join(".weftos");
    lstat_dir(&w)?;
    lstat_dir(&w.join("runtime"))?;
    lstat_dir(&w.join("state"))?;
    for name in STATE_FILES {
        lstat_regular(&w.join("runtime").join(name))?;
        lstat_regular(&w.join("state").join(name))?;
    }
    Ok(())
}

pub fn plan(manifests_dir: &Path, id_or_name: &str) -> Result<Plan, MigrateError> {
    let m = resolve(manifests_dir, id_or_name)?;
    verify_paths(&m.root)?;
    let legacy = m.root.join(".weftos").join("runtime");
    if let Some(why) = live_daemon_in(&legacy) {
        return Err(MigrateError::LegacyDaemonRunning(why, m.root.display().to_string()));
    }
    if m.serve.as_ref().is_some_and(|s| s.via == ServeVia::ChildKernel) {
        return Err(MigrateError::Already(m.id.clone(), "child-kernel"));
    }
    let state = m.root.join(".weftos").join("state");
    let mut actions = Vec::new();
    for name in STATE_FILES {
        let (from, to) = (legacy.join(name), state.join(name));
        if !from.is_file() {
            continue;
        }
        match std::fs::read(&to) {
            Ok(existing) if std::fs::read(&from).ok().as_deref() == Some(existing.as_slice()) => {}
            Ok(_) => actions.push(Action::KeepExisting { to }),
            Err(_) => actions.push(Action::Copy { from, to }),
        }
    }
    actions.push(Action::SetVia(ServeVia::ChildKernel));
    Ok(Plan { manifest: m, actions })
}

/// Apply `plan`. Copies are written 0600 beside a temp name and renamed.
pub fn apply(manifests_dir: &Path, plan: &Plan) -> Result<(), MigrateError> {
    // Planning and applying are not atomic: look again.
    verify_paths(&plan.manifest.root)?;
    for a in &plan.actions {
        match a {
            Action::Copy { from, to } => copy_private(from, to)?,
            Action::KeepExisting { .. } => {}
            Action::SetVia(via) => set_via(manifests_dir, &plan.manifest.id, *via)?,
        }
    }
    Ok(())
}

fn copy_private(from: &Path, to: &Path) -> Result<(), MigrateError> {
    use std::io::Write as _;
    let io = |e: std::io::Error| MigrateError::Io(format!("{}: {e}", to.display()));
    // O_NOFOLLOW on the source: a file swapped for a symlink after the check
    // is refused, not followed.
    let bytes = {
        use std::io::Read as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        let mut f = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(nix::libc::O_NOFOLLOW)
            .open(from)
            .map_err(|e| MigrateError::Io(format!("{}: {e}", from.display())))?;
        if !f.metadata().map_err(io)?.is_file() {
            return Err(MigrateError::Unsafe(from.display().to_string()));
        }
        let mut b = Vec::new();
        f.read_to_end(&mut b).map_err(io)?;
        b
    };
    let dir = to.parent().ok_or_else(|| MigrateError::Io("no parent directory".into()))?;
    std::fs::create_dir_all(dir).map_err(io)?;
    lstat_dir(dir)?;
    let tmp = dir.join(format!(".migrate.{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600).custom_flags(nix::libc::O_NOFOLLOW);
    }
    let mut f = opts.open(&tmp).map_err(io)?;
    f.write_all(&bytes).map_err(io)?;
    f.sync_all().map_err(io)?;
    std::fs::rename(&tmp, to).map_err(io)
}

fn set_via(manifests_dir: &Path, id: &str, via: ServeVia) -> Result<(), MigrateError> {
    update_manifest(manifests_dir, id, |m| {
        m.serve.get_or_insert_with(ServeSection::default).via = via;
    })
    .map_err(|e| MigrateError::Io(e.to_string()))?
    .map(|_| ())
    .ok_or_else(|| MigrateError::NotFound(id.to_owned()))
}

/// `--revert`: flip `via` back to `user-daemon`. Refuses while the child
/// kernel runs. The copied state files stay; the original runtime dir was
/// never modified.
pub fn revert(home: &Path, manifests_dir: &Path, id_or_name: &str, dry_run: bool) -> Result<ProjectManifest, MigrateError> {
    let m = resolve(manifests_dir, id_or_name)?;
    if !m.serve.as_ref().is_some_and(|s| s.via == ServeVia::ChildKernel) {
        return Err(MigrateError::Already(m.id.clone(), "served by the user daemon"));
    }
    let run = user_runtime_root(home).join(&m.id);
    if let Some(pid) = std::fs::read_to_string(run.join(PID_FILE_NAME))
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .filter(|p| pid_alive(*p))
    {
        return Err(MigrateError::ChildRunning(pid, m.id.clone()));
    }
    if !dry_run {
        set_via(manifests_dir, &m.id, ServeVia::UserDaemon)?;
    }
    Ok(m)
}

/// The rollback instruction printed after a migration.
pub fn rollback_line(m: &ProjectManifest) -> String {
    format!(
        "rollback: `weaver kernel stop --project {id}`, then `weaver project migrate-kernel {id} --revert` \
         (or set [serve] via = \"user-daemon\" in the manifest), then restart the old daemon in {root}. \
         The old runtime dir was never modified.",
        id = m.id,
        root = m.root.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawft_types::project::adopt_or_init;

    struct W {
        _t: tempfile::TempDir,
        home: PathBuf,
        mdir: PathBuf,
        m: ProjectManifest,
    }

    fn world() -> W {
        let t = tempfile::tempdir().unwrap();
        let base = t.path().canonicalize().unwrap();
        let home = base.join("home");
        let mdir = home.join(".weftos/projects");
        let root = base.join("proj");
        std::fs::create_dir_all(&root).unwrap();
        let m = adopt_or_init(&root, &mdir, Some("demo")).unwrap();
        W { _t: t, home, mdir, m }
    }

    fn legacy_dir(w: &W) -> PathBuf {
        let d = w.m.root.join(".weftos/runtime");
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn plan_copies_state_and_flips_via_without_touching_the_source() {
        let w = world();
        let d = legacy_dir(&w);
        std::fs::write(d.join("workloads.json"), b"{\"a\":1}").unwrap();
        std::fs::write(d.join("apps.json"), b"[]").unwrap();
        std::fs::write(d.join("node.key"), b"SECRET").unwrap();
        let p = plan(&w.mdir, "demo").unwrap();
        assert_eq!(p.actions.len(), 3);
        apply(&w.mdir, &p).unwrap();
        let st = w.m.root.join(".weftos/state");
        assert_eq!(std::fs::read(st.join("workloads.json")).unwrap(), b"{\"a\":1}");
        assert!(!st.join("node.key").exists(), "keys are never carried over");
        assert!(d.join("workloads.json").exists(), "copied, not moved");
        let m = find_by_id(&w.mdir, &w.m.id).unwrap().unwrap();
        assert_eq!(m.serve.unwrap().via, ServeVia::ChildKernel);
        // Again: already migrated.
        assert!(matches!(plan(&w.mdir, &w.m.id), Err(MigrateError::Already(..))));
    }

    #[test]
    fn dry_run_is_the_plan_alone() {
        let w = world();
        let d = legacy_dir(&w);
        std::fs::write(d.join("apps.json"), b"[]").unwrap();
        let p = plan(&w.mdir, &w.m.id).unwrap();
        assert!(matches!(p.actions[0], Action::Copy { .. }));
        // Nothing applied: nothing on disk changed.
        assert!(!w.m.root.join(".weftos/state").exists());
        assert!(find_by_id(&w.mdir, &w.m.id).unwrap().unwrap().serve.is_none_or(|s| s.via == ServeVia::UserDaemon));
    }

    #[test]
    fn a_differing_destination_is_kept() {
        let w = world();
        let d = legacy_dir(&w);
        std::fs::write(d.join("apps.json"), b"old").unwrap();
        let st = w.m.root.join(".weftos/state");
        std::fs::create_dir_all(&st).unwrap();
        std::fs::write(st.join("apps.json"), b"newer").unwrap();
        let p = plan(&w.mdir, &w.m.id).unwrap();
        assert!(matches!(p.actions[0], Action::KeepExisting { .. }));
        apply(&w.mdir, &p).unwrap();
        assert_eq!(std::fs::read(st.join("apps.json")).unwrap(), b"newer");
    }

    #[test]
    fn a_live_legacy_daemon_blocks_the_migration_and_is_not_signalled() {
        let w = world();
        let d = legacy_dir(&w);
        // Our own pid: certainly alive.
        std::fs::write(d.join("kernel.pid"), std::process::id().to_string()).unwrap();
        let err = plan(&w.mdir, &w.m.id).unwrap_err();
        assert!(matches!(err, MigrateError::LegacyDaemonRunning(..)), "{err}");
        assert!(err.to_string().contains("was not signalled"));
        // A dead pid does not block.
        std::fs::write(d.join("kernel.pid"), "999999").unwrap();
        assert!(plan(&w.mdir, &w.m.id).is_ok());
    }

    #[test]
    fn a_held_lock_blocks_even_without_a_pid_file() {
        let w = world();
        let d = legacy_dir(&w);
        let paths = clawft_types::runtime_paths::RuntimePaths::at(&d);
        let _lock = crate::instance_lock::InstanceLock::acquire(&paths).unwrap();
        std::fs::remove_file(d.join("kernel.pid")).ok();
        assert!(matches!(plan(&w.mdir, &w.m.id), Err(MigrateError::LegacyDaemonRunning(..))));
    }

    #[test]
    fn revert_flips_back_and_refuses_while_the_child_runs() {
        let w = world();
        let p = plan(&w.mdir, &w.m.id).unwrap();
        apply(&w.mdir, &p).unwrap();
        let run = user_runtime_root(&w.home).join(&w.m.id);
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(run.join("kernel.pid"), std::process::id().to_string()).unwrap();
        assert!(matches!(revert(&w.home, &w.mdir, &w.m.id, false), Err(MigrateError::ChildRunning(..))));
        std::fs::remove_file(run.join("kernel.pid")).unwrap();
        // Dry run changes nothing.
        revert(&w.home, &w.mdir, &w.m.id, true).unwrap();
        assert_eq!(
            find_by_id(&w.mdir, &w.m.id).unwrap().unwrap().serve.unwrap().via,
            ServeVia::ChildKernel
        );
        revert(&w.home, &w.mdir, &w.m.id, false).unwrap();
        assert_eq!(
            find_by_id(&w.mdir, &w.m.id).unwrap().unwrap().serve.unwrap().via,
            ServeVia::UserDaemon
        );
        assert!(matches!(revert(&w.home, &w.mdir, &w.m.id, false), Err(MigrateError::Already(..))));
    }

    #[test]
    fn symlinked_sources_and_directories_are_refused() {
        let w = world();
        let d = legacy_dir(&w);
        let secret = w.m.root.join("secret.json");
        std::fs::write(&secret, b"{\"keys\":1}").unwrap();
        // A symlinked source.
        std::os::unix::fs::symlink(&secret, d.join("workloads.json")).unwrap();
        assert!(matches!(plan(&w.mdir, &w.m.id), Err(MigrateError::Unsafe(p)) if p.ends_with("workloads.json")));
        std::fs::remove_file(d.join("workloads.json")).unwrap();
        // A symlinked destination file.
        std::fs::write(d.join("apps.json"), b"[]").unwrap();
        let st = w.m.root.join(".weftos/state");
        std::fs::create_dir_all(&st).unwrap();
        std::os::unix::fs::symlink(&secret, st.join("apps.json")).unwrap();
        assert!(matches!(plan(&w.mdir, &w.m.id), Err(MigrateError::Unsafe(_))));
        std::fs::remove_file(st.join("apps.json")).unwrap();
        // A symlinked state directory (the copy would land elsewhere).
        std::fs::remove_dir(&st).unwrap();
        let elsewhere = w.m.root.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &st).unwrap();
        assert!(matches!(plan(&w.mdir, &w.m.id), Err(MigrateError::Unsafe(_))));
        assert!(std::fs::read_dir(&elsewhere).unwrap().next().is_none(), "nothing was written through it");
        std::fs::remove_file(&st).unwrap();
        // A symlinked runtime directory.
        std::fs::remove_dir_all(&d).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &d).unwrap();
        assert!(matches!(plan(&w.mdir, &w.m.id), Err(MigrateError::Unsafe(_))));
    }

    #[test]
    fn a_source_swapped_for_a_symlink_after_planning_is_not_followed() {
        let w = world();
        let d = legacy_dir(&w);
        std::fs::write(d.join("apps.json"), b"[]").unwrap();
        let p = plan(&w.mdir, &w.m.id).unwrap();
        let secret = w.m.root.join("secret.json");
        std::fs::write(&secret, b"TOP SECRET").unwrap();
        std::fs::remove_file(d.join("apps.json")).unwrap();
        std::os::unix::fs::symlink(&secret, d.join("apps.json")).unwrap();
        assert!(matches!(apply(&w.mdir, &p), Err(MigrateError::Unsafe(_))));
        assert!(!w.m.root.join(".weftos/state/apps.json").exists());
        assert!(find_by_id(&w.mdir, &w.m.id).unwrap().unwrap().serve.is_none_or(|s| s.via == ServeVia::UserDaemon));
    }

    #[test]
    fn unknown_and_ambiguous_names() {
        let w = world();
        assert!(matches!(plan(&w.mdir, "nope"), Err(MigrateError::NotFound(_))));
        assert!(matches!(plan(&w.mdir, "01JB8Z3Q0V6X9KQ4M2N7T5R1WD"), Err(MigrateError::NotFound(_))));
        let _ = &w.home;
    }
}
