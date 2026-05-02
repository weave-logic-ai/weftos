//! Tool registry and [`Tool`] trait definition.
//!
//! Defines the interface that all tool implementations must satisfy
//! ([`Tool`]) and provides a [`ToolRegistry`] that stores registered
//! tools and dispatches execution requests by name.
//!
//! Tool implementations live in the `clawft-tools` crate; this module
//! only defines the contract and registry infrastructure.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::debug;

use clawft_types::routing::UserPermissions;

#[cfg(feature = "native")]
use std::sync::Mutex as StdMutex;
#[cfg(feature = "native")]
use std::sync::OnceLock;
#[cfg(feature = "native")]
use std::path::PathBuf;

// ── WEFT-37: per-path advisory locks ────────────────────────────────
//
// D1 of the pipeline-reliability sprint shipped parallel tool execution
// (see `agent/loop_core.rs` — `futures::join_all` over per-tool-call
// futures) but explicitly punted on per-path serialization. The hazard:
// two tools writing the same canonical path concurrently, producing
// torn reads or interleaved writes. This module provides a process-
// global `path → tokio::Mutex` map. When a tool whose argument JSON
// references a path is executed, the registry acquires the
// corresponding mutex for the duration of the call, serialising
// same-path executions while letting non-overlapping paths run fully
// in parallel.
//
// The lock map is native-only because:
//  - browser/wasm has a single-threaded executor — concurrent FS
//    operations on the same path can't interleave there.
//  - tokio::sync::Mutex is the natural fit for "hold across await
//    points"; pulling in a wasm-friendly equivalent for a hazard that
//    only exists on native is gold-plating.

#[cfg(feature = "native")]
type PathLockMap = HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>;

#[cfg(feature = "native")]
static PATH_LOCKS: OnceLock<StdMutex<PathLockMap>> = OnceLock::new();

#[cfg(feature = "native")]
fn path_locks() -> &'static StdMutex<PathLockMap> {
    PATH_LOCKS.get_or_init(|| StdMutex::new(HashMap::new()))
}

/// Names of common JSON keys that carry filesystem paths in the
/// built-in tool argument schema. Used to extract the path(s) to lock
/// before executing a tool.
#[cfg(feature = "native")]
const PATH_ARG_KEYS: &[&str] = &["path", "file", "file_path", "filepath", "target"];

/// Extract candidate path strings from a tool argument JSON value.
///
/// Returns one [`PathBuf`] per recognised key in [`PATH_ARG_KEYS`].
/// Multi-path tools (e.g. a hypothetical `copy {from, to}`) can lock
/// both keys via the canonical separator semantics — we just collect
/// every key found.
#[cfg(feature = "native")]
fn extract_paths(args: &serde_json::Value) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Some(obj) = args.as_object() else {
        return out;
    };
    for key in PATH_ARG_KEYS {
        if let Some(s) = obj.get(*key).and_then(|v| v.as_str())
            && !s.is_empty()
        {
            out.push(canonicalize_for_lock(s));
        }
    }
    out
}

/// Canonicalise a path string for use as a lock key. Two callers
/// referring to the same file via different relative spellings must
/// land on the same lock entry, so we resolve to an absolute path
/// where possible and fall back to a normalised representation.
///
/// We deliberately do NOT call `std::fs::canonicalize` (which requires
/// the file to exist and would fail for new files); instead we
/// normalise components with `Path::components` to collapse `.` and
/// `..` segments without touching the filesystem.
#[cfg(feature = "native")]
fn canonicalize_for_lock(raw: &str) -> PathBuf {
    use std::path::{Component, Path};
    let p = Path::new(raw);
    let absolute = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
            .join(p)
    };
    let mut out = PathBuf::new();
    for c in absolute.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Acquire (and lazily-create) the advisory lock for the given path.
///
/// The returned `OwnedMutexGuard` keeps the lock map entry alive and
/// is dropped at the end of the tool execution scope. The lock entries
/// are kept in the map indefinitely — the working set of paths a
/// long-lived agent touches is bounded enough in practice that we
/// don't bother garbage-collecting unused entries.
#[cfg(feature = "native")]
async fn acquire_path_lock(
    path: PathBuf,
) -> tokio::sync::OwnedMutexGuard<()> {
    let lock = {
        let mut map = path_locks().lock().expect("path lock map poisoned");
        map.entry(path)
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    };
    lock.lock_owned().await
}

/// Test helper: clear the global advisory lock map between tests so
/// each test starts with a clean state. Inside the `cfg(test)`
/// boundary; the production code path never calls this.
#[cfg(all(test, feature = "native"))]
pub(crate) fn _clear_path_locks_for_test() {
    if let Some(map) = PATH_LOCKS.get() {
        map.lock().expect("path lock map poisoned").clear();
    }
}

/// Error type for tool execution.
///
/// Covers the common failure modes: unknown tool, bad arguments,
/// runtime failures, permission issues, and timeouts.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    /// The requested tool was not found in the registry.
    #[error("tool not found: {0}")]
    NotFound(String),

    /// The arguments provided to the tool are invalid.
    #[error("invalid arguments: {0}")]
    InvalidArgs(String),

    /// The tool execution failed at runtime.
    #[error("execution failed: {0}")]
    ExecutionFailed(String),

    /// The caller lacks permission to invoke this tool.
    ///
    /// `tool` is the name of the tool that was denied.
    /// `reason` explains why (not in allowlist, explicitly denied,
    /// insufficient level, missing custom permission).
    #[error("permission denied for tool '{tool}': {reason}")]
    PermissionDenied { tool: String, reason: String },

    /// A file or resource the tool needs was not found.
    #[error("not found: {0}")]
    FileNotFound(String),

    /// A filesystem path is invalid or traverses outside allowed boundaries.
    #[error("invalid path: {0}")]
    InvalidPath(String),

    /// The tool execution exceeded the allowed time limit.
    #[error("timeout after {0}s")]
    Timeout(u64),
}

// ---------------------------------------------------------------------------
// ToolMetadata
// ---------------------------------------------------------------------------

