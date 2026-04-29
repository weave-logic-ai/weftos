//! Git operations tool plugin for clawft.
//!
//! Provides tools for git operations (clone, commit, branch, diff, blame,
//! log, status) using the `git2` crate.
//!
//! # Security
//!
//! This plugin requests filesystem (read/write) and network permissions.
//! All paths are validated through `git2`, which handles path canonicalization.
//!
//! # Feature Flag
//!
//! This crate is gated behind the workspace `plugin-git` feature flag.

pub mod operations;
pub mod types;

use async_trait::async_trait;
use clawft_plugin::{PluginError, Tool, ToolContext};

use operations::{
    git_blame, git_clone, git_commit, git_create_branch, git_diff, git_log, git_status, open_repo,
};
use types::GitConfig;

// ---------------------------------------------------------------------------
// Shared helper
// ---------------------------------------------------------------------------

/// Resolve the repo path from params or config.
fn resolve_repo_path(params: &serde_json::Value, config: &GitConfig) -> Result<String, String> {
    params
        .get("repo_path")
        .and_then(|v| v.as_str())
        .map(String::from)
        .or_else(|| config.repo_path.clone())
        .ok_or_else(|| "repo_path is required".to_string())
}

// ---------------------------------------------------------------------------
// GitStatusTool
// ---------------------------------------------------------------------------

/// Tool that shows working tree status.
pub struct GitStatusTool {
    config: GitConfig,
}

