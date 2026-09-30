//! Attach façade for `weft mcp-server --attach` (ADR-076 C3 / WEFT-701).
//!
//! Control-plane MCP tools (`status`, `agent_list`, `agent_spawn`,
//! `agent_stop`) dispatch to a live kernel/weave instance via
//! [`clawft_rpc::DaemonClient`] instead of a standalone empty substrate.
//!
//! Standalone mode (no `--attach`) remains the default for offline/dev.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use clawft_rpc::{DaemonClient, Request};
use clawft_services::mcp::ToolDefinition;
use clawft_services::mcp::provider::{CallToolResult, ToolError, ToolProvider};
use serde_json::{Value, json};
use tokio::sync::Mutex;
use tracing::{info, warn};

// ── Public MCP tool names (catalog / ADR-076) ─────────────────────────

/// Public MCP tool names served by the attach façade.
pub const ATTACH_TOOL_NAMES: &[&str] = &["status", "agent_list", "agent_spawn", "agent_stop"];

/// Actionable error when the kernel daemon is not reachable.
pub fn daemon_unavailable_error() -> String {
    format!(
        "no kernel daemon running (attach mode requires a live instance).\n\
         Start the daemon with: weaver kernel start\n\
         Socket path: {}\n\
         Or use standalone mode: weft mcp-server  (without --attach)",
        clawft_rpc::socket_path().display()
    )
}

// ── AttachFacade trait ────────────────────────────────────────────────

/// Live kernel/weave surface used by attach-mode control tools.
///
/// Prefer this small trait over inlining `DaemonClient` in the MCP
/// provider so unit tests can inject a mock without a real socket.
#[async_trait]
pub trait AttachFacade: Send + Sync {
    /// `kernel.status` → public MCP `status`.
    async fn status(&self) -> Result<Value, String>;

    /// `agent.list` → public MCP `agent_list`.
    async fn agent_list(&self) -> Result<Value, String>;

    /// `agent.spawn` → public MCP `agent_spawn`.
    async fn agent_spawn(
        &self,
        agent_id: String,
        parent_pid: Option<u64>,
    ) -> Result<Value, String>;

    /// `agent.stop` → public MCP `agent_stop`.
    async fn agent_stop(&self, pid: u64, force: bool) -> Result<Value, String>;
}

// ── DaemonAttachFacade ────────────────────────────────────────────────

/// Production façade: each RPC goes over the kernel Unix-socket client.
///
/// Connections are re-established when the previous client drops so a
/// long-lived `weft mcp-server --attach` survives daemon restarts
/// between tool calls (still fails clear if the daemon is down).
pub struct DaemonAttachFacade {
    /// Explicit socket override (tests / multi-instance). `None` = default.
    socket_path: Option<PathBuf>,
    client: Mutex<Option<DaemonClient>>,
}

impl DaemonAttachFacade {
    /// Connect once at startup; fail immediately if the daemon is down.
    pub async fn connect() -> Result<Self, String> {
        Self::connect_path(None).await
    }

    /// Connect to an explicit socket path (or default when `None`).
    pub async fn connect_path(socket_path: Option<PathBuf>) -> Result<Self, String> {
        let client = match &socket_path {
            Some(p) => DaemonClient::connect_path(p).await,
            None => crate::commands::daemon_conn::connect_opt().await,
        }
        .ok_or_else(daemon_unavailable_error)?;

        info!(
            socket = %socket_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| clawft_rpc::socket_path().display().to_string()),
            "attach façade connected to kernel daemon"
        );

        Ok(Self {
            socket_path,
            client: Mutex::new(Some(client)),
        })
    }

    async fn ensure_client(&self) -> Result<(), String> {
        let mut guard = self.client.lock().await;
        if guard.is_some() {
            return Ok(());
        }
        let client = match &self.socket_path {
            Some(p) => DaemonClient::connect_path(p).await,
            None => crate::commands::daemon_conn::connect_opt().await,
        }
        .ok_or_else(daemon_unavailable_error)?;
        *guard = Some(client);
        Ok(())
    }

    async fn rpc(&self, request: Request) -> Result<Value, String> {
        self.ensure_client().await?;
        let mut guard = self.client.lock().await;
        let client = guard
            .as_mut()
            .ok_or_else(daemon_unavailable_error)?;
        match client.call(request).await {
            Ok(resp) => {
                if !resp.ok {
                    let msg = resp
                        .error
                        .unwrap_or_else(|| "daemon RPC failed".into());
                    Err(msg)
                } else {
                    Ok(resp.result.unwrap_or(Value::Null))
                }
            }
            Err(e) => {
                // Drop dead client so the next call re-connects.
                *guard = None;
                warn!(error = %e, "attach façade RPC transport error");
                Err(format!(
                    "daemon RPC failed: {e}\n{}",
                    daemon_unavailable_error()
                ))
            }
        }
    }
}

