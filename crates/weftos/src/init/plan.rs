//! `weftos init --claude|--grok|--codex`: select packages, render per host,
//! and diff against disk and the lock into a change plan. Writing happens in
//! [`super::apply`]; `--plan` stops here.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use super::InitError;
use super::apply::{extract_block, read_text};
use super::lock::{LOCK_PATH, Lock, LockAgent, LockFile, RENDERER, SCHEMA, sha256_hex};
use super::package::{Package, team_members};
use super::render::{Host, Layout, Merge, Rendered, project_context_template, render_host};
use super::source::AgentSource;

pub const DEFAULT_TEAM: &str = "weftos-core";
pub const CONTEXT_PATH: &str = ".agents/project-context.md";

#[derive(Clone, Debug, Default)]
pub struct AgentInitOptions {
    /// Target root: the project, or `$HOME` with `global`.
    pub root: PathBuf,
    pub hosts: Vec<Host>,
    pub apply: bool,
    pub force: bool,
    pub global: bool,
    pub from: Option<PathBuf>,
    pub team: Option<String>,
    pub agents: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Action {
    Create,
    Update,
    Unchanged,
    /// On-disk hash differs from the lock; kept unless `--force`.
    Drifted,
    /// File exists but WeftOS never wrote it; kept unless `--force`.
    Unmanaged,
    /// Drifted or unmanaged, overwritten because of `--force`.
    Overwrite,
    /// No longer rendered and unmodified since WeftOS wrote it.
    Delete,
    /// No longer rendered but locally modified: left in place, released from the lock.
    Release,
    /// Project context seeded (absent before).
    Seed,
    /// Project context exists: never overwritten.
    Keep,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Action::Create => "create",
            Action::Update => "update",
            Action::Unchanged => "unchanged",
            Action::Drifted => "drifted",
            Action::Unmanaged => "unmanaged",
            Action::Overwrite => "overwrite",
            Action::Delete => "delete",
            Action::Release => "release",
            Action::Seed => "create",
            Action::Keep => "keep",
        }
    }

    /// Whether apply touches the filesystem for this item.
    pub fn writes(self) -> bool {
        matches!(
            self,
            Action::Create | Action::Update | Action::Overwrite | Action::Delete | Action::Seed
        )
    }
}

#[derive(Clone, Debug)]
pub struct PlanItem {
    pub action: Action,
    pub path: String,
    pub host: Option<Host>,
    pub owner: String,
    pub merge: Merge,
    /// Bytes to write (empty for delete/keep items).
    pub bytes: Vec<u8>,
}

#[derive(Debug)]
pub struct Plan {
    pub root: PathBuf,
    pub source: String,
    pub commit: String,
    pub team: Option<String>,
    pub selected: Vec<String>,
    pub warnings: Vec<String>,
    pub items: Vec<PlanItem>,
    pub lock_json: String,
    pub lock_changed: bool,
}

impl Plan {
    pub fn count(&self, a: Action) -> usize {
        self.items.iter().filter(|i| i.action == a).count()
    }

    /// True when apply would write nothing.
    pub fn is_noop(&self) -> bool {
        !self.lock_changed && !self.items.iter().any(|i| i.action.writes())
    }
}

/// Reject absolute paths and `..` so nothing lands outside the target root.
pub fn safe_rel(path: &str) -> Result<(), InitError> {
    let p = Path::new(path);
    let ok = !path.is_empty() && p.components().all(|c| matches!(c, Component::Normal(_)));
    if ok {
        Ok(())
    } else {
        Err(InitError::Other(format!("unsafe path in render: {path}")))
    }
}

fn load_source(opts: &AgentInitOptions) -> Result<AgentSource, InitError> {
    let src = match &opts.from {
        Some(dir) => AgentSource::from_dir(dir)?,
        None => AgentSource::embedded(),
    };
    if src.package_ids().is_empty() {
        return Err(InitError::Other(format!(
            "no agent packages in source ({}); pass --from <weftos>/agents",
            src.origin
        )));
    }
    Ok(src)
}

