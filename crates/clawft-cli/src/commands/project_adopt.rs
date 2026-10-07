//! `weft project init --adopt <ULID>`: register this tree as an ADR-108
//! workspace of an existing project whose primary lives on another machine.
//!
//! Only `project.toml` (with `role = "workspace"`) and the manifest are
//! written: no key, chain, certificate or `[serve]` section, and the project
//! supervisor refuses to run a kernel for it. The user daemon's dashboard
//! reporter picks the workspace up on its next beat.
//!
//! A tracked `.gitignore` is never edited (the tree may be someone else's
//! repository): `.weftos/` goes into the repository's local
//! `.git/info/exclude` instead.

use std::path::{Path, PathBuf};

use anyhow::bail;

use clawft_types::project::{PROJECT_DIR, adopt_workspace, find_project_toml};

use super::project_cmd::{Env, project_root};

const EXCLUDE_LINE: &str = ".weftos/";

/// `weft project init --adopt <id>`.
pub fn adopt(env: &Env, id: &str, name: Option<&str>, repos: &[PathBuf]) -> anyhow::Result<String> {
    let (root, _) = project_root(env)?;
    let cwd = env.cwd.canonicalize().unwrap_or_else(|_| env.cwd.clone());
    if root != cwd {
        bail!(
            "{} is inside the project at {}; a workspace cannot be nested in another project",
            cwd.display(),
            root.display()
        );
    }
    if let Some(parent) = cwd.parent()
        && let Some(outer) = find_project_toml(parent, Some(&env.home))
    {
        bail!(
            "{} is inside the project at {}; a workspace cannot be nested in another project",
            cwd.display(),
            outer.display()
        );
    }
    let repos = extra_repos(env, &root, repos)?;
    let m = adopt_workspace(&root, &env.manifests_dir, id, name, &repos).map_err(|e| anyhow::anyhow!("{e}"))?;
    let listed: String = m.workspace_repos().iter().map(|r| format!("\n  repo:     {}", r.display())).collect();
    let excluded = match exclude_in_git(&m.root) {
        Ok(Some(path)) => format!("\n  excluded: {EXCLUDE_LINE} in {}", path.display()),
        Ok(None) => String::new(),
        Err(e) => {
            eprintln!("warning: could not add {EXCLUDE_LINE} to .git/info/exclude: {e}");
            String::new()
        }
    };
    Ok(format!(
        "project {} (workspace)\n  id:       {}\n  root:     {}{listed}\n  identity: {}\n  manifest: {}{excluded}\n  \
         no key, chain or kernel here; the primary stays on its home machine",
        m.name,
        m.id,
        m.root.display(),
        m.root.join(PROJECT_DIR).join("project.toml").display(),
        env.manifests_dir.join(format!("{}.toml", m.id)).display(),
    ))
}

/// Most extra repositories per workspace (the reporter sends at most 8 per project).
const MAX_EXTRA_REPOS: usize = 7;

/// Canonical extra repository directories: each must exist, be a directory
/// inside `$HOME`, lie outside `root`, and be a git repository top level.
fn extra_repos(env: &Env, root: &Path, repos: &[PathBuf]) -> anyhow::Result<Vec<PathBuf>> {
    if repos.len() > MAX_EXTRA_REPOS {
        bail!("at most {MAX_EXTRA_REPOS} --repo directories per workspace");
    }
    let home = env.home.canonicalize().unwrap_or_else(|_| env.home.clone());
    repos
        .iter()
        .map(|r| {
            let p = if r.is_absolute() { r.clone() } else { env.cwd.join(r) };
            let c = p.canonicalize().map_err(|e| anyhow::anyhow!("--repo {}: {e}", r.display()))?;
            if !c.is_dir() {
                bail!("--repo {}: not a directory", r.display());
            }
            if !c.starts_with(&home) || c == home {
                bail!("--repo {}: must be inside your home directory", r.display());
            }
            if c.starts_with(root) {
                bail!("--repo {}: already inside the project root (it is scanned anyway)", r.display());
            }
            if !c.join(".git").exists() {
                bail!("--repo {}: not a git repository top level", r.display());
            }
            Ok(c)
        })
        .collect()
}

/// The `info/exclude` file of the repository whose work tree is `root`, if
/// `root` is a repository top level (`.git` directory, or a worktree's
/// `.git` file pointing at its git dir, whose `commondir` holds `info/`).
fn exclude_path(root: &Path) -> Result<Option<PathBuf>, String> {
    let dot = root.join(".git");
    let git_dir = if dot.is_dir() {
        dot
    } else if dot.is_file() {
        let text = std::fs::read_to_string(&dot).map_err(|e| format!("{}: {e}", dot.display()))?;
        let Some(rest) = text.lines().next().and_then(|l| l.strip_prefix("gitdir:")) else {
            return Err(format!("{}: not a gitdir file", dot.display()));
        };
        let p = PathBuf::from(rest.trim());
        if p.is_absolute() { p } else { root.join(p) }
    } else {
        return Ok(None);
    };
    let common = match std::fs::read_to_string(git_dir.join("commondir")) {
        Ok(c) => {
            let p = PathBuf::from(c.trim());
            if p.is_absolute() { p } else { git_dir.join(p) }
        }
        Err(_) => git_dir,
    };
    Ok(Some(common.join("info").join("exclude")))
}

/// Add `.weftos/` to the repository's local exclude file unless present.
/// Returns the file written (or already holding the line), `None` when
/// `root` is not a repository top level.
pub(crate) fn exclude_in_git(root: &Path) -> Result<Option<PathBuf>, String> {
    let Some(path) = exclude_path(root)? else { return Ok(None) };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    if text.lines().any(|l| l.trim() == EXCLUDE_LINE || l.trim() == ".weftos") {
        return Ok(Some(path));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let mut next = text;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str("# WeftOS workspace identity (weft project init --adopt)\n");
    next.push_str(EXCLUDE_LINE);
    next.push('\n');
    std::fs::write(&path, next).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclude_goes_into_a_plain_repository() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        let p = exclude_in_git(tmp.path()).unwrap().unwrap();
        assert_eq!(p, tmp.path().join(".git/info/exclude"));
        assert!(std::fs::read_to_string(&p).unwrap().lines().any(|l| l == ".weftos/"));
        // Idempotent.
        exclude_in_git(tmp.path()).unwrap();
        let n = std::fs::read_to_string(&p).unwrap().matches(".weftos/").count();
        assert_eq!(n, 1);
    }

    #[test]
    fn exclude_follows_a_worktree_to_its_common_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let common = tmp.path().join("main/.git");
        let wt_git = common.join("worktrees/wt");
        std::fs::create_dir_all(&wt_git).unwrap();
        std::fs::write(wt_git.join("commondir"), "../..\n").unwrap();
        let wt = tmp.path().join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", wt_git.display())).unwrap();
        let p = exclude_in_git(&wt).unwrap().unwrap();
        assert_eq!(p.canonicalize().unwrap(), common.join("info/exclude").canonicalize().unwrap());
    }

    #[test]
    fn no_repository_means_no_exclude() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(exclude_in_git(tmp.path()).unwrap().is_none());
    }

    #[test]
    fn a_tracked_gitignore_is_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        std::fs::write(tmp.path().join(".gitignore"), "target/\n").unwrap();
        exclude_in_git(tmp.path()).unwrap();
        assert_eq!(std::fs::read_to_string(tmp.path().join(".gitignore")).unwrap(), "target/\n");
    }
}