/// Permission metadata that a tool can declare.
///
/// Built-in tools define this via the `Tool::metadata()` trait method.
/// MCP tools derive this from their server's tool declaration JSON.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolMetadata {
    /// Minimum permission level required to invoke this tool.
    /// `None` means no level requirement beyond the allowlist check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_permission_level: Option<u8>,

    /// Custom permission keys and values that must match.
    /// Example: `{"exec_enabled": true}` requires the user to have
    /// `custom_permissions["exec_enabled"] == true`.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub required_custom_permissions: HashMap<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Glob matching
// ---------------------------------------------------------------------------

/// Match a tool name against a glob pattern.
///
/// Supports `*` (matches zero or more characters) and `?` (matches exactly
/// one character). This is a minimal implementation sufficient for tool
/// access patterns without pulling in a full glob crate.
///
/// Examples:
///   `glob_matches("file_*", "file_read")` -> true
///   `glob_matches("file_*", "web_search")` -> false
///   `glob_matches("*", "anything")` -> true
///   `glob_matches("read_?", "read_a")` -> true
///   `glob_matches("read_?", "read_file")` -> false
fn glob_matches(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let (plen, tlen) = (pattern.len(), text.len());

    let mut pi = 0;
    let mut ti = 0;
    let mut star_pi = None;
    let mut star_ti = 0;

    while ti < tlen {
        if pi < plen && (pattern[pi] == '?' || pattern[pi] == text[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < plen && pattern[pi] == '*' {
            star_pi = Some(pi);
            star_ti = ti;
            pi += 1;
        } else if let Some(spi) = star_pi {
            pi = spi + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }

    // Consume trailing stars in pattern.
    while pi < plen && pattern[pi] == '*' {
        pi += 1;
    }

    pi == plen
}

/// Check whether a tool name matches any pattern in the given list.
/// Each entry in `patterns` is either an exact name or a glob pattern.
fn matches_any_pattern(tool_name: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|pattern| {
        if pattern.contains('*') || pattern.contains('?') {
            glob_matches(pattern, tool_name)
        } else {
            pattern == tool_name
        }
    })
}

// ---------------------------------------------------------------------------
// Permission checking
// ---------------------------------------------------------------------------

/// Check whether the given permissions allow invoking the named tool.
///
/// This is a pure function with no side effects -- suitable for
/// unit testing without async or I/O.
///
/// Evaluation order:
///   1. Denylist (checked first, deny overrides everything)
///   2. Allowlist (empty = deny all, `["*"]` = allow all, else pattern match)
///   3. Tool metadata `required_permission_level`
///   4. Tool metadata `required_custom_permissions`
///
/// Returns `Ok(())` if allowed, `Err(ToolError::PermissionDenied)` if not.
pub fn check_tool_permission(
    tool_name: &str,
    permissions: &UserPermissions,
    tool_metadata: Option<&ToolMetadata>,
) -> Result<(), ToolError> {
    // Step 1: Denylist check (deny overrides everything, even ["*"] allowlist).
    if matches_any_pattern(tool_name, &permissions.tool_denylist) {
        return Err(ToolError::PermissionDenied {
            tool: tool_name.to_string(),
            reason: "tool is explicitly denied for this user".to_string(),
        });
    }

    // Step 2: Allowlist check.
    let allowed = if permissions.tool_access.is_empty() {
        // Empty allowlist = no tools allowed (zero_trust default).
        false
    } else if permissions.tool_access.iter().any(|s| s == "*") {
        // Wildcard entry = all tools allowed (admin default).
        true
    } else {
        // Check each pattern in tool_access for a match.
        matches_any_pattern(tool_name, &permissions.tool_access)
    };

    if !allowed {
        return Err(ToolError::PermissionDenied {
            tool: tool_name.to_string(),
            reason: format!(
                "tool is not in the allowed tools for permission level {}",
                permissions.level,
            ),
        });
    }

    // Step 2b (WEFT-32): MCP wildcard namespace guard.
    //
    // A bare `["*"]` allowlist must NOT cover sensitive MCP namespaces
    // (`exec_*`, `shell_*`, `system_*`, ...). The operator has to opt
    // in to each sensitive namespace explicitly. This stops the
    // attack vector where a benign-looking `tool_access: ["*"]`
    // exposes a maliciously-registered `exec__shell` tool.
    if let Err(err) = crate::security::validate_mcp_namespace_against_wildcard(
        tool_name,
        &permissions.tool_access,
    ) {
        return Err(ToolError::PermissionDenied {
            tool: tool_name.to_string(),
            reason: format!("MCP namespace guard: {err}"),
        });
    }

    // Step 3: Tool metadata -- required permission level.
    if let Some(meta) = tool_metadata {
        if let Some(required_level) = meta.required_permission_level
            && permissions.level < required_level
        {
            return Err(ToolError::PermissionDenied {
                tool: tool_name.to_string(),
                reason: format!(
                    "tool requires permission level {} but user has level {}",
                    required_level, permissions.level,
                ),
            });
        }

        // Step 4: Tool metadata -- required custom permissions.
        for (key, required_value) in &meta.required_custom_permissions {
            match permissions.custom_permissions.get(key) {
                None => {
                    return Err(ToolError::PermissionDenied {
                        tool: tool_name.to_string(),
                        reason: format!(
                            "tool requires custom permission '{}' which is not set",
                            key,
                        ),
                    });
                }
                Some(actual) if actual != required_value => {
                    return Err(ToolError::PermissionDenied {
                        tool: tool_name.to_string(),
                        reason: format!(
                            "tool requires {}={} but user has {}={}",
                            key, required_value, key, actual,
                        ),
                    });
                }
                Some(_) => {} // Value matches, continue.
            }
        }
    }

    Ok(())
}

/// Extract permission metadata from an MCP tool declaration JSON.
///
/// MCP tool declarations may include:
///   `"required_permission_level": 2`
///   `"required_custom_permissions": {"exec_enabled": true}`
pub fn extract_mcp_metadata(tool_decl: &serde_json::Value) -> ToolMetadata {
    let required_level = tool_decl
        .get("required_permission_level")
        .and_then(|v| v.as_u64())
        .map(|v| v as u8);

    let required_custom = tool_decl
        .get("required_custom_permissions")
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        })
        .unwrap_or_default();

    ToolMetadata {
        required_permission_level: required_level,
        required_custom_permissions: required_custom,
    }
}

