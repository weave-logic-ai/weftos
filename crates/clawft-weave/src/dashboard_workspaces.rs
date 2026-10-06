//! `report.workspaces` (ADR-108 P1): where each registered project is checked
//! out on this machine and the state of its git repositories.
//!
//! Per project of the daemon's manifest index: `{ulid, root, git: [{path,
//! remote, branch, head, dirty, ahead, behind}], last_activity}`. `git` lists
//! each repository at the root or one directory below it (`path` is relative to
//! the root, `.` for the root itself).
//!
//! Privacy: only counts and refnames leave the machine. No file contents, no
//! diffs, no untracked file names (git's output is reduced to a count here), and
//! the `origin` URL is sent with any userinfo, query and fragment removed.
//!
//! The git CLI is run without a shell, with a timeout, bounded output, no
//! terminal prompt, no optional index lock and `core.fsmonitor` forced off (a
//! repository's own config must not get to run a command on every beat).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use async_trait::async_trait;
use serde::Serialize;
use serde_json::{Value, json};

/// Most projects reported per beat.
pub const MAX_PROJECTS: usize = 50;
/// Most repositories reported per project.
pub const MAX_REPOS: usize = 8;
/// The serialized `report` is kept under this many bytes.
pub const MAX_REPORT_BYTES: usize = 16 * 1024;
const MAX_SUBDIRS_SCANNED: usize = 512;
const GIT_TIMEOUT: Duration = Duration::from_secs(10);
/// Time one beat may spend on git across all projects.
const GATHER_BUDGET: Duration = Duration::from_secs(15);
const MAX_GIT_OUTPUT: usize = 1024 * 1024;
const MAX_FIELD: usize = 512;

/// One git repository inside a project root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepoFacts {
    /// Relative to the project root (`.` for the root).
    pub path: String,
    /// `origin` URL, credentials stripped.
    pub remote: Option<String>,
    /// Current branch; `None` when detached.
    pub branch: Option<String>,
    /// Short commit id of HEAD; `None` on an unborn branch.
    pub head: Option<String>,
    /// Changed files per `git status --porcelain` (untracked included).
    pub dirty: u32,
    /// Commits ahead of the upstream; `None` without one.
    pub ahead: Option<u32>,
    /// Commits behind the upstream; `None` without one.
    pub behind: Option<u32>,
}

/// One registered project's checkout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspaceFacts {
    pub ulid: String,
    pub root: String,
    pub git: Vec<RepoFacts>,
    /// RFC 3339: newest mtime of any repository's HEAD or index.
    pub last_activity: Option<String>,
}

/// What a beat reports as `workspaces`.
#[derive(Debug, Clone, Default)]
pub struct WorkspaceReport {
    pub workspaces: Vec<WorkspaceFacts>,
    /// A cap or the time budget cut the list short.
    pub truncated: bool,
}

/// Where workspace facts come from.
#[async_trait]
pub trait WorkspaceSource: Send + Sync {
    async fn workspaces(&self) -> WorkspaceReport;
}

/// The user daemon's manifest index (`~/.weftos/projects`).
pub struct ManifestWorkspaces {
    pub manifests_dir: PathBuf,
}

#[async_trait]
impl WorkspaceSource for ManifestWorkspaces {
    async fn workspaces(&self) -> WorkspaceReport {
        let dir = self.manifests_dir.clone();
        tokio::task::spawn_blocking(move || {
            let list = clawft_types::project::list_manifests(&dir).map(|l| l.manifests).unwrap_or_default();
            let projects: Vec<(String, PathBuf)> = list.into_iter().map(|m| (m.id, m.root)).collect();
            gather(&projects)
        })
        .await
        .unwrap_or_default()
    }
}

/// Facts for `(ulid, root)` projects, capped at [`MAX_PROJECTS`] and the time budget.
pub fn gather(projects: &[(String, PathBuf)]) -> WorkspaceReport {
    let started = Instant::now();
    let mut out = WorkspaceReport::default();
    if projects.len() > MAX_PROJECTS {
        out.truncated = true;
    }
    for (ulid, root) in projects.iter().take(MAX_PROJECTS) {
        if started.elapsed() > GATHER_BUDGET {
            out.truncated = true;
            break;
        }
        let (repos, repo_cap) = find_repos(root);
        out.truncated |= repo_cap;
        let mut git = Vec::new();
        let mut newest: Option<SystemTime> = None;
        for (rel, gitdir) in repos {
            if started.elapsed() > GATHER_BUDGET {
                out.truncated = true;
                break;
            }
            let dir = if rel == "." { root.clone() } else { root.join(&rel) };
            if let Some(f) = repo_facts(&dir, rel) {
                git.push(f);
            }
            for name in ["HEAD", "index"] {
                if let Ok(t) = std::fs::metadata(gitdir.join(name)).and_then(|m| m.modified()) {
                    newest = newest.max(Some(t));
                }
            }
        }
        out.workspaces.push(WorkspaceFacts {
            ulid: ulid.clone(),
            root: root.to_string_lossy().into_owned(),
            git,
            last_activity: newest.map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339()),
        });
    }
    out
}

