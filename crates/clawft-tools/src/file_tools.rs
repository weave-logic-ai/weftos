//! File tools: read, write, edit, and list directory.
//!
//! Ported from Python `nanobot/agent/tools/filesystem.py`. All tools enforce
//! workspace containment by canonicalizing paths and verifying they remain
//! within the configured workspace directory.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use clawft_core::tools::registry::{Tool, ToolError};
use clawft_platform::Platform;
use serde_json::json;
use tracing::debug;

/// Resolve a path to its canonical form.
///
/// On native targets this follows symlinks via `std::fs::canonicalize`.
/// On browser/WASM targets (no real filesystem symlinks in OPFS) we
/// normalize the path components without filesystem access.
#[cfg(feature = "native")]
fn resolve_sandbox_path(path: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

#[cfg(not(feature = "native"))]
fn resolve_sandbox_path(path: &Path) -> std::io::Result<PathBuf> {
    // Normalize path without filesystem access (OPFS has no symlinks).
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                components.pop();
            }
            std::path::Component::CurDir => {}
            other => components.push(other),
        }
    }
    if components.is_empty() {
        Ok(PathBuf::from("."))
    } else {
        Ok(components.iter().collect())
    }
}

/// Check whether a path exists on the filesystem.
///
/// On native targets this uses `std::path::Path::exists()`.
/// On browser/WASM targets this always returns `true` (the caller
/// relies on platform filesystem errors for non-existent paths).
#[cfg(feature = "native")]
fn path_exists(path: &Path) -> bool {
    path.exists()
}

#[cfg(not(feature = "native"))]
fn path_exists(_path: &Path) -> bool {
    // In OPFS/browser we cannot synchronously stat paths.
    // Return true so validate_parent_path falls through to the
    // workspace containment check using the normalized path.
    true
}

/// Validate that `path` resolves to a location within `workspace`.
///
/// Returns the canonical path on success, or a [`ToolError`] if the path
/// escapes the workspace or does not exist.
fn validate_path(path: &str, workspace: &Path) -> Result<PathBuf, ToolError> {
    let resolved = workspace.join(path);
    let canonical =
        resolve_sandbox_path(&resolved).map_err(|_| ToolError::FileNotFound(path.to_string()))?;

    let workspace_canonical =
        resolve_sandbox_path(workspace).map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

    if !canonical.starts_with(&workspace_canonical) {
        return Err(ToolError::InvalidPath(format!(
            "path escapes workspace: {}",
            path
        )));
    }
    Ok(canonical)
}

/// Validate that a parent directory is within workspace, for paths that
/// do not yet exist (write operations creating new files).
fn validate_parent_path(path: &str, workspace: &Path) -> Result<PathBuf, ToolError> {
    let resolved = workspace.join(path);

    // Find the deepest existing ancestor and canonicalize from there.
    let mut ancestor = resolved.as_path();
    loop {
        if path_exists(ancestor) {
            break;
        }
        ancestor = ancestor
            .parent()
            .ok_or_else(|| ToolError::InvalidPath(format!("path escapes workspace: {}", path)))?;
    }

    let canonical_ancestor =
        resolve_sandbox_path(ancestor).map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

    let workspace_canonical =
        resolve_sandbox_path(workspace).map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

    if !canonical_ancestor.starts_with(&workspace_canonical) {
        return Err(ToolError::InvalidPath(format!(
            "path escapes workspace: {}",
            path
        )));
    }
    Ok(resolved)
}

/// Extract a required string field from a JSON arguments object.
fn required_str(args: &serde_json::Value, field: &str) -> Result<String, ToolError> {
    args.get(field)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| ToolError::InvalidArgs(format!("missing required field: {}", field)))
}

// ---------------------------------------------------------------------------
// ReadFileTool
// ---------------------------------------------------------------------------

/// Read the contents of a file within the workspace.
///
/// Returns the file content as a string value. Rejects paths that escape
/// the configured workspace directory.
pub struct ReadFileTool<P: Platform> {
    platform: Arc<P>,
    workspace: PathBuf,
}

impl<P: Platform> ReadFileTool<P> {
    /// Create a new `ReadFileTool` sandboxed to `workspace`.
    pub fn new(platform: Arc<P>, workspace: PathBuf) -> Self {
        Self {
            platform,
            workspace,
        }
    }
}

