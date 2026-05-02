//! MCP tool wrapper for bridging MCP server tools into the tool registry.
//!
//! Wraps MCP server tool definitions as implementations of the
//! [`Tool`](clawft_core::tools::registry::Tool) trait, allowing MCP
//! tools to be invoked by the agent loop just like built-in tools.
//!
//! Requires the `services` feature. When the feature is off, a no-op stub
//! is provided for [`register_mcp_tools`].

#[cfg(feature = "services")]
use std::sync::Arc;

#[cfg(feature = "services")]
use async_trait::async_trait;
#[cfg(feature = "services")]
use tracing::warn;

#[cfg(feature = "services")]
use clawft_core::tools::registry::{Tool, ToolError};
#[cfg(feature = "services")]
use clawft_services::mcp::transport::{HttpTransport, StdioTransport};
#[cfg(feature = "services")]
use clawft_services::mcp::{McpSession, ToolDefinition};
#[cfg(feature = "services")]
use clawft_types::config::MCPServerConfig;

// -- All MCP tool types and functions below are gated behind the `services` feature. --

#[cfg(feature = "services")]
/// Extract text from MCP tool result content blocks.
///
/// MCP tool results follow the format:
/// ```json
/// { "content": [{"type": "text", "text": "..."}], "isError": false }
/// ```
///
/// Returns `Ok(text)` for successful results or `Err(text)` when `isError`
/// is true. Falls back to the raw JSON string if no content blocks exist.
fn extract_mcp_tool_result(raw: &serde_json::Value) -> std::result::Result<String, String> {
    // If isError is true, return Err with concatenated text blocks.
    if raw
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        let text = extract_text_blocks(raw);
        return Err(text);
    }

    // If content array exists, extract text blocks.
    if let Some(text) = try_extract_text_blocks(raw) {
        return Ok(text);
    }

    // Fallback: return raw as string.
    Ok(serde_json::to_string(raw).unwrap_or_default())
}

#[cfg(feature = "services")]
/// Extract and concatenate all text blocks from an MCP content array.
fn extract_text_blocks(raw: &serde_json::Value) -> String {
    try_extract_text_blocks(raw).unwrap_or_else(|| serde_json::to_string(raw).unwrap_or_default())
}

#[cfg(feature = "services")]
/// Try to extract text from the `content` array, returning `None` if
/// the array is missing or contains no text blocks.
fn try_extract_text_blocks(raw: &serde_json::Value) -> Option<String> {
    let content = raw.get("content")?.as_array()?;
    let mut result = String::new();
    for block in content {
        if block.get("type").and_then(|v| v.as_str()) == Some("text")
            && let Some(text) = block.get("text").and_then(|v| v.as_str())
        {
            if !result.is_empty() {
                result.push('\n');
            }
            result.push_str(text);
        }
    }
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

#[cfg(feature = "services")]
/// Wraps an MCP tool definition for use in the `ToolRegistry`.
///
/// Each wrapper holds a reference to the shared [`McpSession`] for its
/// server and delegates execution to [`McpSession::call_tool`].
/// The tool name is prefixed with `{server_name}__` to avoid collisions
/// when multiple MCP servers expose tools with the same base name.
pub struct McpToolWrapper {
    /// Namespaced tool name: `"{server}__{tool}"`.
    full_name: String,
    /// The tool definition from the MCP server.
    tool_def: ToolDefinition,
    /// Shared session (with completed handshake) for this MCP server.
    session: Arc<McpSession>,
}

#[cfg(feature = "services")]
impl McpToolWrapper {
    /// Create a new wrapper.
    ///
    /// The tool will be registered as `"{server_name}__{tool_def.name}"`.
    pub fn new(server_name: &str, tool_def: ToolDefinition, session: Arc<McpSession>) -> Self {
        let full_name = format!("{}__{}", server_name, tool_def.name);
        Self {
            full_name,
            tool_def,
            session,
        }
    }
}

#[cfg(feature = "services")]
#[async_trait]
impl Tool for McpToolWrapper {
    fn name(&self) -> &str {
        &self.full_name
    }

    fn description(&self) -> &str {
        &self.tool_def.description
    }

    fn parameters(&self) -> serde_json::Value {
        self.tool_def.input_schema.clone()
    }

    async fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let raw = self
            .session
            .call_tool(&self.tool_def.name, args)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        // Extract text from MCP content blocks.
        match extract_mcp_tool_result(&raw) {
            Ok(text) => Ok(serde_json::json!({ "output": text })),
            Err(err_text) => Err(ToolError::ExecutionFailed(err_text)),
        }
    }
}

