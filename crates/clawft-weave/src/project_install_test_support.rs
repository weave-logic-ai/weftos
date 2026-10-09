//! Helpers for the install/update/remove tests: temp bare git repositories, a
//! fake `weft` script, and a fake fetcher. Everything lives in temp dirs.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use async_trait::async_trait;

use crate::project_git::GitRunner;
use crate::project_install::{FetchReport, FetchedRepo, InstallRequest, ProjectFetcher};
use crate::project_install_layout::plan;

pub const ULID: &str = "01HZ0000000000000000000000";

pub fn git(cwd: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(cwd)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid", "-c", "commit.gpgsign=false", "-c", "protocol.file.allow=always"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// A bare repository `<base>/<name>.git` with one commit on `main` holding `file`.
pub fn bare_repo(base: &Path, name: &str, file: &str) -> PathBuf {
    let work = base.join(format!("{name}-seed"));
    std::fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "-q", "-b", "main"]);
    std::fs::write(work.join(file), name).unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-q", "-m", "first"]);
    let bare = base.join(format!("{name}.git"));
    git(base, &["clone", "-q", "--bare", work.to_str().unwrap(), bare.to_str().unwrap()]);
    bare
}

/// The runner tests use: file protocol on, short timeout.
pub fn test_runner() -> GitRunner {
    GitRunner { allow_file: true, timeout: std::time::Duration::from_secs(60), ..GitRunner::default() }
}

/// A `weft` stand-in that logs its cwd and arguments to `<dir>/weft.log`.
pub fn fake_weft(dir: &Path, exit: i32) -> PathBuf {
    let bin = dir.join("weft");
    let log = dir.join("weft.log");
    std::fs::write(&bin, format!("#!/bin/sh\npwd -P > '{0}'\nfor a in \"$@\"; do echo \"$a\" >> '{0}'; done\necho 'adopt said no' >&2\nexit {exit}\n", log.display())).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

/// The lines `fake_weft` logged: cwd first, then each argument.
pub fn weft_log(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("weft.log")).unwrap_or_default().lines().map(str::to_owned).collect()
}

/// Writes a file into every destination of the layout, then succeeds or fails.
pub struct FakeFetcher {
    pub fail_after_writing: bool,
    pub calls: Mutex<u32>,
}

impl FakeFetcher {
    pub fn new(fail_after_writing: bool) -> Self {
        Self { fail_after_writing, calls: Mutex::new(0) }
    }
}

#[async_trait]
impl ProjectFetcher for FakeFetcher {
    fn name(&self) -> &'static str {
        "git-remote"
    }
    fn can_fetch(&self, req: &InstallRequest) -> bool {
        !req.sources.is_empty()
    }
    async fn fetch(&self, req: &InstallRequest, dest: &Path) -> Result<FetchReport, String> {
        *self.calls.lock().unwrap() += 1;
        let mut repos = Vec::new();
        for p in plan(req, dest)? {
            std::fs::create_dir_all(&p.dest).unwrap();
            std::fs::write(p.dest.join("f.txt"), "x").unwrap();
            repos.push(FetchedRepo { dir: p.source.dir.clone(), head: Some("deadbeef".into()), remote: Some(p.source.url.clone()) });
        }
        if self.fail_after_writing {
            return Err("boom".into());
        }
        Ok(FetchReport { fetcher: "git-remote", repos, bytes: 0, archived: vec![] })
    }
}