impl GitStatusTool {
    pub fn new(config: GitConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for GitStatusTool {
    fn name(&self) -> &str {
        "git_status"
    }

    fn description(&self) -> &str {
        "Show the working tree status of a git repository"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "repo_path": {
                    "type": "string",
                    "description": "Path to the git repository"
                }
            },
            "required": ["repo_path"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &dyn ToolContext,
    ) -> Result<serde_json::Value, PluginError> {
        let path = resolve_repo_path(&params, &self.config)
            .map_err(PluginError::ExecutionFailed)?;
        let repo = open_repo(&path).map_err(PluginError::ExecutionFailed)?;
        let result = git_status(&repo).map_err(PluginError::ExecutionFailed)?;
        serde_json::to_value(&result).map_err(PluginError::from)
    }
}

// ---------------------------------------------------------------------------
// GitDiffTool
// ---------------------------------------------------------------------------

/// Tool that shows unstaged changes.
pub struct GitDiffTool {
    config: GitConfig,
}

impl GitDiffTool {
    pub fn new(config: GitConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for GitDiffTool {
    fn name(&self) -> &str {
        "git_diff"
    }

    fn description(&self) -> &str {
        "Show unstaged changes in the working directory"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "repo_path": {
                    "type": "string",
                    "description": "Path to the git repository"
                }
            },
            "required": ["repo_path"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &dyn ToolContext,
    ) -> Result<serde_json::Value, PluginError> {
        let path = resolve_repo_path(&params, &self.config)
            .map_err(PluginError::ExecutionFailed)?;
        let repo = open_repo(&path).map_err(PluginError::ExecutionFailed)?;
        let result = git_diff(&repo).map_err(PluginError::ExecutionFailed)?;
        serde_json::to_value(&result).map_err(PluginError::from)
    }
}

// ---------------------------------------------------------------------------
// GitCommitTool
// ---------------------------------------------------------------------------

/// Tool that stages files and creates a commit.
pub struct GitCommitTool {
    config: GitConfig,
}

impl GitCommitTool {
    pub fn new(config: GitConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for GitCommitTool {
    fn name(&self) -> &str {
        "git_commit"
    }

    fn description(&self) -> &str {
        "Stage files and create a git commit"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "repo_path": {
                    "type": "string",
                    "description": "Path to the git repository"
                },
                "message": {
                    "type": "string",
                    "description": "Commit message"
                },
                "paths": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Files to stage. If empty, stages all modified files."
                },
                "author_name": {
                    "type": "string",
                    "description": "Author name for the commit"
                },
                "author_email": {
                    "type": "string",
                    "description": "Author email for the commit"
                }
            },
            "required": ["repo_path", "message", "author_name", "author_email"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &dyn ToolContext,
    ) -> Result<serde_json::Value, PluginError> {
        let path = resolve_repo_path(&params, &self.config)
            .map_err(PluginError::ExecutionFailed)?;
        let message = params
            .get("message")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PluginError::ExecutionFailed("message is required".into()))?;
        let author_name = params
            .get("author_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PluginError::ExecutionFailed("author_name is required".into()))?;
        let author_email = params
            .get("author_email")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PluginError::ExecutionFailed("author_email is required".into()))?;
        let paths: Vec<String> = params
            .get("paths")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let repo = open_repo(&path).map_err(PluginError::ExecutionFailed)?;
        let oid = git_commit(&repo, &paths, message, author_name, author_email)
            .map_err(PluginError::ExecutionFailed)?;

        Ok(serde_json::json!({
            "commit": oid,
            "message": message
        }))
    }
}

// ---------------------------------------------------------------------------
// GitBranchTool
// ---------------------------------------------------------------------------

/// Tool that creates a new branch.
pub struct GitBranchTool {
    config: GitConfig,
}

impl GitBranchTool {
    pub fn new(config: GitConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for GitBranchTool {
    fn name(&self) -> &str {
        "git_branch"
    }

    fn description(&self) -> &str {
        "Create a new git branch at HEAD"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "repo_path": {
                    "type": "string",
                    "description": "Path to the git repository"
                },
                "name": {
                    "type": "string",
                    "description": "Name of the new branch"
                }
            },
            "required": ["repo_path", "name"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &dyn ToolContext,
    ) -> Result<serde_json::Value, PluginError> {
        let path = resolve_repo_path(&params, &self.config)
            .map_err(PluginError::ExecutionFailed)?;
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PluginError::ExecutionFailed("name is required".into()))?;

        let repo = open_repo(&path).map_err(PluginError::ExecutionFailed)?;
        let refname =
            git_create_branch(&repo, name).map_err(PluginError::ExecutionFailed)?;

        Ok(serde_json::json!({
            "branch": name,
            "ref": refname
        }))
    }
}

// ---------------------------------------------------------------------------
// GitLogTool
// ---------------------------------------------------------------------------

/// Tool that shows commit history.
pub struct GitLogTool {
    config: GitConfig,
}

impl GitLogTool {
    pub fn new(config: GitConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for GitLogTool {
    fn name(&self) -> &str {
        "git_log"
    }

    fn description(&self) -> &str {
        "Show commit history of a git repository"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "repo_path": {
                    "type": "string",
                    "description": "Path to the git repository"
                },
                "max_count": {
                    "type": "integer",
                    "description": "Maximum number of commits to show",
                    "default": 20
                }
            },
            "required": ["repo_path"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &dyn ToolContext,
    ) -> Result<serde_json::Value, PluginError> {
        let path = resolve_repo_path(&params, &self.config)
            .map_err(PluginError::ExecutionFailed)?;
        let max_count = params
            .get("max_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(20) as usize;

        let repo = open_repo(&path).map_err(PluginError::ExecutionFailed)?;
        let entries = git_log(&repo, max_count).map_err(PluginError::ExecutionFailed)?;
        serde_json::to_value(&entries).map_err(PluginError::from)
    }
}

// ---------------------------------------------------------------------------
// GitBlameTool
// ---------------------------------------------------------------------------

/// Tool that shows file blame (per-line last modification info).
pub struct GitBlameTool {
    config: GitConfig,
}

impl GitBlameTool {
    pub fn new(config: GitConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for GitBlameTool {
    fn name(&self) -> &str {
        "git_blame"
    }

    fn description(&self) -> &str {
        "Show per-line blame information for a file"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "repo_path": {
                    "type": "string",
                    "description": "Path to the git repository"
                },
                "file": {
                    "type": "string",
                    "description": "File path relative to the repo root"
                },
                "start_line": {
                    "type": "integer",
                    "description": "Start line (1-based, optional)"
                },
                "end_line": {
                    "type": "integer",
                    "description": "End line (1-based, optional)"
                }
            },
            "required": ["repo_path", "file"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &dyn ToolContext,
    ) -> Result<serde_json::Value, PluginError> {
        let path = resolve_repo_path(&params, &self.config)
            .map_err(PluginError::ExecutionFailed)?;
        let file = params
            .get("file")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PluginError::ExecutionFailed("file is required".into()))?;
        let start_line = params
            .get("start_line")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);
        let end_line = params
            .get("end_line")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);

        let repo = open_repo(&path).map_err(PluginError::ExecutionFailed)?;
        let blame_lines = git_blame(&repo, file, start_line, end_line)
            .map_err(PluginError::ExecutionFailed)?;
        serde_json::to_value(&blame_lines).map_err(PluginError::from)
    }
}

// ---------------------------------------------------------------------------
// GitCloneTool
// ---------------------------------------------------------------------------

/// Tool that clones a repository.
pub struct GitCloneTool {
    #[allow(dead_code)]
    config: GitConfig,
}

impl GitCloneTool {
    pub fn new(config: GitConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for GitCloneTool {
    fn name(&self) -> &str {
        "git_clone"
    }

    fn description(&self) -> &str {
        "Clone a git repository from a URL to a local path"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "URL of the repository to clone"
                },
                "path": {
                    "type": "string",
                    "description": "Local path to clone into"
                }
            },
            "required": ["url", "path"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &dyn ToolContext,
    ) -> Result<serde_json::Value, PluginError> {
        let url = params
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PluginError::ExecutionFailed("url is required".into()))?;
        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PluginError::ExecutionFailed("path is required".into()))?;

        let result = git_clone(url, path).map_err(PluginError::ExecutionFailed)?;

        Ok(serde_json::json!({
            "result": result
        }))
    }
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

/// Create all git tools with the given configuration.
pub fn all_git_tools(config: GitConfig) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(GitStatusTool::new(config.clone())),
        Box::new(GitDiffTool::new(config.clone())),
        Box::new(GitCommitTool::new(config.clone())),
        Box::new(GitBranchTool::new(config.clone())),
        Box::new(GitLogTool::new(config.clone())),
        Box::new(GitBlameTool::new(config.clone())),
        Box::new(GitCloneTool::new(config)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawft_plugin::KeyValueStore;

    struct MockKvStore;

    #[async_trait]
    impl KeyValueStore for MockKvStore {
        async fn get(&self, _key: &str) -> Result<Option<String>, PluginError> {
            Ok(None)
        }
        async fn set(&self, _key: &str, _value: &str) -> Result<(), PluginError> {
            Ok(())
        }
        async fn delete(&self, _key: &str) -> Result<bool, PluginError> {
            Ok(false)
        }
        async fn list_keys(
            &self,
            _prefix: Option<&str>,
        ) -> Result<Vec<String>, PluginError> {
            Ok(vec![])
        }
    }

    struct MockToolContext;

    impl ToolContext for MockToolContext {
        fn key_value_store(&self) -> &dyn KeyValueStore {
            &MockKvStore
        }
        fn plugin_id(&self) -> &str {
            "clawft-plugin-git"
        }
        fn agent_id(&self) -> &str {
            "test-agent"
        }
    }

    #[test]
    fn all_tools_returns_seven() {
        let tools = all_git_tools(GitConfig::default());
        assert_eq!(tools.len(), 7);

        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"git_status"));
        assert!(names.contains(&"git_diff"));
        assert!(names.contains(&"git_commit"));
        assert!(names.contains(&"git_branch"));
        assert!(names.contains(&"git_log"));
        assert!(names.contains(&"git_blame"));
        assert!(names.contains(&"git_clone"));
    }

    #[test]
    fn tool_schemas_are_objects() {
        let tools = all_git_tools(GitConfig::default());
        for tool in &tools {
            let schema = tool.parameters_schema();
            assert!(schema.is_object(), "schema not object for {}", tool.name());
            assert_eq!(schema["type"], "object");
        }
    }

    #[test]
    fn tool_descriptions_non_empty() {
        let tools = all_git_tools(GitConfig::default());
        for tool in &tools {
            assert!(
                !tool.description().is_empty(),
                "empty description for {}",
                tool.name()
            );
        }
    }

    #[tokio::test]
    async fn git_status_tool_on_test_repo() {
        let dir = tempfile::tempdir().unwrap();
        // Init repo with initial commit
        let repo = git2::Repository::init(dir.path()).unwrap();
        let sig = git2::Signature::now("Test", "test@test.com").unwrap();
        std::fs::write(dir.path().join("README.md"), "# Test\n").unwrap();
        {
            let mut index = repo.index().unwrap();
            index
                .add_path(std::path::Path::new("README.md"))
                .unwrap();
            index.write().unwrap();
            let tree_oid = index.write_tree().unwrap();
            let tree = repo.find_tree(tree_oid).unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "Init", &tree, &[])
                .unwrap();
        }

        let tool = GitStatusTool::new(GitConfig::default());
        let ctx = MockToolContext;

        let params = serde_json::json!({
            "repo_path": dir.path().to_str().unwrap()
        });

        let result = tool.execute(params, &ctx).await.unwrap();
        assert!(result.is_object());
        assert!(result.get("branch").is_some());
    }
}