/// Package ids requested by `--agent` / `--team` (before lock carry-over).
fn select(
    src: &AgentSource,
    opts: &AgentInitOptions,
    warnings: &mut Vec<String>,
) -> Result<(Vec<String>, Option<String>), InitError> {
    let known = src.package_ids();
    if !opts.agents.is_empty() {
        for id in &opts.agents {
            if !known.contains(id) {
                return Err(InitError::Other(format!(
                    "unknown agent '{id}' (known: {})",
                    known.join(", ")
                )));
            }
        }
        return Ok((opts.agents.clone(), opts.team.clone()));
    }
    let team = opts
        .team
        .clone()
        .unwrap_or_else(|| DEFAULT_TEAM.to_string());
    match team_members(src, &team)? {
        Some(members) => {
            let (ok, missing): (Vec<_>, Vec<_>) = members
                .active
                .into_iter()
                .partition(|id| known.contains(id));
            for id in missing {
                warnings.push(format!(
                    "team {team}: '{id}' is not a package in the source; skipped"
                ));
            }
            if !members.inactive.is_empty() {
                warnings.push(format!(
                    "team {team}: inactive members not installed (add with --agent): {}",
                    members.inactive.join(", ")
                ));
            }
            Ok((ok, Some(team)))
        }
        None => {
            warnings.push(format!(
                "team '{team}' not found (agents/teams/{team}/team.yaml); installing all packages"
            ));
            let mut all = Vec::new();
            for id in known {
                if Package::load(src, &id)?.kind != "template" {
                    all.push(id);
                }
            }
            Ok((all, Some(team)))
        }
    }
}

/// Current on-disk content of a managed target (whole file or marked block).
fn on_disk(root: &Path, r: &Rendered) -> Result<Option<Vec<u8>>, InitError> {
    match r.merge {
        Merge::File => {
            let p = root.join(&r.path);
            if p.is_file() {
                Ok(Some(std::fs::read(p)?))
            } else {
                Ok(None)
            }
        }
        Merge::Block => Ok(read_text(&root.join(&r.path))?
            .and_then(|t| extract_block(&t).map(|b| b.as_bytes().to_vec()))),
    }
}

pub fn plan(opts: &AgentInitOptions) -> Result<Plan, InitError> {
    let src = load_source(opts)?;
    let mut warnings = Vec::new();
    let (base, team) = select(&src, opts, &mut warnings)?;
    let prior = Lock::read(&opts.root)?;
    let prior_lock = prior.clone().unwrap_or_else(Lock::empty);
    let prior_files = prior_lock.files_by_path();
    let known = src.package_ids();
    let layout = Layout {
        global: opts.global,
    };

    let mut cache: BTreeMap<String, Package> = BTreeMap::new();
    let mut rendered: BTreeMap<String, Rendered> = BTreeMap::new();
    let mut selected: BTreeSet<String> = BTreeSet::new();
    for &host in &opts.hosts {
        // Installs are cumulative per host: agents already in the lock stay.
        let mut ids: Vec<String> = base.clone();
        for id in prior_lock.agents_for(host) {
            if known.contains(&id) && !ids.contains(&id) {
                ids.push(id);
            }
        }
        ids.sort();
        let mut pkgs = Vec::new();
        for id in &ids {
            if !cache.contains_key(id) {
                cache.insert(id.clone(), Package::load(&src, id)?);
            }
            selected.insert(id.clone());
            pkgs.push(cache.remove(id).expect("cached"));
        }
        for r in render_host(host, &pkgs, layout) {
            safe_rel(&r.path)?;
            if let Some(prev) = rendered.get(&r.path)
                && prev.owner != r.owner
            {
                return Err(InitError::Other(format!(
                    "{} is rendered by both '{}' and '{}'",
                    r.path, prev.owner, r.owner
                )));
            }
            rendered.insert(r.path.clone(), r);
        }
        for p in pkgs {
            cache.insert(p.id.clone(), p);
        }
    }

    let mut items = Vec::new();
    for r in rendered.values() {
        let disk = on_disk(&opts.root, r)?;
        let lock_hash = prior_files
            .get(r.path.as_str())
            .map(|(_, f)| f.sha256.as_str());
        let action = match (&disk, lock_hash) {
            (None, _) => Action::Create,
            (Some(d), _) if *d == r.bytes => Action::Unchanged,
            (Some(d), Some(h)) if sha256_hex(d) == h => Action::Update,
            (Some(_), Some(_)) if opts.force => Action::Overwrite,
            (Some(_), Some(_)) => Action::Drifted,
            (Some(_), None) if opts.force => Action::Overwrite,
            (Some(_), None) => Action::Unmanaged,
        };
        items.push(PlanItem {
            action,
            path: r.path.clone(),
            host: Some(r.host),
            owner: r.owner.clone(),
            merge: r.merge,
            bytes: r.bytes.clone(),
        });
    }

    // Stale: managed for a host in this run but no longer rendered.
    for (path, (owner, f)) in &prior_files {
        if !opts.hosts.contains(&f.host) || rendered.contains_key(*path) {
            continue;
        }
        safe_rel(path)?;
        let probe = Rendered {
            path: path.to_string(),
            bytes: Vec::new(),
            host: f.host,
            owner: owner.to_string(),
            merge: f.merge,
        };
        let Some(disk) = on_disk(&opts.root, &probe)? else {
            continue;
        };
        let action = if sha256_hex(&disk) == f.sha256 {
            Action::Delete
        } else {
            Action::Release
        };
        items.push(PlanItem {
            action,
            path: path.to_string(),
            host: Some(f.host),
            owner: owner.to_string(),
            merge: f.merge,
            bytes: Vec::new(),
        });
    }

    if !opts.global {
        let exists = opts.root.join(CONTEXT_PATH).exists();
        let template = project_context_template(selected.iter().filter_map(|id| cache.get(id)));
        items.push(PlanItem {
            action: if exists { Action::Keep } else { Action::Seed },
            path: CONTEXT_PATH.into(),
            host: None,
            owner: "project".into(),
            merge: Merge::File,
            bytes: if exists {
                Vec::new()
            } else {
                template.into_bytes()
            },
        });
    }

    let lock = next_lock(
        opts,
        &src,
        team.clone(),
        prior.as_ref(),
        &prior_lock,
        &items,
    );
    let lock_json = lock.to_json();
    let current = read_text(&opts.root.join(LOCK_PATH))?;
    let lock_changed = current.as_deref() != Some(lock_json.as_str());

    Ok(Plan {
        root: opts.root.clone(),
        source: src.origin.clone(),
        commit: src.commit.clone(),
        team,
        selected: selected.into_iter().collect(),
        warnings,
        items,
        lock_json,
        lock_changed,
    })
}

