//! Bidirectional MCP bridge between clawft and Claude Code.
//!
//! The bridge orchestrates both directions of MCP communication:
//!
//! - **Outbound (clawft -> Claude Code)**: clawft's MCP `server.rs` exposes
//!   tools to Claude Code. Claude Code registers clawft as an MCP server
//!   via `claude mcp add`.
//!
//! - **Inbound (Claude Code -> clawft)**: clawft connects to Claude Code's
//!   MCP server as a client, making Claude Code's tools available in clawft.
//!
//! # Hot-reload
//!
//! The bridge supports hot-reload via the drain-and-swap protocol defined
//! in `discovery.rs`. When bridge configuration changes, the old connection
//! is drained and a new one established.
//!
//! # Configuration
//!
//! ```toml
//! [tools.mcp_servers.claude-code]
//! command = "claude"
//! args = ["mcp", "serve"]
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use super::transport::{
    DefaultTransportFactory, McpTransportFactory, TransportFactoryConfig, TransportSpec,
};
use super::{McpSession, ToolDefinition};
use crate::error::{Result, ServiceError};

/// Bridge status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgeStatus {
    /// Bridge is not configured.
    Unconfigured,
    /// Bridge is initializing.
    Initializing,
    /// Bridge is active (both directions working).
    Active,
    /// Outbound only (clawft -> Claude Code).
    OutboundOnly,
    /// Inbound only (Claude Code -> clawft).
    InboundOnly,
    /// Bridge encountered an error.
    Error,
    /// Bridge is shutting down.
    ShuttingDown,
}

/// Configuration for the MCP bridge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeConfig {
    /// Whether the bridge is enabled.
    #[serde(default)]
    pub enabled: bool,

    /// Command to start Claude Code's MCP server.
    #[serde(default = "default_claude_command")]
    pub claude_command: String,

    /// Arguments for the Claude Code MCP server.
    #[serde(default = "default_claude_args")]
    pub claude_args: Vec<String>,

    /// Optional environment variables for the Claude Code process.
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Tool namespace prefix for Claude Code tools in clawft.
    /// Tools are exposed as `mcp:claude-code:<tool-name>`.
    #[serde(default = "default_namespace")]
    pub namespace: String,
}

fn default_claude_command() -> String {
    "claude".into()
}

fn default_claude_args() -> Vec<String> {
    vec!["mcp".into(), "serve".into()]
}

fn default_namespace() -> String {
    "claude-code".into()
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            claude_command: default_claude_command(),
            claude_args: default_claude_args(),
            env: HashMap::new(),
            namespace: default_namespace(),
        }
    }
}

/// Bidirectional MCP bridge manager.
///
/// Manages the lifecycle of the clawft <-> Claude Code MCP connection.
pub struct McpBridge {
    config: BridgeConfig,
    status: BridgeStatus,
    /// Tools discovered from Claude Code's MCP server.
    inbound_tools: Vec<String>,
    /// Tools exposed by clawft to Claude Code.
    outbound_tools: Vec<String>,
    /// Live session against Claude Code (set after `connect_inbound`).
    inbound_session: Option<Arc<Mutex<McpSession>>>,
    /// Transport factory used for the inbound connection.
    factory: Arc<dyn McpTransportFactory>,
}

impl McpBridge {
    /// Create a new bridge with the given configuration and a default
    /// (lenient) transport factory.
    pub fn new(config: BridgeConfig) -> Self {
        Self::with_factory(
            config,
            Arc::new(DefaultTransportFactory::new(
                TransportFactoryConfig::lenient(),
            )),
        )
    }

    /// Create a new bridge with a custom transport factory.
    ///
    /// Useful for tests (inject a mock factory) or for hardening
    /// (inject `TransportFactoryConfig::strict()`).
    pub fn with_factory(config: BridgeConfig, factory: Arc<dyn McpTransportFactory>) -> Self {
        Self {
            config,
            status: BridgeStatus::Unconfigured,
            inbound_tools: Vec::new(),
            outbound_tools: Vec::new(),
            inbound_session: None,
            factory,
        }
    }

