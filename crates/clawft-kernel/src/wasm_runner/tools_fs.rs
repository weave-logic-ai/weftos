//! Built-in filesystem tool implementations.

use chrono::{DateTime, Utc};

use super::catalog::builtin_tool_catalog;
use super::registry::BuiltinTool;
use super::types::*;

/// Max file read size (8 MiB, matching PluginSandbox).
const MAX_READ_SIZE: u64 = 8 * 1024 * 1024;

/// Built-in `fs.read_file` tool.
///
/// Reads file contents with optional offset and limit.
/// Always runs natively (no WASM needed for reference impl).
/// Supports multi-layer sandboxing via [`SandboxConfig`].
pub struct FsReadFileTool {
    spec: BuiltinToolSpec,
    sandbox: SandboxConfig,
}

impl Default for FsReadFileTool {
    fn default() -> Self {
        Self::new()
    }
}

impl FsReadFileTool {
    pub fn new() -> Self {
        let catalog = builtin_tool_catalog();
        let spec = catalog
            .into_iter()
            .find(|s| s.name == "fs.read_file")
            .expect("fs.read_file must be in catalog");
        Self {
            spec,
            sandbox: SandboxConfig::default(),
        }
    }

    /// Create a sandboxed instance that restricts file access.
    pub fn with_sandbox(sandbox: SandboxConfig) -> Self {
        let catalog = builtin_tool_catalog();
        let spec = catalog
            .into_iter()
            .find(|s| s.name == "fs.read_file")
            .expect("fs.read_file must be in catalog");
        Self { spec, sandbox }
    }
}

impl BuiltinTool for FsReadFileTool {
    fn name(&self) -> &str {
        "fs.read_file"
    }

    fn spec(&self) -> &BuiltinToolSpec {
        &self.spec
    }

    fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("missing 'path' parameter".into()))?;

        let path = std::path::Path::new(path);

        // Sandbox path check (K4 B1)
        if !self.sandbox.is_path_allowed(path) {
            return Err(ToolError::PermissionDenied(format!(
                "path outside sandbox: {}",
                path.display()
            )));
        }

        if !path.exists() {
            return Err(ToolError::FileNotFound(path.display().to_string()));
        }

        let metadata =
            std::fs::metadata(path).map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        if metadata.len() > MAX_READ_SIZE {
            return Err(ToolError::FileTooLarge {
                size: metadata.len(),
                limit: MAX_READ_SIZE,
            });
        }

        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);

        let bytes = std::fs::read(path).map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        let end = match limit {
            Some(l) => std::cmp::min(offset + l, bytes.len()),
            None => bytes.len(),
        };
        let start = std::cmp::min(offset, bytes.len());
        let slice = &bytes[start..end];

        let content = String::from_utf8_lossy(slice).into_owned();
        let modified = metadata
            .modified()
            .ok()
            .map(|t| {
                let dt: DateTime<Utc> = t.into();
                dt.to_rfc3339()
            })
            .unwrap_or_default();

        Ok(serde_json::json!({
            "content": content,
            "size": metadata.len(),
            "modified": modified,
        }))
    }
}

/// Built-in `fs.write_file` tool.
pub struct FsWriteFileTool {
    spec: BuiltinToolSpec,
    sandbox: SandboxConfig,
}

impl FsWriteFileTool {
    pub fn new() -> Self {
        let spec = builtin_tool_catalog()
            .into_iter()
            .find(|s| s.name == "fs.write_file")
            .unwrap();
        Self {
            spec,
            sandbox: SandboxConfig::default(),
        }
    }
    pub fn with_sandbox(sandbox: SandboxConfig) -> Self {
        let spec = builtin_tool_catalog()
            .into_iter()
            .find(|s| s.name == "fs.write_file")
            .unwrap();
        Self { spec, sandbox }
    }
}

impl BuiltinTool for FsWriteFileTool {
    fn name(&self) -> &str {
        "fs.write_file"
    }
    fn spec(&self) -> &BuiltinToolSpec {
        &self.spec
    }
    fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let path_str = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("missing 'path'".into()))?;
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("missing 'content'".into()))?;
        let append = args
            .get("append")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let path = std::path::Path::new(path_str);
        if !self.sandbox.is_path_allowed(path) {
            return Err(ToolError::PermissionDenied(format!(
                "path outside sandbox: {}",
                path.display()
            )));
        }
        if append {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
            f.write_all(content.as_bytes())
                .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        } else {
            std::fs::write(path, content).map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        }

        // Chain event: log filesystem write
        #[cfg(feature = "exochain")]
        clawft_core::chain_event::push_chain_event(
            "wasm_fs",
            "wasm.fs.write",
            Some(
                serde_json::json!({"path": path_str, "bytes_written": content.len(), "append": append}),
            ),
        );

        Ok(serde_json::json!({"written": content.len(), "path": path_str}))
    }
}

