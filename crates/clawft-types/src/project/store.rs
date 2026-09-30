//! Reading, writing and locating project files. Paths are always injected.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::schema::SCHEMA_VERSION;
use super::{ProjectError, ProjectManifest, ProjectToml, validate_id};

/// Directory inside a project root holding weftos state.
pub const PROJECT_DIR: &str = ".weftos";
/// Identity file name inside [`PROJECT_DIR`].
pub const PROJECT_TOML: &str = "project.toml";

/// `<root>/.weftos/project.toml`.
pub fn project_toml_path(root: &Path) -> PathBuf {
    root.join(PROJECT_DIR).join(PROJECT_TOML)
}

/// `<manifests_dir>/<id>.toml`; validates `id` first.
pub fn manifest_path(manifests_dir: &Path, id: &str) -> Result<PathBuf, ProjectError> {
    validate_id(id)?;
    Ok(manifests_dir.join(format!("{id}.toml")))
}

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Write `contents` to `path` via temp file + rename, with `mode` on unix.
fn atomic_write(path: &Path, contents: &str, mode: u32) -> Result<(), ProjectError> {
    let dir = path
        .parent()
        .ok_or_else(|| ProjectError::BadRoot(path.to_path_buf()))?;
    std::fs::create_dir_all(dir).map_err(|e| ProjectError::io(dir, e))?;
    let tmp = dir.join(format!(
        ".{}.tmp.{}.{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
        std::process::id(),
        TMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(mode);
        }
        #[cfg(not(unix))]
        let _ = mode;
        let mut f = opts.open(&tmp)?;
        f.write_all(contents.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)?;
        sync_dir(dir);
        Ok::<(), std::io::Error>(())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(ProjectError::io(path, e));
    }
    Ok(())
}

/// Best-effort fsync of a directory so a rename survives a crash.
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

/// Publish `contents` at `path` only if nothing is there yet. Complete or
/// absent, never partial: written to a temp file, then hard-linked into
/// place (fails with `AlreadyExists` if another writer got there first).
/// Returns `Ok(true)` when this call created the file.
fn publish_new(path: &Path, contents: &str, mode: u32) -> Result<bool, ProjectError> {
    let dir = path
        .parent()
        .ok_or_else(|| ProjectError::BadRoot(path.to_path_buf()))?;
    std::fs::create_dir_all(dir).map_err(|e| ProjectError::io(dir, e))?;
    let tmp = dir.join(format!(
        ".{}.tmp.{}.{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
        std::process::id(),
        TMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(mode);
        }
        #[cfg(not(unix))]
        let _ = mode;
        let mut f = opts.open(&tmp)?;
        f.write_all(contents.as_bytes())?;
        f.sync_all()?;
        match std::fs::hard_link(&tmp, path) {
            Ok(()) => {
                sync_dir(dir);
                Ok(true)
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
            Err(e) => Err(e),
        }
    })();
    let _ = std::fs::remove_file(&tmp);
    result.map_err(|e| ProjectError::io(path, e))
}

fn read_optional(path: &Path) -> Result<Option<String>, ProjectError> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(ProjectError::io(path, e)),
    }
}

fn check_schema(path: &Path, found: u32) -> Result<(), ProjectError> {
    if found > SCHEMA_VERSION {
        return Err(ProjectError::UnsupportedSchema {
            path: path.to_path_buf(),
            found,
            supported: SCHEMA_VERSION,
        });
    }
    Ok(())
}

/// Read `<root>/.weftos/project.toml`; `Ok(None)` when absent.
pub fn read_project_toml(root: &Path) -> Result<Option<ProjectToml>, ProjectError> {
    let path = project_toml_path(root);
    let Some(text) = read_optional(&path)? else {
        return Ok(None);
    };
    let pt: ProjectToml = toml::from_str(&text).map_err(|e| ProjectError::Parse {
        path: path.clone(),
        message: e.to_string(),
    })?;
    validate_id(&pt.id)?;
    check_schema(&path, pt.schema_version)?;
    Ok(Some(pt))
}

/// Atomically write `<root>/.weftos/project.toml` (mode 0644).
pub fn write_project_toml(root: &Path, pt: &ProjectToml) -> Result<(), ProjectError> {
    validate_id(&pt.id)?;
    let path = project_toml_path(root);
    let text = toml::to_string_pretty(pt).map_err(|e| ProjectError::Serialize {
        path: path.clone(),
        message: e.to_string(),
    })?;
    atomic_write(&path, &text, 0o644)
}

/// Create `<root>/.weftos/project.toml` only if absent (mode 0644).
/// `Ok(false)` means another writer already created it; the caller must
/// re-read it and adopt its id. This is the cross-process arbiter.
pub(super) fn create_project_toml(root: &Path, pt: &ProjectToml) -> Result<bool, ProjectError> {
    validate_id(&pt.id)?;
    let path = project_toml_path(root);
    let text = toml::to_string_pretty(pt).map_err(|e| ProjectError::Serialize {
        path: path.clone(),
        message: e.to_string(),
    })?;
    publish_new(&path, &text, 0o644)
}