    /// Create a disabled bridge.
    pub fn disabled() -> Self {
        Self::new(BridgeConfig::default())
    }

    /// Get the current bridge status.
    pub fn status(&self) -> BridgeStatus {
        self.status
    }

    /// Get the bridge configuration.
    pub fn config(&self) -> &BridgeConfig {
        &self.config
    }

    /// Whether the bridge is enabled.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Register the set of tools that clawft exposes outbound to Claude
    /// Code, and mark the bridge as initializing.
    ///
    /// This is the synchronous part of bridge bring-up. Use
    /// [`Self::connect_inbound`] to actually spawn the Claude Code
    /// process and complete the inbound MCP handshake.
    pub fn initialize(&mut self, outbound_tools: Vec<String>) {
        if !self.config.enabled {
            debug!("bridge not enabled, skipping initialization");
            return;
        }

        self.outbound_tools = outbound_tools;
        self.status = BridgeStatus::Initializing;
        info!(
            outbound_tool_count = self.outbound_tools.len(),
            namespace = %self.config.namespace,
            "MCP bridge initializing"
        );
    }

    /// Spawn Claude Code, perform the MCP `initialize` handshake, and
    /// register the resulting tool list under the bridge namespace.
    ///
    /// This is the real implementation of the inbound direction:
    /// 1. Build a [`TransportSpec::Stdio`] from the bridge config.
    /// 2. Validate via the transport factory and spawn the process.
    /// 3. [`McpSession::connect`] performs `initialize` +
    ///    `notifications/initialized`.
    /// 4. `tools/list` is fetched and namespaced via
    ///    [`Self::namespaced_tool_name`].
    ///
    /// On success the session is stashed inside the bridge for later
    /// `tools/call` use; on failure the bridge transitions to
    /// `BridgeStatus::Error`.
    pub async fn connect_inbound(&mut self) -> Result<Vec<ToolDefinition>> {
        if !self.config.enabled {
            return Err(ServiceError::McpTransport("bridge not enabled".into()));
        }

        let spec = TransportSpec::Stdio {
            command: self.config.claude_command.clone(),
            args: self.config.claude_args.clone(),
            env: self.config.env.clone(),
        };

        let transport = match self.factory.create(spec).await {
            Ok(t) => t,
            Err(e) => {
                self.set_error(&format!("transport spawn failed: {e}"));
                return Err(e);
            }
        };

        let session = match McpSession::connect(transport).await {
            Ok(s) => s,
            Err(e) => {
                self.set_error(&format!("handshake failed: {e}"));
                return Err(e);
            }
        };

        info!(
            server_name = %session.server_info.name,
            server_version = %session.server_info.version,
            protocol_version = %session.protocol_version,
            "Claude Code MCP handshake complete"
        );

        let tools = match session.list_tools().await {
            Ok(t) => t,
            Err(e) => {
                self.set_error(&format!("tools/list failed: {e}"));
                return Err(e);
            }
        };

        // Build the namespaced tool definition list and the inbound
        // tool-name registry that drives status transitions.
        let namespaced: Vec<ToolDefinition> = tools
            .into_iter()
            .map(|td| ToolDefinition {
                name: self.namespaced_tool_name(&td.name),
                description: td.description,
                input_schema: td.input_schema,
            })
            .collect();

        self.inbound_tools = namespaced.iter().map(|t| t.name.clone()).collect();
        self.inbound_session = Some(Arc::new(Mutex::new(session)));
        self.update_status();

        info!(
            inbound_tool_count = self.inbound_tools.len(),
            namespace = %self.config.namespace,
            "Claude Code tools registered under bridge namespace"
        );

        Ok(namespaced)
    }

