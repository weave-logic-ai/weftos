//! Non-git content of a project for `project.fetch` (ADR-108 P3b, owner
//! decision D-C): what the primary sends as a checksummed tar, what it never
//! sends, and how the member unpacks it.
//!
//! **What is sent**: regular files and directories under the project root that
//! are outside every git repository, plus, inside each repository of the
//! project, the paths git ignores (`git ls-files --others --ignored`): data,
//! models, build output. Untracked files that are not ignored stay out: they
//! are uncommitted source and belong in a commit, not in a second copy.
//!
//! **What is never sent** (nothing is silently skipped: counts come back in the
//! result):
//! - `.weftos/` anywhere (the project's identity, key, chain and certificate
//!   live there), the manifest's chain dir and runtime dir wherever they are,
//!   and `.git/`;
//! - paths listed in `<root>/.weftos/archive.toml` (`[[archive]] path, reason`):
//!   reported as `archived`;
//! - symbolic links, whatever they point at (a link out of the project is the
//!   classic exfiltration path);
//! - credential-shaped names: `.env*`, `*.env`, `.netrc`, `.git-credentials`, `.npmrc`,
//!   `.pypirc`, `.ssh/`, `.gnupg/`, `.aws/`, `id_rsa*`, `id_ed25519*`,
//!   `id_ecdsa*`, `*.pem`, `*.key`, `*.p12`, `*.pfx`, `*.jks`, `*.keystore`.
//!
//! Unmarked content over [`LARGE_BYTES`] is a warning the member sees before
//! anything is fetched (D-C: "warns before fetching").

use std::collections::BTreeSet;
use std::io::{BufReader, BufWriter};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::project_fetch_repos::{RepoEntry, git};

/// The archive list, under `<root>/.weftos/`.
pub const ARCHIVE_TOML: &str = "archive.toml";
/// Unmarked non-git content above this is "very large" (D-C warning).
pub const LARGE_BYTES: u64 = 256 * 1024 * 1024;
/// Most files one tar carries; the plan stops (and says so) past this.
pub const MAX_FILES: usize = 100_000;
const MAX_ARCHIVE_ENTRIES: usize = 64;
const MAX_ARCHIVE_BYTES: u64 = 64 * 1024;
const MAX_DEPTH: usize = 64;

#[derive(Debug, Deserialize)]
struct ArchiveToml {
    #[serde(default = "one")]
    version: u32,
    #[serde(default)]
    archive: Vec<ArchiveEntry>,
}

fn one() -> u32 {
    1
}

/// One path the project keeps on the primary only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveEntry {
    /// Relative to the project root, no `..`.
    pub path: String,
    #[serde(default)]
    pub reason: String,
}

/// Read and validate `<root>/.weftos/archive.toml`; absent means empty.
pub fn read_archive(root: &Path) -> Result<Vec<ArchiveEntry>, String> {
    let path = root.join(clawft_types::project::PROJECT_DIR).join(ARCHIVE_TOML);
    let Ok(meta) = std::fs::metadata(&path) else { return Ok(Vec::new()) };
    if meta.len() > MAX_ARCHIVE_BYTES {
        return Err(format!("{ARCHIVE_TOML} is too large"));
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{ARCHIVE_TOML}: {e}"))?;
    let t: ArchiveToml = toml::from_str(&text).map_err(|e| format!("{ARCHIVE_TOML}: {e}"))?;
    if t.version != 1 {
        return Err(format!("{ARCHIVE_TOML}: unsupported version {}", t.version));
    }
    if t.archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(format!("{ARCHIVE_TOML}: more than {MAX_ARCHIVE_ENTRIES} entries"));
    }
    t.archive
        .into_iter()
        .map(|mut e| {
            let p = e.path.trim().trim_end_matches('/');
            let rel = Path::new(p);
            if p.is_empty() || p.len() > 512 || rel.components().any(|c| !matches!(c, Component::Normal(_))) {
                return Err(format!("{ARCHIVE_TOML}: {:?} must be a relative path inside the project", e.path));
            }
            e.path = p.to_owned();
            e.reason = e.reason.chars().take(200).collect();
            Ok(e)
        })
        .collect()
}

/// What a tar would carry, before it is built.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// Directories (relative to the root), parents first.
    pub dirs: Vec<String>,
    /// Regular files (relative to the root) and their sizes.
    pub files: Vec<(String, u64)>,
    /// Total file bytes.
    pub bytes: u64,
    /// Archive entries that matched something (`path (reason)` strings).
    pub archived: Vec<String>,
    /// Symlinks and credential-shaped names left out.
    pub excluded: usize,
    /// [`MAX_FILES`] stopped the walk.
    pub truncated: bool,
}

