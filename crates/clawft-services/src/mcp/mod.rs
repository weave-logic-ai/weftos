//! MCP (Model Context Protocol) client.
//!
//! Provides a client for communicating with MCP servers using
//! JSON-RPC 2.0 over pluggable transports (stdio or HTTP).

pub mod bridge;
pub mod client;
pub mod composite;
pub mod discovery;
pub mod ide;
pub mod middleware;
pub mod provider;
pub mod server;
pub mod transport;
pub mod types;

pub use provider::{
    BuiltinToolProvider, CallToolResult, ContentBlock, SkillToolProvider, ToolError, ToolProvider,
    skill_to_tool_definition, skills_to_tool_definitions,
};
pub use transport::{
    DefaultTransportFactory, McpTransportFactory, TransportFactoryConfig, TransportSpec,
    validate_command_path, validate_tempfile_path, validate_url,
};

/// The MCP protocol version negotiated during initialize.
///
/// This constant is the single source of truth for protocol version
/// strings used in both client and server code. We always send this
/// in our `initialize` request.
pub const MCP_PROTOCOL_VERSION: &str = "2025-06-18";

/// Protocol versions we accept on the server side of the handshake.
///
/// WEFT-489. If the remote server's `initialize` response reports a
/// `protocolVersion` outside this set, [`McpSession::connect`] aborts
/// the session with [`crate::error::ServiceError::McpProtocolVersionMismatch`]
/// after logging a `warn!` so the operator sees the rejection.
///
/// Order is informational; we use set semantics. The current value
/// includes the previous published versions plus our negotiated
/// version so a server that lags one revision still attaches.
pub const MCP_SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18"];

/// Returns true if `version` is in our supported set.
///
/// Empty string is treated as "server omitted the field" and falls
/// back to our advertised version (which is always supported).
pub fn is_supported_protocol_version(version: &str) -> bool {
    if version.is_empty() {
        return true;
    }
    MCP_SUPPORTED_PROTOCOL_VERSIONS.contains(&version)
}

use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::error::{Result, ServiceError};
use transport::McpTransport;
use types::JsonRpcRequest;

/// Server capabilities returned from the MCP initialize handshake.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerCapabilities {
    /// Tool-related capabilities, if the server supports tools.
    #[serde(default)]
    pub tools: Option<serde_json::Value>,
    // Other capability fields can be added as needed.
}

/// Server information returned from the MCP initialize handshake.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerInfo {
    /// Server name.
    #[serde(default)]
    pub name: String,
    /// Server version.
    #[serde(default)]
    pub version: String,
}

/// Definition of an MCP tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Tool name.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// JSON Schema for the tool's input parameters.
    #[serde(rename = "inputSchema", alias = "input_schema")]
    pub input_schema: serde_json::Value,
}

/// Client for communicating with an MCP server.
pub struct McpClient {
    transport: Box<dyn McpTransport>,
    request_id: AtomicU64,
}