#[async_trait]
impl AttachFacade for DaemonAttachFacade {
    async fn status(&self) -> Result<Value, String> {
        self.rpc(Request::new("kernel.status")).await
    }

    async fn agent_list(&self) -> Result<Value, String> {
        self.rpc(Request::new("agent.list")).await
    }

    async fn agent_spawn(
        &self,
        agent_id: String,
        parent_pid: Option<u64>,
    ) -> Result<Value, String> {
        let params = json!({
            "agent_id": agent_id,
            "parent_pid": parent_pid,
        });
        self.rpc(Request::with_params("agent.spawn", params)).await
    }

    async fn agent_stop(&self, pid: u64, force: bool) -> Result<Value, String> {
        let params = json!({
            "pid": pid,
            "graceful": !force,
        });
        self.rpc(Request::with_params("agent.stop", params)).await
    }
}

// ── Attach tool definitions ───────────────────────────────────────────

/// Tool definitions for the attach control surface (catalog public names).
pub fn attach_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "status".into(),
            description: "Kernel / instance status from the live WeftOS daemon \
                (weave `kernel.status`). Requires `weft mcp-server --attach`."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        },
        ToolDefinition {
            name: "agent_list".into(),
            description: "List agents running on the live kernel (weave `agent.list`). \
                Requires `weft mcp-server --attach`."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        },
        ToolDefinition {
            name: "agent_spawn".into(),
            description: "Spawn a supervised agent on the live kernel (weave `agent.spawn`). \
                Requires `weft mcp-server --attach`."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "agent_id": {
                        "type": "string",
                        "description": "Unique agent identifier"
                    },
                    "parent_pid": {
                        "type": "integer",
                        "description": "Optional parent PID for spawn lineage"
                    }
                },
                "required": ["agent_id"],
                "additionalProperties": false
            }),
        },
        ToolDefinition {
            name: "agent_stop".into(),
            description: "Stop a running agent on the live kernel (weave `agent.stop`). \
                Requires `weft mcp-server --attach`."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pid": {
                        "type": "integer",
                        "description": "PID of the agent process to stop"
                    },
                    "force": {
                        "type": "boolean",
                        "description": "Force immediate stop (skip graceful shutdown)",
                        "default": false
                    }
                },
                "required": ["pid"],
                "additionalProperties": false
            }),
        },
    ]
}

/// Filter attach tool defs by serve profile (control tools only).
pub fn filter_attach_tools_by_profile(
    tools: Vec<ToolDefinition>,
    allows: impl Fn(&str) -> bool,
) -> Vec<ToolDefinition> {
    tools.into_iter().filter(|t| allows(&t.name)).collect()
}

// ── AttachToolProvider ────────────────────────────────────────────────

/// MCP [`ToolProvider`] that dispatches control tools to an [`AttachFacade`].
///
/// Namespace is empty so tools list as public names (`status`, `agent_list`,
/// …) when composed via [`CompositeToolProvider`] (empty-ns = no prefix).
pub struct AttachToolProvider {
    tools: Vec<ToolDefinition>,
    facade: Arc<dyn AttachFacade>,
}

impl AttachToolProvider {
    /// Build a provider with the given façade and (already profile-filtered) tools.
    pub fn new(facade: Arc<dyn AttachFacade>, tools: Vec<ToolDefinition>) -> Self {
        Self { tools, facade }
    }

    /// Convenience: full attach surface filtered by a profile allow predicate.
    pub fn with_profile(
        facade: Arc<dyn AttachFacade>,
        allows: impl Fn(&str) -> bool,
    ) -> Self {
        let tools = filter_attach_tools_by_profile(attach_tool_definitions(), allows);
        Self::new(facade, tools)
    }
}

impl std::fmt::Debug for AttachToolProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AttachToolProvider")
            .field("tool_count", &self.tools.len())
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl ToolProvider for AttachToolProvider {
    fn namespace(&self) -> &str {
        // Empty → CompositeToolProvider lists public names (ADR-076 §3).
        ""
    }

    fn list_tools(&self) -> Vec<ToolDefinition> {
        self.tools.clone()
    }

    async fn call_tool(&self, name: &str, args: Value) -> Result<CallToolResult, ToolError> {
        if !self.tools.iter().any(|t| t.name == name) {
            return Err(ToolError::NotFound(name.to_string()));
        }

        let result = match name {
            "status" => self.facade.status().await,
            "agent_list" => self.facade.agent_list().await,
            "agent_spawn" => {
                let agent_id = args
                    .get("agent_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        ToolError::ExecutionFailed(
                            "missing required argument 'agent_id'".into(),
                        )
                    })?
                    .to_string();
                let parent_pid = args.get("parent_pid").and_then(|v| v.as_u64());
                self.facade.agent_spawn(agent_id, parent_pid).await
            }
            "agent_stop" => {
                let pid = args.get("pid").and_then(|v| v.as_u64()).ok_or_else(|| {
                    ToolError::ExecutionFailed("missing required argument 'pid'".into())
                })?;
                let force = args
                    .get("force")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                self.facade.agent_stop(pid, force).await
            }
            other => {
                return Err(ToolError::NotFound(other.to_string()));
            }
        };

        match result {
            Ok(value) => {
                let text = serde_json::to_string_pretty(&value)
                    .unwrap_or_else(|_| value.to_string());
                Ok(CallToolResult::text(text))
            }
            Err(msg) => Ok(CallToolResult::error(msg)),
        }
    }
}