impl Plan {
    /// Over the D-C threshold.
    pub fn is_large(&self) -> bool {
        self.bytes > LARGE_BYTES
    }

    /// The biggest top-level entries, for the warning text: `name (MiB)`.
    pub fn largest(&self, n: usize) -> Vec<String> {
        let mut by_top: std::collections::BTreeMap<&str, u64> = Default::default();
        for (p, b) in &self.files {
            *by_top.entry(p.split('/').next().unwrap_or(p)).or_default() += b;
        }
        let mut v: Vec<(&str, u64)> = by_top.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        v.into_iter().take(n).map(|(p, b)| format!("{p} ({} MiB)", b >> 20)).collect()
    }
}

/// Everything the walk needs to decide what stays home.
pub struct Exclusions<'a> {
    pub root: &'a Path,
    pub archive: &'a [ArchiveEntry],
    pub repos: &'a [RepoEntry],
    /// The manifest's chain dir and runtime dir (absolute).
    pub secret_dirs: &'a [PathBuf],
}

/// A credential-shaped file or directory name.
pub fn secret_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with(".env")
        || lower.ends_with(".env")
        || matches!(lower.as_str(), ".netrc" | ".git-credentials" | ".npmrc" | ".pypirc" | ".ssh" | ".gnupg" | ".aws")
        || lower.starts_with("id_rsa")
        || lower.starts_with("id_ed25519")
        || lower.starts_with("id_ecdsa")
        || [".pem", ".key", ".p12", ".pfx", ".jks", ".keystore"].iter().any(|s| lower.ends_with(s))
}

impl Exclusions<'_> {
    fn archived(&self, rel: &str) -> Option<&ArchiveEntry> {
        self.archive.iter().find(|a| rel == a.path || rel.starts_with(&format!("{}/", a.path)))
    }

    fn secret_dir(&self, full: &Path) -> bool {
        self.secret_dirs.iter().any(|d| full.starts_with(d))
    }

    fn is_repo(&self, full: &Path) -> bool {
        self.repos.iter().any(|r| r.path == full) || std::fs::symlink_metadata(full.join(".git")).is_ok()
    }
}

/// Decide the fate of one path. `Ok(true)` means walk/add it.
fn admit(ex: &Exclusions<'_>, plan: &mut Plan, full: &Path, rel: &str) -> bool {
    let name = full.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name == clawft_types::project::PROJECT_DIR || name == ".git" || ex.secret_dir(full) {
        return false;
    }
    if ex.archived(rel).is_some() {
        return false;
    }
    let Ok(meta) = std::fs::symlink_metadata(full) else { return false };
    if meta.file_type().is_symlink() || secret_name(name) {
        plan.excluded += 1;
        return false;
    }
    true
}

