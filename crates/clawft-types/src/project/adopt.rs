//! `adopt_or_init` and `reinit_fork`: give a directory a project identity.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, SubsecRound, Utc};

use super::schema::SCHEMA_VERSION;
use super::store::{
    create_project_toml, find_by_root, lock_manifests, read_manifest, read_project_toml,
    reap_orphans, write_manifest, write_project_toml,
};
use super::{
    LegacySection, ProjectError, ProjectManifest, ProjectState, ProjectToml, ProjectTomlPresence,
    SeedSection, ServeSection, new_id,
};

/// Serialises read-modify-write of manifests within this process. The
/// cross-process guard is the flock in `lock_manifests`; the arbiter for a
/// root's id is `project.toml` itself (see `create_project_toml`).
pub(super) static WRITE_LOCK: Mutex<()> = Mutex::new(());

pub(super) fn now() -> DateTime<Utc> {
    Utc::now().trunc_subsecs(0)
}

pub(super) fn default_name(root: &Path) -> String {
    root.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project")
        .to_string()
}

fn canonical_root(project_root: &Path) -> Result<PathBuf, ProjectError> {
    std::fs::canonicalize(project_root)
        .ok()
        .filter(|p| p.is_dir())
        .ok_or_else(|| ProjectError::BadRoot(project_root.to_path_buf()))
}

fn new_manifest(
    id: String,
    name: String,
    root: PathBuf,
    created: DateTime<Utc>,
) -> ProjectManifest {
    let stamp = now();
    ProjectManifest {
        schema_version: SCHEMA_VERSION,
        id,
        name,
        legacy: Some(LegacySection {
            runtime_dir: Some(root.join(".weftos").join("runtime")),
        }),
        root,
        state: ProjectState::Active,
        created,
        last_seen: stamp,
        project_toml: ProjectTomlPresence::Present,
        seed: Some(SeedSection {
            source: "project-init".into(),
            legacy_name: None,
        }),
        serve: Some(ServeSection::default()),
        chain: None,
        binary: None,
        extra: Default::default(),
    }
}

/// Ensure `project_root` has a `project.toml` and a manifest.
///
/// Id precedence: an existing `project.toml` wins; else a manifest already
/// registered for the canonical root (a seeded entry) is adopted; else a new
/// ULID is minted. `project.toml` is written first and atomically-exclusive:
/// if another process created it in the meantime its id is adopted, so
/// concurrent callers on one root always agree. An existing `project.toml`
/// is never rewritten. Calling again returns the same id and changes nothing.
///
/// Fails with [`ProjectError::RootConflict`] when the tree's id is already
/// registered for a different live root (a copy); see [`reinit_fork`].
pub fn adopt_or_init(
    project_root: &Path,
    manifests_dir: &Path,
    name: Option<&str>,
) -> Result<ProjectManifest, ProjectError> {
    let root = canonical_root(project_root)?;
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _flock = lock_manifests(manifests_dir)?;
    reap_orphans(manifests_dir);

    let seeded = find_by_root(manifests_dir, &root)?;
    let pt = match read_project_toml(&root)? {
        Some(pt) => pt,
        None => {
            let candidate = ProjectToml {
                schema_version: SCHEMA_VERSION,
                id: seeded.as_ref().map_or_else(new_id, |m| m.id.clone()),
                name: name
                    .map(str::to_string)
                    .or_else(|| seeded.as_ref().map(|m| m.name.clone()))
                    .unwrap_or_else(|| default_name(&root)),
                created: seeded.as_ref().map_or_else(now, |m| m.created),
                parent: None,
                governance: None,
                weave: None,
                extra: Default::default(),
            };
            if create_project_toml(&root, &candidate)? {
                candidate
            } else {
                // Lost the race: the winner's project.toml is the truth.
                read_project_toml(&root)?.ok_or_else(|| ProjectError::BadRoot(root.clone()))?
            }
        }
    };
    register(manifests_dir, &pt, root)
}

/// Make sure a manifest for `pt` exists, is active and points at `root`.
fn register(
    manifests_dir: &Path,
    pt: &ProjectToml,
    root: PathBuf,
) -> Result<ProjectManifest, ProjectError> {
    let Some(mut m) = read_manifest(manifests_dir, &pt.id)? else {
        let m = new_manifest(pt.id.clone(), pt.name.clone(), root, pt.created);
        write_manifest(manifests_dir, &m)?;
        return Ok(m);
    };
    if m.root != root && m.state != ProjectState::Missing && m.root.exists() {
        return Err(ProjectError::RootConflict {
            id: pt.id.clone(),
            root,
            existing: m.root,
        });
    }
    if m.root == root
        && m.state == ProjectState::Active
        && m.project_toml == ProjectTomlPresence::Present
    {
        return Ok(m);
    }
    m.root = root;
    m.state = ProjectState::Active;
    m.project_toml = ProjectTomlPresence::Present;
    m.last_seen = now();
    write_manifest(manifests_dir, &m)?;
    Ok(m)
}

/// Give a copied tree its own identity: mint a new ULID, rewrite
/// `project.toml` with it and `parent = <old id>`, and create its manifest.
///
/// This is the remedy for [`ProjectError::RootConflict`] (a clone, or a stale
/// copy left behind by a move). The original's manifest is never touched.
/// With no `project.toml` present it simply initialises a fresh identity.
pub fn reinit_fork(
    project_root: &Path,
    manifests_dir: &Path,
    name: Option<&str>,
) -> Result<ProjectManifest, ProjectError> {
    let root = canonical_root(project_root)?;
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _flock = lock_manifests(manifests_dir)?;
    reap_orphans(manifests_dir);

    let old = read_project_toml(&root)?;
    let mut pt = old.clone().unwrap_or_else(|| ProjectToml {
        schema_version: SCHEMA_VERSION,
        id: String::new(),
        name: default_name(&root),
        created: now(),
        parent: None,
        governance: None,
        weave: None,
        extra: Default::default(),
    });
    pt.parent = old.map(|o| o.id);
    pt.id = new_id();
    pt.created = now();
    if let Some(n) = name {
        pt.name = n.to_string();
    }
    write_project_toml(&root, &pt)?;
    let m = new_manifest(pt.id.clone(), pt.name.clone(), root, pt.created);
    write_manifest(manifests_dir, &m)?;
    Ok(m)
}
