//! `git-remote-weftos`: git's remote helper for `weftos://<node>/<ULID>/<dir>`
//! URLs (ADR-108 P3b). Git runs it for clone, fetch and pull; it speaks the
//! remote-helper protocol on stdin/stdout and talks to the local WeftOS user
//! daemon over its RPC socket, which forwards each `project.fetch` call to the
//! project's primary node on the signed mesh channel.
//!
//! Capabilities: `fetch` only (read-only; push goes through the project's
//! normal git remotes). `list` answers the primary's branches, tags and HEAD;
//! `fetch` asks the primary for a bundle of the wanted refs minus every ref tip
//! this repository already has, then `git bundle unbundle`s it, so a later pull
//! moves only the new commits.
//!
//! Debug lines go to stderr when `GIT_TRANSPORT_HELPER_DEBUG` is set. Nothing
//! secret is ever on the command line or in the output.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clawft_weave::project_fetch_client::{self as client, BundleMode, DaemonChannel, RemoteUrl};
use clawft_weave::project_fetch_repos::{MAX_HAVES, git, is_oid};

fn debug(msg: &str) {
    if std::env::var_os("GIT_TRANSPORT_HELPER_DEBUG").is_some() {
        eprintln!("git-remote-weftos: {msg}");
    }
}

fn fail(msg: &str) -> ExitCode {
    eprintln!("git-remote-weftos: {msg}");
    ExitCode::from(1)
}

/// The repository's git dir: `GIT_DIR` (git sets it), else discovered.
fn git_dir() -> Result<PathBuf, String> {
    if let Some(d) = std::env::var_os("GIT_DIR") {
        return Ok(PathBuf::from(d));
    }
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let out = git(&cwd, &["rev-parse", "--absolute-git-dir"], None, Duration::from_secs(10))?;
    Ok(PathBuf::from(String::from_utf8_lossy(&out).trim()))
}

/// Tips of every local ref: what the primary may leave out of the bundle.
fn haves(gitdir: &std::path::Path) -> Vec<String> {
    let Ok(out) = git(gitdir, &["for-each-ref", "--format=%(objectname)"], None, Duration::from_secs(30)) else {
        return Vec::new();
    };
    let mut v: Vec<String> = String::from_utf8_lossy(&out).lines().filter(|l| is_oid(l)).map(str::to_owned).collect();
    v.sort_unstable();
    v.dedup();
    v.truncate(MAX_HAVES);
    v
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // `git remote-weftos <remote> <url>`, or just `<url>`.
    let Some(raw) = args.get(1).or(args.first()) else {
        return fail("usage: git-remote-weftos <remote> <url>");
    };
    let url = match RemoteUrl::parse(raw) {
        Ok(u) => u,
        Err(e) => return fail(&e),
    };
    let gitdir = match git_dir() {
        Ok(d) => d,
        Err(e) => return fail(&format!("cannot find the git dir: {e}")),
    };
    let ch = DaemonChannel::new(url.node.clone(), None);
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    let mut head: Option<String> = None;
    let mut wants: Vec<String> = Vec::new();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        debug(&format!("< {line}"));
        let line = line.trim_end();
        if line == "capabilities" {
            let _ = writeln!(out, "fetch\n");
        } else if line == "list" {
            match client::refs(&ch, &url.project, &url.dir).await {
                Ok(refs) => {
                    for (oid, name) in &refs.refs {
                        let _ = writeln!(out, "{oid} {name}");
                    }
                    if let Some(h) = refs.head.as_deref().filter(|h| refs.refs.iter().any(|(_, n)| n == h)) {
                        let _ = writeln!(out, "@{h} HEAD");
                    }
                    head = refs.head;
                    let _ = writeln!(out);
                }
                Err(e) => return fail(&format!("list: {e}")),
            }
        } else if line.starts_with("list for-push") {
            return fail("push over weftos:// is not supported; push to the project's git remote");
        } else if let Some(rest) = line.strip_prefix("fetch ") {
            let mut it = rest.splitn(2, ' ');
            let (_oid, name) = (it.next().unwrap_or(""), it.next().unwrap_or(""));
            let name = if name == "HEAD" { head.clone().unwrap_or_default() } else { name.to_owned() };
            if !name.is_empty() && !wants.contains(&name) {
                wants.push(name);
            }
        } else if line.is_empty() {
            if wants.is_empty() {
                continue;
            }
            let want = std::mem::take(&mut wants);
            let have = haves(&gitdir);
            debug(&format!("fetch {} ref(s), {} have(s)", want.len(), have.len()));
            match client::fetch_bundle(&ch, &url.project, &url.dir, &want, &have, &gitdir, BundleMode::Unbundle).await {
                Ok(bytes) => debug(&format!("bundle: {:?} bytes", bytes)),
                Err(e) => return fail(&format!("fetch: {e}")),
            }
            let _ = writeln!(out);
        } else {
            return fail(&format!("unsupported command {line:?}"));
        }
        let _ = out.flush();
    }
    ExitCode::SUCCESS
}