// ── Standalone stub (optional explicit empty surface) ─────────────────

/// Standalone façade that always fails with an attach-required message.
///
/// Not used by default (standalone mode simply omits attach tools), but
/// available if a caller wants explicit fail-closed control tools.
#[allow(dead_code)] // exercised in unit tests; reserved for explicit fail-closed wiring
pub struct StandaloneAttachFacade;

#[async_trait]
impl AttachFacade for StandaloneAttachFacade {
    async fn status(&self) -> Result<Value, String> {
        Err(standalone_control_error("status"))
    }
    async fn agent_list(&self) -> Result<Value, String> {
        Err(standalone_control_error("agent_list"))
    }
    async fn agent_spawn(
        &self,
        _agent_id: String,
        _parent_pid: Option<u64>,
    ) -> Result<Value, String> {
        Err(standalone_control_error("agent_spawn"))
    }
    async fn agent_stop(&self, _pid: u64, _force: bool) -> Result<Value, String> {
        Err(standalone_control_error("agent_stop"))
    }
}

#[allow(dead_code)] // used by StandaloneAttachFacade (test + reserved API)
pub fn standalone_control_error(tool: &str) -> String {
    format!(
        "tool '{tool}' requires a live kernel (standalone MCP server has no instance).\n\
         Re-run with: weft mcp-server --attach\n\
         And ensure the daemon is up: weaver kernel start"
    )
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use clawft_services::mcp::provider::ContentBlock;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// In-memory mock façade for unit tests (no daemon).
    struct MockAttachFacade {
        status_calls: AtomicUsize,
        list_calls: AtomicUsize,
        spawn_calls: AtomicUsize,
        stop_calls: AtomicUsize,
        fail: bool,
    }

    impl MockAttachFacade {
        fn new() -> Self {
            Self {
                status_calls: AtomicUsize::new(0),
                list_calls: AtomicUsize::new(0),
                spawn_calls: AtomicUsize::new(0),
                stop_calls: AtomicUsize::new(0),
                fail: false,
            }
        }

        fn failing() -> Self {
            Self {
                fail: true,
                ..Self::new()
            }
        }
    }

    #[async_trait]
    impl AttachFacade for MockAttachFacade {
        async fn status(&self) -> Result<Value, String> {
            self.status_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                return Err(daemon_unavailable_error());
            }
            Ok(json!({
                "ok": true,
                "mode": "mock",
                "uptime_secs": 42
            }))
        }

        async fn agent_list(&self) -> Result<Value, String> {
            self.list_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                return Err(daemon_unavailable_error());
            }
            Ok(json!([
                {"pid": 1, "agent_id": "alpha", "state": "running"},
                {"pid": 2, "agent_id": "beta", "state": "idle"}
            ]))
        }

        async fn agent_spawn(
            &self,
            agent_id: String,
            parent_pid: Option<u64>,
        ) -> Result<Value, String> {
            self.spawn_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                return Err(daemon_unavailable_error());
            }
            Ok(json!({
                "pid": 99,
                "agent_id": agent_id,
                "parent_pid": parent_pid
            }))
        }

        async fn agent_stop(&self, pid: u64, force: bool) -> Result<Value, String> {
            self.stop_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                return Err(daemon_unavailable_error());
            }
            Ok(json!({ "pid": pid, "stopped": true, "force": force }))
        }
    }

    #[test]
    fn attach_tool_definitions_public_names() {
        let defs = attach_tool_definitions();
        let names: Vec<&str> = defs.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ATTACH_TOOL_NAMES);
        for t in &defs {
            assert!(!t.description.is_empty());
            assert_eq!(t.input_schema["type"], "object");
        }
    }

    #[test]
    fn filter_attach_tools_honors_allow_predicate() {
        let filtered = filter_attach_tools_by_profile(attach_tool_definitions(), |n| {
            n == "status" || n == "agent_list"
        });
        let names: Vec<&str> = filtered.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["status", "agent_list"]);
    }

    #[test]
    fn daemon_unavailable_error_is_actionable() {
        let msg = daemon_unavailable_error();
        assert!(msg.contains("no kernel daemon"));
        assert!(msg.contains("weaver kernel start"));
        assert!(msg.contains("without --attach"));
    }

    #[tokio::test]
    async fn provider_dispatches_status_and_agent_list() {
        let facade = Arc::new(MockAttachFacade::new());
        let provider = AttachToolProvider::with_profile(facade.clone(), |_| true);

        assert_eq!(provider.namespace(), "");
        let listed: Vec<_> = provider.list_tools().iter().map(|t| t.name.clone()).collect();
        assert!(listed.contains(&"status".into()));
        assert!(listed.contains(&"agent_list".into()));

        let status = provider.call_tool("status", json!({})).await.unwrap();
        assert!(!status.is_error);
        match &status.content[0] {
            ContentBlock::Text { text } => {
                let v: Value = serde_json::from_str(text).unwrap();
                assert_eq!(v["mode"], "mock");
                assert_eq!(v["uptime_secs"], 42);
            }
        }
        assert_eq!(facade.status_calls.load(Ordering::SeqCst), 1);

        let list = provider.call_tool("agent_list", json!({})).await.unwrap();
        assert!(!list.is_error);
        match &list.content[0] {
            ContentBlock::Text { text } => {
                let v: Value = serde_json::from_str(text).unwrap();
                assert!(v.is_array());
                assert_eq!(v.as_array().unwrap().len(), 2);
            }
        }
        assert_eq!(facade.list_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn provider_dispatches_spawn_and_stop() {
        let facade = Arc::new(MockAttachFacade::new());
        let provider = AttachToolProvider::new(facade.clone(), attach_tool_definitions());

        let spawn = provider
            .call_tool(
                "agent_spawn",
                json!({"agent_id": "coder", "parent_pid": 7}),
            )
            .await
            .unwrap();
        assert!(!spawn.is_error);
        match &spawn.content[0] {
            ContentBlock::Text { text } => {
                let v: Value = serde_json::from_str(text).unwrap();
                assert_eq!(v["agent_id"], "coder");
                assert_eq!(v["pid"], 99);
                assert_eq!(v["parent_pid"], 7);
            }
        }

        let stop = provider
            .call_tool("agent_stop", json!({"pid": 99, "force": true}))
            .await
            .unwrap();
        assert!(!stop.is_error);
        match &stop.content[0] {
            ContentBlock::Text { text } => {
                let v: Value = serde_json::from_str(text).unwrap();
                assert_eq!(v["pid"], 99);
                assert_eq!(v["force"], true);
            }
        }
        assert_eq!(facade.spawn_calls.load(Ordering::SeqCst), 1);
        assert_eq!(facade.stop_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn provider_spawn_requires_agent_id() {
        let facade = Arc::new(MockAttachFacade::new());
        let provider = AttachToolProvider::new(facade, attach_tool_definitions());
        let err = provider
            .call_tool("agent_spawn", json!({}))
            .await
            .unwrap_err();
        match err {
            ToolError::ExecutionFailed(msg) => assert!(msg.contains("agent_id")),
            other => panic!("expected ExecutionFailed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn provider_fails_clear_when_facade_unavailable() {
        let facade = Arc::new(MockAttachFacade::failing());
        let provider = AttachToolProvider::new(facade, attach_tool_definitions());

        let result = provider.call_tool("status", json!({})).await.unwrap();
        assert!(result.is_error, "must surface error, not empty success");
        match &result.content[0] {
            ContentBlock::Text { text } => {
                assert!(text.contains("no kernel daemon"));
                assert!(text.contains("weaver kernel start"));
            }
        }
    }

    #[tokio::test]
    async fn provider_not_found_for_unknown_tool() {
        let facade = Arc::new(MockAttachFacade::new());
        let provider = AttachToolProvider::new(facade, attach_tool_definitions());
        let err = provider
            .call_tool("read_file", json!({}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::NotFound(_)));
    }

    #[tokio::test]
    async fn standalone_facade_rejects_with_attach_hint() {
        let f = StandaloneAttachFacade;
        let err = f.status().await.unwrap_err();
        assert!(err.contains("--attach"));
        assert!(err.contains("weaver kernel start"));
    }

    #[tokio::test]
    async fn connect_path_isolated_socket_fails_clear() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("kernel.sock");
        let result = DaemonAttachFacade::connect_path(Some(sock)).await;
        let err = match result {
            Ok(_) => panic!("expected connect failure on empty socket path"),
            Err(e) => e,
        };
        assert!(err.contains("no kernel daemon"));
        assert!(err.contains("without --attach"));
    }
}
