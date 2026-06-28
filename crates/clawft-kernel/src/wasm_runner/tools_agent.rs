//! Built-in agent and IPC tool implementations.

use std::sync::Arc;

use super::catalog::builtin_tool_catalog;
use super::registry::BuiltinTool;
use super::types::*;

/// Built-in `agent.spawn` tool.
///
/// Spawns a new agent process via the kernel's AgentSupervisor.
/// Always runs natively (needs direct kernel struct access).
pub struct AgentSpawnTool {
    spec: BuiltinToolSpec,
    process_table: Arc<crate::process::ProcessTable>,
}

impl AgentSpawnTool {
    pub fn new(process_table: Arc<crate::process::ProcessTable>) -> Self {
        let catalog = builtin_tool_catalog();
        let spec = catalog
            .into_iter()
            .find(|s| s.name == "agent.spawn")
            .expect("agent.spawn must be in catalog");
        Self {
            spec,
            process_table,
        }
    }
}

impl BuiltinTool for AgentSpawnTool {
    fn name(&self) -> &str {
        "agent.spawn"
    }

    fn spec(&self) -> &BuiltinToolSpec {
        &self.spec
    }

    fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let agent_id = args
            .get("agent_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("missing 'agent_id' parameter".into()))?;

        let backend = args
            .get("backend")
            .and_then(|v| v.as_str())
            .unwrap_or("native");

        if backend == "wasm" {
            return Err(ToolError::ExecutionFailed(
                "WASM backend not yet available for agent.spawn".into(),
            ));
        }

        // Create a process entry directly in the process table.
        // In production this would go through AgentSupervisor::spawn(),
        // but for the reference tool impl we create the entry directly.
        let entry = crate::process::ProcessEntry {
            pid: 0, // assigned by insert()
            agent_id: agent_id.to_string(),
            state: crate::process::ProcessState::Running,
            capabilities: crate::capability::AgentCapabilities::default(),
            resource_usage: crate::process::ResourceUsage::default(),
            cancel_token: crate::process::CancellationToken::new(),
            parent_pid: None,
        };

        let pid = self
            .process_table
            .insert(entry)
            .map_err(|e| ToolError::ExecutionFailed(format!("spawn failed: {e}")))?;

        Ok(serde_json::json!({
            "pid": pid,
            "agent_id": agent_id,
            "state": "running",
        }))
    }
}

/// Built-in `agent.stop` tool.
pub struct AgentStopTool {
    spec: BuiltinToolSpec,
    process_table: Arc<crate::process::ProcessTable>,
}

impl AgentStopTool {
    pub fn new(process_table: Arc<crate::process::ProcessTable>) -> Self {
        let spec = builtin_tool_catalog()
            .into_iter()
            .find(|s| s.name == "agent.stop")
            .unwrap();
        Self {
            spec,
            process_table,
        }
    }
}

impl BuiltinTool for AgentStopTool {
    fn name(&self) -> &str {
        "agent.stop"
    }
    fn spec(&self) -> &BuiltinToolSpec {
        &self.spec
    }
    fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let pid = args
            .get("pid")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| ToolError::InvalidArgs("missing 'pid'".into()))?;
        let entry = self
            .process_table
            .get(pid)
            .ok_or_else(|| ToolError::NotFound(format!("pid {pid}")))?;
        entry.cancel_token.cancel();
        self.process_table
            .update_state(pid, crate::process::ProcessState::Stopping)
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        Ok(serde_json::json!({"stopped": pid, "agent_id": entry.agent_id}))
    }
}

/// Built-in `agent.list` tool.
pub struct AgentListTool {
    spec: BuiltinToolSpec,
    process_table: Arc<crate::process::ProcessTable>,
}

impl AgentListTool {
    pub fn new(process_table: Arc<crate::process::ProcessTable>) -> Self {
        let spec = builtin_tool_catalog()
            .into_iter()
            .find(|s| s.name == "agent.list")
            .unwrap();
        Self {
            spec,
            process_table,
        }
    }
}