fn walk(ex: &Exclusions<'_>, plan: &mut Plan, dir: &Path, rel: &str, depth: usize, in_repo_ignored: bool) {
    if depth > MAX_DEPTH || plan.truncated {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut names: Vec<String> = rd.filter_map(Result::ok).filter_map(|e| e.file_name().into_string().ok()).collect();
    names.sort();
    for name in names {
        if plan.files.len() >= MAX_FILES {
            plan.truncated = true;
            return;
        }
        let full = dir.join(&name);
        let child = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        if !admit(ex, plan, &full, &child) {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(&full) else { continue };
        if meta.is_dir() {
            // A repository inside non-git content is fetched as a repository
            // (or not at all); its ignored files are added by the repo pass.
            if !in_repo_ignored && ex.is_repo(&full) {
                continue;
            }
            plan.dirs.push(child.clone());
            walk(ex, plan, &full, &child, depth + 1, in_repo_ignored);
        } else if meta.is_file() {
            plan.bytes += meta.len();
            plan.files.push((child, meta.len()));
        }
    }
}

/// Plan the non-git content of the project at `ex.root`.
pub fn plan(ex: &Exclusions<'_>) -> Plan {
    let mut plan = Plan::default();
    // A root that is itself a repository has no files outside git; only its
    // ignored paths (below) count.
    if !ex.is_repo(ex.root) {
        walk(ex, &mut plan, ex.root, "", 0, false);
    }
    for repo in ex.repos.iter().filter(|r| r.path.starts_with(ex.root)) {
        let Ok(out) = git(&repo.path, &["ls-files", "-z", "--others", "--ignored", "--exclude-standard", "--directory"], None, Duration::from_secs(60)) else {
            continue;
        };
        let mut ignored: Vec<&str> = out.split(|b| *b == 0).filter_map(|s| std::str::from_utf8(s).ok()).filter(|s| !s.is_empty()).collect();
        ignored.sort_unstable();
        for p in ignored {
            if plan.files.len() >= MAX_FILES {
                plan.truncated = true;
                break;
            }
            let p = p.trim_end_matches('/');
            let full = repo.path.join(p);
            let Ok(relp) = full.strip_prefix(ex.root) else { continue };
            let Some(rel) = relp.to_str().map(str::to_owned) else { continue };
            if relp.components().any(|c| !matches!(c, Component::Normal(_))) || !admit(ex, &mut plan, &full, &rel) {
                continue;
            }
            let Ok(meta) = std::fs::symlink_metadata(&full) else { continue };
            if meta.is_dir() {
                plan.dirs.push(rel.clone());
                walk(ex, &mut plan, &full, &rel, 1, true);
            } else if meta.is_file() {
                plan.bytes += meta.len();
                plan.files.push((rel, meta.len()));
            }
        }
    }
    // Archived paths are listed whenever they exist, whatever git thinks of them.
    for a in ex.archive.iter().filter(|a| std::fs::symlink_metadata(ex.root.join(&a.path)).is_ok()) {
        let s = if a.reason.is_empty() { a.path.clone() } else { format!("{} ({})", a.path, a.reason) };
        if !plan.archived.contains(&s) {
            plan.archived.push(s);
        }
    }
    plan.dirs = BTreeSet::from_iter(plan.dirs.drain(..)).into_iter().collect();
    plan.files.sort();
    plan
}

/// Write the planned content of `root` to the tar at `out`; returns its size.
pub fn build_tar(root: &Path, plan: &Plan, out: &Path) -> Result<u64, String> {
    let file = std::fs::File::create(out).map_err(|e| format!("tar: {e}"))?;
    let mut b = tar::Builder::new(BufWriter::new(file));
    b.follow_symlinks(false);
    for d in &plan.dirs {
        b.append_dir(d, root.join(d)).map_err(|e| format!("tar {d}: {e}"))?;
    }
    for (f, _) in &plan.files {
        // A file swapped for a link since the plan is written as a link entry,
        // which the member refuses.
        b.append_path_with_name(root.join(f), f).map_err(|e| format!("tar {f}: {e}"))?;
    }
    let w = b.into_inner().map_err(|e| format!("tar: {e}"))?;
    let f = w.into_inner().map_err(|e| format!("tar: {e}"))?;
    f.sync_all().map_err(|e| format!("tar: {e}"))?;
    Ok(f.metadata().map_err(|e| format!("tar: {e}"))?.len())
}

/// What an unpack did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Unpacked {
    pub files: usize,
    pub bytes: u64,
    /// Entries refused (links, devices, paths that leave `dest`).
    pub refused: usize,
}

/// Unpack the tar at `file` under `dest`: regular files and directories only,
/// each path checked to stay inside `dest`; nothing existing is overwritten.
pub fn unpack_tar(file: &Path, dest: &Path) -> Result<Unpacked, String> {
    let f = std::fs::File::open(file).map_err(|e| format!("tar: {e}"))?;
    let mut a = tar::Archive::new(BufReader::new(f));
    a.set_preserve_permissions(true);
    a.set_overwrite(false);
    a.set_unpack_xattrs(false);
    let mut out = Unpacked::default();
    for entry in a.entries().map_err(|e| format!("tar: {e}"))? {
        let mut entry = entry.map_err(|e| format!("tar: {e}"))?;
        let kind = entry.header().entry_type();
        let path_ok = entry
            .path()
            .map(|p| p.components().all(|c| matches!(c, Component::Normal(_))))
            .unwrap_or(false);
        if !(kind.is_file() || kind.is_dir()) || !path_ok {
            out.refused += 1;
            continue;
        }
        let size = entry.header().size().unwrap_or(0);
        match entry.unpack_in(dest) {
            Ok(true) if kind.is_file() => {
                out.files += 1;
                out.bytes += size;
            }
            Ok(_) => {}
            Err(e) => return Err(format!("tar: {e}")),
        }
    }
    Ok(out)
}

#[cfg(test)]
#[path = "project_fetch_tar_tests.rs"]
mod tests;