/// The lock after this run: entries for hosts not in the run are kept; a
/// drifted file keeps its old hash so it stays reported as drifted.
fn next_lock(
    opts: &AgentInitOptions,
    src: &AgentSource,
    team: Option<String>,
    prior: Option<&Lock>,
    prior_lock: &Lock,
    items: &[PlanItem],
) -> Lock {
    let mut agents: BTreeMap<String, LockAgent> = BTreeMap::new();
    for a in &prior_lock.agents {
        let mut kept = a.clone();
        kept.files.retain(|f| !opts.hosts.contains(&f.host));
        agents.insert(a.id.clone(), kept);
    }
    let prior_files = prior_lock.files_by_path();
    for it in items {
        let Some(host) = it.host else { continue };
        let entry = match it.action {
            Action::Create | Action::Update | Action::Unchanged | Action::Overwrite => {
                Some(sha256_hex(&it.bytes))
            }
            Action::Drifted => prior_files
                .get(it.path.as_str())
                .map(|(_, f)| f.sha256.clone()),
            _ => None,
        };
        let Some(sha256) = entry else { continue };
        let agent = agents.entry(it.owner.clone()).or_insert_with(|| LockAgent {
            id: it.owner.clone(),
            ..Default::default()
        });
        if it.action != Action::Drifted {
            agent.weftos_version = crate::VERSION.to_string();
            agent.commit = src.commit.clone();
        }
        agent.files.push(LockFile {
            path: it.path.clone(),
            host,
            sha256,
            merge: it.merge,
        });
    }
    let mut lock = Lock {
        schema: SCHEMA,
        renderer: RENDERER.into(),
        weftos_version: crate::VERSION.to_string(),
        commit: src.commit.clone(),
        source: src.origin.clone(),
        team: team.or_else(|| prior.and_then(|p| p.team.clone())),
        agents: agents.into_values().collect(),
    };
    lock.normalize();
    lock
}

#[cfg(test)]
mod tests {
    use super::safe_rel;

    #[test]
    fn safe_rel_rejects_escapes() {
        assert!(safe_rel(".claude/skills/x/SKILL.md").is_ok());
        assert!(safe_rel("../etc/passwd").is_err());
        assert!(safe_rel("/abs/path").is_err());
        assert!(safe_rel("").is_err());
    }
}