#[cfg(feature = "services")]
/// Create an MCP session from server configuration.
///
/// Chooses the transport based on config fields:
/// - If `command` is non-empty, spawns a child process via [`StdioTransport`].
/// - If `url` is non-empty (and command is empty), uses [`HttpTransport`].
/// - If both are empty, returns `None` with a warning log.
///
/// After creating the transport, performs the MCP initialize handshake
/// via [`McpSession::connect`] so that subsequent `tools/list` and
/// `tools/call` requests are accepted by the server.
pub async fn create_mcp_client(server_name: &str, config: &MCPServerConfig) -> Option<McpSession> {
    let transport: Box<dyn clawft_services::mcp::transport::McpTransport> =
        if !config.command.is_empty() {
            match StdioTransport::new(&config.command, &config.args, &config.env).await {
                Ok(transport) => Box::new(transport),
                Err(e) => {
                    warn!(
                        server = %server_name,
                        error = %e,
                        "failed to spawn MCP stdio transport"
                    );
                    return None;
                }
            }
        } else if !config.url.is_empty() {
            Box::new(HttpTransport::new(config.url.clone()))
        } else {
            warn!(server = %server_name, "MCP server has no command or URL, skipping");
            return None;
        };

    match McpSession::connect(transport).await {
        Ok(session) => Some(session),
        Err(e) => {
            warn!(
                server = %server_name,
                error = %e,
                "MCP initialize handshake failed"
            );
            None
        }
    }
}

#[cfg(feature = "services")]
/// Discover MCP servers and optionally register their tools.
///
/// For each MCP server in the config:
/// - Creates a client session (always).
/// - If `internal_only` is false, lists tools and registers them in the registry.
/// - If `internal_only` is true, the session is created but tools are NOT
///   registered (the server is available for internal use only).
///
/// Returns a map of all sessions (both internal and external) keyed by
/// server name. Callers can use these sessions for internal MCP calls.
pub async fn register_mcp_tools(
    config: &clawft_types::config::Config,
    registry: &mut clawft_core::tools::registry::ToolRegistry,
) -> std::collections::HashMap<String, Arc<McpSession>> {
    let mut sessions = std::collections::HashMap::new();

    for (server_name, server_config) in &config.tools.mcp_servers {
        match create_mcp_client(server_name, server_config).await {
            Some(session) => {
                let session = Arc::new(session);
                sessions.insert(server_name.clone(), session.clone());

                if server_config.internal_only {
                    tracing::info!(
                        server = %server_name,
                        "MCP server connected as internal-only (tools not registered)"
                    );
                    continue;
                }

                match session.list_tools().await {
                    Ok(tools) => {
                        let count = tools.len();
                        for tool_def in tools {
                            let wrapper =
                                McpToolWrapper::new(server_name, tool_def, session.clone());
                            registry.register(Arc::new(wrapper));
                        }
                        tracing::info!(
                            server = %server_name,
                            tools = count,
                            "registered MCP tools"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            server = %server_name,
                            error = %e,
                            "failed to list MCP tools, skipping"
                        );
                    }
                }
            }
            None => {
                // Warning already logged by create_mcp_client.
            }
        }
    }

    sessions
}

/// No-op: MCP tools require the `services` feature.
///
/// Returns an empty sessions map.
#[cfg(not(feature = "services"))]
pub async fn register_mcp_tools(
    _config: &clawft_types::config::Config,
    _registry: &mut clawft_core::tools::registry::ToolRegistry,
) -> std::collections::HashMap<String, std::sync::Arc<()>> {
    // MCP services feature not compiled in.
    std::collections::HashMap::new()
}