/// Repositories at `root` or one level below: `(relative path, git dir)`, and
/// whether the [`MAX_REPOS`] cap cut the list.
fn find_repos(root: &Path) -> (Vec<(String, PathBuf)>, bool) {
    let mut found = Vec::new();
    if let Some(g) = git_dir_of(root) {
        found.push((".".to_owned(), g));
    }
    let Ok(rd) = std::fs::read_dir(root) else { return (found, false) };
    let mut names: Vec<String> = rd
        .filter_map(Result::ok)
        .take(MAX_SUBDIRS_SCANNED)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir())) // symlinks are not followed
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.'))
        .collect();
    names.sort();
    let mut capped = false;
    for n in names {
        if let Some(g) = git_dir_of(&root.join(&n)) {
            if found.len() >= MAX_REPOS {
                capped = true;
                break;
            }
            found.push((n, g));
        }
    }
    (found, capped)
}

/// The git directory of a repository at `dir` (`.git` directory, or the target
/// of a `.git` file as in a worktree), if `dir` is one.
fn git_dir_of(dir: &Path) -> Option<PathBuf> {
    let dot = dir.join(".git");
    let meta = std::fs::symlink_metadata(&dot).ok()?;
    if meta.is_dir() {
        return Some(dot);
    }
    if meta.is_file() {
        let text = std::fs::read_to_string(&dot).ok()?;
        let target = text.lines().next()?.strip_prefix("gitdir:")?.trim();
        let p = Path::new(target);
        return Some(if p.is_absolute() { p.to_path_buf() } else { dir.join(p) });
    }
    None
}

fn repo_facts(dir: &Path, rel: String) -> Option<RepoFacts> {
    let status = run_git(dir, &["status", "--porcelain=v2", "--branch", "--untracked-files=normal"])?;
    let mut f = parse_status(&status);
    f.path = rel;
    f.remote = run_git(dir, &["config", "--get", "remote.origin.url"])
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|s| sanitize_remote(s.trim()));
    Some(f)
}

/// Fields of `git status --porcelain=v2 --branch` (counts only, never names).
pub fn parse_status(out: &[u8]) -> RepoFacts {
    let text = String::from_utf8_lossy(out);
    let mut f = RepoFacts { path: String::new(), remote: None, branch: None, head: None, dirty: 0, ahead: None, behind: None };
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("# branch.oid ") {
            f.head = (rest != "(initial)" && rest.len() >= 7 && rest.is_ascii()).then(|| rest[..rest.len().min(8)].to_owned());
        } else if let Some(rest) = line.strip_prefix("# branch.head ") {
            f.branch = (rest != "(detached)").then(|| rest.chars().take(MAX_FIELD).collect());
        } else if let Some(rest) = line.strip_prefix("# branch.ab ") {
            let mut it = rest.split_whitespace();
            f.ahead = it.next().and_then(|a| a.strip_prefix('+')).and_then(|n| n.parse().ok());
            f.behind = it.next().and_then(|b| b.strip_prefix('-')).and_then(|n| n.parse().ok());
        } else if matches!(line.get(..2), Some("1 " | "2 " | "u " | "? ")) {
            f.dirty = f.dirty.saturating_add(1);
        }
    }
    f
}

/// An `origin` URL with userinfo, query and fragment removed; `None` when empty.
pub fn sanitize_remote(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    let url = url.split(['?', '#']).next().unwrap_or("");
    let cleaned = match url.split_once("://") {
        Some((scheme, rest)) => {
            let (authority, path) = rest.split_once('/').map_or((rest, None), |(a, p)| (a, Some(p)));
            let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
            match path {
                Some(p) => format!("{scheme}://{host}/{p}"),
                None => format!("{scheme}://{host}"),
            }
        }
        // scp-like `user@host:path`; a local path has no `@` before its first `/`.
        None => match url.split_once('/').map_or(url, |(h, _)| h).contains('@') {
            true => url.split_once('@').map_or(url.to_owned(), |(_, r)| r.to_owned()),
            false => url.to_owned(),
        },
    };
    Some(cleaned.chars().take(MAX_FIELD).collect())
}

/// Run `git -C dir args` and return bounded stdout on success. `None` on a
/// spawn failure, non-zero exit or timeout.
fn run_git(dir: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "core.fsmonitor=false", "-c", "core.quotepath=false"])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    // Keep draining after the cap so git never blocks on a full pipe.
    let reader = std::thread::spawn(move || {
        let mut kept = Vec::new();
        let mut buf = [0u8; 8192];
        while let Ok(n) = stdout.read(&mut buf) {
            if n == 0 {
                break;
            }
            if kept.len() < MAX_GIT_OUTPUT {
                let take = n.min(MAX_GIT_OUTPUT - kept.len());
                kept.extend_from_slice(&buf[..take]);
            }
        }
        kept
    });
    let deadline = Instant::now() + GIT_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return None;
            }
        }
    };
    let out = reader.join().ok()?;
    status.success().then_some(out)
}

/// The `workspaces` array for a report, trimmed from the end until the whole
/// serialized `report` fits [`MAX_REPORT_BYTES`]. Sets `workspaces_truncated`.
pub fn attach(report: &mut Value, ws: WorkspaceReport) {
    let mut list = ws.workspaces;
    let mut truncated = ws.truncated;
    let set = |report: &mut Value, list: &[WorkspaceFacts], truncated: bool| {
        report["workspaces"] = serde_json::to_value(list).unwrap_or_else(|_| json!([]));
        report["workspaces_truncated"] = json!(truncated);
    };
    set(report, &list, truncated);
    while report.to_string().len() > MAX_REPORT_BYTES && !list.is_empty() {
        list.pop();
        truncated = true;
        set(report, &list, truncated);
    }
}

#[cfg(test)]
#[path = "dashboard_workspaces_tests.rs"]
mod tests;