// ---------------------------------------------------------------------------
// Tool trait
// ---------------------------------------------------------------------------

/// A tool that can be invoked by the agent pipeline.
///
/// Implementations provide a name, description, JSON Schema for parameters,
/// and an async `execute` method. Tools are registered in a [`ToolRegistry`]
/// and dispatched by the agent loop when the LLM emits a tool-use request.
///
/// # Implementing a tool
///
/// ```rust,ignore
/// use async_trait::async_trait;
/// use clawft_core::tools::registry::{Tool, ToolError};
///
/// struct EchoTool;
///
/// #[async_trait]
/// impl Tool for EchoTool {
///     fn name(&self) -> &str { "echo" }
///     fn description(&self) -> &str { "Echo back the input" }
///     fn parameters(&self) -> serde_json::Value {
///         serde_json::json!({
///             "type": "object",
///             "properties": {
///                 "text": { "type": "string", "description": "Text to echo" }
///             },
///             "required": ["text"]
///         })
///     }
///     async fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
///         let text = args.get("text")
///             .and_then(|v| v.as_str())
///             .ok_or_else(|| ToolError::InvalidArgs("missing 'text'".into()))?;
///         Ok(serde_json::json!({ "output": text }))
///     }
/// }
/// ```
#[cfg_attr(not(feature = "browser"), async_trait)]
#[cfg_attr(feature = "browser", async_trait(?Send))]
pub trait Tool: Send + Sync {
    /// The unique name of this tool (used in LLM function calling).
    fn name(&self) -> &str;

    /// A human-readable description of what this tool does.
    fn description(&self) -> &str;

    /// JSON Schema describing the tool's parameters.
    ///
    /// Should return a valid JSON Schema object (type: "object" with
    /// properties, required, etc.) suitable for OpenAI function calling.
    fn parameters(&self) -> serde_json::Value;

    /// Execute the tool with the given arguments.
    ///
    /// Arguments are a JSON object matching the schema from [`parameters`].
    /// Returns a JSON value representing the tool's output, or a
    /// [`ToolError`] on failure.
    async fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError>;

    /// Optional permission metadata for this tool.
    ///
    /// Override to declare minimum permission levels or required
    /// custom permissions. Default: no requirements (returns `None`).
    fn metadata(&self) -> Option<ToolMetadata> {
        None
    }
}

/// Registry of available tools, indexed by name.
///
/// Provides lookup, listing, schema generation in OpenAI function calling
/// format, and dispatch-by-name execution.
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
    metadata: HashMap<String, ToolMetadata>,
}