    /// Access the live inbound session (post-`connect_inbound`).
    pub fn inbound_session(&self) -> Option<&Arc<Mutex<McpSession>>> {
        self.inbound_session.as_ref()
    }

    /// Mark the inbound connection as active with discovered tools.
    pub fn set_inbound_connected(&mut self, tools: Vec<String>) {
        self.inbound_tools = tools;
        self.update_status();
        info!(
            inbound_tool_count = self.inbound_tools.len(),
            "inbound MCP connection active"
        );
    }

    /// Mark the outbound connection as active.
    pub fn set_outbound_connected(&mut self) {
        self.update_status();
        info!("outbound MCP connection active");
    }

    /// Set error status with a reason.
    pub fn set_error(&mut self, reason: &str) {
        self.status = BridgeStatus::Error;
        warn!(reason = %reason, "MCP bridge error");
    }

    /// Shut down the bridge.
    pub fn shutdown(&mut self) {
        self.status = BridgeStatus::ShuttingDown;
        self.inbound_tools.clear();
        // Drop the live session so the child process's stdin closes
        // and the reader task exits.
        self.inbound_session = None;
        info!("MCP bridge shutting down");
    }

    /// Tools discovered from Claude Code (inbound).
    pub fn inbound_tools(&self) -> &[String] {
        &self.inbound_tools
    }

    /// Tools exposed to Claude Code (outbound).
    pub fn outbound_tools(&self) -> &[String] {
        &self.outbound_tools
    }

    /// Get the namespaced tool name for an inbound Claude Code tool.
    ///
    /// Returns `mcp:<namespace>:<tool_name>`.
    pub fn namespaced_tool_name(&self, tool_name: &str) -> String {
        format!("mcp:{}:{}", self.config.namespace, tool_name)
    }

    /// Update status based on connection state.
    fn update_status(&mut self) {
        let has_inbound = !self.inbound_tools.is_empty();
        let has_outbound = !self.outbound_tools.is_empty();

        self.status = match (has_inbound, has_outbound) {
            (true, true) => BridgeStatus::Active,
            (true, false) => BridgeStatus::InboundOnly,
            (false, true) => BridgeStatus::OutboundOnly,
            (false, false) => BridgeStatus::Initializing,
        };
    }
}

