//! Git operations using the `git2` crate.
//!
//! Each function performs a single git operation and returns a
//! structured result type. The caller (tool implementation) handles
//! serialization to JSON.

use std::path::Path;

use git2::{
    BlameOptions, DiffOptions, IndexAddOption, Repository, Signature, StatusOptions, StatusShow,
};
use tracing::debug;

use crate::types::{
    DiffPatch, FileStatus, GitBlameLine, GitDiffResult, GitLogEntry, GitStatusResult,
};

/// Open a repository at the given path.
pub fn open_repo(path: &str) -> Result<Repository, String> {
    Repository::open(path).map_err(|e| format!("failed to open repository at '{path}': {e}"))
}

/// Get the status of the working tree and index.
pub fn git_status(repo: &Repository) -> Result<GitStatusResult, String> {
    let mut opts = StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .show(StatusShow::IndexAndWorkdir);

    let statuses = repo
        .statuses(Some(&mut opts))
        .map_err(|e| format!("failed to get status: {e}"))?;

    let mut staged = Vec::new();
    let mut unstaged = Vec::new();
    let mut untracked = Vec::new();

    for entry in statuses.iter() {
        let path = entry.path().unwrap_or("<invalid-utf8>").to_string();
        let status = entry.status();

        if status.is_index_new()
            || status.is_index_modified()
            || status.is_index_deleted()
            || status.is_index_renamed()
        {
            let kind = if status.is_index_new() {
                "added"
            } else if status.is_index_modified() {
                "modified"
            } else if status.is_index_deleted() {
                "deleted"
            } else {
                "renamed"
            };
            staged.push(FileStatus {
                path: path.clone(),
                status: kind.to_string(),
            });
        }

        if status.is_wt_modified() || status.is_wt_deleted() || status.is_wt_renamed() {
            let kind = if status.is_wt_modified() {
                "modified"
            } else if status.is_wt_deleted() {
                "deleted"
            } else {
                "renamed"
            };
            unstaged.push(FileStatus {
                path: path.clone(),
                status: kind.to_string(),
            });
        }

        if status.is_wt_new() {
            untracked.push(path);
        }
    }

    let branch = repo
        .head()
        .ok()
        .and_then(|r| r.shorthand().map(String::from));

    Ok(GitStatusResult {
        staged,
        unstaged,
        untracked,
        branch,
    })
}

/// Show diff of unstaged changes (working directory vs index).
pub fn git_diff(repo: &Repository) -> Result<GitDiffResult, String> {
    let mut opts = DiffOptions::new();
    let diff = repo
        .diff_index_to_workdir(None, Some(&mut opts))
        .map_err(|e| format!("failed to compute diff: {e}"))?;

    let stats = diff.stats().map_err(|e| format!("failed to get diff stats: {e}"))?;

    let mut patches = Vec::new();
    for (idx, delta) in diff.deltas().enumerate() {
        let path = delta
            .new_file()
            .path()
            .unwrap_or(Path::new("<unknown>"))
            .to_string_lossy()
            .to_string();

        let patch_text = if let Ok(Some(mut p)) = git2::Patch::from_diff(&diff, idx) {
            let buf = p.to_buf().unwrap_or_default();
            String::from_utf8_lossy(buf.as_ref()).to_string()
        } else {
            String::new()
        };

        patches.push(DiffPatch {
            path,
            diff: patch_text,
        });
    }

    Ok(GitDiffResult {
        files_changed: stats.files_changed(),
        insertions: stats.insertions(),
        deletions: stats.deletions(),
        patches,
    })
}