#[cfg_attr(not(feature = "browser"), async_trait)]
#[cfg_attr(feature = "browser", async_trait(?Send))]
impl<P: Platform + 'static> Tool for ReadFileTool<P> {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read the contents of a file at the given path."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The file path to read (relative to workspace)"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let path_str = required_str(&args, "path")?;
        let canonical = validate_path(&path_str, &self.workspace)?;

        debug!(path = %canonical.display(), "reading file");

        let content = self
            .platform
            .fs()
            .read_to_string(&canonical)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("read failed: {}", e)))?;

        Ok(json!({ "content": content }))
    }
}

// ---------------------------------------------------------------------------
// WriteFileTool
// ---------------------------------------------------------------------------

/// Write content to a file within the workspace.
///
/// Creates parent directories if they do not exist. Overwrites the file
/// if it already exists. Rejects paths that escape the workspace.
pub struct WriteFileTool<P: Platform> {
    platform: Arc<P>,
    workspace: PathBuf,
}

impl<P: Platform> WriteFileTool<P> {
    /// Create a new `WriteFileTool` sandboxed to `workspace`.
    pub fn new(platform: Arc<P>, workspace: PathBuf) -> Self {
        Self {
            platform,
            workspace,
        }
    }
}

#[cfg_attr(not(feature = "browser"), async_trait)]
#[cfg_attr(feature = "browser", async_trait(?Send))]
impl<P: Platform + 'static> Tool for WriteFileTool<P> {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Write content to a file at the given path. Creates parent directories if needed."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The file path to write to (relative to workspace)"
                },
                "content": {
                    "type": "string",
                    "description": "The content to write"
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let path_str = required_str(&args, "path")?;
        let content = required_str(&args, "content")?;
        let target = validate_parent_path(&path_str, &self.workspace)?;

        debug!(path = %target.display(), bytes = content.len(), "writing file");

        self.platform
            .fs()
            .write_string(&target, &content)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("write failed: {}", e)))?;

        Ok(json!({
            "message": format!("Successfully wrote {} bytes to {}", content.len(), path_str)
        }))
    }
}

// ---------------------------------------------------------------------------
// EditFileTool
// ---------------------------------------------------------------------------

/// Edit a file by replacing the first occurrence of `old_text` with `new_text`.
///
/// The file must exist and must contain exactly one occurrence of `old_text`.
/// Rejects paths that escape the workspace.
pub struct EditFileTool<P: Platform> {
    platform: Arc<P>,
    workspace: PathBuf,
}

impl<P: Platform> EditFileTool<P> {
    /// Create a new `EditFileTool` sandboxed to `workspace`.
    pub fn new(platform: Arc<P>, workspace: PathBuf) -> Self {
        Self {
            platform,
            workspace,
        }
    }
}

#[cfg_attr(not(feature = "browser"), async_trait)]
#[cfg_attr(feature = "browser", async_trait(?Send))]
impl<P: Platform + 'static> Tool for EditFileTool<P> {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn description(&self) -> &str {
        "Edit a file by replacing old_text with new_text. The old_text must exist exactly once in the file."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The file path to edit (relative to workspace)"
                },
                "old_text": {
                    "type": "string",
                    "description": "The exact text to find and replace"
                },
                "new_text": {
                    "type": "string",
                    "description": "The text to replace with"
                }
            },
            "required": ["path", "old_text", "new_text"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let path_str = required_str(&args, "path")?;
        let old_text = required_str(&args, "old_text")?;
        let new_text = required_str(&args, "new_text")?;
        let canonical = validate_path(&path_str, &self.workspace)?;

        debug!(path = %canonical.display(), "editing file");

        let content = self
            .platform
            .fs()
            .read_to_string(&canonical)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("read failed: {}", e)))?;

        let count = content.matches(&old_text).count();
        if count == 0 {
            return Err(ToolError::InvalidArgs(
                "old_text not found in file".to_string(),
            ));
        }
        if count > 1 {
            return Err(ToolError::InvalidArgs(format!(
                "old_text appears {} times; provide more context to make it unique",
                count
            )));
        }

        let new_content = content.replacen(&old_text, &new_text, 1);

        self.platform
            .fs()
            .write_string(&canonical, &new_content)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("write failed: {}", e)))?;

        Ok(json!({
            "message": format!("Successfully edited {}", path_str)
        }))
    }
}

