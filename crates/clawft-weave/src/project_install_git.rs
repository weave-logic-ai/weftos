//! The `git-remote` fetcher (ADR-108 P3a): clones each repository of an
//! install request from the project's own git remotes with the member's own
//! git credentials (their credential helper or ssh agent; nothing prompts).
//!
//! Layout and rollback live in [`crate::project_install_layout`]. Git runs
//! through [`GitRunner`]: argument vector, no shell, no terminal prompt, no
//! `file`/`ext` transports, timeout per repository, bounded output. Test code
//! builds the fetcher with [`GitRemoteFetcher::with_runner`] to allow `file://`
//! so it can clone from temp bare repositories.

use std::path::Path;

use async_trait::async_trait;

use crate::project_git::GitRunner;
use crate::project_install::{FetchReport, FetchedRepo, InstallRequest, ProjectFetcher};
use crate::project_install_layout::{Rollback, check_free, plan};

/// Clones from the project's own git remotes.
#[derive(Debug, Clone, Default)]
pub struct GitRemoteFetcher {
    git: GitRunner,
}

impl GitRemoteFetcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// Use a specific runner (tests: file protocol, short timeout).
    pub fn with_runner(git: GitRunner) -> Self {
        Self { git }
    }

    async fn clone_one(&self, url: &str, branch: Option<&str>, dest: &Path) -> Result<(), String> {
        let parent = dest.parent().ok_or("destination has no parent directory")?;
        std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        let dest_s = dest.to_str().ok_or("destination path is not UTF-8")?;
        let mut args = vec!["clone", "--quiet"];
        if let Some(b) = branch {
            args.extend(["--branch", b]);
        }
        args.extend(["--", url, dest_s]);
        self.git.run(parent, &args).await.map(|_| ())
    }
}

#[async_trait]
impl ProjectFetcher for GitRemoteFetcher {
    fn name(&self) -> &'static str {
        "git-remote"
    }

    fn can_fetch(&self, req: &InstallRequest) -> bool {
        !req.sources.is_empty()
    }

    async fn fetch(&self, req: &InstallRequest, dest: &Path) -> Result<FetchReport, String> {
        let placements = plan(req, dest)?;
        check_free(&placements, dest)?;
        let rollback = Rollback::snapshot(&placements, dest);
        let mut repos = Vec::new();
        for p in &placements {
            let step = async {
                self.clone_one(&p.source.url, p.source.branch.as_deref(), &p.dest).await?;
                let head = self.git.run(&p.dest, &["rev-parse", "--short=8", "HEAD"]).await.ok().map(|s| s.trim().to_owned());
                Ok::<_, String>(FetchedRepo { dir: p.source.dir.clone(), head, remote: Some(p.source.url.clone()) })
            };
            match step.await {
                Ok(r) => repos.push(r),
                Err(e) => {
                    rollback.undo();
                    return Err(format!("clone of source {} failed: {e}", p.source.dir));
                }
            }
        }
        if placements.iter().all(|p| p.source.dir != ".") {
            // No `.` source: the target is a plain directory holding the clones.
            std::fs::create_dir_all(dest).map_err(|e| format!("cannot create {}: {e}", dest.display()))?;
        }
        Ok(FetchReport { fetcher: "git-remote", repos, ..Default::default() })
    }
}

#[cfg(all(test, unix))]
#[path = "project_install_git_tests.rs"]
mod tests;