impl BuiltinTool for AgentListTool {
    fn name(&self) -> &str {
        "agent.list"
    }
    fn spec(&self) -> &BuiltinToolSpec {
        &self.spec
    }
    fn execute(&self, _args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let list = self.process_table.list();
        let entries: Vec<serde_json::Value> = list
            .iter()
            .map(|e| {
                serde_json::json!({
                    "pid": e.pid,
                    "agent_id": e.agent_id,
                    "state": format!("{:?}", e.state),
                })
            })
            .collect();
        Ok(serde_json::json!({"agents": entries, "count": entries.len()}))
    }
}

/// Built-in `agent.inspect` tool.
pub struct AgentInspectTool {
    spec: BuiltinToolSpec,
    process_table: Arc<crate::process::ProcessTable>,
}

impl AgentInspectTool {
    pub fn new(process_table: Arc<crate::process::ProcessTable>) -> Self {
        let spec = builtin_tool_catalog()
            .into_iter()
            .find(|s| s.name == "agent.inspect")
            .unwrap();
        Self {
            spec,
            process_table,
        }
    }
}

impl BuiltinTool for AgentInspectTool {
    fn name(&self) -> &str {
        "agent.inspect"
    }
    fn spec(&self) -> &BuiltinToolSpec {
        &self.spec
    }
    fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let pid = args
            .get("pid")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| ToolError::InvalidArgs("missing 'pid'".into()))?;
        let entry = self
            .process_table
            .get(pid)
            .ok_or_else(|| ToolError::NotFound(format!("pid {pid}")))?;
        Ok(serde_json::json!({
            "pid": entry.pid,
            "agent_id": entry.agent_id,
            "state": format!("{:?}", entry.state),
            "parent_pid": entry.parent_pid,
            "resource_usage": {
                "messages_sent": entry.resource_usage.messages_sent,
                "tool_calls": entry.resource_usage.tool_calls,
                "cpu_time_ms": entry.resource_usage.cpu_time_ms,
            },
            "capabilities": {
                "can_spawn": entry.capabilities.can_spawn,
                "can_ipc": entry.capabilities.can_ipc,
                "can_exec_tools": entry.capabilities.can_exec_tools,
                "can_network": entry.capabilities.can_network,
            },
        }))
    }
}

/// Built-in `agent.send` tool.
#[cfg(feature = "native")]
pub struct AgentSendTool {
    spec: BuiltinToolSpec,
    process_table: Arc<crate::process::ProcessTable>,
    a2a: Arc<crate::a2a::A2ARouter>,
}

#[cfg(feature = "native")]
impl AgentSendTool {
    pub fn new(
        process_table: Arc<crate::process::ProcessTable>,
        a2a: Arc<crate::a2a::A2ARouter>,
    ) -> Self {
        let spec = builtin_tool_catalog()
            .into_iter()
            .find(|s| s.name == "agent.send")
            .unwrap();
        Self {
            spec,
            process_table,
            a2a,
        }
    }
}

#[cfg(feature = "native")]
impl BuiltinTool for AgentSendTool {
    fn name(&self) -> &str {
        "agent.send"
    }
    fn spec(&self) -> &BuiltinToolSpec {
        &self.spec
    }
    fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let pid = args
            .get("pid")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| ToolError::InvalidArgs("missing 'pid'".into()))?;
        let message = args
            .get("message")
            .cloned()
            .ok_or_else(|| ToolError::InvalidArgs("missing 'message'".into()))?;
        // Verify target exists
        let _ = self
            .process_table
            .get(pid)
            .ok_or_else(|| ToolError::NotFound(format!("pid {pid}")))?;
        let msg = crate::ipc::KernelMessage::new(
            0, // from kernel
            crate::ipc::MessageTarget::Process(pid),
            crate::ipc::MessagePayload::Json(message),
        );
        let msg_id = msg.id.clone();
        // Use blocking send since BuiltinTool::execute is sync
        // In production, agent.send would go through the async agent loop
        let a2a = self.a2a.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async { a2a.send(msg).await })
        })
        .join()
        .map_err(|_| ToolError::ExecutionFailed("send thread panicked".into()))?
        .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        Ok(serde_json::json!({"sent": true, "pid": pid, "msg_id": msg_id}))
    }
}