/// Stage files and create a commit.
pub fn git_commit(
    repo: &Repository,
    paths: &[String],
    message: &str,
    author_name: &str,
    author_email: &str,
) -> Result<String, String> {
    let mut index = repo.index().map_err(|e| format!("failed to get index: {e}"))?;

    if paths.is_empty() {
        // Stage all modified files
        index
            .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
            .map_err(|e| format!("failed to stage files: {e}"))?;
    } else {
        for path in paths {
            index
                .add_path(Path::new(path))
                .map_err(|e| format!("failed to stage '{path}': {e}"))?;
        }
    }
    index
        .write()
        .map_err(|e| format!("failed to write index: {e}"))?;

    let tree_oid = index
        .write_tree()
        .map_err(|e| format!("failed to write tree: {e}"))?;
    let tree = repo
        .find_tree(tree_oid)
        .map_err(|e| format!("failed to find tree: {e}"))?;

    let sig =
        Signature::now(author_name, author_email).map_err(|e| format!("invalid signature: {e}"))?;

    // Get the parent commit (HEAD), if it exists
    let parent_commit = repo.head().ok().and_then(|head| head.peel_to_commit().ok());

    let parents: Vec<&git2::Commit<'_>> = parent_commit.iter().collect();

    let oid = repo
        .commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
        .map_err(|e| format!("failed to create commit: {e}"))?;

    debug!(oid = %oid, message = %message, "created commit");

    Ok(oid.to_string())
}

/// Create a new branch at HEAD.
pub fn git_create_branch(repo: &Repository, name: &str) -> Result<String, String> {
    let head = repo
        .head()
        .map_err(|e| format!("failed to get HEAD: {e}"))?;
    let commit = head
        .peel_to_commit()
        .map_err(|e| format!("failed to resolve HEAD to commit: {e}"))?;

    let branch = repo
        .branch(name, &commit, false)
        .map_err(|e| format!("failed to create branch '{name}': {e}"))?;

    let refname = branch
        .get()
        .name()
        .unwrap_or("<unknown>")
        .to_string();

    debug!(branch = %name, refname = %refname, "created branch");
    Ok(refname)
}

/// Get commit log entries.
pub fn git_log(repo: &Repository, max_count: usize) -> Result<Vec<GitLogEntry>, String> {
    let mut revwalk = repo
        .revwalk()
        .map_err(|e| format!("failed to create revwalk: {e}"))?;

    revwalk
        .push_head()
        .map_err(|e| format!("failed to push HEAD: {e}"))?;

    let mut entries = Vec::new();

    for (i, oid_result) in revwalk.enumerate() {
        if i >= max_count {
            break;
        }
        let oid = oid_result.map_err(|e| format!("revwalk error: {e}"))?;
        let commit = repo
            .find_commit(oid)
            .map_err(|e| format!("failed to find commit {oid}: {e}"))?;

        let author = commit.author();
        let timestamp = chrono::DateTime::from_timestamp(commit.time().seconds(), 0)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_else(|| "unknown".to_string());

        entries.push(GitLogEntry {
            hash: oid.to_string()[..8].to_string(),
            author: author.name().unwrap_or("unknown").to_string(),
            email: author.email().unwrap_or("unknown").to_string(),
            message: commit
                .summary()
                .unwrap_or("")
                .to_string(),
            timestamp,
        });
    }

    Ok(entries)
}

/// Blame a file (show last-modifying commit per line).
pub fn git_blame(
    repo: &Repository,
    file_path: &str,
    start_line: Option<usize>,
    end_line: Option<usize>,
) -> Result<Vec<GitBlameLine>, String> {
    let mut opts = BlameOptions::new();
    if let Some(start) = start_line {
        opts.min_line(start);
    }
    if let Some(end) = end_line {
        opts.max_line(end);
    }

    let blame = repo
        .blame_file(Path::new(file_path), Some(&mut opts))
        .map_err(|e| format!("failed to blame '{file_path}': {e}"))?;

    // Read file content for line text
    let full_path = repo
        .workdir()
        .ok_or_else(|| "bare repository, cannot read file".to_string())?
        .join(file_path);

    let content =
        std::fs::read_to_string(&full_path).unwrap_or_default();
    let lines: Vec<&str> = content.lines().collect();

    let mut result = Vec::new();
    for hunk_idx in 0..blame.len() {
        let hunk = blame
            .get_index(hunk_idx)
            .ok_or_else(|| format!("blame hunk {hunk_idx} not found"))?;

        let commit_id = hunk.final_commit_id();
        let author = hunk
            .final_signature()
            .name()
            .unwrap_or("unknown")
            .to_string();

        let start = hunk.final_start_line();
        let count = hunk.lines_in_hunk();

        for offset in 0..count {
            let line_num = start + offset;
            let line_content = lines
                .get(line_num.saturating_sub(1))
                .unwrap_or(&"")
                .to_string();

            result.push(GitBlameLine {
                line: line_num,
                commit: commit_id.to_string()[..8].to_string(),
                author: author.clone(),
                content: line_content,
            });
        }
    }

    Ok(result)
}