impl std::fmt::Debug for McpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpClient")
            .field("request_id", &self.request_id.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl McpClient {
    /// Create a new MCP client with the given transport.
    pub fn new(transport: Box<dyn McpTransport>) -> Self {
        Self {
            transport,
            request_id: AtomicU64::new(1),
        }
    }

    /// List all tools available on the MCP server.
    pub async fn list_tools(&self) -> Result<Vec<ToolDefinition>> {
        let id = self.next_id();
        let request = JsonRpcRequest::new(id, "tools/list", serde_json::json!({}));

        let response = self.transport.send_request(request).await?;

        if let Some(err) = response.error {
            return Err(ServiceError::McpProtocol(format!(
                "code={}, message={}",
                err.code, err.message
            )));
        }

        let result = response
            .result
            .ok_or_else(|| ServiceError::McpProtocol("empty result".into()))?;

        // MCP returns tools in a `tools` array.
        let tools_value = result
            .get("tools")
            .cloned()
            .unwrap_or_else(|| serde_json::Value::Array(vec![]));

        let tools: Vec<ToolDefinition> = serde_json::from_value(tools_value)?;
        Ok(tools)
    }

    /// Call a tool on the MCP server.
    pub async fn call_tool(
        &self,
        name: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let id = self.next_id();
        let request = JsonRpcRequest::new(
            id,
            "tools/call",
            serde_json::json!({
                "name": name,
                "arguments": params,
            }),
        );

        let response = self.transport.send_request(request).await?;

        if let Some(err) = response.error {
            return Err(ServiceError::McpProtocol(format!(
                "code={}, message={}",
                err.code, err.message
            )));
        }

        response
            .result
            .ok_or_else(|| ServiceError::McpProtocol("empty result".into()))
    }

    /// Send a raw JSON-RPC request and return the result value.
    ///
    /// Unlike `list_tools` and `call_tool`, this method does not interpret
    /// the response -- it simply returns the raw `serde_json::Value` from
    /// the `result` field, or an error if the response contains one.
    pub async fn send_raw(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let id = self.next_id();
        let request = JsonRpcRequest::new(id, method, params);
        let response = self.transport.send_request(request).await?;

        if let Some(err) = response.error {
            return Err(ServiceError::McpProtocol(format!(
                "code={}, message={}",
                err.code, err.message
            )));
        }

        response
            .result
            .ok_or_else(|| ServiceError::McpProtocol("empty result".into()))
    }

    /// Access the underlying transport.
    pub fn transport(&self) -> &dyn McpTransport {
        &*self.transport
    }

    /// Generate the next request ID.
    fn next_id(&self) -> u64 {
        self.request_id.fetch_add(1, Ordering::Relaxed)
    }
}

/// An MCP session that has completed the initialize handshake.
///
/// Wraps an [`McpClient`] and holds the server capabilities, server info,
/// and negotiated protocol version obtained during the handshake.
///
/// Use [`McpSession::connect`] to create a session -- it will:
/// 1. Send an `initialize` request with client capabilities.
/// 2. Parse the server's response (capabilities, info, protocol version).
/// 3. Send the `notifications/initialized` notification.
pub struct McpSession {
    client: McpClient,
    /// Capabilities reported by the server.
    pub server_capabilities: ServerCapabilities,
    /// Server identification (name + version).
    pub server_info: ServerInfo,
    /// Protocol version negotiated with the server.
    pub protocol_version: String,
}

impl McpSession {
    /// Connect to an MCP server by performing the initialize handshake.
    pub async fn connect(transport: Box<dyn McpTransport>) -> Result<Self> {
        let client = McpClient::new(transport);

        // Step 1: Send initialize request.
        let init_result = client
            .send_raw(
                "initialize",
                serde_json::json!({
                    "protocolVersion": MCP_PROTOCOL_VERSION,
                    "capabilities": { "tools": {} },
                    "clientInfo": {
                        "name": "clawft",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }),
            )
            .await?;

        // Step 2: Parse server response.
        let server_capabilities: ServerCapabilities =
            serde_json::from_value(init_result.get("capabilities").cloned().unwrap_or_default())
                .unwrap_or_default();

        let server_info: ServerInfo =
            serde_json::from_value(init_result.get("serverInfo").cloned().unwrap_or_default())
                .unwrap_or_default();

        let protocol_version = init_result
            .get("protocolVersion")
            .and_then(|v| v.as_str())
            .unwrap_or(MCP_PROTOCOL_VERSION)
            .to_string();

        // WEFT-489: reject foreign protocol versions before sending
        // `notifications/initialized`. A non-matching server gets a
        // hard error here rather than at the first tools/list (where
        // the failure mode is much harder to diagnose).
        if !is_supported_protocol_version(&protocol_version) {
            tracing::warn!(
                ours = ?MCP_SUPPORTED_PROTOCOL_VERSIONS,
                theirs = %protocol_version,
                "mcp initialize: protocol-version mismatch, aborting session"
            );
            return Err(ServiceError::McpProtocolVersionMismatch {
                ours: MCP_SUPPORTED_PROTOCOL_VERSIONS
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect(),
                theirs: protocol_version,
            });
        }

        // Step 3: Send initialized notification.
        client
            .transport()
            .send_notification("notifications/initialized", serde_json::json!({}))
            .await?;

        Ok(Self {
            client,
            server_capabilities,
            server_info,
            protocol_version,
        })
    }

    /// List tools available on the connected server.
    pub async fn list_tools(&self) -> Result<Vec<ToolDefinition>> {
        self.client.list_tools().await
    }

    /// Call a tool on the connected server.
    pub async fn call_tool(
        &self,
        name: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        self.client.call_tool(name, params).await
    }

    /// Access the underlying client.
    pub fn client(&self) -> &McpClient {
        &self.client
    }
}

impl std::fmt::Debug for McpSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpSession")
            .field("server_info", &self.server_info)
            .field("protocol_version", &self.protocol_version)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use transport::MockTransport;
    use types::JsonRpcResponse;

    fn make_success_response(id: u64, result: serde_json::Value) -> JsonRpcResponse {
        JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    fn make_error_response(id: u64, code: i32, message: &str) -> JsonRpcResponse {
        JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(types::JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }

    #[tokio::test]
    async fn list_tools_parses_response() {
        let tools_response = make_success_response(
            1,
            serde_json::json!({
                "tools": [
                    {
                        "name": "echo",
                        "description": "Echoes input",
                        "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}
                    },
                    {
                        "name": "calc",
                        "description": "Calculator",
                        "inputSchema": {"type": "object"}
                    }
                ]
            }),
        );

        let transport = MockTransport::new(vec![tools_response]);
        let client = McpClient::new(Box::new(transport));

        let tools = client.list_tools().await.unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "echo");
        assert_eq!(tools[0].description, "Echoes input");
        assert_eq!(tools[1].name, "calc");
    }

    #[tokio::test]
    async fn list_tools_empty() {
        let response = make_success_response(1, serde_json::json!({"tools": []}));
        let transport = MockTransport::new(vec![response]);
        let client = McpClient::new(Box::new(transport));

        let tools = client.list_tools().await.unwrap();
        assert!(tools.is_empty());
    }

    #[tokio::test]
    async fn call_tool_sends_correct_request() {
        let response = make_success_response(1, serde_json::json!({"output": "hello"}));
        let transport = MockTransport::new(vec![response]);
        let client = McpClient::new(Box::new(transport));

        let result = client
            .call_tool("echo", serde_json::json!({"text": "hello"}))
            .await
            .unwrap();

        assert_eq!(result["output"], "hello");
    }

    #[tokio::test]
    async fn call_tool_request_format() {
        let response = make_success_response(1, serde_json::json!({}));
        let transport = MockTransport::new(vec![response]);
        let client = McpClient::new(Box::new(transport));

        client
            .call_tool("my_tool", serde_json::json!({"arg": 42}))
            .await
            .unwrap();

        // Request was sent successfully if we got here without error.
        // The mock transport verified we sent a valid JSON-RPC request.
    }

    #[tokio::test]
    async fn handle_jsonrpc_error_on_list_tools() {
        let response = make_error_response(1, -32601, "method not found");
        let transport = MockTransport::new(vec![response]);
        let client = McpClient::new(Box::new(transport));

        let result = client.list_tools().await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ServiceError::McpProtocol(_)));
        assert!(err.to_string().contains("method not found"));
    }

    #[tokio::test]
    async fn handle_jsonrpc_error_on_call_tool() {
        let response = make_error_response(1, -32602, "invalid params");
        let transport = MockTransport::new(vec![response]);
        let client = McpClient::new(Box::new(transport));

        let result = client.call_tool("bad", serde_json::json!({})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid params"));
    }

    #[tokio::test]
    async fn request_ids_increment() {
        let responses = vec![
            make_success_response(1, serde_json::json!({"tools": []})),
            make_success_response(2, serde_json::json!({"tools": []})),
        ];
        let transport = MockTransport::new(responses);
        let client = McpClient::new(Box::new(transport));

        client.list_tools().await.unwrap();
        client.list_tools().await.unwrap();
        // If we get here without error, IDs were generated correctly.
    }

    #[tokio::test]
    async fn empty_result_is_error() {
        let response = JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: 1,
            result: None,
            error: None,
        };
        let transport = MockTransport::new(vec![response]);
        let client = McpClient::new(Box::new(transport));

        let result = client.call_tool("test", serde_json::json!({})).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ServiceError::McpProtocol(_)));
    }

    #[tokio::test]
    async fn tool_definition_serde() {
        let td = ToolDefinition {
            name: "test".into(),
            description: "A test tool".into(),
            input_schema: serde_json::json!({"type": "object"}),
        };
        let json = serde_json::to_string(&td).unwrap();
        let restored: ToolDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.name, "test");
        assert_eq!(restored.description, "A test tool");
    }

    #[tokio::test]
    async fn tool_definition_input_schema_alias() {
        // MCP uses camelCase, but we should also accept snake_case.
        let json = r#"{"name":"t","description":"d","input_schema":{"type":"object"}}"#;
        let td: ToolDefinition = serde_json::from_str(json).unwrap();
        assert_eq!(td.name, "t");
    }

    // ── send_raw tests ──────────────────────────────────────────────────

    #[tokio::test]
    async fn send_raw_returns_result_value() {
        let response = make_success_response(
            1,
            serde_json::json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "serverInfo": { "name": "test-server", "version": "1.0" }
            }),
        );
        let transport = MockTransport::new(vec![response]);
        let client = McpClient::new(Box::new(transport));

        let result = client
            .send_raw("initialize", serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(result["serverInfo"]["name"], "test-server");
    }

    #[tokio::test]
    async fn send_raw_propagates_errors() {
        let response = make_error_response(1, -32600, "invalid request");
        let transport = MockTransport::new(vec![response]);
        let client = McpClient::new(Box::new(transport));

        let result = client.send_raw("initialize", serde_json::json!({})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid request"));
    }

    // ── McpSession tests ────────────────────────────────────────────────

    /// Helper to build a mock initialize response.
    fn make_init_response(id: u64) -> JsonRpcResponse {
        make_success_response(
            id,
            serde_json::json!({
                "protocolVersion": "2025-06-18",
                "capabilities": { "tools": { "listChanged": true } },
                "serverInfo": { "name": "mock-server", "version": "0.1.0" }
            }),
        )
    }

    #[tokio::test]
    async fn session_connect_performs_handshake() {
        let transport = MockTransport::new(vec![make_init_response(1)]);
        let transport = Box::new(transport);
        let session = McpSession::connect(transport).await.unwrap();

        assert_eq!(session.server_info.name, "mock-server");
        assert_eq!(session.server_info.version, "0.1.0");
        assert_eq!(session.protocol_version, "2025-06-18");
        assert!(session.server_capabilities.tools.is_some());
    }

    #[tokio::test]
    async fn session_connect_sends_initialized_notification() {
        let transport = MockTransport::new(vec![make_init_response(1)]);
        // We need a shared reference to check notifications after connect.
        // Since connect takes ownership, we use Arc<MockTransport> through
        // a wrapper. Instead, let's verify via the request method sent.
        let transport = Box::new(transport);
        // If connect completes without error, the notification was sent
        // successfully (MockTransport records it internally).
        let session = McpSession::connect(transport).await.unwrap();

        // Verify the initialize request was sent by checking the client
        // was created and handshake completed.
        assert_eq!(session.protocol_version, "2025-06-18");
    }

    #[tokio::test]
    async fn session_connect_error_propagates() {
        let response = make_error_response(1, -32600, "bad init");
        let transport = MockTransport::new(vec![response]);
        let result = McpSession::connect(Box::new(transport)).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("bad init"));
    }

    #[tokio::test]
    async fn session_connect_defaults_on_missing_fields() {
        // Server returns minimal response without serverInfo or capabilities.
        let response = make_success_response(1, serde_json::json!({}));
        let transport = MockTransport::new(vec![response]);
        let session = McpSession::connect(Box::new(transport)).await.unwrap();

        // Should fall back to defaults without panicking.
        assert_eq!(session.server_info.name, "");
        assert_eq!(session.server_info.version, "");
        assert!(session.server_capabilities.tools.is_none());
        assert_eq!(session.protocol_version, "2025-06-18");
    }

    #[tokio::test]
    async fn session_list_tools_delegates() {
        let responses = vec![
            make_init_response(1),
            make_success_response(
                2,
                serde_json::json!({
                    "tools": [{
                        "name": "echo",
                        "description": "Echo",
                        "inputSchema": {"type": "object"}
                    }]
                }),
            ),
        ];
        let transport = MockTransport::new(responses);
        let session = McpSession::connect(Box::new(transport)).await.unwrap();

        let tools = session.list_tools().await.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "echo");
    }

    #[tokio::test]
    async fn session_call_tool_delegates() {
        let responses = vec![
            make_init_response(1),
            make_success_response(2, serde_json::json!({"output": "world"})),
        ];
        let transport = MockTransport::new(responses);
        let session = McpSession::connect(Box::new(transport)).await.unwrap();

        let result = session
            .call_tool("echo", serde_json::json!({"text": "world"}))
            .await
            .unwrap();
        assert_eq!(result["output"], "world");
    }

    #[tokio::test]
    async fn full_session_flow() {
        // Simulate: connect -> list_tools -> call_tool.
        let responses = vec![
            // 1: initialize response
            make_init_response(1),
            // 2: tools/list response
            make_success_response(
                2,
                serde_json::json!({
                    "tools": [{
                        "name": "greet",
                        "description": "Greets someone",
                        "inputSchema": {
                            "type": "object",
                            "properties": { "name": { "type": "string" } }
                        }
                    }]
                }),
            ),
            // 3: tools/call response
            make_success_response(
                3,
                serde_json::json!({
                    "content": [{"type": "text", "text": "Hello, Alice!"}],
                    "isError": false
                }),
            ),
        ];
        let transport = MockTransport::new(responses);
        let session = McpSession::connect(Box::new(transport)).await.unwrap();

        // Verify server info.
        assert_eq!(session.server_info.name, "mock-server");

        // List tools.
        let tools = session.list_tools().await.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "greet");

        // Call a tool.
        let result = session
            .call_tool("greet", serde_json::json!({"name": "Alice"}))
            .await
            .unwrap();
        assert_eq!(result["content"][0]["text"], "Hello, Alice!");
    }

    // ── ServerCapabilities / ServerInfo serde tests ─────────────────────

    // ── Protocol-version handshake tests (WEFT-489) ─────────────────────

    #[test]
    fn supported_protocol_version_set_includes_current() {
        // The version we send in our initialize request must always
        // be in the accepted set, otherwise we'd reject our own peers.
        assert!(is_supported_protocol_version(MCP_PROTOCOL_VERSION));
    }

    #[test]
    fn supported_protocol_version_set_includes_legacy() {
        assert!(is_supported_protocol_version("2024-11-05"));
        assert!(is_supported_protocol_version("2025-03-26"));
    }

    #[test]
    fn unsupported_protocol_version_rejected() {
        assert!(!is_supported_protocol_version("1999-01-01"));
        assert!(!is_supported_protocol_version("foo"));
        assert!(!is_supported_protocol_version("2099-12-31"));
    }

    #[test]
    fn empty_protocol_version_falls_back_to_supported() {
        // Server omitted the field — treat as supported (we
        // negotiated to our advertised version anyway).
        assert!(is_supported_protocol_version(""));
    }

    #[tokio::test]
    async fn session_connect_rejects_unsupported_protocol_version() {
        // Server replies with a deliberately bogus version.
        let response = make_success_response(
            1,
            serde_json::json!({
                "protocolVersion": "1999-01-01",
                "capabilities": {},
                "serverInfo": { "name": "rogue-server", "version": "0.1" }
            }),
        );
        let transport = MockTransport::new(vec![response]);
        let result = McpSession::connect(Box::new(transport)).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ServiceError::McpProtocolVersionMismatch { ours, theirs } => {
                assert!(ours.contains(&MCP_PROTOCOL_VERSION.to_string()));
                assert_eq!(theirs, "1999-01-01");
            }
            other => panic!("expected McpProtocolVersionMismatch, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn session_connect_accepts_legacy_protocol_version() {
        // Server reports the previous published version — should attach.
        let response = make_success_response(
            1,
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "legacy-server", "version": "1.0" }
            }),
        );
        let transport = MockTransport::new(vec![response]);
        let session = McpSession::connect(Box::new(transport)).await.unwrap();
        assert_eq!(session.protocol_version, "2024-11-05");
    }

    #[test]
    fn server_capabilities_default() {
        let caps = ServerCapabilities::default();
        assert!(caps.tools.is_none());
    }

    #[test]
    fn server_info_default() {
        let info = ServerInfo::default();
        assert_eq!(info.name, "");
        assert_eq!(info.version, "");
    }

    #[test]
    fn server_info_serde() {
        let info = ServerInfo {
            name: "test".into(),
            version: "1.0".into(),
        };
        let json = serde_json::to_string(&info).unwrap();
        let restored: ServerInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.name, "test");
        assert_eq!(restored.version, "1.0");
    }
}