/// Built-in `fs.read_dir` tool.
pub struct FsReadDirTool {
    spec: BuiltinToolSpec,
    sandbox: SandboxConfig,
}

impl FsReadDirTool {
    pub fn new() -> Self {
        let spec = builtin_tool_catalog()
            .into_iter()
            .find(|s| s.name == "fs.read_dir")
            .unwrap();
        Self {
            spec,
            sandbox: SandboxConfig::default(),
        }
    }
    pub fn with_sandbox(sandbox: SandboxConfig) -> Self {
        let spec = builtin_tool_catalog()
            .into_iter()
            .find(|s| s.name == "fs.read_dir")
            .unwrap();
        Self { spec, sandbox }
    }
}

impl BuiltinTool for FsReadDirTool {
    fn name(&self) -> &str {
        "fs.read_dir"
    }
    fn spec(&self) -> &BuiltinToolSpec {
        &self.spec
    }
    fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let path_str = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("missing 'path'".into()))?;
        let path = std::path::Path::new(path_str);
        if !self.sandbox.is_path_allowed(path) {
            return Err(ToolError::PermissionDenied(format!(
                "path outside sandbox: {}",
                path.display()
            )));
        }
        if !path.exists() {
            return Err(ToolError::FileNotFound(path.display().to_string()));
        }
        let entries: Vec<serde_json::Value> = std::fs::read_dir(path)
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?
            .filter_map(|e| e.ok())
            .map(|e| {
                let ft = e.file_type().ok();
                serde_json::json!({
                    "name": e.file_name().to_string_lossy(),
                    "is_dir": ft.as_ref().map(|t| t.is_dir()).unwrap_or(false),
                    "is_file": ft.as_ref().map(|t| t.is_file()).unwrap_or(false),
                })
            })
            .collect();
        Ok(serde_json::json!({"entries": entries, "count": entries.len()}))
    }
}

/// Built-in `fs.create_dir` tool.
pub struct FsCreateDirTool {
    spec: BuiltinToolSpec,
    sandbox: SandboxConfig,
}

impl FsCreateDirTool {
    pub fn new() -> Self {
        let spec = builtin_tool_catalog()
            .into_iter()
            .find(|s| s.name == "fs.create_dir")
            .unwrap();
        Self {
            spec,
            sandbox: SandboxConfig::default(),
        }
    }
}

impl BuiltinTool for FsCreateDirTool {
    fn name(&self) -> &str {
        "fs.create_dir"
    }
    fn spec(&self) -> &BuiltinToolSpec {
        &self.spec
    }
    fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let path_str = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("missing 'path'".into()))?;
        let path = std::path::Path::new(path_str);
        if !self.sandbox.is_path_allowed(path) {
            return Err(ToolError::PermissionDenied(format!(
                "path outside sandbox: {}",
                path.display()
            )));
        }
        let recursive = args
            .get("recursive")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        if recursive {
            std::fs::create_dir_all(path).map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        } else {
            std::fs::create_dir(path).map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        }

        // Chain event: log directory creation
        #[cfg(feature = "exochain")]
        clawft_core::chain_event::push_chain_event(
            "wasm_fs",
            "wasm.fs.create_dir",
            Some(serde_json::json!({"path": path_str, "recursive": recursive})),
        );

        Ok(serde_json::json!({"created": path_str}))
    }
}

/// Built-in `fs.remove` tool.
pub struct FsRemoveTool {
    spec: BuiltinToolSpec,
    sandbox: SandboxConfig,
}

impl FsRemoveTool {
    pub fn new() -> Self {
        let spec = builtin_tool_catalog()
            .into_iter()
            .find(|s| s.name == "fs.remove")
            .unwrap();
        Self {
            spec,
            sandbox: SandboxConfig::default(),
        }
    }
}