/// Clone a repository from a URL to a local path.
pub fn git_clone(url: &str, path: &str) -> Result<String, String> {
    debug!(url = %url, path = %path, "cloning repository");
    let _repo =
        Repository::clone(url, path).map_err(|e| format!("failed to clone '{url}': {e}"))?;
    Ok(format!("cloned {url} to {path}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_test_repo(dir: &Path) -> Repository {
        let repo = Repository::init(dir).unwrap();

        // Create an initial commit so HEAD exists
        let sig = Signature::now("Test", "test@test.com").unwrap();
        let tree_oid = {
            let mut index = repo.index().unwrap();
            // Write a file
            std::fs::write(dir.join("README.md"), "# Test\n").unwrap();
            index.add_path(Path::new("README.md")).unwrap();
            index.write().unwrap();
            index.write_tree().unwrap()
        };
        {
            let tree = repo.find_tree(tree_oid).unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "Initial commit", &tree, &[])
                .unwrap();
        }

        repo
    }

    #[test]
    fn test_git_status_clean_repo() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_test_repo(dir.path());

        let status = git_status(&repo).unwrap();
        assert!(status.staged.is_empty());
        assert!(status.unstaged.is_empty());
        // README.md was committed, so no untracked
        assert!(status.untracked.is_empty());
        assert!(status.branch.is_some());
    }

    #[test]
    fn test_git_status_with_changes() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_test_repo(dir.path());

        // Modify a tracked file
        std::fs::write(dir.path().join("README.md"), "# Modified\n").unwrap();
        // Add an untracked file
        std::fs::write(dir.path().join("new_file.txt"), "new\n").unwrap();

        let status = git_status(&repo).unwrap();
        assert!(!status.unstaged.is_empty() || !status.untracked.is_empty());
    }

    #[test]
    fn test_git_commit() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_test_repo(dir.path());

        // Create a new file and commit it
        std::fs::write(dir.path().join("file.txt"), "content\n").unwrap();
        let oid = git_commit(
            &repo,
            &["file.txt".to_string()],
            "Add file",
            "Test",
            "test@test.com",
        )
        .unwrap();
        assert!(!oid.is_empty());
    }

    #[test]
    fn test_git_create_branch() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_test_repo(dir.path());

        let refname = git_create_branch(&repo, "feature/test").unwrap();
        assert!(refname.contains("feature/test"));
    }

    #[test]
    fn test_git_log() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_test_repo(dir.path());

        let entries = git_log(&repo, 10).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message, "Initial commit");
        assert_eq!(entries[0].author, "Test");
    }

    #[test]
    fn test_git_diff_no_changes() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_test_repo(dir.path());

        let diff = git_diff(&repo).unwrap();
        assert_eq!(diff.files_changed, 0);
    }

    #[test]
    fn test_git_diff_with_changes() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_test_repo(dir.path());

        // Modify tracked file
        std::fs::write(dir.path().join("README.md"), "# Changed\nNew line\n").unwrap();

        let diff = git_diff(&repo).unwrap();
        assert!(diff.files_changed > 0);
        assert!(!diff.patches.is_empty());
    }

    #[test]
    fn test_git_blame() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_test_repo(dir.path());

        let blame = git_blame(&repo, "README.md", None, None).unwrap();
        assert!(!blame.is_empty());
        assert_eq!(blame[0].line, 1);
        assert_eq!(blame[0].content, "# Test");
    }
}