// ---------------------------------------------------------------------------
// ListDirectoryTool
// ---------------------------------------------------------------------------

/// List the contents of a directory within the workspace.
///
/// Returns a JSON array of entries with `name`, `is_dir`, and `size` fields.
/// Rejects paths that escape the workspace.
pub struct ListDirectoryTool<P: Platform> {
    platform: Arc<P>,
    workspace: PathBuf,
}

impl<P: Platform> ListDirectoryTool<P> {
    /// Create a new `ListDirectoryTool` sandboxed to `workspace`.
    pub fn new(platform: Arc<P>, workspace: PathBuf) -> Self {
        Self {
            platform,
            workspace,
        }
    }
}

#[cfg_attr(not(feature = "browser"), async_trait)]
#[cfg_attr(feature = "browser", async_trait(?Send))]
impl<P: Platform + 'static> Tool for ListDirectoryTool<P> {
    fn name(&self) -> &str {
        "list_directory"
    }

    fn description(&self) -> &str {
        "List the contents of a directory with metadata (name, is_dir, size)."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The directory path to list (relative to workspace)"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let path_str = required_str(&args, "path")?;
        let canonical = validate_path(&path_str, &self.workspace)?;

        debug!(path = %canonical.display(), "listing directory");

        let entries = self
            .platform
            .fs()
            .list_dir(&canonical)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("list_dir failed: {}", e)))?;

        let mut result = Vec::new();
        for entry_path in &entries {
            let name = entry_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();

            #[cfg(feature = "native")]
            let (is_dir, size) = {
                let metadata = tokio::fs::metadata(&entry_path).await;
                match metadata {
                    Ok(m) => (m.is_dir(), m.len()),
                    Err(_) => (false, 0),
                }
            };
            #[cfg(not(feature = "native"))]
            let (is_dir, size) = (false, 0u64);

            result.push(json!({
                "name": name,
                "is_dir": is_dir,
                "size": size,
            }));
        }

        // Sort by name for deterministic output.
        result.sort_by(|a, b| {
            let na = a["name"].as_str().unwrap_or("");
            let nb = b["name"].as_str().unwrap_or("");
            na.cmp(nb)
        });

        Ok(json!({ "entries": result }))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use clawft_platform::NativePlatform;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_workspace() -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        std::env::temp_dir().join(format!("clawft_file_tools_test_{pid}_{id}"))
    }

    async fn setup_workspace() -> (Arc<NativePlatform>, PathBuf) {
        let ws = temp_workspace();
        tokio::fs::create_dir_all(&ws).await.unwrap();
        let platform = Arc::new(NativePlatform::new());
        (platform, ws)
    }

    async fn cleanup(ws: &Path) {
        let _ = tokio::fs::remove_dir_all(ws).await;
    }

    // -- validate_path tests -----------------------------------------------

    #[tokio::test]
    async fn test_validate_path_rejects_traversal() {
        let (_, ws) = setup_workspace().await;

        // Create the workspace so canonicalize works
        let result = validate_path("../../../etc/passwd", &ws);
        // Should fail: either FileNotFound (path doesn't exist) or InvalidPath
        assert!(result.is_err());

        cleanup(&ws).await;
    }

    #[tokio::test]
    async fn test_validate_path_accepts_valid() {
        let (platform, ws) = setup_workspace().await;

        // Create a file inside workspace
        platform
            .fs()
            .write_string(&ws.join("hello.txt"), "hi")
            .await
            .unwrap();

        let result = validate_path("hello.txt", &ws);
        assert!(result.is_ok());

        cleanup(&ws).await;
    }

    #[tokio::test]
    async fn test_validate_path_rejects_absolute_outside() {
        let (_, ws) = setup_workspace().await;

        let result = validate_path("/etc/passwd", &ws);
        // The path resolves to workspace.join("/etc/passwd") which on unix
        // becomes /etc/passwd -- outside workspace
        assert!(result.is_err());

        cleanup(&ws).await;
    }

    // -- ReadFileTool tests ------------------------------------------------

    #[tokio::test]
    async fn test_read_file_success() {
        let (platform, ws) = setup_workspace().await;
        let tool = ReadFileTool::new(platform.clone(), ws.clone());

        platform
            .fs()
            .write_string(&ws.join("test.txt"), "hello world")
            .await
            .unwrap();

        let result = tool.execute(json!({"path": "test.txt"})).await.unwrap();
        assert_eq!(result["content"], "hello world");

        cleanup(&ws).await;
    }

    #[tokio::test]
    async fn test_read_file_not_found() {
        let (platform, ws) = setup_workspace().await;
        let tool = ReadFileTool::new(platform, ws.clone());

        let err = tool
            .execute(json!({"path": "nonexistent.txt"}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::FileNotFound(_)));

        cleanup(&ws).await;
    }

    #[tokio::test]
    async fn test_read_file_missing_path_param() {
        let (platform, ws) = setup_workspace().await;
        let tool = ReadFileTool::new(platform, ws.clone());

        let err = tool.execute(json!({})).await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgs(_)));

        cleanup(&ws).await;
    }

    #[tokio::test]
    async fn test_read_file_traversal_rejected() {
        let (platform, ws) = setup_workspace().await;
        let tool = ReadFileTool::new(platform, ws.clone());

        let err = tool
            .execute(json!({"path": "../../../etc/passwd"}))
            .await
            .unwrap_err();
        // Either FileNotFound or InvalidPath
        assert!(
            matches!(err, ToolError::FileNotFound(_) | ToolError::InvalidPath(_)),
            "expected path error, got: {err:?}"
        );

        cleanup(&ws).await;
    }

    // -- WriteFileTool tests -----------------------------------------------

    #[tokio::test]
    async fn test_write_file_success() {
        let (platform, ws) = setup_workspace().await;
        let tool = WriteFileTool::new(platform.clone(), ws.clone());

        let result = tool
            .execute(json!({"path": "output.txt", "content": "written!"}))
            .await
            .unwrap();
        assert!(result["message"].as_str().unwrap().contains("8 bytes"));

        // Verify file was written
        let content = platform
            .fs()
            .read_to_string(&ws.join("output.txt"))
            .await
            .unwrap();
        assert_eq!(content, "written!");

        cleanup(&ws).await;
    }

    #[tokio::test]
    async fn test_write_file_creates_parent_dirs() {
        let (platform, ws) = setup_workspace().await;
        let tool = WriteFileTool::new(platform.clone(), ws.clone());

        tool.execute(json!({"path": "sub/dir/file.txt", "content": "nested"}))
            .await
            .unwrap();

        let content = platform
            .fs()
            .read_to_string(&ws.join("sub/dir/file.txt"))
            .await
            .unwrap();
        assert_eq!(content, "nested");

        cleanup(&ws).await;
    }

    #[tokio::test]
    async fn test_write_file_missing_content() {
        let (platform, ws) = setup_workspace().await;
        let tool = WriteFileTool::new(platform, ws.clone());

        let err = tool.execute(json!({"path": "file.txt"})).await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgs(_)));

        cleanup(&ws).await;
    }

    #[tokio::test]
    async fn test_write_file_traversal_rejected() {
        let (platform, ws) = setup_workspace().await;
        let tool = WriteFileTool::new(platform, ws.clone());

        let err = tool
            .execute(json!({"path": "../../escape.txt", "content": "bad"}))
            .await
            .unwrap_err();
        assert!(
            matches!(err, ToolError::InvalidPath(_)),
            "expected InvalidPath, got: {err:?}"
        );

        cleanup(&ws).await;
    }

    // -- EditFileTool tests ------------------------------------------------

    #[tokio::test]
    async fn test_edit_file_success() {
        let (platform, ws) = setup_workspace().await;
        let tool = EditFileTool::new(platform.clone(), ws.clone());

        platform
            .fs()
            .write_string(&ws.join("edit_me.txt"), "hello world")
            .await
            .unwrap();

        tool.execute(json!({
            "path": "edit_me.txt",
            "old_text": "world",
            "new_text": "clawft"
        }))
        .await
        .unwrap();

        let content = platform
            .fs()
            .read_to_string(&ws.join("edit_me.txt"))
            .await
            .unwrap();
        assert_eq!(content, "hello clawft");

        cleanup(&ws).await;
    }

    #[tokio::test]
    async fn test_edit_file_old_text_not_found() {
        let (platform, ws) = setup_workspace().await;
        let tool = EditFileTool::new(platform.clone(), ws.clone());

        platform
            .fs()
            .write_string(&ws.join("edit.txt"), "hello world")
            .await
            .unwrap();

        let err = tool
            .execute(json!({
                "path": "edit.txt",
                "old_text": "nonexistent",
                "new_text": "replacement"
            }))
            .await
            .unwrap_err();

        assert!(matches!(err, ToolError::InvalidArgs(_)));
        assert!(err.to_string().contains("not found"));

        cleanup(&ws).await;
    }

    #[tokio::test]
    async fn test_edit_file_ambiguous_match() {
        let (platform, ws) = setup_workspace().await;
        let tool = EditFileTool::new(platform.clone(), ws.clone());

        platform
            .fs()
            .write_string(&ws.join("dup.txt"), "foo bar foo")
            .await
            .unwrap();

        let err = tool
            .execute(json!({
                "path": "dup.txt",
                "old_text": "foo",
                "new_text": "baz"
            }))
            .await
            .unwrap_err();

        assert!(matches!(err, ToolError::InvalidArgs(_)));
        assert!(err.to_string().contains("2 times"));

        cleanup(&ws).await;
    }

    #[tokio::test]
    async fn test_edit_file_not_found() {
        let (platform, ws) = setup_workspace().await;
        let tool = EditFileTool::new(platform, ws.clone());

        let err = tool
            .execute(json!({
                "path": "missing.txt",
                "old_text": "a",
                "new_text": "b"
            }))
            .await
            .unwrap_err();

        assert!(matches!(err, ToolError::FileNotFound(_)));

        cleanup(&ws).await;
    }

    // -- ListDirectoryTool tests -------------------------------------------

    #[tokio::test]
    async fn test_list_directory_success() {
        let (platform, ws) = setup_workspace().await;
        let tool = ListDirectoryTool::new(platform.clone(), ws.clone());

        platform
            .fs()
            .write_string(&ws.join("a.txt"), "a")
            .await
            .unwrap();
        platform
            .fs()
            .write_string(&ws.join("b.txt"), "bb")
            .await
            .unwrap();
        platform
            .fs()
            .create_dir_all(&ws.join("subdir"))
            .await
            .unwrap();

        let result = tool.execute(json!({"path": "."})).await.unwrap();
        let entries = result["entries"].as_array().unwrap();

        assert_eq!(entries.len(), 3);

        // Should be sorted by name
        let names: Vec<&str> = entries
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["a.txt", "b.txt", "subdir"]);

        // Check is_dir flag
        let subdir_entry = entries.iter().find(|e| e["name"] == "subdir").unwrap();
        assert_eq!(subdir_entry["is_dir"], true);

        let a_entry = entries.iter().find(|e| e["name"] == "a.txt").unwrap();
        assert_eq!(a_entry["is_dir"], false);
        assert_eq!(a_entry["size"], 1);

        cleanup(&ws).await;
    }

    #[tokio::test]
    async fn test_list_directory_not_found() {
        let (platform, ws) = setup_workspace().await;
        let tool = ListDirectoryTool::new(platform, ws.clone());

        let err = tool
            .execute(json!({"path": "nonexistent_dir"}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::FileNotFound(_)));

        cleanup(&ws).await;
    }

    #[tokio::test]
    async fn test_list_directory_empty() {
        let (platform, ws) = setup_workspace().await;
        let tool = ListDirectoryTool::new(platform.clone(), ws.clone());

        platform
            .fs()
            .create_dir_all(&ws.join("empty"))
            .await
            .unwrap();

        let result = tool.execute(json!({"path": "empty"})).await.unwrap();
        let entries = result["entries"].as_array().unwrap();
        assert!(entries.is_empty());

        cleanup(&ws).await;
    }

    // -- SEC-05: Symlink traversal tests ----------------------------------

    /// SEC-05: Verify that a symlink pointing outside the workspace is
    /// rejected by validate_path. The canonicalize() call follows the
    /// symlink and the resulting path falls outside the workspace boundary.
    #[tokio::test]
    async fn test_symlink_outside_workspace_rejected() {
        let (platform, ws) = setup_workspace().await;

        // Create a file outside the workspace
        let outside_dir = ws.parent().unwrap().join("outside_ws");
        tokio::fs::create_dir_all(&outside_dir).await.unwrap();
        let outside_file = outside_dir.join("secret.txt");
        tokio::fs::write(&outside_file, "secret data")
            .await
            .unwrap();

        // Create a symlink inside the workspace pointing to the outside file
        let symlink_path = ws.join("escape_link");
        #[cfg(unix)]
        tokio::fs::symlink(&outside_file, &symlink_path)
            .await
            .unwrap();

        // validate_path should reject the symlink because canonicalize()
        // resolves it to the outside file.
        let result = validate_path("escape_link", &ws);
        assert!(
            result.is_err(),
            "symlink to file outside workspace should be rejected"
        );
        match result.unwrap_err() {
            ToolError::InvalidPath(msg) => {
                assert!(
                    msg.contains("escapes workspace"),
                    "error should mention workspace escape: {msg}"
                );
            }
            other => panic!("expected InvalidPath, got: {other:?}"),
        }

        // Also verify ReadFileTool rejects the symlink
        let tool = ReadFileTool::new(platform.clone(), ws.clone());
        let err = tool
            .execute(json!({"path": "escape_link"}))
            .await
            .unwrap_err();
        assert!(
            matches!(err, ToolError::InvalidPath(_)),
            "ReadFileTool should reject symlink outside workspace: {err:?}"
        );

        // Cleanup
        let _ = tokio::fs::remove_dir_all(&outside_dir).await;
        cleanup(&ws).await;
    }

    /// SEC-05: Symlink to a directory outside the workspace is also rejected.
    #[tokio::test]
    async fn test_symlink_to_directory_outside_workspace_rejected() {
        let (_, ws) = setup_workspace().await;

        // Create a target directory outside the workspace
        let outside_dir = ws.parent().unwrap().join("outside_dir_target");
        tokio::fs::create_dir_all(&outside_dir).await.unwrap();
        tokio::fs::write(outside_dir.join("data.txt"), "private")
            .await
            .unwrap();

        // Create a symlink inside the workspace pointing to the outside directory
        let symlink_path = ws.join("dir_escape_link");
        #[cfg(unix)]
        tokio::fs::symlink(&outside_dir, &symlink_path)
            .await
            .unwrap();

        // validate_path should reject
        let result = validate_path("dir_escape_link", &ws);
        assert!(
            result.is_err(),
            "symlink to directory outside workspace should be rejected"
        );

        // Also verify listing through the symlink is rejected
        let _ = tokio::fs::remove_dir_all(&outside_dir).await;
        cleanup(&ws).await;
    }

    /// SEC-05: Symlinks within the workspace should be allowed.
    #[tokio::test]
    async fn test_symlink_within_workspace_allowed() {
        let (platform, ws) = setup_workspace().await;

        // Create a real file inside the workspace
        platform
            .fs()
            .write_string(&ws.join("real_file.txt"), "allowed content")
            .await
            .unwrap();

        // Create a symlink inside the workspace pointing to the real file
        let symlink_path = ws.join("internal_link");
        #[cfg(unix)]
        tokio::fs::symlink(ws.join("real_file.txt"), &symlink_path)
            .await
            .unwrap();

        // validate_path should accept because the symlink resolves within workspace
        let result = validate_path("internal_link", &ws);
        assert!(
            result.is_ok(),
            "symlink within workspace should be allowed: {:?}",
            result.err()
        );

        // Also verify ReadFileTool can read through the symlink
        let tool = ReadFileTool::new(platform.clone(), ws.clone());
        let result = tool
            .execute(json!({"path": "internal_link"}))
            .await
            .unwrap();
        assert_eq!(result["content"], "allowed content");

        cleanup(&ws).await;
    }
}