fn parse_manifest(path: &Path, text: &str) -> Result<ProjectManifest, ProjectError> {
    let m: ProjectManifest = toml::from_str(text).map_err(|e| ProjectError::Parse {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    validate_id(&m.id)?;
    check_schema(path, m.schema_version)?;
    Ok(m)
}

/// Read `<manifests_dir>/<id>.toml`; `Ok(None)` when absent.
pub fn read_manifest(
    manifests_dir: &Path,
    id: &str,
) -> Result<Option<ProjectManifest>, ProjectError> {
    let path = manifest_path(manifests_dir, id)?;
    let Some(text) = read_optional(&path)? else {
        return Ok(None);
    };
    let m = parse_manifest(&path, &text)?;
    if m.id != id {
        return Err(ProjectError::IdMismatch { path, id: m.id });
    }
    Ok(Some(m))
}

/// Atomically write a manifest (mode 0600).
pub fn write_manifest(manifests_dir: &Path, m: &ProjectManifest) -> Result<(), ProjectError> {
    let path = manifest_path(manifests_dir, &m.id)?;
    let text = toml::to_string_pretty(m).map_err(|e| ProjectError::Serialize {
        path: path.clone(),
        message: e.to_string(),
    })?;
    atomic_write(&path, &text, 0o600)
}

/// Canonicalise even when the leaf is gone: resolve the deepest existing
/// ancestor and re-append the rest, so `/tmp/x` and `/private/tmp/x` agree.
pub(super) fn canonical_lenient(path: &Path) -> PathBuf {
    if let Ok(c) = std::fs::canonicalize(path) {
        return c;
    }
    let mut tail = Vec::new();
    let mut cur = path;
    while let Some(parent) = cur.parent() {
        if let Some(name) = cur.file_name() {
            tail.push(name.to_os_string());
        }
        if let Ok(mut c) = std::fs::canonicalize(parent) {
            c.extend(tail.iter().rev());
            return c;
        }
        cur = parent;
    }
    path.to_path_buf()
}

/// Exclusive advisory lock on `<manifests_dir>/.lock`, held until dropped.
/// Serialises manifest read-modify-write across processes (and threads:
/// each guard opens its own file description). No-op off unix.
pub(super) struct ManifestLock {
    _file: std::fs::File,
}

pub(super) fn lock_manifests(manifests_dir: &Path) -> Result<ManifestLock, ProjectError> {
    std::fs::create_dir_all(manifests_dir).map_err(|e| ProjectError::io(manifests_dir, e))?;
    let path = manifests_dir.join(".lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| ProjectError::io(&path, e))?;
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        loop {
            // SAFETY: flock on a valid fd owned by `file`.
            let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
            if rc == 0 {
                break;
            }
            let err = std::io::Error::last_os_error();
            if err.kind() != std::io::ErrorKind::Interrupted {
                return Err(ProjectError::io(&path, err));
            }
        }
    }
    Ok(ManifestLock { _file: file })
}

/// Remove `.*.tmp.*` leftovers from crashed writers once they are stale.
pub(super) fn reap_orphans(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with('.') && name.contains(".tmp.")) {
            continue;
        }
        let stale = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > std::time::Duration::from_secs(60));
        if stale {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Result of scanning a manifest directory.
#[derive(Debug, Default)]
pub struct ManifestListing {
    pub manifests: Vec<ProjectManifest>,
    /// Files that could not be used, with the reason. Never a panic.
    pub skipped: Vec<(PathBuf, String)>,
}

/// Read every `*.toml` manifest in `manifests_dir` (sorted by id).
/// A missing directory yields an empty listing.
pub fn list_manifests(manifests_dir: &Path) -> Result<ManifestListing, ProjectError> {
    let mut out = ManifestListing::default();
    reap_orphans(manifests_dir);
    let rd = match std::fs::read_dir(manifests_dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(ProjectError::io(manifests_dir, e)),
    };
    for entry in rd {
        let path = entry
            .map_err(|e| ProjectError::io(manifests_dir, e))?
            .path();
        let is_toml = path.extension().and_then(|e| e.to_str()) == Some("toml");
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if !is_toml || stem.starts_with('.') {
            continue;
        }
        let loaded = std::fs::read_to_string(&path)
            .map_err(|e| ProjectError::io(&path, e))
            .and_then(|t| parse_manifest(&path, &t))
            .and_then(|m| {
                if m.id == stem {
                    Ok(m)
                } else {
                    Err(ProjectError::IdMismatch {
                        path: path.clone(),
                        id: m.id,
                    })
                }
            });
        match loaded {
            Ok(m) => out.manifests.push(m),
            Err(e) => out.skipped.push((path, e.to_string())),
        }
    }
    out.manifests.sort_by(|a, b| a.id.cmp(&b.id));
    out.skipped.sort();
    Ok(out)
}

/// Look a manifest up by id.
pub fn find_by_id(manifests_dir: &Path, id: &str) -> Result<Option<ProjectManifest>, ProjectError> {
    read_manifest(manifests_dir, id)
}

/// Look a manifest up by root. `root` is canonicalised when it exists,
/// otherwise compared as given (so `missing` entries stay findable).
pub fn find_by_root(
    manifests_dir: &Path,
    root: &Path,
) -> Result<Option<ProjectManifest>, ProjectError> {
    let want = canonical_lenient(root);
    Ok(list_manifests(manifests_dir)?
        .manifests
        .into_iter()
        .find(|m| m.root == want))
}

/// Walk up from `start` looking for `.weftos/project.toml`; returns the
/// project root. `stop_at` (typically `$HOME`, injected) is exclusive: it
/// and its ancestors are never examined.
pub fn find_project_toml(start: &Path, stop_at: Option<&Path>) -> Option<PathBuf> {
    let mut cur = Some(start);
    while let Some(dir) = cur {
        if stop_at.is_some_and(|s| s == dir) {
            return None;
        }
        if project_toml_path(dir).is_file() {
            return Some(dir.to_path_buf());
        }
        cur = dir.parent();
    }
    None
}
