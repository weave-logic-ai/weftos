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

/// Register `project_root` as an ADR-108 workspace of the existing project
/// `id` (its primary lives on another machine). Writes `project.toml` and a
/// manifest carrying `role = "workspace"`, and nothing else: no key, chain,
/// certificate, runtime dir or `[serve]` section, so no kernel is ever
/// served for it here.
///
/// Idempotent for the same id at the same root. Refuses when the tree's
/// `project.toml` names another project, when a manifest for this root is
/// another project, or ([`ProjectError::RootConflict`]) when `id` is already
/// registered at a different live root on this machine.
pub fn adopt_workspace(
    project_root: &Path,
    manifests_dir: &Path,
    id: &str,
    name: Option<&str>,
    repos: &[PathBuf],
) -> Result<ProjectManifest, ProjectError> {
    super::validate_id(id)?;
    let root = canonical_root(project_root)?;
    let refused = |reason: &str| ProjectError::AdoptRefused {
        id: id.to_owned(),
        root: root.clone(),
        reason: reason.to_owned(),
    };
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _flock = lock_manifests(manifests_dir)?;
    reap_orphans(manifests_dir);

    if let Some(seeded) = find_by_root(manifests_dir, &root)?
        && seeded.id != id
    {
        return Err(refused(&format!("this root is already registered as project {}", seeded.id)));
    }
    if let Some(r) = repos.iter().find(|r| !r.is_absolute() || r.starts_with(&root)) {
        return Err(refused(&format!("extra repository {} must be an absolute path outside the root", r.display())));
    }
    let role = || {
        let mut t = toml::Table::new();
        t.insert("role".into(), toml::Value::String(super::WORKSPACE_ROLE.into()));
        t
    };
    let pt = match read_project_toml(&root)? {
        Some(pt) if pt.id != id => {
            return Err(refused(&format!("its project.toml already names project {}", pt.id)));
        }
        Some(pt) if !pt.is_workspace() => {
            return Err(refused("its project.toml is this project's primary identity, not a workspace"));
        }
        Some(pt) => pt,
        None => {
            let candidate = ProjectToml {
                schema_version: SCHEMA_VERSION,
                id: id.to_owned(),
                name: name.map_or_else(|| default_name(&root), str::to_string),
                created: now(),
                parent: None,
                governance: None,
                weave: None,
                extra: role(),
            };
            if !create_project_toml(&root, &candidate)? {
                return Err(refused("another process created project.toml first"));
            }
            candidate
        }
    };
    let mut extra_repos: Vec<String> = Vec::new();
    if let Some(m) = read_manifest(manifests_dir, id)? {
        if m.root != root && m.state != ProjectState::Missing && m.root.exists() {
            return Err(ProjectError::RootConflict { id: id.to_owned(), root, existing: m.root });
        }
        extra_repos = m.workspace_repos().iter().map(|p| p.to_string_lossy().into_owned()).collect();
        let adds_nothing = repos.iter().all(|r| extra_repos.iter().any(|e| Path::new(e) == r));
        if m.root == root && m.is_workspace() && m.state == ProjectState::Active && adds_nothing {
            return Ok(m);
        }
    }
    for r in repos {
        let s = r.to_string_lossy().into_owned();
        if !extra_repos.contains(&s) {
            extra_repos.push(s);
        }
    }
    let m = ProjectManifest {
        schema_version: SCHEMA_VERSION,
        id: pt.id.clone(),
        name: pt.name.clone(),
        root,
        state: ProjectState::Active,
        created: pt.created,
        last_seen: now(),
        project_toml: ProjectTomlPresence::Present,
        seed: Some(SeedSection { source: "project-adopt".into(), legacy_name: None }),
        legacy: None,
        serve: None,
        chain: None,
        binary: None,
        extra: {
            let mut t = role();
            if !extra_repos.is_empty() {
                t.insert(
                    "repos".into(),
                    toml::Value::Array(extra_repos.into_iter().map(toml::Value::String).collect()),
                );
            }
            t
        },
    };
    write_manifest(manifests_dir, &m)?;
    Ok(m)
}

/// Register an existing child identity under a registered active master.
/// All manifest validation and the write use the same cross-process store
/// lock; a different project.toml id cannot be adopted between validation
/// and registration. This never mints an identity or rewrites project.toml.
pub fn register_existing_nested(
    child_root: &Path,
    manifests_dir: &Path,
    master_id: &str,
) -> Result<ProjectManifest, ProjectError> {
    super::validate_id(master_id)?;
    let root = canonical_root(child_root)?;
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _flock = lock_manifests(manifests_dir)?;
    let master = read_manifest(manifests_dir, master_id)?
        .ok_or_else(|| ProjectError::NestedRefused("master is not registered".into()))?;
    if master.state != ProjectState::Active {
        return Err(ProjectError::NestedRefused("master is not active".into()));
    }
    let parent_root = canonical_root(&master.root)?;
    if root == parent_root || !root.starts_with(&parent_root) {
        return Err(ProjectError::NestedRefused("child root is outside the master".into()));
    }
    let parent_toml = read_project_toml(&parent_root)?
        .ok_or_else(|| ProjectError::NestedRefused("master has no project.toml".into()))?;
    if parent_toml.id != master_id || !parent_toml.is_weave_master() {
        return Err(ProjectError::NestedRefused("master identity or weave.master changed".into()));
    }
    let child_toml = read_project_toml(&root)?
        .ok_or_else(|| ProjectError::NestedRefused("child has no project.toml".into()))?;
    if child_toml.id == master_id || child_toml.parent.as_deref() != Some(master_id) {
        return Err(ProjectError::NestedRefused("child does not name this master".into()));
    }
    register(manifests_dir, &child_toml, root)
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
///
/// Refuses ([`ProjectError::RegisteredHome`]) when the tree's current id is
/// registered with a manifest whose root is this very tree, since that is the
/// original, not a copy. With `force` that manifest is archived first so
/// `find_by_root` stays unambiguous.
pub fn reinit_fork(
    project_root: &Path,
    manifests_dir: &Path,
    name: Option<&str>,
    force: bool,
) -> Result<ProjectManifest, ProjectError> {
    let root = canonical_root(project_root)?;
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _flock = lock_manifests(manifests_dir)?;
    reap_orphans(manifests_dir);

    let old = read_project_toml(&root)?;
    if let Some(o) = &old
        && let Some(mut home) = read_manifest(manifests_dir, &o.id)?
        && home.root == root
        && home.state != ProjectState::Archived
    {
        if !force {
            return Err(ProjectError::RegisteredHome {
                id: o.id.clone(),
                root,
            });
        }
        home.state = ProjectState::Archived;
        write_manifest(manifests_dir, &home)?;
    }
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