impl BuiltinTool for FsRemoveTool {
    fn name(&self) -> &str {
        "fs.remove"
    }
    fn spec(&self) -> &BuiltinToolSpec {
        &self.spec
    }
    fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let path_str = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("missing 'path'".into()))?;
        let path = std::path::Path::new(path_str);
        if !self.sandbox.is_path_allowed(path) {
            return Err(ToolError::PermissionDenied(format!(
                "path outside sandbox: {}",
                path.display()
            )));
        }
        if !path.exists() {
            return Err(ToolError::FileNotFound(path.display().to_string()));
        }
        let recursive = args
            .get("recursive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if path.is_dir() {
            if recursive {
                std::fs::remove_dir_all(path)
                    .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
            } else {
                std::fs::remove_dir(path).map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
            }
        } else {
            std::fs::remove_file(path).map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        }

        // Chain event: log filesystem removal
        #[cfg(feature = "exochain")]
        clawft_core::chain_event::push_chain_event(
            "wasm_fs",
            "wasm.fs.remove",
            Some(serde_json::json!({"path": path_str, "recursive": recursive})),
        );

        Ok(serde_json::json!({"removed": path_str}))
    }
}

/// Built-in `fs.copy` tool.
pub struct FsCopyTool {
    spec: BuiltinToolSpec,
    sandbox: SandboxConfig,
}

impl FsCopyTool {
    pub fn new() -> Self {
        let spec = builtin_tool_catalog()
            .into_iter()
            .find(|s| s.name == "fs.copy")
            .unwrap();
        Self {
            spec,
            sandbox: SandboxConfig::default(),
        }
    }
}

impl BuiltinTool for FsCopyTool {
    fn name(&self) -> &str {
        "fs.copy"
    }
    fn spec(&self) -> &BuiltinToolSpec {
        &self.spec
    }
    fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let src_str = args
            .get("src")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("missing 'src'".into()))?;
        let dst_str = args
            .get("dst")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("missing 'dst'".into()))?;
        let src = std::path::Path::new(src_str);
        let dst = std::path::Path::new(dst_str);
        if !self.sandbox.is_path_allowed(src) || !self.sandbox.is_path_allowed(dst) {
            return Err(ToolError::PermissionDenied("path outside sandbox".into()));
        }
        if !src.exists() {
            return Err(ToolError::FileNotFound(src.display().to_string()));
        }
        let bytes =
            std::fs::copy(src, dst).map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        // Chain event: log filesystem copy
        #[cfg(feature = "exochain")]
        clawft_core::chain_event::push_chain_event(
            "wasm_fs",
            "wasm.fs.copy",
            Some(serde_json::json!({"src": src_str, "dst": dst_str, "bytes_copied": bytes})),
        );

        Ok(serde_json::json!({"copied": bytes, "src": src_str, "dst": dst_str}))
    }
}

/// Built-in `fs.move` tool.
pub struct FsMoveTool {
    spec: BuiltinToolSpec,
    sandbox: SandboxConfig,
}

impl FsMoveTool {
    pub fn new() -> Self {
        let spec = builtin_tool_catalog()
            .into_iter()
            .find(|s| s.name == "fs.move")
            .unwrap();
        Self {
            spec,
            sandbox: SandboxConfig::default(),
        }
    }
}

impl BuiltinTool for FsMoveTool {
    fn name(&self) -> &str {
        "fs.move"
    }
    fn spec(&self) -> &BuiltinToolSpec {
        &self.spec
    }
    fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let src_str = args
            .get("src")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("missing 'src'".into()))?;
        let dst_str = args
            .get("dst")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("missing 'dst'".into()))?;
        let src = std::path::Path::new(src_str);
        let dst = std::path::Path::new(dst_str);
        if !self.sandbox.is_path_allowed(src) || !self.sandbox.is_path_allowed(dst) {
            return Err(ToolError::PermissionDenied("path outside sandbox".into()));
        }
        if !src.exists() {
            return Err(ToolError::FileNotFound(src.display().to_string()));
        }
        std::fs::rename(src, dst).map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        // Chain event: log filesystem move
        #[cfg(feature = "exochain")]
        clawft_core::chain_event::push_chain_event(
            "wasm_fs",
            "wasm.fs.move",
            Some(serde_json::json!({"src": src_str, "dst": dst_str})),
        );

        Ok(serde_json::json!({"moved": true, "src": src_str, "dst": dst_str}))
    }
}

/// Built-in `fs.stat` tool.
pub struct FsStatTool {
    spec: BuiltinToolSpec,
    sandbox: SandboxConfig,
}

impl FsStatTool {
    pub fn new() -> Self {
        let spec = builtin_tool_catalog()
            .into_iter()
            .find(|s| s.name == "fs.stat")
            .unwrap();
        Self {
            spec,
            sandbox: SandboxConfig::default(),
        }
    }
}

