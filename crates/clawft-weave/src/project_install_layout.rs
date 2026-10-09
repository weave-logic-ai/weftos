//! Where an install's repositories land, and undoing a half-finished one
//! (ADR-108 P3a; the "Layout" paragraph of `docs/plans/adr-108-p2b-p3-contract.md`).
//!
//! - the `.` source goes into `target_path`;
//! - a sibling `dir` goes into `<parent of target_path>/<dir>`;
//! - with no `.` source, every repository goes into `<target_path>/<dir>` and
//!   `target_path` itself is a plain directory (the workspace root).
//!
//! Every destination must be absent or an empty directory before anything is
//! written. [`Rollback`] remembers what existed beforehand so a failure removes
//! only what the install created.

use std::path::{Path, PathBuf};

use crate::project_install::{InstallRequest, SourceRepo};

/// One repository and the directory it is cloned into.
#[derive(Debug, Clone)]
pub struct Placement<'a> {
    pub source: &'a SourceRepo,
    pub dest: PathBuf,
}

/// Destinations for every source of `req` under `target`.
pub fn plan<'a>(req: &'a InstallRequest, target: &Path) -> Result<Vec<Placement<'a>>, String> {
    let has_root = req.sources.iter().any(|s| s.dir == ".");
    let parent = target.parent().ok_or("target path has no parent directory")?;
    let placements: Vec<Placement> = req
        .sources
        .iter()
        .map(|s| {
            let dest = match (s.dir.as_str(), has_root) {
                (".", _) => target.to_path_buf(),
                (d, true) => parent.join(d),
                (d, false) => target.join(d),
            };
            Placement { source: s, dest }
        })
        .collect();
    for (i, p) in placements.iter().enumerate() {
        if placements[..i].iter().any(|q| q.dest == p.dest) {
            return Err(format!("source {i}: its directory collides with another repository's destination"));
        }
    }
    Ok(placements)
}

/// Every destination, and `target` itself, must be absent or an empty directory.
pub fn check_free(placements: &[Placement], target: &Path) -> Result<(), String> {
    for dir in std::iter::once(target).chain(placements.iter().map(|p| p.dest.as_path())) {
        match std::fs::read_dir(dir) {
            Ok(mut rd) => {
                if rd.next().is_some() {
                    return Err(format!("{} already exists and is not empty", dir.display()));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // A dangling symlink reads as NotFound but is not absent.
                if std::fs::symlink_metadata(dir).is_ok() {
                    return Err(format!("{} exists and is not a directory", dir.display()));
                }
            }
            Err(_) => return Err(format!("{} exists and is not a directory", dir.display())),
        }
    }
    Ok(())
}

/// What existed before an install started; undoes only what it added.
#[derive(Debug, Default)]
pub struct Rollback {
    /// Destinations that did not exist (removed whole).
    created: Vec<PathBuf>,
    /// Destinations that were empty directories (emptied, kept).
    emptied: Vec<PathBuf>,
    /// Ancestors that did not exist, shallowest first (removed if left empty).
    ancestors: Vec<PathBuf>,
}

impl Rollback {
    /// Snapshot before writing. Call after [`check_free`].
    pub fn snapshot(placements: &[Placement], target: &Path) -> Self {
        let mut r = Rollback::default();
        let mut dests: Vec<&Path> = placements.iter().map(|p| p.dest.as_path()).collect();
        if !dests.contains(&target) {
            dests.push(target);
        }
        for d in dests {
            if d.exists() {
                r.emptied.push(d.to_path_buf());
            } else {
                r.created.push(d.to_path_buf());
                let mut missing = Vec::new();
                let mut cur = d.parent();
                while let Some(p) = cur.filter(|p| !p.exists()) {
                    missing.push(p.to_path_buf());
                    cur = p.parent();
                }
                for m in missing.into_iter().rev() {
                    if !r.ancestors.contains(&m) {
                        r.ancestors.push(m);
                    }
                }
            }
        }
        r
    }

    /// Remove what the install created. Idempotent; best effort.
    pub fn undo(&self) {
        for d in &self.created {
            let _ = std::fs::remove_dir_all(d);
        }
        for d in &self.emptied {
            if let Ok(rd) = std::fs::read_dir(d) {
                for e in rd.flatten() {
                    let p = e.path();
                    if e.file_type().is_ok_and(|t| t.is_dir()) {
                        let _ = std::fs::remove_dir_all(&p);
                    } else {
                        let _ = std::fs::remove_file(&p);
                    }
                }
            }
        }
        for a in self.ancestors.iter().rev() {
            let _ = std::fs::remove_dir(a);
        }
    }
}
