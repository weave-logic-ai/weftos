//! Types for git tool operations.

use serde::{Deserialize, Serialize};

/// Configuration for git operations.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GitConfig {
    /// Path to the repository. Defaults to current directory.
    #[serde(default)]
    pub repo_path: Option<String>,
}

/// Result of a git status operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitStatusResult {
    /// Files with changes staged for commit.
    pub staged: Vec<FileStatus>,

    /// Files with unstaged changes.
    pub unstaged: Vec<FileStatus>,

    /// Untracked files.
    pub untracked: Vec<String>,

    /// Current branch name (if on a branch).
    pub branch: Option<String>,
}

/// A file's status in the git index/worktree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileStatus {
    /// File path relative to the repo root.
    pub path: String,

    /// Status kind (e.g., "modified", "added", "deleted", "renamed").
    pub status: String,
}

/// Result of a git diff operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitDiffResult {
    /// Number of files changed.
    pub files_changed: usize,

    /// Total insertions.
    pub insertions: usize,

    /// Total deletions.
    pub deletions: usize,

    /// Per-file diff patches.
    pub patches: Vec<DiffPatch>,
}

/// A diff patch for a single file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffPatch {
    /// File path.
    pub path: String,

    /// Diff content (unified format).
    pub diff: String,
}

/// Result of a git log operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitLogEntry {
    /// Commit hash (abbreviated).
    pub hash: String,

    /// Commit author name.
    pub author: String,

    /// Commit author email.
    pub email: String,

    /// Commit message (first line).
    pub message: String,

    /// Commit timestamp (ISO 8601).
    pub timestamp: String,
}

/// Result of a git blame operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitBlameLine {
    /// Line number (1-based).
    pub line: usize,

    /// Commit hash that last modified this line.
    pub commit: String,

    /// Author of the last change.
    pub author: String,

    /// Content of the line.
    pub content: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_config_default() {
        let config = GitConfig::default();
        assert!(config.repo_path.is_none());
    }

    #[test]
    fn git_status_result_serde() {
        let result = GitStatusResult {
            staged: vec![FileStatus {
                path: "src/lib.rs".to_string(),
                status: "modified".to_string(),
            }],
            unstaged: vec![],
            untracked: vec!["new_file.txt".to_string()],
            branch: Some("main".to_string()),
        };
        let json = serde_json::to_string(&result).unwrap();
        let restored: GitStatusResult = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.branch, Some("main".to_string()));
        assert_eq!(restored.staged.len(), 1);
        assert_eq!(restored.untracked.len(), 1);
    }

    #[test]
    fn git_log_entry_serde() {
        let entry = GitLogEntry {
            hash: "abc1234".to_string(),
            author: "Test User".to_string(),
            email: "test@example.com".to_string(),
            message: "Initial commit".to_string(),
            timestamp: "2025-01-01T00:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let restored: GitLogEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.hash, "abc1234");
    }
}