impl BuiltinTool for FsStatTool {
    fn name(&self) -> &str {
        "fs.stat"
    }
    fn spec(&self) -> &BuiltinToolSpec {
        &self.spec
    }
    fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let path_str = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("missing 'path'".into()))?;
        let path = std::path::Path::new(path_str);
        if !self.sandbox.is_path_allowed(path) {
            return Err(ToolError::PermissionDenied(format!(
                "path outside sandbox: {}",
                path.display()
            )));
        }
        let meta =
            std::fs::metadata(path).map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        let modified = meta
            .modified()
            .ok()
            .map(|t| {
                let dt: DateTime<Utc> = t.into();
                dt.to_rfc3339()
            })
            .unwrap_or_default();
        Ok(serde_json::json!({
            "size": meta.len(),
            "is_file": meta.is_file(),
            "is_dir": meta.is_dir(),
            "readonly": meta.permissions().readonly(),
            "modified": modified,
        }))
    }
}

/// Built-in `fs.exists` tool.
pub struct FsExistsTool {
    spec: BuiltinToolSpec,
}

impl FsExistsTool {
    pub fn new() -> Self {
        let spec = builtin_tool_catalog()
            .into_iter()
            .find(|s| s.name == "fs.exists")
            .unwrap();
        Self { spec }
    }
}

impl BuiltinTool for FsExistsTool {
    fn name(&self) -> &str {
        "fs.exists"
    }
    fn spec(&self) -> &BuiltinToolSpec {
        &self.spec
    }
    fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let path_str = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("missing 'path'".into()))?;
        let path = std::path::Path::new(path_str);
        let exists = path.exists();
        let is_file = path.is_file();
        let is_dir = path.is_dir();
        Ok(serde_json::json!({"exists": exists, "is_file": is_file, "is_dir": is_dir}))
    }
}

/// Built-in `fs.glob` tool.
pub struct FsGlobTool {
    spec: BuiltinToolSpec,
    sandbox: SandboxConfig,
}

impl FsGlobTool {
    pub fn new() -> Self {
        let spec = builtin_tool_catalog()
            .into_iter()
            .find(|s| s.name == "fs.glob")
            .unwrap();
        Self {
            spec,
            sandbox: SandboxConfig::default(),
        }
    }
}

impl BuiltinTool for FsGlobTool {
    fn name(&self) -> &str {
        "fs.glob"
    }
    fn spec(&self) -> &BuiltinToolSpec {
        &self.spec
    }
    fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let pattern = args
            .get("pattern")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("missing 'pattern'".into()))?;
        let base_dir = args.get("base_dir").and_then(|v| v.as_str()).unwrap_or(".");
        let base = std::path::Path::new(base_dir);
        if !self.sandbox.is_path_allowed(base) {
            return Err(ToolError::PermissionDenied(
                "base_dir outside sandbox".into(),
            ));
        }
        // Simple recursive walk with pattern matching
        let mut matches = Vec::new();
        fn walk(dir: &std::path::Path, pattern: &str, matches: &mut Vec<String>) {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    if simple_glob_match(pattern, &name) {
                        matches.push(path.display().to_string());
                    }
                    if path.is_dir() {
                        walk(&path, pattern, matches);
                    }
                }
            }
        }
        walk(base, pattern, &mut matches);
        matches.sort();
        Ok(serde_json::json!({"matches": matches, "count": matches.len()}))
    }
}

/// Simple glob pattern match supporting `*` and `?` wildcards.
pub(crate) fn simple_glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    simple_glob_match_inner(&p, &t, 0, 0)
}

fn simple_glob_match_inner(pattern: &[char], text: &[char], pi: usize, ti: usize) -> bool {
    if pi == pattern.len() && ti == text.len() {
        return true;
    }
    if pi == pattern.len() {
        return false;
    }
    match pattern[pi] {
        '*' => {
            // Match zero or more characters
            for i in ti..=text.len() {
                if simple_glob_match_inner(pattern, text, pi + 1, i) {
                    return true;
                }
            }
            false
        }
        '?' => {
            if ti < text.len() {
                simple_glob_match_inner(pattern, text, pi + 1, ti + 1)
            } else {
                false
            }
        }
        c => {
            if ti < text.len() && text[ti] == c {
                simple_glob_match_inner(pattern, text, pi + 1, ti + 1)
            } else {
                false
            }
        }
    }
}