/// Register the delegation tool if an Anthropic API key is available.
///
/// Resolves the API key using a two-step lookup:
/// 1. `ANTHROPIC_API_KEY` environment variable (highest priority)
/// 2. `config_api_key` from the providers config section (fallback)
///
/// Creates a [`ClaudeDelegator`] and [`DelegateTaskTool`] and registers
/// them in the tool registry.
///
/// Gracefully degrades: if no API key is found or delegation is disabled
/// in config, delegation is simply not available (not a fatal error).
#[cfg(feature = "delegate")]
pub fn register_delegation(
    config: &clawft_types::delegation::DelegationConfig,
    registry: &mut clawft_core::tools::registry::ToolRegistry,
    config_api_key: Option<&str>,
) {
    use clawft_services::delegation::DelegationEngine;
    use clawft_services::delegation::claude::ClaudeDelegator;
    use clawft_tools::delegate_tool::DelegateTaskTool;

    if !config.claude_enabled {
        tracing::info!("delegation disabled in config, skipping");
        return;
    }

    // Resolve API key: env var > config providers section.
    let api_key = match std::env::var("ANTHROPIC_API_KEY") {
        Ok(key) if !key.is_empty() => key,
        Ok(_) => {
            // Env var is set but empty -- fall through to config fallback.
            tracing::debug!("ANTHROPIC_API_KEY env var is empty, trying config fallback");
            match config_api_key {
                Some(key) if !key.is_empty() => key.to_string(),
                _ => {
                    tracing::info!(
                        "ANTHROPIC_API_KEY env var is set but empty and no key in providers config; \
                         delegation disabled"
                    );
                    return;
                }
            }
        }
        Err(_) => {
            // Env var not set at all -- try config fallback.
            match config_api_key {
                Some(key) if !key.is_empty() => {
                    tracing::debug!("using Anthropic API key from providers config");
                    key.to_string()
                }
                _ => {
                    tracing::info!(
                        "ANTHROPIC_API_KEY not set and no key in providers config; \
                         delegation disabled"
                    );
                    return;
                }
            }
        }
    };

    let delegator = match ClaudeDelegator::new(config, api_key) {
        Some(d) => Arc::new(d),
        None => {
            tracing::warn!("failed to create ClaudeDelegator, delegation disabled");
            return;
        }
    };

    let engine = Arc::new(DelegationEngine::new(config.clone()));

    // Snapshot the current tool schemas before registering the delegate tool
    // (to avoid the delegate tool appearing in its own tool list).
    let tool_schemas = registry.schemas();

    // Create a snapshot of the current registry so the delegate tool can
    // execute tool calls from the Claude sub-agent. The snapshot contains
    // all tools registered so far (but not the delegate tool itself,
    // preventing recursive delegation).
    let registry_snapshot = Arc::new(registry.snapshot());

    let delegate_tool = DelegateTaskTool::new(delegator, engine, tool_schemas, registry_snapshot);

    registry.register(Arc::new(delegate_tool));
    tracing::info!("delegation tool registered");
}

/// No-op stub when the `delegate` feature is not enabled.
#[cfg(not(feature = "delegate"))]
pub fn register_delegation(
    _config: &clawft_types::delegation::DelegationConfig,
    _registry: &mut clawft_core::tools::registry::ToolRegistry,
    _config_api_key: Option<&str>,
) {
    // Delegation feature not compiled in.
}

#[cfg(all(test, feature = "services"))]
mod tests {
    

    use super::*;
    use clawft_services::mcp::transport::McpTransport;
    use clawft_services::mcp::types::{JsonRpcRequest, JsonRpcResponse};

    /// A minimal mock transport for testing within this crate.
    ///
    /// The `MockTransport` in `clawft-services` is `#[cfg(test)]`-gated
    /// and therefore unavailable outside that crate, so we provide our own.
    struct TestTransport {
        responses: tokio::sync::Mutex<Vec<JsonRpcResponse>>,
    }

    impl TestTransport {
        fn new(responses: Vec<JsonRpcResponse>) -> Self {
            Self {
                responses: tokio::sync::Mutex::new(responses),
            }
        }
    }

    #[async_trait]
    impl McpTransport for TestTransport {
        async fn send_request(
            &self,
            _request: JsonRpcRequest,
        ) -> clawft_services::error::Result<JsonRpcResponse> {
            let mut responses = self.responses.lock().await;
            if responses.is_empty() {
                Err(clawft_services::error::ServiceError::McpTransport(
                    "no more mock responses".into(),
                ))
            } else {
                Ok(responses.remove(0))
            }
        }

        async fn send_notification(
            &self,
            _method: &str,
            _params: serde_json::Value,
        ) -> clawft_services::error::Result<()> {
            Ok(())
        }
    }

