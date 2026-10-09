//! The primary's side of git transfer for `project.fetch` (ADR-108 P3b):
//! which repositories a project has, their refs, and a `git bundle` of what
//! the caller lacks.
//!
//! A repository is the project root itself (`.`), a non-hidden directory one
//! level below it that has a `.git`, or a sibling directory the manifest lists
//! (`repos`), named by its directory name; at most [`MAX_REPOS`]. Transfer is a
//! bundle per repository: the full history on a clone, and on a later fetch
//! only what is not reachable from the `have` commits the caller sends, so pulls
//! are incremental. A bundle is a file git itself writes and reads, so the wire
//! carries no object parsing of ours.
//!
//! Every git call runs without a shell, with a timeout, the repository's own
//! hooks and `fsmonitor` off, and no terminal prompt. Wants are refnames that
//! must appear in the repository's own `refs/heads` or `refs/tags` (never a
//! free-form revision), haves are object ids.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub use crate::project_install::MAX_REPOS;

/// Most refnames one bundle request may ask for.
pub const MAX_WANTS: usize = 512;
/// Most `have` ids one bundle request may send.
pub const MAX_HAVES: usize = 256;
const GIT_TIMEOUT: Duration = Duration::from_secs(30);
/// Creating a bundle of a large repository may take a while.
const BUNDLE_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const MAX_OUTPUT: usize = 4 * 1024 * 1024;
const MAX_SUBDIRS: usize = 512;

/// One repository of a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoEntry {
    /// `.` or the directory name (what the URL and the install layout use).
    pub dir: String,
    /// Where it is on the primary.
    pub path: PathBuf,
}

fn is_repo(dir: &Path) -> bool {
    std::fs::symlink_metadata(dir.join(".git")).is_ok_and(|m| m.is_dir() || m.is_file())
}

/// The project's repositories: root, subdirectories, then manifest siblings;
/// a sibling whose name a subdirectory already uses is dropped.
pub fn discover(root: &Path, extra: &[PathBuf]) -> Vec<RepoEntry> {
    let mut out = Vec::new();
    if is_repo(root) {
        out.push(RepoEntry { dir: ".".into(), path: root.to_path_buf() });
    }
    let mut names: Vec<String> = std::fs::read_dir(root)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .take(MAX_SUBDIRS)
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir())) // symlinks are not followed
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| crate::project_install::dir_ok(n) && n != ".")
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    for n in names {
        let p = root.join(&n);
        if is_repo(&p) && out.len() < MAX_REPOS {
            out.push(RepoEntry { dir: n, path: p });
        }
    }
    for p in extra {
        let Some(name) = p.file_name().and_then(|n| n.to_str()) else { continue };
        if !p.is_absolute()
            || !crate::project_install::dir_ok(name)
            || out.iter().any(|r| r.dir == name)
            || !std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir())
            || !is_repo(p)
            || out.len() >= MAX_REPOS
        {
            continue;
        }
        out.push(RepoEntry { dir: name.to_owned(), path: p.clone() });
    }
    out
}

/// The repository `dir` names, if the project has one.
pub fn resolve(root: &Path, extra: &[PathBuf], dir: &str) -> Result<PathBuf, String> {
    if !crate::project_install::dir_ok(dir) {
        return Err("dir must be '.' or one plain path segment".into());
    }
    discover(root, extra)
        .into_iter()
        .find(|r| r.dir == dir)
        .map(|r| r.path)
        .ok_or_else(|| "no such repository in this project".into())
}

/// Refs a fetch may ask for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Refs {
    /// `(object id, refname)` for `refs/heads/*` and `refs/tags/*`.
    pub refs: Vec<(String, String)>,
    /// The refname HEAD points at, when it is a branch.
    pub head: Option<String>,
}

/// Branches and tags of `repo`, and its HEAD branch.
pub fn list_refs(repo: &Path) -> Result<Refs, String> {
    let out = git(repo, &["for-each-ref", "--format=%(objectname) %(refname)", "refs/heads", "refs/tags"], None, GIT_TIMEOUT)?;
    let refs = String::from_utf8_lossy(&out)
        .lines()
        .filter_map(|l| l.split_once(' '))
        .filter(|(oid, name)| is_oid(oid) && ref_ok(name))
        .map(|(o, n)| (o.to_owned(), n.to_owned()))
        .collect();
    let head = git(repo, &["symbolic-ref", "--quiet", "HEAD"], None, GIT_TIMEOUT)
        .ok()
        .map(|b| String::from_utf8_lossy(&b).trim().to_owned())
        .filter(|h| ref_ok(h));
    Ok(Refs { refs, head })
}