impl ToolRegistry {
    /// Create an empty tool registry.
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            metadata: HashMap::new(),
        }
    }

    /// Register a tool in the registry.
    ///
    /// If a tool with the same name already exists, it is replaced.
    /// If the tool provides metadata via [`Tool::metadata()`], it is stored
    /// in the metadata map automatically.
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        let name = tool.name().to_string();
        debug!(tool = %name, "registering tool");
        // Extract metadata from the tool if provided.
        if let Some(meta) = tool.metadata() {
            self.metadata.insert(name.clone(), meta);
        }
        self.tools.insert(name.clone(), tool);

        // Chain event marker for tool registration.
        crate::chain_event!(
            "tools",
            crate::chain_event::EVENT_KIND_TOOL_REGISTER,
            { "tool_name": name }
        );
    }

    /// Register a tool with explicit metadata (used for MCP tools whose
    /// metadata comes from JSON declarations rather than the Rust trait).
    pub fn register_with_metadata(&mut self, tool: Arc<dyn Tool>, metadata: ToolMetadata) {
        let name = tool.name().to_string();
        debug!(tool = %name, "registering tool with metadata");
        self.metadata.insert(name.clone(), metadata);
        self.tools.insert(name, tool);
    }

    /// Check if a tool with the given name is registered.
    pub fn has(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Look up a tool by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    /// Look up metadata for a tool by name.
    pub fn get_metadata(&self, name: &str) -> Option<&ToolMetadata> {
        self.metadata.get(name)
    }

    /// List all registered tool names (sorted alphabetically).
    pub fn list(&self) -> Vec<String> {
        let mut names: Vec<String> = self.tools.keys().cloned().collect();
        names.sort();
        names
    }

    /// Generate tool schemas in OpenAI function calling format.
    ///
    /// Returns one schema object per registered tool:
    /// ```json
    /// {
    ///   "type": "function",
    ///   "function": {
    ///     "name": "tool_name",
    ///     "description": "tool description",
    ///     "parameters": { ... json schema ... }
    ///   }
    /// }
    /// ```
    ///
    /// The returned vector is sorted by tool name for deterministic output.
    pub fn schemas(&self) -> Vec<serde_json::Value> {
        let mut schemas: Vec<(String, serde_json::Value)> = self
            .tools
            .iter()
            .map(|(name, tool)| {
                let schema = serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": name,
                        "description": tool.description(),
                        "parameters": tool.parameters(),
                    }
                });
                (name.clone(), schema)
            })
            .collect();

        schemas.sort_by(|a, b| a.0.cmp(&b.0));
        schemas.into_iter().map(|(_, v)| v).collect()
    }

    /// Generate tool schemas for a specified subset of tools.
    ///
    /// Only includes tools whose names match at least one pattern in
    /// `allowed`. Patterns support glob syntax (`*` and `?`).
    ///
    /// This enables per-turn tool filtering: skills declare which tools
    /// they need, and only those schemas are sent to the LLM.
    pub fn schemas_for_tools(&self, allowed: &[String]) -> Vec<serde_json::Value> {
        let mut schemas: Vec<(String, serde_json::Value)> = self
            .tools
            .iter()
            .filter(|(name, _)| matches_any_pattern(name, allowed))
            .map(|(name, tool)| {
                let schema = serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": name,
                        "description": tool.description(),
                        "parameters": tool.parameters(),
                    }
                });
                (name.clone(), schema)
            })
            .collect();

        schemas.sort_by(|a, b| a.0.cmp(&b.0));
        schemas.into_iter().map(|(_, v)| v).collect()
    }

    /// Execute a tool by name with optional permission enforcement.
    ///
    /// When `permissions` is `None`, all tools are allowed (backward
    /// compatibility for StaticRouter mode and unit tests).
    /// When `Some`, permissions are checked before the tool runs.
    ///
    /// Returns [`ToolError::NotFound`] if no tool with that name is registered.
    /// Returns [`ToolError::PermissionDenied`] if the caller lacks permission.
    ///
    /// WEFT-37: when a tool's argument JSON references a filesystem
    /// path (via `path`, `file`, `file_path`, etc.), this method
    /// acquires the corresponding per-path advisory lock for the
    /// duration of execution. Concurrent invocations against the same
    /// canonical path serialise; non-overlapping paths still run in
    /// full parallel. Native-only — browser/wasm has a single-threaded
    /// executor.
    pub async fn execute(
        &self,
        name: &str,
        args: serde_json::Value,
        permissions: Option<&UserPermissions>,
    ) -> Result<serde_json::Value, ToolError> {
        // Look up the tool first (NotFound fires before PermissionDenied).
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| ToolError::NotFound(name.to_string()))?;

        // Permission check (only when permissions are provided).
        if let Some(perms) = permissions {
            let meta = self.metadata.get(name);
            check_tool_permission(name, perms, meta)?;
        }

        debug!(tool = %name, "executing tool");

        // WEFT-37: per-path advisory locks (native only).
        #[cfg(feature = "native")]
        {
            let paths = extract_paths(&args);
            // Sort paths so two callers locking the same set of paths
            // acquire them in the same order — prevents A-then-B vs
            // B-then-A deadlocks when a tool args object references
            // multiple paths at once.
            let mut sorted = paths;
            sorted.sort();
            sorted.dedup();
            // Hold guards for the duration of the call.
            let mut _guards: Vec<tokio::sync::OwnedMutexGuard<()>> =
                Vec::with_capacity(sorted.len());
            for p in sorted {
                _guards.push(acquire_path_lock(p).await);
            }
            return tool.execute(args).await;
        }
        #[cfg(not(feature = "native"))]
        {
            tool.execute(args).await
        }
    }

    /// Return the number of registered tools.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Return true if no tools are registered.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Create a filtered snapshot of this registry containing only tools
    /// whose names are in the given allow list.
    ///
    /// If `allow` is empty, all tools are included (no filtering).
    /// If `deny` is non-empty, matching tools are excluded even if
    /// they appear in the allow list (deny overrides allow).
    ///
    /// This is used by the kernel supervisor to create per-agent
    /// tool registries that respect capability restrictions.
    pub fn filtered_tools(&self, allow: &[String], deny: &[String]) -> Self {
        let mut filtered = Self::new();
        for (name, tool) in &self.tools {
            // Check deny list first
            if deny.iter().any(|d| d == name) {
                continue;
            }
            // Check allow list (empty = all allowed)
            if !allow.is_empty() && !allow.iter().any(|a| a == name) {
                continue;
            }
            filtered.tools.insert(name.clone(), tool.clone());
            if let Some(meta) = self.metadata.get(name) {
                filtered.metadata.insert(name.clone(), meta.clone());
            }
        }
        filtered
    }

    /// Create a snapshot of this registry as a new `ToolRegistry`.
    ///
    /// The returned registry contains clones of all `Arc<dyn Tool>`
    /// handles currently registered. This is useful for passing a
    /// frozen copy of the tool set to components that need shared
    /// access (e.g., wrapped in `Arc<ToolRegistry>`) without requiring
    /// the original registry to be `Arc`-wrapped itself.
    pub fn snapshot(&self) -> Self {
        Self {
            tools: self.tools.clone(),
            metadata: self.metadata.clone(),
        }
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A simple test tool that echoes its input.
    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }

        fn description(&self) -> &str {
            "Echo back the input text"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "Text to echo"
                    }
                },
                "required": ["text"]
            })
        }

        async fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
            let text = args
                .get("text")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidArgs("missing 'text' field".into()))?;
            Ok(serde_json::json!({ "output": text }))
        }
    }

    /// A tool that always fails for testing error paths.
    struct FailTool;

    #[async_trait]
    impl Tool for FailTool {
        fn name(&self) -> &str {
            "fail"
        }

        fn description(&self) -> &str {
            "A tool that always fails"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {}
            })
        }

        async fn execute(&self, _args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
            Err(ToolError::ExecutionFailed("intentional failure".into()))
        }
    }

    /// A math tool for testing multiple registrations.
    struct AddTool;

    #[async_trait]
    impl Tool for AddTool {
        fn name(&self) -> &str {
            "add"
        }

        fn description(&self) -> &str {
            "Add two numbers"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "a": { "type": "number" },
                    "b": { "type": "number" }
                },
                "required": ["a", "b"]
            })
        }

        async fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
            let a = args
                .get("a")
                .and_then(|v| v.as_f64())
                .ok_or_else(|| ToolError::InvalidArgs("missing 'a'".into()))?;
            let b = args
                .get("b")
                .and_then(|v| v.as_f64())
                .ok_or_else(|| ToolError::InvalidArgs("missing 'b'".into()))?;
            Ok(serde_json::json!({ "result": a + b }))
        }
    }

    /// Helper: build admin permissions (level 2, wildcard tool_access).
    fn admin_permissions() -> UserPermissions {
        UserPermissions {
            level: 2,
            tool_access: vec!["*".into()],
            ..UserPermissions::default()
        }
    }

    /// Helper: build user-level permissions with specific tools.
    fn user_permissions(tools: Vec<&str>) -> UserPermissions {
        UserPermissions {
            level: 1,
            tool_access: tools.into_iter().map(String::from).collect(),
            ..UserPermissions::default()
        }
    }

    /// Helper: build zero-trust permissions (empty tool_access).
    fn zero_trust_permissions() -> UserPermissions {
        UserPermissions::default()
    }

    #[test]
    fn new_registry_is_empty() {
        let registry = ToolRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
        assert!(registry.list().is_empty());
    }

    #[test]
    fn default_registry_is_empty() {
        let registry = ToolRegistry::default();
        assert!(registry.is_empty());
    }

    #[test]
    fn register_and_get() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));

        let tool = registry.get("echo").unwrap();
        assert_eq!(tool.name(), "echo");
        assert_eq!(tool.description(), "Echo back the input text");
    }

    #[test]
    fn get_nonexistent_returns_none() {
        let registry = ToolRegistry::new();
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn list_returns_sorted_names() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        registry.register(Arc::new(AddTool));
        registry.register(Arc::new(FailTool));

        let names = registry.list();
        assert_eq!(names, vec!["add", "echo", "fail"]);
    }

    #[test]
    fn len_reflects_registered_count() {
        let mut registry = ToolRegistry::new();
        assert_eq!(registry.len(), 0);

        registry.register(Arc::new(EchoTool));
        assert_eq!(registry.len(), 1);

        registry.register(Arc::new(AddTool));
        assert_eq!(registry.len(), 2);
    }

    #[test]
    fn register_replaces_existing() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        registry.register(Arc::new(EchoTool)); // Same name, should replace.
        assert_eq!(registry.len(), 1);
    }

    #[tokio::test]
    async fn execute_echo_tool() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));

        let result = registry
            .execute("echo", serde_json::json!({ "text": "hello" }), None)
            .await
            .unwrap();

        assert_eq!(result["output"], "hello");
    }

    #[tokio::test]
    async fn execute_add_tool() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(AddTool));

        let result = registry
            .execute("add", serde_json::json!({ "a": 3, "b": 4 }), None)
            .await
            .unwrap();

        assert_eq!(result["result"], 7.0);
    }

    #[tokio::test]
    async fn execute_not_found() {
        let registry = ToolRegistry::new();
        let result = registry
            .execute("missing", serde_json::json!({}), None)
            .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            ToolError::NotFound(name) => assert_eq!(name, "missing"),
            other => panic!("expected NotFound, got: {other}"),
        }
    }

    #[tokio::test]
    async fn execute_tool_that_fails() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(FailTool));

        let result = registry
            .execute("fail", serde_json::json!({}), None)
            .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ToolError::ExecutionFailed(msg) => {
                assert_eq!(msg, "intentional failure");
            }
            other => panic!("expected ExecutionFailed, got: {other}"),
        }
    }

    #[tokio::test]
    async fn execute_with_invalid_args() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));

        let result = registry
            .execute("echo", serde_json::json!({}), None) // missing "text"
            .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            ToolError::InvalidArgs(msg) => {
                assert!(msg.contains("text"));
            }
            other => panic!("expected InvalidArgs, got: {other}"),
        }
    }

    #[test]
    fn schemas_openai_format() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        registry.register(Arc::new(AddTool));

        let schemas = registry.schemas();
        assert_eq!(schemas.len(), 2);

        // Schemas should be sorted by name: "add" before "echo".
        let first = &schemas[0];
        assert_eq!(first["type"], "function");
        assert_eq!(first["function"]["name"], "add");
        assert_eq!(first["function"]["description"], "Add two numbers");
        assert!(
            first["function"]["parameters"]["properties"]
                .get("a")
                .is_some()
        );

        let second = &schemas[1];
        assert_eq!(second["type"], "function");
        assert_eq!(second["function"]["name"], "echo");
        assert_eq!(
            second["function"]["description"],
            "Echo back the input text"
        );
        assert!(
            second["function"]["parameters"]["properties"]
                .get("text")
                .is_some()
        );
    }

    #[test]
    fn schemas_empty_registry() {
        let registry = ToolRegistry::new();
        assert!(registry.schemas().is_empty());
    }

    #[test]
    fn schemas_for_tools_filters_by_pattern() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        registry.register(Arc::new(AddTool));
        registry.register(Arc::new(FailTool));

        // Exact name filter
        let schemas = registry.schemas_for_tools(&["echo".into()]);
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0]["function"]["name"], "echo");

        // Glob filter
        let schemas = registry.schemas_for_tools(&["*".into()]);
        assert_eq!(schemas.len(), 3);

        // No match
        let schemas = registry.schemas_for_tools(&["nonexistent".into()]);
        assert!(schemas.is_empty());

        // Multiple patterns
        let schemas = registry.schemas_for_tools(&["echo".into(), "add".into()]);
        assert_eq!(schemas.len(), 2);
    }

    #[test]
    fn tool_error_display() {
        let err = ToolError::NotFound("web_search".into());
        assert_eq!(err.to_string(), "tool not found: web_search");

        let err = ToolError::InvalidArgs("missing required field".into());
        assert_eq!(err.to_string(), "invalid arguments: missing required field");

        let err = ToolError::ExecutionFailed("command failed".into());
        assert_eq!(err.to_string(), "execution failed: command failed");

        let err = ToolError::PermissionDenied {
            tool: "test".into(),
            reason: "no exec access".into(),
        };
        assert_eq!(
            err.to_string(),
            "permission denied for tool 'test': no exec access"
        );

        let err = ToolError::FileNotFound("/tmp/missing.txt".into());
        assert_eq!(err.to_string(), "not found: /tmp/missing.txt");

        let err = ToolError::InvalidPath("../../../etc/passwd".into());
        assert_eq!(err.to_string(), "invalid path: ../../../etc/passwd");

        let err = ToolError::Timeout(30);
        assert_eq!(err.to_string(), "timeout after 30s");
    }

    #[test]
    fn tool_trait_is_object_safe() {
        // Verify Tool can be used as a trait object.
        fn accepts_tool(_t: &dyn Tool) {}
        let tool = EchoTool;
        accepts_tool(&tool);
    }

    #[test]
    fn tool_parameters_returns_valid_json_schema() {
        let tool = EchoTool;
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        assert!(params["properties"].is_object());
        assert!(params["required"].is_array());
    }

    // ── Glob matching unit tests ──────────────────────────────────────

    #[test]
    fn test_glob_star_matches() {
        assert!(glob_matches("file_*", "file_read"));
        assert!(glob_matches("file_*", "file_write"));
        assert!(!glob_matches("file_*", "web_search"));
        assert!(glob_matches("*", "anything"));
        assert!(glob_matches("*", ""));
        assert!(glob_matches("myserver__*", "myserver__search"));
        assert!(!glob_matches("myserver__*", "otherserver__search"));
    }

    #[test]
    fn test_glob_question_mark() {
        assert!(glob_matches("read_?", "read_a"));
        assert!(!glob_matches("read_?", "read_file"));
        assert!(glob_matches("?", "a"));
        assert!(!glob_matches("?", ""));
    }

    #[test]
    fn test_glob_exact_match_fast_path() {
        // matches_any_pattern uses exact match when no wildcards.
        assert!(matches_any_pattern("read_file", &["read_file".into()]));
        assert!(!matches_any_pattern("write_file", &["read_file".into()]));
    }

    // ── Permission check unit tests ───────────────────────────────────

    #[test]
    fn test_wildcard_allows_all_tools() {
        let perms = admin_permissions();
        assert!(check_tool_permission("exec_shell", &perms, None).is_ok());
        assert!(check_tool_permission("read_file", &perms, None).is_ok());
        assert!(check_tool_permission("myserver__tool", &perms, None).is_ok());
    }

    #[test]
    fn test_empty_allowlist_denies_all_tools() {
        let perms = zero_trust_permissions();
        let result = check_tool_permission("read_file", &perms, None);
        assert!(result.is_err());
        match result.unwrap_err() {
            ToolError::PermissionDenied { tool, reason } => {
                assert_eq!(tool, "read_file");
                assert!(reason.contains("not in the allowed tools"));
            }
            other => panic!("expected PermissionDenied, got: {other}"),
        }
    }

    #[test]
    fn test_explicit_allowlist_permits_listed_tool() {
        let perms = user_permissions(vec!["read_file", "write_file"]);
        assert!(check_tool_permission("read_file", &perms, None).is_ok());
        assert!(check_tool_permission("write_file", &perms, None).is_ok());
    }

    #[test]
    fn test_explicit_allowlist_denies_unlisted_tool() {
        let perms = user_permissions(vec!["read_file", "write_file"]);
        assert!(check_tool_permission("exec_shell", &perms, None).is_err());
    }

    #[test]
    fn test_denylist_overrides_wildcard() {
        let perms = UserPermissions {
            level: 2,
            tool_access: vec!["*".into()],
            tool_denylist: vec!["exec_shell".into()],
            ..UserPermissions::default()
        };
        assert!(check_tool_permission("exec_shell", &perms, None).is_err());
        assert!(check_tool_permission("read_file", &perms, None).is_ok());
    }

    #[test]
    fn test_denylist_overrides_explicit_allow() {
        let perms = UserPermissions {
            level: 1,
            tool_access: vec!["exec_shell".into()],
            tool_denylist: vec!["exec_shell".into()],
            ..UserPermissions::default()
        };
        assert!(check_tool_permission("exec_shell", &perms, None).is_err());
    }

    #[test]
    fn test_glob_allowlist_pattern() {
        let perms = UserPermissions {
            level: 1,
            tool_access: vec!["file_*".into()],
            ..UserPermissions::default()
        };
        assert!(check_tool_permission("file_read", &perms, None).is_ok());
        assert!(check_tool_permission("file_write", &perms, None).is_ok());
        assert!(check_tool_permission("web_search", &perms, None).is_err());
    }

    #[test]
    fn test_glob_denylist_pattern() {
        let perms = UserPermissions {
            level: 2,
            tool_access: vec!["*".into()],
            tool_denylist: vec!["exec_*".into()],
            ..UserPermissions::default()
        };
        assert!(check_tool_permission("exec_shell", &perms, None).is_err());
        assert!(check_tool_permission("exec_spawn", &perms, None).is_err());
        assert!(check_tool_permission("read_file", &perms, None).is_ok());
    }

    #[test]
    fn test_metadata_required_level_blocks_low_user() {
        let perms = UserPermissions {
            level: 1,
            tool_access: vec!["*".into()],
            ..UserPermissions::default()
        };
        let meta = ToolMetadata {
            required_permission_level: Some(2),
            ..ToolMetadata::default()
        };
        let result = check_tool_permission("admin_tool", &perms, Some(&meta));
        assert!(result.is_err());
        match result.unwrap_err() {
            ToolError::PermissionDenied { reason, .. } => {
                assert!(reason.contains("requires permission level 2"));
                assert!(reason.contains("user has level 1"));
            }
            other => panic!("expected PermissionDenied, got: {other}"),
        }
    }

    #[test]
    fn test_metadata_required_level_allows_sufficient_user() {
        let perms = UserPermissions {
            level: 2,
            tool_access: vec!["*".into()],
            ..UserPermissions::default()
        };
        let meta = ToolMetadata {
            required_permission_level: Some(2),
            ..ToolMetadata::default()
        };
        assert!(check_tool_permission("admin_tool", &perms, Some(&meta)).is_ok());
    }

    #[test]
    fn test_metadata_custom_permission_missing() {
        let perms = admin_permissions();
        let meta = ToolMetadata {
            required_custom_permissions: HashMap::from([(
                "exec_enabled".into(),
                serde_json::json!(true),
            )]),
            ..ToolMetadata::default()
        };
        let result = check_tool_permission("custom_tool", &perms, Some(&meta));
        assert!(result.is_err());
        match result.unwrap_err() {
            ToolError::PermissionDenied { reason, .. } => {
                assert!(reason.contains("exec_enabled"));
                assert!(reason.contains("not set"));
            }
            other => panic!("expected PermissionDenied, got: {other}"),
        }
    }

    #[test]
    fn test_metadata_custom_permission_wrong_value() {
        let perms = UserPermissions {
            level: 2,
            tool_access: vec!["*".into()],
            custom_permissions: HashMap::from([(
                "exec_enabled".into(),
                serde_json::json!(false),
            )]),
            ..UserPermissions::default()
        };
        let meta = ToolMetadata {
            required_custom_permissions: HashMap::from([(
                "exec_enabled".into(),
                serde_json::json!(true),
            )]),
            ..ToolMetadata::default()
        };
        let result = check_tool_permission("custom_tool", &perms, Some(&meta));
        assert!(result.is_err());
    }

    #[test]
    fn test_mcp_namespaced_tool_exact_match() {
        let perms = UserPermissions {
            level: 1,
            tool_access: vec!["myserver__search".into()],
            ..UserPermissions::default()
        };
        assert!(check_tool_permission("myserver__search", &perms, None).is_ok());
        assert!(check_tool_permission("myserver__exec", &perms, None).is_err());
    }

    #[test]
    fn test_mcp_namespaced_tool_glob_match() {
        let perms = UserPermissions {
            level: 1,
            tool_access: vec!["myserver__*".into()],
            ..UserPermissions::default()
        };
        assert!(check_tool_permission("myserver__search", &perms, None).is_ok());
        assert!(check_tool_permission("otherserver__search", &perms, None).is_err());
    }

    // ── Registry integration tests ────────────────────────────────────

    #[tokio::test]
    async fn test_registry_execute_with_none_permissions() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));

        // None permissions = backward compat, all tools allowed.
        let result = registry
            .execute("echo", serde_json::json!({ "text": "hello" }), None)
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_registry_execute_with_denied_permissions() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));

        let perms = zero_trust_permissions();
        let result = registry
            .execute(
                "echo",
                serde_json::json!({ "text": "hello" }),
                Some(&perms),
            )
            .await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ToolError::PermissionDenied { .. }
        ));
    }

    #[tokio::test]
    async fn test_registry_execute_not_found_before_permission_check() {
        let registry = ToolRegistry::new();
        let perms = zero_trust_permissions();

        // NotFound should fire before PermissionDenied.
        let result = registry
            .execute("nonexistent", serde_json::json!({}), Some(&perms))
            .await;
        assert!(matches!(result.unwrap_err(), ToolError::NotFound(_)));
    }

    // ── Security-critical tests ───────────────────────────────────────

    #[test]
    fn test_zero_trust_blocked_from_exec_and_spawn() {
        let perms = zero_trust_permissions();
        assert!(check_tool_permission("exec_shell", &perms, None).is_err());
        assert!(check_tool_permission("spawn", &perms, None).is_err());
    }

    #[test]
    fn test_user_level_blocked_from_exec_and_spawn() {
        let perms = user_permissions(vec![
            "read_file",
            "write_file",
            "edit_file",
            "list_dir",
            "web_search",
            "web_fetch",
            "message",
        ]);
        assert!(check_tool_permission("exec_shell", &perms, None).is_err());
        assert!(check_tool_permission("spawn", &perms, None).is_err());
    }

    #[test]
    fn test_admin_can_call_all_tools() {
        let perms = admin_permissions();
        assert!(check_tool_permission("exec_shell", &perms, None).is_ok());
        assert!(check_tool_permission("spawn", &perms, None).is_ok());
        assert!(check_tool_permission("read_file", &perms, None).is_ok());
        assert!(check_tool_permission("myserver__tool", &perms, None).is_ok());
    }

    #[test]
    fn test_permission_denied_error_includes_tool_name() {
        let perms = zero_trust_permissions();
        let result = check_tool_permission("exec_shell", &perms, None);
        match result.unwrap_err() {
            ToolError::PermissionDenied { tool, reason } => {
                assert_eq!(tool, "exec_shell");
                assert!(!reason.is_empty());
            }
            other => panic!("expected PermissionDenied, got: {other}"),
        }
    }

    #[test]
    fn test_permission_denied_error_display_format() {
        let err = ToolError::PermissionDenied {
            tool: "exec_shell".into(),
            reason: "tool is explicitly denied for this user".into(),
        };
        assert_eq!(
            err.to_string(),
            "permission denied for tool 'exec_shell': tool is explicitly denied for this user"
        );
    }

    // ── MCP metadata extraction ───────────────────────────────────────

    #[test]
    fn test_extract_mcp_metadata() {
        let decl = serde_json::json!({
            "required_permission_level": 2,
            "required_custom_permissions": {
                "exec_enabled": true
            }
        });
        let meta = extract_mcp_metadata(&decl);
        assert_eq!(meta.required_permission_level, Some(2));
        assert_eq!(
            meta.required_custom_permissions.get("exec_enabled"),
            Some(&serde_json::json!(true))
        );
    }

    #[test]
    fn test_extract_mcp_metadata_empty() {
        let decl = serde_json::json!({});
        let meta = extract_mcp_metadata(&decl);
        assert!(meta.required_permission_level.is_none());
        assert!(meta.required_custom_permissions.is_empty());
    }

    // ── Register with metadata ────────────────────────────────────────

    #[test]
    fn test_register_with_metadata_stores_metadata() {
        let mut registry = ToolRegistry::new();
        let meta = ToolMetadata {
            required_permission_level: Some(2),
            ..ToolMetadata::default()
        };
        registry.register_with_metadata(Arc::new(EchoTool), meta);

        let stored = registry.get_metadata("echo").unwrap();
        assert_eq!(stored.required_permission_level, Some(2));
    }

    #[tokio::test]
    async fn test_registry_execute_with_admin_and_metadata() {
        let mut registry = ToolRegistry::new();
        let meta = ToolMetadata {
            required_permission_level: Some(2),
            ..ToolMetadata::default()
        };
        registry.register_with_metadata(Arc::new(EchoTool), meta);

        let perms = admin_permissions();
        let result = registry
            .execute(
                "echo",
                serde_json::json!({ "text": "hello" }),
                Some(&perms),
            )
            .await;
        assert!(result.is_ok());
    }

    // ── filtered_tools tests ───────────────────────────────────────

    #[test]
    fn filtered_tools_allow_empty_returns_all() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        registry.register(Arc::new(AddTool));
        registry.register(Arc::new(FailTool));

        let filtered = registry.filtered_tools(&[], &[]);
        assert_eq!(filtered.len(), 3);
    }

    #[test]
    fn filtered_tools_allow_restricts() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        registry.register(Arc::new(AddTool));
        registry.register(Arc::new(FailTool));

        let filtered = registry.filtered_tools(&["echo".into(), "add".into()], &[]);
        assert_eq!(filtered.len(), 2);
        assert!(filtered.has("echo"));
        assert!(filtered.has("add"));
        assert!(!filtered.has("fail"));
    }

    #[test]
    fn filtered_tools_deny_overrides_allow() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        registry.register(Arc::new(AddTool));

        let filtered = registry.filtered_tools(
            &["echo".into(), "add".into()],
            &["echo".into()],
        );
        assert_eq!(filtered.len(), 1);
        assert!(filtered.has("add"));
        assert!(!filtered.has("echo"));
    }

    #[test]
    fn filtered_tools_deny_only() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        registry.register(Arc::new(AddTool));
        registry.register(Arc::new(FailTool));

        let filtered = registry.filtered_tools(&[], &["fail".into()]);
        assert_eq!(filtered.len(), 2);
        assert!(!filtered.has("fail"));
    }

    #[test]
    fn filtered_tools_preserves_metadata() {
        let mut registry = ToolRegistry::new();
        let meta = ToolMetadata {
            required_permission_level: Some(2),
            ..ToolMetadata::default()
        };
        registry.register_with_metadata(Arc::new(EchoTool), meta);
        registry.register(Arc::new(AddTool));

        let filtered = registry.filtered_tools(&["echo".into()], &[]);
        assert!(filtered.get_metadata("echo").is_some());
        assert_eq!(
            filtered.get_metadata("echo").unwrap().required_permission_level,
            Some(2)
        );
    }

    #[tokio::test]
    async fn test_registry_execute_with_low_level_and_metadata() {
        let mut registry = ToolRegistry::new();
        let meta = ToolMetadata {
            required_permission_level: Some(2),
            ..ToolMetadata::default()
        };
        registry.register_with_metadata(Arc::new(EchoTool), meta);

        let perms = UserPermissions {
            level: 1,
            tool_access: vec!["*".into()],
            ..UserPermissions::default()
        };
        let result = registry
            .execute(
                "echo",
                serde_json::json!({ "text": "hello" }),
                Some(&perms),
            )
            .await;
        assert!(matches!(
            result.unwrap_err(),
            ToolError::PermissionDenied { .. }
        ));
    }

    // ── WEFT-37: per-path advisory locks ───────────────────────────

    /// A tool that records each invocation's "scope" — defined as
    /// "entered before previous one exited" — into a shared vector.
    /// If two invocations against the same path are properly
    /// serialised, every call records `{started: T, finished: T+ε}`
    /// with no overlap.
    #[cfg(feature = "native")]
    struct InterleaveTool {
        log: Arc<StdMutex<Vec<(u64, &'static str)>>>,
        delay_ms: u64,
    }

    #[cfg(feature = "native")]
    #[async_trait]
    impl Tool for InterleaveTool {
        fn name(&self) -> &str {
            "interleave"
        }
        fn description(&self) -> &str {
            "test tool that logs entry/exit events"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            })
        }
        async fn execute(
            &self,
            args: serde_json::Value,
        ) -> Result<serde_json::Value, ToolError> {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let id = args.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
            let _ = path; // unused in event log; lock is keyed off it
            {
                let mut log = self.log.lock().expect("log poisoned");
                log.push((id, "enter"));
            }
            tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;
            {
                let mut log = self.log.lock().expect("log poisoned");
                log.push((id, "exit"));
            }
            Ok(serde_json::json!({ "ok": true }))
        }
    }

    #[cfg(feature = "native")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn weft37_same_path_executions_serialize() {
        _clear_path_locks_for_test();

        let log = Arc::new(StdMutex::new(Vec::<(u64, &'static str)>::new()));
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(InterleaveTool {
            log: log.clone(),
            delay_ms: 20,
        }));
        let registry = Arc::new(registry);

        // Spawn 10 parallel calls all hitting the same path.
        let mut handles = Vec::new();
        for id in 0..10u64 {
            let r = registry.clone();
            handles.push(tokio::spawn(async move {
                r.execute(
                    "interleave",
                    serde_json::json!({ "path": "/tmp/weft37-shared", "id": id }),
                    None,
                )
                .await
            }));
        }
        for h in handles {
            h.await.expect("task panicked").expect("tool failed");
        }

        // Verify the log is strictly enter/exit per call — no two
        // enters in a row.
        let log = log.lock().expect("log poisoned");
        let mut depth: i32 = 0;
        for (_id, kind) in log.iter() {
            match *kind {
                "enter" => {
                    depth += 1;
                    assert!(
                        depth <= 1,
                        "two parallel calls overlapped on the same path; \
                         events: {:?}",
                        log
                    );
                }
                "exit" => depth -= 1,
                _ => panic!("unexpected kind {kind}"),
            }
        }
        assert_eq!(depth, 0, "every enter must be matched by an exit");
        assert_eq!(log.len(), 20, "expected 10 enter + 10 exit events");
    }

    #[cfg(feature = "native")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn weft37_distinct_paths_run_in_parallel() {
        _clear_path_locks_for_test();

        let log = Arc::new(StdMutex::new(Vec::<(u64, &'static str)>::new()));
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(InterleaveTool {
            log: log.clone(),
            delay_ms: 50,
        }));
        let registry = Arc::new(registry);

        let start = std::time::Instant::now();
        let mut handles = Vec::new();
        for id in 0..4u64 {
            let r = registry.clone();
            handles.push(tokio::spawn(async move {
                r.execute(
                    "interleave",
                    serde_json::json!({
                        "path": format!("/tmp/weft37-distinct-{id}"),
                        "id": id,
                    }),
                    None,
                )
                .await
            }));
        }
        for h in handles {
            h.await.expect("task panicked").expect("tool failed");
        }
        let elapsed = start.elapsed();

        // 4 parallel 50ms tasks should finish in well under serial
        // 4 × 50ms = 200ms. Allow generous slack for CI noise.
        assert!(
            elapsed.as_millis() < 180,
            "distinct-path executions did not run in parallel; took {}ms",
            elapsed.as_millis()
        );
    }

    #[cfg(feature = "native")]
    #[test]
    fn weft37_canonicalize_collapses_dot_segments() {
        let a = canonicalize_for_lock("/tmp/foo/./bar");
        let b = canonicalize_for_lock("/tmp/foo/bar");
        let c = canonicalize_for_lock("/tmp/foo/baz/../bar");
        assert_eq!(a, b);
        assert_eq!(a, c);
    }

    #[cfg(feature = "native")]
    #[test]
    fn weft37_extract_paths_supports_common_keys() {
        for key in &["path", "file", "file_path", "filepath", "target"] {
            let args = serde_json::json!({ *key: "/tmp/x" });
            let paths = extract_paths(&args);
            assert_eq!(paths.len(), 1, "key {key} did not extract");
        }
        // Args without a recognised key produce no paths.
        let none = extract_paths(&serde_json::json!({ "text": "hello" }));
        assert!(none.is_empty());
    }
}
