//! Tool provider abstraction for MCP tool delegation.
//!
//! Defines [`ToolProvider`], the trait that unifies local (builtin) and
//! remote (MCP server) tool sources behind a single interface, and
//! [`BuiltinToolProvider`], which wraps the existing
//! [`clawft_core::tools::registry::ToolRegistry`] dispatch mechanism.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::ToolDefinition;

// ---------------------------------------------------------------------------
// Result / content types
// ---------------------------------------------------------------------------

/// A single content block returned by a tool execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum ContentBlock {
    /// Plain text content.
    #[serde(rename = "text")]
    Text { text: String },
}

/// The result of calling a tool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CallToolResult {
    /// Content blocks produced by the tool.
    pub content: Vec<ContentBlock>,
    /// Whether the tool execution resulted in an error.
    #[serde(default, rename = "isError")]
    pub is_error: bool,
}

impl CallToolResult {
    /// Convenience constructor for a successful text result.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![ContentBlock::Text { text: text.into() }],
            is_error: false,
        }
    }

    /// Convenience constructor for an error text result.
    pub fn error(text: impl Into<String>) -> Self {
        Self {
            content: vec![ContentBlock::Text { text: text.into() }],
            is_error: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur during tool provider operations.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    /// The requested tool was not found.
    #[error("not found: {0}")]
    NotFound(String),

    /// The tool execution failed.
    #[error("execution failed: {0}")]
    ExecutionFailed(String),

    /// The caller lacks permission to invoke the tool.
    ///
    /// `tool` is the name of the tool that was denied.
    /// `reason` explains why access was denied.
    #[error("permission denied for tool '{tool}': {reason}")]
    PermissionDenied { tool: String, reason: String },
}

// ---------------------------------------------------------------------------
// ToolProvider trait
// ---------------------------------------------------------------------------

/// Abstraction over a source of tools.
///
/// Implementors may serve tools from a local registry (see
/// [`BuiltinToolProvider`]) or from a remote MCP server.
#[async_trait]
pub trait ToolProvider: Send + Sync {
    /// Namespace prefix for this provider's tools (e.g. `"builtin"`,
    /// `"mcp:server-name"`).
    fn namespace(&self) -> &str;

    /// List the tool definitions available from this provider.
    fn list_tools(&self) -> Vec<ToolDefinition>;

    /// Execute a tool by name with the given JSON arguments.
    async fn call_tool(&self, name: &str, args: Value) -> Result<CallToolResult, ToolError>;
}

// ---------------------------------------------------------------------------
// BuiltinToolProvider
// ---------------------------------------------------------------------------

/// Type alias for the dispatcher function used by [`BuiltinToolProvider`].
///
/// Accepts a tool name and JSON arguments, returns a future that resolves
/// to either a success string or an error string.
type DispatchFn = dyn Fn(&str, Value) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send>>
    + Send
    + Sync;

/// A [`ToolProvider`] backed by local tool definitions and a dispatch
/// function.
///
/// Rather than depending directly on `ToolRegistry` (which lives in a
/// different crate), this provider accepts a list of tool definitions
/// and a closure that dispatches execution requests. This keeps the
/// dependency graph clean and makes testing straightforward.
pub struct BuiltinToolProvider {
    tools: Vec<ToolDefinition>,
    dispatcher: Arc<DispatchFn>,
}

impl BuiltinToolProvider {
    /// Create a new builtin tool provider.
    ///
    /// # Arguments
    ///
    /// * `tools` - Tool definitions to expose via [`ToolProvider::list_tools`].
    /// * `dispatcher` - Closure that executes a tool by name. It receives
    ///   the tool name and JSON arguments, and returns `Ok(output)` on
    ///   success or `Err(message)` on failure.
    pub fn new<F>(tools: Vec<ToolDefinition>, dispatcher: F) -> Self
    where
        F: Fn(&str, Value) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send>>
            + Send
            + Sync
            + 'static,
    {
        Self {
            tools,
            dispatcher: Arc::new(dispatcher),
        }
    }
}

impl std::fmt::Debug for BuiltinToolProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BuiltinToolProvider")
            .field("tool_count", &self.tools.len())
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl ToolProvider for BuiltinToolProvider {
    fn namespace(&self) -> &str {
        "builtin"
    }

    fn list_tools(&self) -> Vec<ToolDefinition> {
        self.tools.clone()
    }

    async fn call_tool(&self, name: &str, args: Value) -> Result<CallToolResult, ToolError> {
        // Verify the tool exists in our definition list.
        if !self.tools.iter().any(|t| t.name == name) {
            return Err(ToolError::NotFound(name.to_string()));
        }

        let fut = (self.dispatcher)(name, args);
        match fut.await {
            Ok(output) => Ok(CallToolResult::text(output)),
            Err(msg) => Ok(CallToolResult::error(msg)),
        }
    }
}

// ---------------------------------------------------------------------------
// SkillToolProvider
// ---------------------------------------------------------------------------

/// Type alias for the dispatcher function used by [`SkillToolProvider`].
///
/// Accepts a skill (tool) name and JSON arguments, returns a future that
/// resolves to either a success string or an error string.
type SkillDispatchFn =
    dyn Fn(&str, Value) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send>>
        + Send
        + Sync;

/// A [`ToolProvider`] that exposes loaded skills as MCP tools.
///
/// Like [`BuiltinToolProvider`], this avoids depending on `clawft-core`
/// directly. The caller supplies:
///
/// - A list of [`ToolDefinition`]s (one per skill) built from
///   `SkillDefinition` metadata at the integration layer.
/// - A dispatch closure that routes `tools/call` to skill execution.
///
/// The tool list is wrapped in `Arc<std::sync::RwLock>` so that the
/// hot-reload watcher can swap in an updated list without replacing
/// the provider instance.
pub struct SkillToolProvider {
    tools: Arc<std::sync::RwLock<Vec<ToolDefinition>>>,
    dispatcher: Arc<SkillDispatchFn>,
}

impl SkillToolProvider {
    /// Create a new skill tool provider.
    ///
    /// # Arguments
    ///
    /// * `tools` - Initial tool definitions derived from loaded skills.
    /// * `dispatcher` - Closure that executes a skill tool by name.
    pub fn new<F>(tools: Vec<ToolDefinition>, dispatcher: F) -> Self
    where
        F: Fn(&str, Value) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send>>
            + Send
            + Sync
            + 'static,
    {
        Self {
            tools: Arc::new(std::sync::RwLock::new(tools)),
            dispatcher: Arc::new(dispatcher),
        }
    }

    /// Replace the tool list with an updated set of skill tools.
    ///
    /// Called by the hot-reload watcher after the skill registry is
    /// rebuilt. Returns the number of tools in the new list.
    pub fn refresh(&self, tools: Vec<ToolDefinition>) -> usize {
        let count = tools.len();
        let mut lock = self.tools.write().expect("SkillToolProvider lock poisoned");
        *lock = tools;
        count
    }

    /// Get a shared handle to the tool list for external inspection.
    pub fn tools_handle(&self) -> Arc<std::sync::RwLock<Vec<ToolDefinition>>> {
        Arc::clone(&self.tools)
    }

    /// Number of currently registered skill tools.
    pub fn tool_count(&self) -> usize {
        self.tools.read().expect("SkillToolProvider lock poisoned").len()
    }
}

impl std::fmt::Debug for SkillToolProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let count = self.tools.read().map(|t| t.len()).unwrap_or(0);
        f.debug_struct("SkillToolProvider")
            .field("tool_count", &count)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl ToolProvider for SkillToolProvider {
    fn namespace(&self) -> &str {
        "skill"
    }

    fn list_tools(&self) -> Vec<ToolDefinition> {
        self.tools
            .read()
            .expect("SkillToolProvider lock poisoned")
            .clone()
    }

    /// Execute a skill as an MCP `tools/call`.
    ///
    /// # Contract
    ///
    /// A skill tool call returns the **SKILL.md prompt body** as the
    /// result content, NOT the output of running the skill against the
    /// LLM. The returned text is the same instructional prompt the
    /// skill would have surfaced if loaded directly via the agent
    /// loop — the calling LLM is expected to treat it as a refresher
    /// or "expand this skill into action" directive and continue
    /// reasoning with that prompt now in context.
    ///
    /// This contract exists because skills are LLM prompts plus
    /// metadata, not standalone executables. Returning the prompt
    /// body lets a remote MCP client (Claude Desktop, the WeftOS
    /// `weft mcp-server` REPL, an IDE bridge) ask "what does this
    /// skill say to do?" without needing access to the local skill
    /// registry. The remote LLM then either follows the prompt
    /// directly or chains into other tool calls described by it.
    ///
    /// # Lookup
    ///
    /// 1. The tool name must be in the registered tool list. Names
    ///    not in the list return [`ToolError::NotFound`] — the
    ///    middleware pipeline never sees an unknown skill.
    /// 2. The `args` JSON value is forwarded to the dispatcher
    ///    closure verbatim. The dispatcher is responsible for
    ///    interpreting any template variables (the skill's
    ///    `variables` list informs `inputSchema` generation in
    ///    [`skill_to_tool_definition`], but argument substitution
    ///    happens inside the dispatcher).
    /// 3. The dispatcher returns either the prompt body string (the
    ///    happy path) or an error message. Errors are wrapped as a
    ///    [`CallToolResult::error`] (NOT a [`ToolError`]) so the
    ///    LLM sees them in-band as a tool result and can reason
    ///    about them, instead of breaking the call chain.
    ///
    /// # See also
    ///
    /// `docs/architecture/skill-tool-contract.md` for the wire-level
    /// example, the SKILL.md → ToolDefinition mapping, and the
    /// failure-mode taxonomy.
    async fn call_tool(&self, name: &str, args: Value) -> Result<CallToolResult, ToolError> {
        // Snapshot the tool list to avoid holding the lock during dispatch.
        let has_tool = {
            let tools = self.tools.read().expect("SkillToolProvider lock poisoned");
            tools.iter().any(|t| t.name == name)
        };

        if !has_tool {
            return Err(ToolError::NotFound(name.to_string()));
        }

        let fut = (self.dispatcher)(name, args);
        match fut.await {
            // Per-contract: the dispatcher returns the SKILL.md prompt
            // body. The MCP client treats this as in-band text content
            // (the LLM uses it as a refresher / instruction).
            Ok(output) => Ok(CallToolResult::text(output)),
            Err(msg) => Ok(CallToolResult::error(msg)),
        }
    }
}

// ---------------------------------------------------------------------------
// Skill-to-tool conversion
// ---------------------------------------------------------------------------

/// Convert a [`SkillDefinition`](clawft_types::skill::SkillDefinition) to a
/// [`ToolDefinition`] suitable for MCP exposure.
///
/// Generates a JSON Schema `inputSchema` from the skill's declared
/// variables. If the skill has no variables, the schema is a plain
/// `{"type": "object"}`.
///
/// This function lives in `clawft-services` (rather than `clawft-core`)
/// because the output type `ToolDefinition` is defined here.
pub fn skill_to_tool_definition(
    skill: &clawft_types::skill::SkillDefinition,
) -> ToolDefinition {
    let input_schema = if skill.variables.is_empty() {
        serde_json::json!({
            "type": "object",
            "properties": {
                "args": {
                    "type": "string",
                    "description": "Free-form arguments passed to the skill"
                }
            }
        })
    } else {
        let mut properties = serde_json::Map::new();
        for var in &skill.variables {
            properties.insert(
                var.clone(),
                serde_json::json!({
                    "type": "string",
                    "description": format!("Value for template variable '{var}'")
                }),
            );
        }
        serde_json::json!({
            "type": "object",
            "properties": properties,
            "required": skill.variables
        })
    };

    ToolDefinition {
        name: skill.name.clone(),
        description: skill.description.clone(),
        input_schema,
    }
}

/// Convert a slice of skill definitions to tool definitions.
///
/// Convenience wrapper over [`skill_to_tool_definition`].
pub fn skills_to_tool_definitions(
    skills: &[clawft_types::skill::SkillDefinition],
) -> Vec<ToolDefinition> {
    skills.iter().map(skill_to_tool_definition).collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: build a small set of tool definitions for testing.
    fn sample_tools() -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "echo".into(),
                description: "Echoes input".into(),
                input_schema: serde_json::json!({"type": "object", "properties": {"text": {"type": "string"}}}),
            },
            ToolDefinition {
                name: "add".into(),
                description: "Adds numbers".into(),
                input_schema: serde_json::json!({"type": "object", "properties": {"a": {"type": "number"}, "b": {"type": "number"}}}),
            },
        ]
    }

    /// Helper: build a [`BuiltinToolProvider`] with a simple dispatcher.
    fn make_provider() -> BuiltinToolProvider {
        BuiltinToolProvider::new(sample_tools(), |name, args| {
            let name = name.to_string();
            Box::pin(async move {
                match name.as_str() {
                    "echo" => {
                        let text = args
                            .get("text")
                            .and_then(|v| v.as_str())
                            .unwrap_or("(empty)");
                        Ok(format!("echo: {text}"))
                    }
                    "add" => {
                        let a = args.get("a").and_then(|v| v.as_f64()).unwrap_or(0.0);
                        let b = args.get("b").and_then(|v| v.as_f64()).unwrap_or(0.0);
                        Ok(format!("{}", a + b))
                    }
                    _ => Err(format!("unknown tool: {name}")),
                }
            })
        })
    }

    #[test]
    fn namespace_returns_builtin() {
        let provider = make_provider();
        assert_eq!(provider.namespace(), "builtin");
    }

    #[test]
    fn list_tools_returns_registered_tools() {
        let provider = make_provider();
        let tools = provider.list_tools();
        assert_eq!(tools.len(), 2);

        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"echo"));
        assert!(names.contains(&"add"));
    }

    #[tokio::test]
    async fn call_tool_dispatches_correctly() {
        let provider = make_provider();

        let result = provider
            .call_tool("echo", serde_json::json!({"text": "hello"}))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(result.content.len(), 1);
        match &result.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "echo: hello"),
        }
    }

    #[tokio::test]
    async fn call_tool_add_dispatches_correctly() {
        let provider = make_provider();

        let result = provider
            .call_tool("add", serde_json::json!({"a": 3.0, "b": 4.0}))
            .await
            .unwrap();

        assert!(!result.is_error);
        match &result.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "7"),
        }
    }

    #[tokio::test]
    async fn call_tool_not_found() {
        let provider = make_provider();

        let result = provider
            .call_tool("nonexistent", serde_json::json!({}))
            .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            ToolError::NotFound(name) => assert_eq!(name, "nonexistent"),
            other => panic!("expected NotFound, got: {other}"),
        }
    }

    #[tokio::test]
    async fn call_tool_dispatcher_error_returns_error_result() {
        // A dispatcher that always returns Err.
        let provider = BuiltinToolProvider::new(
            vec![ToolDefinition {
                name: "broken".into(),
                description: "Always fails".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }],
            |_name, _args| Box::pin(async { Err("intentional failure".to_string()) }),
        );

        let result = provider
            .call_tool("broken", serde_json::json!({}))
            .await
            .unwrap();

        assert!(result.is_error);
        match &result.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "intentional failure"),
        }
    }

    #[test]
    fn call_tool_result_text_convenience() {
        let result = CallToolResult::text("hello");
        assert!(!result.is_error);
        assert_eq!(result.content.len(), 1);
        assert_eq!(
            result.content[0],
            ContentBlock::Text {
                text: "hello".into()
            }
        );
    }

    #[test]
    fn call_tool_result_error_convenience() {
        let result = CallToolResult::error("oops");
        assert!(result.is_error);
        assert_eq!(
            result.content[0],
            ContentBlock::Text {
                text: "oops".into()
            }
        );
    }

    #[test]
    fn call_tool_result_serde_roundtrip() {
        let result = CallToolResult {
            content: vec![ContentBlock::Text {
                text: "output".into(),
            }],
            is_error: false,
        };

        let json = serde_json::to_string(&result).unwrap();
        let restored: CallToolResult = serde_json::from_str(&json).unwrap();
        assert_eq!(result, restored);
    }

    #[test]
    fn content_block_serde_roundtrip() {
        let block = ContentBlock::Text {
            text: "hello world".into(),
        };
        let json = serde_json::to_string(&block).unwrap();
        assert!(json.contains(r#""type":"text""#));

        let restored: ContentBlock = serde_json::from_str(&json).unwrap();
        assert_eq!(block, restored);
    }

    #[test]
    fn call_tool_result_is_error_defaults_false() {
        let json = r#"{"content":[{"type":"text","text":"hi"}]}"#;
        let result: CallToolResult = serde_json::from_str(json).unwrap();
        assert!(!result.is_error);
    }

    #[test]
    fn tool_error_display() {
        let err = ToolError::NotFound("missing".into());
        assert_eq!(err.to_string(), "not found: missing");

        let err = ToolError::ExecutionFailed("boom".into());
        assert_eq!(err.to_string(), "execution failed: boom");

        let err = ToolError::PermissionDenied {
            tool: "test".into(),
            reason: "nope".into(),
        };
        assert_eq!(
            err.to_string(),
            "permission denied for tool 'test': nope"
        );
    }

    #[test]
    fn debug_format() {
        let provider = make_provider();
        let debug = format!("{:?}", provider);
        assert!(debug.contains("BuiltinToolProvider"));
        assert!(debug.contains("tool_count: 2"));
    }

    // ── SkillToolProvider tests ─────────────────────────────────────────

    /// Build a set of skill tool definitions for testing.
    fn skill_tools() -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "research".into(),
                description: "Deep research on a topic".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "topic": { "type": "string" },
                        "depth": { "type": "string" }
                    },
                    "required": ["topic"]
                }),
            },
            ToolDefinition {
                name: "code-review".into(),
                description: "Review code changes".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "diff": { "type": "string" }
                    }
                }),
            },
        ]
    }

    /// Build a [`SkillToolProvider`] with a simple dispatcher.
    fn make_skill_provider() -> SkillToolProvider {
        SkillToolProvider::new(skill_tools(), |name, args| {
            let name = name.to_string();
            Box::pin(async move {
                match name.as_str() {
                    "research" => {
                        let topic = args
                            .get("topic")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");
                        Ok(format!("Researching: {topic}"))
                    }
                    "code-review" => {
                        let diff = args
                            .get("diff")
                            .and_then(|v| v.as_str())
                            .unwrap_or("(no diff)");
                        Ok(format!("Reviewing: {diff}"))
                    }
                    _ => Err(format!("unknown skill: {name}")),
                }
            })
        })
    }

    #[test]
    fn skill_namespace_returns_skill() {
        let provider = make_skill_provider();
        assert_eq!(provider.namespace(), "skill");
    }

    #[test]
    fn skill_list_tools_returns_registered() {
        let provider = make_skill_provider();
        let tools = provider.list_tools();
        assert_eq!(tools.len(), 2);

        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"research"));
        assert!(names.contains(&"code-review"));
    }

    #[test]
    fn skill_tool_count() {
        let provider = make_skill_provider();
        assert_eq!(provider.tool_count(), 2);
    }

    #[tokio::test]
    async fn skill_call_tool_dispatches_correctly() {
        let provider = make_skill_provider();

        let result = provider
            .call_tool("research", serde_json::json!({"topic": "Rust async"}))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(result.content.len(), 1);
        match &result.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "Researching: Rust async"),
        }
    }

    #[tokio::test]
    async fn skill_call_tool_not_found() {
        let provider = make_skill_provider();

        let result = provider
            .call_tool("nonexistent", serde_json::json!({}))
            .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            ToolError::NotFound(name) => assert_eq!(name, "nonexistent"),
            other => panic!("expected NotFound, got: {other}"),
        }
    }

    #[tokio::test]
    async fn skill_call_tool_dispatcher_error_returns_error_result() {
        let provider = SkillToolProvider::new(
            vec![ToolDefinition {
                name: "broken-skill".into(),
                description: "A broken skill".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }],
            |_name, _args| Box::pin(async { Err("skill execution failed".to_string()) }),
        );

        let result = provider
            .call_tool("broken-skill", serde_json::json!({}))
            .await
            .unwrap();

        assert!(result.is_error);
        match &result.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "skill execution failed"),
        }
    }

    #[test]
    fn skill_refresh_replaces_tool_list() {
        let provider = make_skill_provider();
        assert_eq!(provider.tool_count(), 2);

        let new_tools = vec![ToolDefinition {
            name: "new-skill".into(),
            description: "A freshly loaded skill".into(),
            input_schema: serde_json::json!({"type": "object"}),
        }];

        let count = provider.refresh(new_tools);
        assert_eq!(count, 1);
        assert_eq!(provider.tool_count(), 1);

        let tools = provider.list_tools();
        assert_eq!(tools[0].name, "new-skill");
    }

    #[tokio::test]
    async fn skill_refresh_affects_call_routing() {
        let provider = make_skill_provider();

        // "research" works before refresh.
        let result = provider
            .call_tool("research", serde_json::json!({"topic": "test"}))
            .await;
        assert!(result.is_ok());

        // Replace with a different tool set.
        provider.refresh(vec![ToolDefinition {
            name: "only-skill".into(),
            description: "The only skill".into(),
            input_schema: serde_json::json!({"type": "object"}),
        }]);

        // "research" no longer exists.
        let result = provider
            .call_tool("research", serde_json::json!({"topic": "test"}))
            .await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ToolError::NotFound(_)));
    }

    #[test]
    fn skill_refresh_to_empty() {
        let provider = make_skill_provider();
        assert_eq!(provider.tool_count(), 2);

        let count = provider.refresh(vec![]);
        assert_eq!(count, 0);
        assert_eq!(provider.tool_count(), 0);
        assert!(provider.list_tools().is_empty());
    }

    #[test]
    fn skill_tools_handle_shares_state() {
        let provider = make_skill_provider();
        let handle = provider.tools_handle();

        // Read through the handle.
        {
            let tools = handle.read().unwrap();
            assert_eq!(tools.len(), 2);
        }

        // Refresh through provider.
        provider.refresh(vec![ToolDefinition {
            name: "via-handle".into(),
            description: "Test".into(),
            input_schema: serde_json::json!({"type": "object"}),
        }]);

        // Handle sees the update.
        {
            let tools = handle.read().unwrap();
            assert_eq!(tools.len(), 1);
            assert_eq!(tools[0].name, "via-handle");
        }
    }

    #[test]
    fn skill_debug_format() {
        let provider = make_skill_provider();
        let debug = format!("{:?}", provider);
        assert!(debug.contains("SkillToolProvider"));
        assert!(debug.contains("tool_count: 2"));
    }

    #[test]
    fn skill_empty_provider() {
        let provider = SkillToolProvider::new(
            vec![],
            |_name, _args| Box::pin(async { Ok("noop".to_string()) }),
        );
        assert_eq!(provider.namespace(), "skill");
        assert_eq!(provider.tool_count(), 0);
        assert!(provider.list_tools().is_empty());
    }

    // ── skill_to_tool_definition tests ──────────────────────────────────

    #[test]
    fn convert_skill_with_variables() {
        use clawft_types::skill::SkillDefinition;

        let mut skill = SkillDefinition::new("research", "Deep research");
        skill.variables = vec!["topic".into(), "depth".into()];

        let tool = skill_to_tool_definition(&skill);
        assert_eq!(tool.name, "research");
        assert_eq!(tool.description, "Deep research");

        let schema = &tool.input_schema;
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["topic"].is_object());
        assert!(schema["properties"]["depth"].is_object());
        assert_eq!(
            schema["required"],
            serde_json::json!(["topic", "depth"])
        );
    }

    #[test]
    fn convert_skill_without_variables() {
        use clawft_types::skill::SkillDefinition;

        let skill = SkillDefinition::new("simple", "A simple skill");

        let tool = skill_to_tool_definition(&skill);
        assert_eq!(tool.name, "simple");
        assert_eq!(tool.description, "A simple skill");

        // Should have a fallback "args" property.
        let schema = &tool.input_schema;
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["args"].is_object());
        assert_eq!(schema["properties"]["args"]["type"], "string");
    }

    #[test]
    fn convert_skills_batch() {
        use clawft_types::skill::SkillDefinition;

        let skills = vec![
            SkillDefinition::new("alpha", "Alpha"),
            SkillDefinition::new("beta", "Beta"),
            SkillDefinition::new("gamma", "Gamma"),
        ];

        let tools = skills_to_tool_definitions(&skills);
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[0].name, "alpha");
        assert_eq!(tools[1].name, "beta");
        assert_eq!(tools[2].name, "gamma");
    }

    #[test]
    fn convert_empty_skills_batch() {
        let tools = skills_to_tool_definitions(&[]);
        assert!(tools.is_empty());
    }
}
