//! Seed manifests from the legacy `~/.clawft/workspaces.json` registry.
//!
//! Never modifies the source registry and never writes into a project tree.

use std::path::{Path, PathBuf};

use chrono::SubsecRound;

use crate::workspace::{WorkspaceEntry, WorkspaceRegistry};

use super::adopt::{WRITE_LOCK, default_name, now};
use super::schema::SCHEMA_VERSION;
use super::store::{find_by_root, read_manifest, read_project_toml, write_manifest};
use super::{
    LegacySection, ProjectError, ProjectManifest, ProjectState, ProjectTomlPresence, SeedSection,
    ServeSection, new_id,
};

/// What a seeding pass did. Ids are ULIDs; `skipped` carries a reason.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SeedReport {
    /// New manifests with a freshly minted id (`project_toml = pending`).
    pub created: Vec<String>,
    /// Manifests created reusing an id from an existing `project.toml`, or
    /// an existing manifest re-activated.
    pub adopted: Vec<String>,
    /// Entries whose path is gone; marked `state = missing`, never deleted.
    pub missing: Vec<String>,
    /// Entries already seeded and unchanged (what makes a re-run a no-op).
    pub unchanged: Vec<String>,
    /// Entries that could not be seeded: `(path, reason)`.
    pub skipped: Vec<(PathBuf, String)>,
}

/// Seed from a `workspaces.json` file. A missing file is an empty registry.
pub fn seed_from_workspaces(
    workspaces_json: &Path,
    manifests_dir: &Path,
) -> Result<SeedReport, ProjectError> {
    let registry = WorkspaceRegistry::load(workspaces_json).map_err(|e| ProjectError::Parse {
        path: workspaces_json.to_path_buf(),
        message: e.to_string(),
    })?;
    seed_from_registry(&registry, manifests_dir)
}

/// Seed from an already-loaded registry. Idempotent; match is by canonical
/// root path.
pub fn seed_from_registry(
    registry: &WorkspaceRegistry,
    manifests_dir: &Path,
) -> Result<SeedReport, ProjectError> {
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut report = SeedReport::default();
    for entry in &registry.workspaces {
        if let Err(e) = seed_entry(entry, manifests_dir, &mut report) {
            report.skipped.push((entry.path.clone(), e.to_string()));
        }
    }
    Ok(report)
}

/// Canonicalise even when the leaf is gone: resolve the deepest existing
/// ancestor and re-append the rest, so `/tmp/x` and `/private/tmp/x` agree.
fn canonical_lenient(path: &Path) -> PathBuf {
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

fn seed_entry(
    entry: &WorkspaceEntry,
    dir: &Path,
    report: &mut SeedReport,
) -> Result<(), ProjectError> {
    let root = canonical_lenient(&entry.path);
    let exists = root.is_dir();
    let known = find_by_root(dir, &root)?;

    if let Some(mut m) = known {
        let want = if exists {
            ProjectState::Active
        } else {
            ProjectState::Missing
        };
        if m.state == ProjectState::Archived || m.state == want {
            report.unchanged.push(m.id);
            return Ok(());
        }
        m.state = want;
        write_manifest(dir, &m)?;
        if exists {
            report.adopted.push(m.id)
        } else {
            report.missing.push(m.id)
        }
        return Ok(());
    }

    let stamp = now();
    let created = entry.created_at.map_or(stamp, |t| t.trunc_subsecs(0));
    let last_seen = entry.last_accessed.map_or(created, |t| t.trunc_subsecs(0));
    let (id, name, presence, reused) = match read_project_toml(&root)?.filter(|_| exists) {
        Some(pt) => (pt.id, pt.name, ProjectTomlPresence::Present, true),
        None => {
            let name = if entry.name.is_empty() {
                default_name(&root)
            } else {
                entry.name.clone()
            };
            (new_id(), name, ProjectTomlPresence::Pending, false)
        }
    };
    if let Some(other) = read_manifest(dir, &id)? {
        return Err(ProjectError::RootConflict {
            id,
            existing: other.root,
        });
    }
    let m = ProjectManifest {
        schema_version: SCHEMA_VERSION,
        id: id.clone(),
        name,
        legacy: Some(LegacySection {
            runtime_dir: Some(root.join(".weftos").join("runtime")),
        }),
        root,
        state: if exists {
            ProjectState::Active
        } else {
            ProjectState::Missing
        },
        created,
        last_seen,
        project_toml: presence,
        seed: Some(SeedSection {
            source: "workspaces.json".into(),
            legacy_name: Some(entry.name.clone()),
        }),
        serve: Some(ServeSection::default()),
        chain: None,
        binary: None,
        extra: Default::default(),
    };
    write_manifest(dir, &m)?;
    match (exists, reused) {
        (false, _) => report.missing.push(id),
        (true, true) => report.adopted.push(id),
        (true, false) => report.created.push(id),
    }
    Ok(())
}