    fn make_tool_def() -> ToolDefinition {
        ToolDefinition {
            name: "echo".into(),
            description: "Echo input".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string" }
                }
            }),
        }
    }

    /// Build a mock initialize handshake response.
    fn make_init_response(id: u64) -> JsonRpcResponse {
        JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id,
            result: Some(serde_json::json!({
                "protocolVersion": "2025-06-18",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "test-server", "version": "0.1.0" }
            })),
            error: None,
        }
    }

    /// Create a mock session that has already completed the initialize handshake.
    ///
    /// Prepends an init response so `McpSession::connect` succeeds, then
    /// the remaining `responses` are available for subsequent requests.
    async fn make_session(responses: Vec<JsonRpcResponse>) -> Arc<McpSession> {
        let mut all = vec![make_init_response(1)];
        all.extend(responses);
        let transport = TestTransport::new(all);
        Arc::new(McpSession::connect(Box::new(transport)).await.unwrap())
    }

    // ── McpToolWrapper unit tests ───────────────────────────────────────

    #[tokio::test]
    async fn wrapper_name_is_namespaced() {
        let session = make_session(vec![]).await;
        let wrapper = McpToolWrapper::new("myserver", make_tool_def(), session);

        assert_eq!(wrapper.name(), "myserver__echo");
    }

    #[tokio::test]
    async fn wrapper_description_delegates() {
        let session = make_session(vec![]).await;
        let wrapper = McpToolWrapper::new("srv", make_tool_def(), session);

        assert_eq!(wrapper.description(), "Echo input");
    }

    #[tokio::test]
    async fn wrapper_parameters_returns_schema() {
        let session = make_session(vec![]).await;
        let wrapper = McpToolWrapper::new("srv", make_tool_def(), session);

        let params = wrapper.parameters();
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["text"].is_object());
    }

    #[tokio::test]
    async fn wrapper_execute_delegates_to_client() {
        // MCP content block format.
        let response = JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: 2,
            result: Some(serde_json::json!({
                "content": [{"type": "text", "text": "hello"}],
                "isError": false
            })),
            error: None,
        };
        let session = make_session(vec![response]).await;
        let wrapper = McpToolWrapper::new("srv", make_tool_def(), session);

        let result = wrapper.execute(serde_json::json!({"text": "hello"})).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap()["output"], "hello");
    }

    #[tokio::test]
    async fn wrapper_execute_maps_transport_error() {
        // Session with no remaining responses will produce a transport error.
        let session = make_session(vec![]).await;
        let wrapper = McpToolWrapper::new("srv", make_tool_def(), session);

        let result = wrapper.execute(serde_json::json!({"text": "hello"})).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ToolError::ExecutionFailed(msg) => {
                assert!(msg.contains("no more mock responses"));
            }
            other => panic!("expected ExecutionFailed, got: {other}"),
        }
    }

    #[tokio::test]
    async fn wrapper_execute_maps_protocol_error() {
        let response = JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: 2,
            result: None,
            error: Some(clawft_services::mcp::types::JsonRpcError {
                code: -32601,
                message: "method not found".into(),
                data: None,
            }),
        };
        let session = make_session(vec![response]).await;
        let wrapper = McpToolWrapper::new("srv", make_tool_def(), session);

        let result = wrapper.execute(serde_json::json!({})).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ToolError::ExecutionFailed(msg) => {
                assert!(msg.contains("method not found"));
            }
            other => panic!("expected ExecutionFailed, got: {other}"),
        }
    }

    #[tokio::test]
    async fn wrapper_is_object_safe() {
        // Verify McpToolWrapper can be used as a `dyn Tool` trait object.
        fn accepts_tool(_t: &dyn Tool) {}
        let session = make_session(vec![]).await;
        let wrapper = McpToolWrapper::new("srv", make_tool_def(), session);
        accepts_tool(&wrapper);
    }

    // ── create_mcp_client tests ─────────────────────────────────────────

    #[tokio::test]
    async fn create_client_with_url_attempts_handshake() {
        // With a URL pointing to a non-existent server, the handshake
        // will fail and `create_mcp_client` should return `None`.
        let config = MCPServerConfig {
            url: "http://localhost:19876".into(),
            ..Default::default()
        };
        let result = create_mcp_client("test", &config).await;
        // Handshake fails against a non-existent server.
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn create_client_empty_returns_none() {
        let config = MCPServerConfig::default();
        let result = create_mcp_client("test", &config).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn create_client_bad_command_returns_none() {
        // A command that does not exist should fail gracefully.
        let config = MCPServerConfig {
            command: "__nonexistent_binary_clawft_test__".into(),
            ..Default::default()
        };
        let result = create_mcp_client("test", &config).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn create_client_prefers_command_over_url() {
        // When both command and url are set, command takes priority.
        // We use a bad command to verify it tried the command path
        // (returns None from spawn failure) rather than the URL path.
        let config = MCPServerConfig {
            command: "__nonexistent_binary_clawft_test__".into(),
            url: "http://localhost:19876".into(),
            ..Default::default()
        };
        let result = create_mcp_client("test", &config).await;
        // Command spawn fails, so we get None.
        assert!(result.is_none());
    }

    // ── Content extraction tests ────────────────────────────────────────

    #[test]
    fn extract_single_text_block() {
        let raw = serde_json::json!({
            "content": [{"type": "text", "text": "hello"}],
            "isError": false
        });
        let result = extract_mcp_tool_result(&raw);
        assert_eq!(result, Ok("hello".to_string()));
    }

    #[test]
    fn extract_multiple_text_blocks() {
        let raw = serde_json::json!({
            "content": [
                {"type": "text", "text": "a"},
                {"type": "text", "text": "b"}
            ]
        });
        let result = extract_mcp_tool_result(&raw);
        assert_eq!(result, Ok("a\nb".to_string()));
    }

    #[test]
    fn extract_error_result() {
        let raw = serde_json::json!({
            "content": [{"type": "text", "text": "error msg"}],
            "isError": true
        });
        let result = extract_mcp_tool_result(&raw);
        assert_eq!(result, Err("error msg".to_string()));
    }

    #[test]
    fn extract_error_no_content() {
        let raw = serde_json::json!({
            "isError": true
        });
        let result = extract_mcp_tool_result(&raw);
        // Falls back to raw JSON string.
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("isError"));
    }

    #[test]
    fn extract_fallback_to_raw_json() {
        let raw = serde_json::json!({"output": "raw"});
        let result = extract_mcp_tool_result(&raw);
        assert!(result.is_ok());
        let text = result.unwrap();
        assert!(text.contains("raw"));
    }

    #[test]
    fn extract_skips_non_text_blocks() {
        let raw = serde_json::json!({
            "content": [
                {"type": "image", "data": "base64..."},
                {"type": "text", "text": "visible"}
            ],
            "isError": false
        });
        let result = extract_mcp_tool_result(&raw);
        assert_eq!(result, Ok("visible".to_string()));
    }

    #[test]
    fn extract_empty_content_falls_back() {
        let raw = serde_json::json!({
            "content": [],
            "isError": false
        });
        let result = extract_mcp_tool_result(&raw);
        assert!(result.is_ok());
        // Falls back to raw JSON since no text blocks found.
        let text = result.unwrap();
        assert!(text.contains("content"));
    }

    #[test]
    fn extract_is_error_false_explicitly() {
        let raw = serde_json::json!({
            "content": [{"type": "text", "text": "ok"}],
            "isError": false
        });
        assert_eq!(extract_mcp_tool_result(&raw), Ok("ok".to_string()));
    }

    #[test]
    fn extract_is_error_absent_treated_as_false() {
        let raw = serde_json::json!({
            "content": [{"type": "text", "text": "ok"}]
        });
        assert_eq!(extract_mcp_tool_result(&raw), Ok("ok".to_string()));
    }

    #[tokio::test]
    async fn wrapper_execute_extracts_content_blocks() {
        let response = JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: 2,
            result: Some(serde_json::json!({
                "content": [
                    {"type": "text", "text": "line1"},
                    {"type": "text", "text": "line2"}
                ],
                "isError": false
            })),
            error: None,
        };
        let session = make_session(vec![response]).await;
        let wrapper = McpToolWrapper::new("srv", make_tool_def(), session);

        let result = wrapper.execute(serde_json::json!({})).await.unwrap();
        assert_eq!(result["output"], "line1\nline2");
    }

    #[tokio::test]
    async fn wrapper_execute_returns_error_on_is_error() {
        let response = JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: 2,
            result: Some(serde_json::json!({
                "content": [{"type": "text", "text": "tool failed"}],
                "isError": true
            })),
            error: None,
        };
        let session = make_session(vec![response]).await;
        let wrapper = McpToolWrapper::new("srv", make_tool_def(), session);

        let result = wrapper.execute(serde_json::json!({})).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ToolError::ExecutionFailed(msg) => {
                assert_eq!(msg, "tool failed");
            }
            other => panic!("expected ExecutionFailed, got: {other}"),
        }
    }

    #[tokio::test]
    async fn wrapper_execute_fallback_for_raw_response() {
        // Some MCP servers may return non-standard result shapes.
        let response = JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: 2,
            result: Some(serde_json::json!({"data": 42})),
            error: None,
        };
        let session = make_session(vec![response]).await;
        let wrapper = McpToolWrapper::new("srv", make_tool_def(), session);

        let result = wrapper.execute(serde_json::json!({})).await.unwrap();
        // Falls back to raw JSON wrapped in output.
        let output = result["output"].as_str().unwrap();
        assert!(output.contains("42"));
    }
}