/// Short id of HEAD, if the repository has a commit.
pub fn head_short(repo: &Path) -> Option<String> {
    git(repo, &["rev-parse", "--short=8", "--verify", "--quiet", "HEAD"], None, GIT_TIMEOUT)
        .ok()
        .map(|b| String::from_utf8_lossy(&b).trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// What a bundle request produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bundle {
    /// Everything wanted is reachable from the haves: nothing to send.
    Empty,
    /// A bundle of `bytes` was written.
    Written { bytes: u64 },
}

/// Write to `out` a bundle of `want` (refnames of `repo`) minus what is
/// reachable from `have` (object ids the caller holds; ones this repository
/// does not know are ignored).
pub fn make_bundle(repo: &Path, out: &Path, want: &[String], have: &[String]) -> Result<Bundle, String> {
    if want.is_empty() || want.len() > MAX_WANTS {
        return Err(format!("want must list 1..={MAX_WANTS} refs"));
    }
    if have.len() > MAX_HAVES {
        return Err(format!("have may list at most {MAX_HAVES} ids"));
    }
    let known = list_refs(repo)?;
    for w in want {
        if !known.refs.iter().any(|(_, n)| n == w) {
            return Err(format!("{w:?} is not a branch or tag of this repository"));
        }
    }
    if have.iter().any(|h| !is_oid(h)) {
        return Err("have must be object ids".into());
    }
    let have = known_commits(repo, have)?;
    let out_s = out.to_str().ok_or("bundle path is not UTF-8")?;
    let mut args: Vec<&str> = vec!["bundle", "create", "--quiet", out_s];
    args.extend(want.iter().map(String::as_str));
    if !have.is_empty() {
        args.push("--not");
        args.extend(have.iter().map(String::as_str));
    }
    match git(repo, &args, None, BUNDLE_TIMEOUT) {
        Ok(_) => {}
        Err(e) if e.contains("empty bundle") => return Ok(Bundle::Empty),
        Err(e) => return Err(e),
    }
    let bytes = std::fs::metadata(out).map_err(|e| format!("bundle: {e}"))?.len();
    Ok(Bundle::Written { bytes })
}

/// The subset of `ids` that are commits or tags this repository has.
fn known_commits(repo: &Path, ids: &[String]) -> Result<Vec<String>, String> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut stdin = ids.join("\n");
    stdin.push('\n');
    let out = git(repo, &["cat-file", "--batch-check"], Some(stdin.as_bytes()), GIT_TIMEOUT)?;
    Ok(String::from_utf8_lossy(&out)
        .lines()
        .filter_map(|l| {
            let mut it = l.split(' ');
            let (id, kind) = (it.next()?, it.next()?);
            matches!(kind, "commit" | "tag").then(|| id.to_owned())
        })
        .collect())
}

/// 40 or 64 lower-case hex.
pub fn is_oid(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A plain refname under `refs/heads/` or `refs/tags/`.
pub fn ref_ok(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("refs/heads/").or_else(|| name.strip_prefix("refs/tags/")) else {
        return false;
    };
    !rest.is_empty()
        && rest.len() <= 200
        && !rest.starts_with('-')
        && !rest.contains("..")
        && !rest.ends_with('/')
        && rest.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.'))
}

/// Run `git -C dir args` without a shell; stdout on success, the trimmed
/// stderr as the error otherwise. Output is bounded and never blocks git.
pub fn git(dir: &Path, args: &[&str], stdin: Option<&[u8]>, timeout: Duration) -> Result<Vec<u8>, String> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "core.fsmonitor=false", "-c", "core.hooksPath=/dev/null", "-c", "core.quotepath=false"])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("git: {e}"))?;
    if let (Some(mut pipe), Some(data)) = (child.stdin.take(), stdin) {
        let data = data.to_vec();
        std::thread::spawn(move || {
            let _ = pipe.write_all(&data);
        });
    }
    let out = drain(child.stdout.take());
    let err = drain(child.stderr.take());
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("git {} timed out", args.first().unwrap_or(&"")));
            }
        }
    };
    let (out, err) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
    if status.success() {
        Ok(out)
    } else {
        let msg = String::from_utf8_lossy(&err).trim().chars().take(400).collect::<String>();
        Err(format!("git {}: {}", args.first().unwrap_or(&""), if msg.is_empty() { status.to_string() } else { msg }))
    }
}

fn drain(pipe: Option<impl Read + Send + 'static>) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut kept = Vec::new();
        let Some(mut pipe) = pipe else { return kept };
        let mut buf = [0u8; 16384];
        while let Ok(n) = pipe.read(&mut buf) {
            if n == 0 {
                break;
            }
            if kept.len() < MAX_OUTPUT {
                let take = n.min(MAX_OUTPUT - kept.len());
                kept.extend_from_slice(&buf[..take]);
            }
        }
        kept
    })
}
