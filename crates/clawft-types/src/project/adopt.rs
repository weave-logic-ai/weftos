//! `adopt_or_init`: give a directory a project identity, idempotently.

use std::path::Path;
use std::sync::Mutex;

use chrono::{DateTime, SubsecRound, Utc};

use super::schema::SCHEMA_VERSION;
use super::store::{
    find_by_root, read_manifest, read_project_toml, write_manifest, write_project_toml,
};
use super::{
    ProjectError, ProjectManifest, ProjectState, ProjectToml, ProjectTomlPresence, SeedSection,
    ServeSection, new_id,
};

/// Serialises read-modify-write of manifests within this process.
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

/// Ensure `project_root` has a `project.toml` and a manifest.
///
/// Id precedence: an existing `project.toml` wins; else a manifest already
/// registered for the canonical root (a seeded entry) is adopted; else a new
/// ULID is minted. An existing `project.toml` is never rewritten. Calling
/// again returns the same id and changes nothing.
pub fn adopt_or_init(
    project_root: &Path,
    manifests_dir: &Path,
    name: Option<&str>,
) -> Result<ProjectManifest, ProjectError> {
    let root = std::fs::canonicalize(project_root)
        .ok()
        .filter(|p| p.is_dir())
        .ok_or_else(|| ProjectError::BadRoot(project_root.to_path_buf()))?;
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let existing_toml = read_project_toml(&root)?;
    let seeded = find_by_root(manifests_dir, &root)?;
    let stamp = now();

    let (id, project_name) = match (&existing_toml, &seeded) {
        (Some(pt), _) => (pt.id.clone(), pt.name.clone()),
        (None, Some(m)) => (
            m.id.clone(),
            name.map_or_else(|| m.name.clone(), str::to_string),
        ),
        (None, None) => (
            new_id(),
            name.map_or_else(|| default_name(&root), str::to_string),
        ),
    };

    if existing_toml.is_none() {
        write_project_toml(
            &root,
            &ProjectToml {
                schema_version: SCHEMA_VERSION,
                id: id.clone(),
                name: project_name.clone(),
                created: seeded.as_ref().map_or(stamp, |m| m.created),
                parent: None,
                governance: None,
                weave: None,
                extra: Default::default(),
            },
        )?;
    }

    // The toml's id may already name a manifest for a different root.
    if let Some(m) = read_manifest(manifests_dir, &id)? {
        if m.root != root && m.state != ProjectState::Missing && m.root.exists() {
            return Err(ProjectError::RootConflict {
                id,
                existing: m.root,
            });
        }
        if m.root == root
            && m.state == ProjectState::Active
            && m.project_toml == ProjectTomlPresence::Present
        {
            return Ok(m);
        }
        let mut m = m;
        m.root = root;
        m.state = ProjectState::Active;
        m.project_toml = ProjectTomlPresence::Present;
        m.last_seen = stamp;
        write_manifest(manifests_dir, &m)?;
        return Ok(m);
    }

    let m = ProjectManifest {
        schema_version: SCHEMA_VERSION,
        id,
        name: project_name,
        legacy: Some(super::LegacySection {
            runtime_dir: Some(root.join(".weftos").join("runtime")),
        }),
        root,
        state: ProjectState::Active,
        created: stamp,
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
    };
    write_manifest(manifests_dir, &m)?;
    Ok(m)
}