impl std::fmt::Debug for McpBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpBridge")
            .field("status", &self.status)
            .field("enabled", &self.config.enabled)
            .field("inbound_tools", &self.inbound_tools.len())
            .field("outbound_tools", &self.outbound_tools.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_disabled_by_default() {
        let bridge = McpBridge::disabled();
        assert!(!bridge.is_enabled());
        assert_eq!(bridge.status(), BridgeStatus::Unconfigured);
    }

    #[test]
    fn bridge_initialize() {
        let mut bridge = McpBridge::new(BridgeConfig {
            enabled: true,
            ..Default::default()
        });

        bridge.initialize(vec!["read_file".into(), "write_file".into()]);
        assert_eq!(bridge.status(), BridgeStatus::Initializing);
        assert_eq!(bridge.outbound_tools().len(), 2);
    }

    #[test]
    fn bridge_skip_init_when_disabled() {
        let mut bridge = McpBridge::disabled();
        bridge.initialize(vec!["tool1".into()]);
        // Status remains unconfigured because bridge is disabled.
        assert_eq!(bridge.status(), BridgeStatus::Unconfigured);
        assert!(bridge.outbound_tools().is_empty());
    }

    #[test]
    fn bridge_active_when_both_connected() {
        let mut bridge = McpBridge::new(BridgeConfig {
            enabled: true,
            ..Default::default()
        });
        bridge.initialize(vec!["out_tool".into()]);
        bridge.set_inbound_connected(vec!["in_tool".into()]);
        assert_eq!(bridge.status(), BridgeStatus::Active);
    }

    #[test]
    fn bridge_inbound_only() {
        let mut bridge = McpBridge::new(BridgeConfig {
            enabled: true,
            ..Default::default()
        });
        // No outbound tools, but inbound connected.
        bridge.set_inbound_connected(vec!["tool1".into()]);
        assert_eq!(bridge.status(), BridgeStatus::InboundOnly);
    }

    #[test]
    fn bridge_outbound_only() {
        let mut bridge = McpBridge::new(BridgeConfig {
            enabled: true,
            ..Default::default()
        });
        bridge.initialize(vec!["tool1".into()]);
        bridge.set_outbound_connected();
        assert_eq!(bridge.status(), BridgeStatus::OutboundOnly);
    }

    #[test]
    fn bridge_error() {
        let mut bridge = McpBridge::new(BridgeConfig {
            enabled: true,
            ..Default::default()
        });
        bridge.set_error("connection refused");
        assert_eq!(bridge.status(), BridgeStatus::Error);
    }

    #[test]
    fn bridge_shutdown() {
        let mut bridge = McpBridge::new(BridgeConfig {
            enabled: true,
            ..Default::default()
        });
        bridge.initialize(vec!["tool".into()]);
        bridge.set_inbound_connected(vec!["in".into()]);
        bridge.shutdown();
        assert_eq!(bridge.status(), BridgeStatus::ShuttingDown);
        assert!(bridge.inbound_tools().is_empty());
    }

    #[test]
    fn namespaced_tool_name() {
        let bridge = McpBridge::new(BridgeConfig {
            namespace: "claude-code".into(),
            ..Default::default()
        });
        assert_eq!(
            bridge.namespaced_tool_name("read_file"),
            "mcp:claude-code:read_file"
        );
    }

    #[test]
    fn bridge_config_defaults() {
        let cfg = BridgeConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.claude_command, "claude");
        assert_eq!(cfg.claude_args, vec!["mcp", "serve"]);
        assert_eq!(cfg.namespace, "claude-code");
    }

    #[test]
    fn bridge_config_serde() {
        let cfg = BridgeConfig {
            enabled: true,
            claude_command: "claude-dev".into(),
            claude_args: vec!["mcp".into(), "start".into()],
            env: {
                let mut m = HashMap::new();
                m.insert("CLAUDE_KEY".into(), "test".into());
                m
            },
            namespace: "dev".into(),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let restored: BridgeConfig = serde_json::from_str(&json).unwrap();
        assert!(restored.enabled);
        assert_eq!(restored.claude_command, "claude-dev");
        assert_eq!(restored.namespace, "dev");
    }

    #[test]
    fn bridge_status_serde() {
        let json = serde_json::to_string(&BridgeStatus::Active).unwrap();
        assert_eq!(json, "\"active\"");

        let restored: BridgeStatus = serde_json::from_str("\"shutting_down\"").unwrap();
        assert_eq!(restored, BridgeStatus::ShuttingDown);
    }

    // ── connect_inbound tests (WEFT-182) ────────────────────────────────

    use super::super::transport::{McpTransport, McpTransportFactory, TransportSpec};
    use super::super::types::JsonRpcResponse;
    use crate::error::{Result as SvcResult, ServiceError};
    use async_trait::async_trait;

    /// A mock factory that hands out a pre-programmed in-memory
    /// transport. Used to exercise `connect_inbound` without spawning
    /// a real child process.
    struct MockFactory {
        responses: std::sync::Mutex<Option<Vec<JsonRpcResponse>>>,
    }

    impl MockFactory {
        fn new(responses: Vec<JsonRpcResponse>) -> Self {
            Self {
                responses: std::sync::Mutex::new(Some(responses)),
            }
        }
    }

    #[async_trait]
    impl McpTransportFactory for MockFactory {
        fn validate(&self, _spec: &TransportSpec) -> SvcResult<()> {
            Ok(())
        }

        async fn create(&self, _spec: TransportSpec) -> SvcResult<Box<dyn McpTransport>> {
            let responses = self
                .responses
                .lock()
                .unwrap()
                .take()
                .ok_or_else(|| ServiceError::McpTransport("factory used twice".into()))?;
            Ok(Box::new(super::super::transport::MockTransport::new(
                responses,
            )))
        }
    }

    fn ok_response(id: u64, result: serde_json::Value) -> JsonRpcResponse {
        JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    fn err_response(id: u64, code: i32, message: &str) -> JsonRpcResponse {
        JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(super::super::types::JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }

    #[tokio::test]
    async fn connect_inbound_disabled_errors() {
        let mut bridge = McpBridge::disabled();
        let result = bridge.connect_inbound().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn connect_inbound_handshake_and_tools_list() {
        // Mocked initialize response.
        let init = ok_response(
            1,
            serde_json::json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {"tools": {"listChanged": true}},
                "serverInfo": {"name": "claude-code", "version": "0.1.2"}
            }),
        );
        // Mocked tools/list response.
        let tools = ok_response(
            2,
            serde_json::json!({
                "tools": [
                    {
                        "name": "read_file",
                        "description": "Read a file",
                        "inputSchema": {"type": "object"}
                    },
                    {
                        "name": "write_file",
                        "description": "Write a file",
                        "inputSchema": {"type": "object"}
                    }
                ]
            }),
        );
        let factory = Arc::new(MockFactory::new(vec![init, tools]));

        let mut bridge = McpBridge::with_factory(
            BridgeConfig {
                enabled: true,
                namespace: "claude-code".into(),
                ..Default::default()
            },
            factory,
        );

        let registered = bridge.connect_inbound().await.unwrap();
        assert_eq!(registered.len(), 2);

        // Tools must be registered under the namespaced prefix.
        assert_eq!(registered[0].name, "mcp:claude-code:read_file");
        assert_eq!(registered[1].name, "mcp:claude-code:write_file");
        assert_eq!(bridge.inbound_tools().len(), 2);
        assert!(bridge.inbound_session().is_some());
    }

    #[tokio::test]
    async fn connect_inbound_handshake_failure_marks_error() {
        // Initialize fails.
        let factory = Arc::new(MockFactory::new(vec![err_response(1, -32600, "bad init")]));

        let mut bridge = McpBridge::with_factory(
            BridgeConfig {
                enabled: true,
                ..Default::default()
            },
            factory,
        );

        let result = bridge.connect_inbound().await;
        assert!(result.is_err());
        assert_eq!(bridge.status(), BridgeStatus::Error);
        assert!(bridge.inbound_session().is_none());
    }

    #[tokio::test]
    async fn connect_inbound_tools_list_failure_marks_error() {
        let init = ok_response(
            1,
            serde_json::json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "serverInfo": {"name": "claude-code", "version": "0.1.0"}
            }),
        );
        let tools_err = err_response(2, -32601, "tools/list not supported");
        let factory = Arc::new(MockFactory::new(vec![init, tools_err]));

        let mut bridge = McpBridge::with_factory(
            BridgeConfig {
                enabled: true,
                ..Default::default()
            },
            factory,
        );

        let result = bridge.connect_inbound().await;
        assert!(result.is_err());
        assert_eq!(bridge.status(), BridgeStatus::Error);
    }

    #[tokio::test]
    async fn shutdown_drops_session() {
        let init = ok_response(
            1,
            serde_json::json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "serverInfo": {"name": "x", "version": "y"}
            }),
        );
        let tools = ok_response(2, serde_json::json!({"tools": []}));
        let factory = Arc::new(MockFactory::new(vec![init, tools]));
        let mut bridge = McpBridge::with_factory(
            BridgeConfig {
                enabled: true,
                ..Default::default()
            },
            factory,
        );
        let _ = bridge.connect_inbound().await.unwrap();
        assert!(bridge.inbound_session().is_some());
        bridge.shutdown();
        assert!(bridge.inbound_session().is_none());
    }
}