/// Built-in `agent.suspend` tool.
pub struct AgentSuspendTool {
    spec: BuiltinToolSpec,
    process_table: Arc<crate::process::ProcessTable>,
}

impl AgentSuspendTool {
    pub fn new(process_table: Arc<crate::process::ProcessTable>) -> Self {
        let spec = builtin_tool_catalog()
            .into_iter()
            .find(|s| s.name == "agent.suspend")
            .unwrap();
        Self {
            spec,
            process_table,
        }
    }
}

impl BuiltinTool for AgentSuspendTool {
    fn name(&self) -> &str {
        "agent.suspend"
    }
    fn spec(&self) -> &BuiltinToolSpec {
        &self.spec
    }
    fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let pid = args
            .get("pid")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| ToolError::InvalidArgs("missing 'pid'".into()))?;
        self.process_table
            .update_state(pid, crate::process::ProcessState::Suspended)
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        Ok(serde_json::json!({"suspended": pid}))
    }
}

/// Built-in `agent.resume` tool.
pub struct AgentResumeTool {
    spec: BuiltinToolSpec,
    process_table: Arc<crate::process::ProcessTable>,
}

impl AgentResumeTool {
    pub fn new(process_table: Arc<crate::process::ProcessTable>) -> Self {
        let spec = builtin_tool_catalog()
            .into_iter()
            .find(|s| s.name == "agent.resume")
            .unwrap();
        Self {
            spec,
            process_table,
        }
    }
}

impl BuiltinTool for AgentResumeTool {
    fn name(&self) -> &str {
        "agent.resume"
    }
    fn spec(&self) -> &BuiltinToolSpec {
        &self.spec
    }
    fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let pid = args
            .get("pid")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| ToolError::InvalidArgs("missing 'pid'".into()))?;
        self.process_table
            .update_state(pid, crate::process::ProcessState::Running)
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        Ok(serde_json::json!({"resumed": pid}))
    }
}

// ---------------------------------------------------------------------------
// IPC tool implementations
// ---------------------------------------------------------------------------

/// Built-in `ipc.send` tool.
///
/// Sends a message to a target PID or topic via kernel IPC.
pub struct IpcSendTool {
    spec: BuiltinToolSpec,
}

impl Default for IpcSendTool {
    fn default() -> Self {
        Self::new()
    }
}

impl IpcSendTool {
    pub fn new() -> Self {
        let catalog = builtin_tool_catalog();
        let spec = catalog
            .into_iter()
            .find(|s| s.name == "ipc.send")
            .expect("ipc.send must be in catalog");
        Self { spec }
    }
}

impl BuiltinTool for IpcSendTool {
    fn name(&self) -> &str {
        "ipc.send"
    }
    fn spec(&self) -> &BuiltinToolSpec {
        &self.spec
    }
    fn execute(&self, _args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        // Stub: real implementation will route through KernelIpc
        Err(ToolError::ExecutionFailed(
            "ipc.send requires async kernel context".into(),
        ))
    }
}

/// Built-in `ipc.subscribe` tool.
///
/// Subscribes the calling agent to a topic for receiving messages.
pub struct IpcSubscribeTool {
    spec: BuiltinToolSpec,
}

impl Default for IpcSubscribeTool {
    fn default() -> Self {
        Self::new()
    }
}

impl IpcSubscribeTool {
    pub fn new() -> Self {
        let catalog = builtin_tool_catalog();
        let spec = catalog
            .into_iter()
            .find(|s| s.name == "ipc.subscribe")
            .expect("ipc.subscribe must be in catalog");
        Self { spec }
    }
}

impl BuiltinTool for IpcSubscribeTool {
    fn name(&self) -> &str {
        "ipc.subscribe"
    }
    fn spec(&self) -> &BuiltinToolSpec {
        &self.spec
    }
    fn execute(&self, _args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        // Stub: real implementation will route through TopicRouter
        Err(ToolError::ExecutionFailed(
            "ipc.subscribe requires async kernel context".into(),
        ))
    }
}
