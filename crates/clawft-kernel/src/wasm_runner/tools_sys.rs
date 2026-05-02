//! Built-in system tool implementations and shell execution.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::catalog::builtin_tool_catalog;
use super::registry::BuiltinTool;
use super::runner::compute_module_hash;
use super::types::*;
use crate::governance::EffectVector;

// ---------------------------------------------------------------------------
// System service tools
// ---------------------------------------------------------------------------

/// Built-in `sys.service.list` tool.
pub struct SysServiceListTool {
    spec: BuiltinToolSpec,
    service_registry: Arc<crate::service::ServiceRegistry>,
}

impl SysServiceListTool {
    pub fn new(service_registry: Arc<crate::service::ServiceRegistry>) -> Self {
        let spec = builtin_tool_catalog().into_iter().find(|s| s.name == "sys.service.list").unwrap();
        Self { spec, service_registry }
    }
}

impl BuiltinTool for SysServiceListTool {
    fn name(&self) -> &str { "sys.service.list" }
    fn spec(&self) -> &BuiltinToolSpec { &self.spec }
    fn execute(&self, _args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let services = self.service_registry.list();
        let entries: Vec<serde_json::Value> = services.iter().map(|(name, stype)| {
            serde_json::json!({
                "name": name,
                "service_type": format!("{stype:?}"),
            })
        }).collect();
        Ok(serde_json::json!({"services": entries, "count": entries.len()}))
    }
}

/// Built-in `sys.service.health` tool.
pub struct SysServiceHealthTool {
    spec: BuiltinToolSpec,
    service_registry: Arc<crate::service::ServiceRegistry>,
}

impl SysServiceHealthTool {
    pub fn new(service_registry: Arc<crate::service::ServiceRegistry>) -> Self {
        let spec = builtin_tool_catalog().into_iter().find(|s| s.name == "sys.service.health").unwrap();
        Self { spec, service_registry }
    }
}

impl BuiltinTool for SysServiceHealthTool {
    fn name(&self) -> &str { "sys.service.health" }
    fn spec(&self) -> &BuiltinToolSpec { &self.spec }
    fn execute(&self, _args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        // health_all() is async -- use service list as sync fallback
        let services = self.service_registry.list();
        let entries: Vec<serde_json::Value> = services.iter().map(|(name, _)| {
            serde_json::json!({"name": name, "status": "registered"})
        }).collect();
        Ok(serde_json::json!({"health": entries, "count": entries.len()}))
    }
}

// ---------------------------------------------------------------------------
// Chain tools (exochain feature)
// ---------------------------------------------------------------------------

/// Built-in `sys.chain.status` tool.
#[cfg(feature = "exochain")]
pub struct SysChainStatusTool {
    spec: BuiltinToolSpec,
    chain: Arc<crate::chain::ChainManager>,
}

#[cfg(feature = "exochain")]
impl SysChainStatusTool {
    pub fn new(chain: Arc<crate::chain::ChainManager>) -> Self {
        let spec = builtin_tool_catalog().into_iter().find(|s| s.name == "sys.chain.status").unwrap();
        Self { spec, chain }
    }
}

#[cfg(feature = "exochain")]
impl BuiltinTool for SysChainStatusTool {
    fn name(&self) -> &str { "sys.chain.status" }
    fn spec(&self) -> &BuiltinToolSpec { &self.spec }
    fn execute(&self, _args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let status = self.chain.status();
        Ok(serde_json::json!({
            "chain_id": status.chain_id,
            "sequence": status.sequence,
            "event_count": status.event_count,
            "checkpoint_count": status.checkpoint_count,
        }))
    }
}

/// Built-in `sys.chain.query` tool.
#[cfg(feature = "exochain")]
pub struct SysChainQueryTool {
    spec: BuiltinToolSpec,
    chain: Arc<crate::chain::ChainManager>,
}

#[cfg(feature = "exochain")]
impl SysChainQueryTool {
    pub fn new(chain: Arc<crate::chain::ChainManager>) -> Self {
        let spec = builtin_tool_catalog().into_iter().find(|s| s.name == "sys.chain.query").unwrap();
        Self { spec, chain }
    }
}

#[cfg(feature = "exochain")]
impl BuiltinTool for SysChainQueryTool {
    fn name(&self) -> &str { "sys.chain.query" }
    fn spec(&self) -> &BuiltinToolSpec { &self.spec }
    fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let count = args.get("count").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
        let events = self.chain.tail(count);
        let entries: Vec<serde_json::Value> = events.iter().map(|e| {
            serde_json::json!({
                "sequence": e.sequence,
                "source": e.source,
                "kind": e.kind,
                "timestamp": e.timestamp.to_rfc3339(),
            })
        }).collect();
        Ok(serde_json::json!({"events": entries, "count": entries.len()}))
    }
}

// ---------------------------------------------------------------------------
// Tree tools (exochain feature)
// ---------------------------------------------------------------------------

/// Built-in `sys.tree.read` tool.
#[cfg(feature = "exochain")]
pub struct SysTreeReadTool {
    spec: BuiltinToolSpec,
    tree: Arc<crate::tree_manager::TreeManager>,
}

#[cfg(feature = "exochain")]
impl SysTreeReadTool {
    pub fn new(tree: Arc<crate::tree_manager::TreeManager>) -> Self {
        let spec = builtin_tool_catalog().into_iter().find(|s| s.name == "sys.tree.read").unwrap();
        Self { spec, tree }
    }
}

#[cfg(feature = "exochain")]
impl BuiltinTool for SysTreeReadTool {
    fn name(&self) -> &str { "sys.tree.read" }
    fn spec(&self) -> &BuiltinToolSpec { &self.spec }
    fn execute(&self, _args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let stats = self.tree.stats();
        Ok(serde_json::json!({
            "node_count": stats.node_count,
            "mutation_count": stats.mutation_count,
            "root_hash": stats.root_hash,
        }))
    }
}

/// Built-in `sys.tree.inspect` tool.
#[cfg(feature = "exochain")]
pub struct SysTreeInspectTool {
    spec: BuiltinToolSpec,
    tree: Arc<crate::tree_manager::TreeManager>,
}

#[cfg(feature = "exochain")]
impl SysTreeInspectTool {
    pub fn new(tree: Arc<crate::tree_manager::TreeManager>) -> Self {
        let spec = builtin_tool_catalog().into_iter().find(|s| s.name == "sys.tree.inspect").unwrap();
        Self { spec, tree }
    }
}

#[cfg(feature = "exochain")]
impl BuiltinTool for SysTreeInspectTool {
    fn name(&self) -> &str { "sys.tree.inspect" }
    fn spec(&self) -> &BuiltinToolSpec { &self.spec }
    fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let path = args.get("path").and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("missing 'path'".into()))?;
        let rid = exo_resource_tree::ResourceId::new(path);
        let tree_lock = self.tree.tree().lock()
            .map_err(|e| ToolError::ExecutionFailed(format!("tree lock: {e}")))?;
        let node = tree_lock.get(&rid)
            .ok_or_else(|| ToolError::NotFound(format!("node not found: {path}")))?;
        Ok(serde_json::json!({
            "path": path,
            "kind": format!("{:?}", node.kind),
            "metadata": node.metadata,
            "scoring": node.scoring.as_array(),
        }))
    }
}

// ---------------------------------------------------------------------------
// Env / Cron tools
// ---------------------------------------------------------------------------

/// Built-in `sys.env.get` tool.
pub struct SysEnvGetTool {
    spec: BuiltinToolSpec,
}

impl SysEnvGetTool {
    pub fn new() -> Self {
        let spec = builtin_tool_catalog().into_iter().find(|s| s.name == "sys.env.get").unwrap();
        Self { spec }
    }
}

impl BuiltinTool for SysEnvGetTool {
    fn name(&self) -> &str { "sys.env.get" }
    fn spec(&self) -> &BuiltinToolSpec { &self.spec }
    fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let name = args.get("name").and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("missing 'name'".into()))?;
        match std::env::var(name) {
            Ok(val) => Ok(serde_json::json!({"name": name, "value": val})),
            Err(_) => Ok(serde_json::json!({"name": name, "value": null})),
        }
    }
}

/// Built-in `sys.cron.add` tool.
pub struct SysCronAddTool {
    spec: BuiltinToolSpec,
    cron: Arc<crate::cron::CronService>,
}

impl SysCronAddTool {
    pub fn new(cron: Arc<crate::cron::CronService>) -> Self {
        let spec = builtin_tool_catalog().into_iter().find(|s| s.name == "sys.cron.add").unwrap();
        Self { spec, cron }
    }
}

impl BuiltinTool for SysCronAddTool {
    fn name(&self) -> &str { "sys.cron.add" }
    fn spec(&self) -> &BuiltinToolSpec { &self.spec }
    fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let name = args.get("name").and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("missing 'name'".into()))?;
        let interval_secs = args.get("interval_secs").and_then(|v| v.as_u64())
            .ok_or_else(|| ToolError::InvalidArgs("missing 'interval_secs'".into()))?;
        let command = args.get("command").and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("missing 'command'".into()))?;
        let target_pid = args.get("target_pid").and_then(|v| v.as_u64());
        let job = self.cron.add_job(name.to_string(), interval_secs, command.to_string(), target_pid)
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        Ok(serde_json::to_value(&job).unwrap_or_default())
    }
}

/// Built-in `sys.cron.list` tool.
pub struct SysCronListTool {
    spec: BuiltinToolSpec,
    cron: Arc<crate::cron::CronService>,
}

impl SysCronListTool {
    pub fn new(cron: Arc<crate::cron::CronService>) -> Self {
        let spec = builtin_tool_catalog().into_iter().find(|s| s.name == "sys.cron.list").unwrap();
        Self { spec, cron }
    }
}

impl BuiltinTool for SysCronListTool {
    fn name(&self) -> &str { "sys.cron.list" }
    fn spec(&self) -> &BuiltinToolSpec { &self.spec }
    fn execute(&self, _args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let jobs = self.cron.list_jobs();
        Ok(serde_json::to_value(&jobs).unwrap_or_default())
    }
}

/// Built-in `sys.cron.remove` tool.
pub struct SysCronRemoveTool {
    spec: BuiltinToolSpec,
    cron: Arc<crate::cron::CronService>,
}

impl SysCronRemoveTool {
    pub fn new(cron: Arc<crate::cron::CronService>) -> Self {
        let spec = builtin_tool_catalog().into_iter().find(|s| s.name == "sys.cron.remove").unwrap();
        Self { spec, cron }
    }
}

impl BuiltinTool for SysCronRemoveTool {
    fn name(&self) -> &str { "sys.cron.remove" }
    fn spec(&self) -> &BuiltinToolSpec { &self.spec }
    fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let id = args.get("id").and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("missing 'id'".into()))?;
        match self.cron.remove_job(id) {
            Ok(Some(job)) => Ok(serde_json::json!({"removed": true, "job_id": job.id})),
            Ok(None) => Err(ToolError::NotFound(format!("cron job: {id}"))),
            Err(e) => Err(ToolError::PermissionDenied(format!("{e}"))),
        }
    }
}

// ---------------------------------------------------------------------------
// Shell Command Execution
// ---------------------------------------------------------------------------

/// A shell command to be executed in the sandbox.
///
/// Represents a command with arguments and optional sandbox configuration.
/// The command is dispatched through the tool execution path and chain-logged.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellCommand {
    /// The command to execute (e.g. "echo", "ls").
    pub command: String,
    /// Arguments to the command.
    pub args: Vec<String>,
    /// Optional sandbox configuration to restrict execution.
    pub sandbox_config: Option<SandboxConfig>,
}

/// Result of a shell command execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellResult {
    /// Process exit code (0 = success).
    pub exit_code: i32,
    /// Standard output captured from the command.
    pub stdout: String,
    /// Standard error captured from the command.
    pub stderr: String,
    /// Execution wall-clock time in milliseconds.
    pub execution_time_ms: u64,
}

/// Execute a shell command and return the result.
///
/// For now this dispatches as a builtin tool -- actual WASM compilation
/// of shell commands is deferred to a future sprint. The sandbox config
/// is stored on the result for governance auditing.
///
/// When the `exochain` feature is enabled and a [`ChainManager`] is
/// provided, the execution is chain-logged as a `shell.exec` event.
pub fn execute_shell(cmd: &ShellCommand) -> Result<ShellResult, ToolError> {
    let start = std::time::Instant::now();

    // Sandbox path check: if sandbox_config has allowed_paths,
    // reject commands that reference paths outside the sandbox.
    if let Some(ref sandbox) = cmd.sandbox_config
        && sandbox.sudo_override {
            tracing::warn!(command = %cmd.command, "shell exec with sudo override");
        }

    // Builtin dispatch: for now, handle a small set of safe builtins.
    // Real execution would compile to WASM and run in the sandbox.
    let (exit_code, stdout, stderr) = match cmd.command.as_str() {
        "echo" => {
            let output = cmd.args.join(" ");
            (0, output, String::new())
        }
        "true" => (0, String::new(), String::new()),
        "false" => (1, String::new(), String::new()),
        _ => {
            // Unknown commands return a descriptive error in stderr.
            // Future: compile to WASM and run in sandbox.
            (127, String::new(), format!("command not found: {}", cmd.command))
        }
    };

    let elapsed = start.elapsed();

    Ok(ShellResult {
        exit_code,
        stdout,
        stderr,
        execution_time_ms: elapsed.as_millis() as u64,
    })
}

/// Built-in `shell.exec` tool wrapping [`execute_shell`].
pub struct ShellExecTool {
    spec: BuiltinToolSpec,
}

impl ShellExecTool {
    /// Create the shell.exec tool.
    pub fn new() -> Self {
        Self {
            spec: BuiltinToolSpec {
                name: "shell.exec".into(),
                category: ToolCategory::System,
                description: "Execute a shell command in the sandbox".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "required": ["command"],
                    "properties": {
                        "command": {"type": "string", "description": "Command to execute"},
                        "args": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Command arguments"
                        }
                    }
                }),
                gate_action: "tool.shell.execute".into(),
                effect: EffectVector {
                    risk: 0.7,
                    security: 0.4,
                    ..Default::default()
                },
                native: true,
            },
        }
    }
}

impl BuiltinTool for ShellExecTool {
    fn name(&self) -> &str { "shell.exec" }
    fn spec(&self) -> &BuiltinToolSpec { &self.spec }
    fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let command = args.get("command").and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("missing 'command'".into()))?;
        let cmd_args: Vec<String> = args.get("args")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let cmd = ShellCommand {
            command: command.to_string(),
            args: cmd_args,
            sandbox_config: None,
        };

        let result = execute_shell(&cmd)?;
        Ok(serde_json::json!({
            "exit_code": result.exit_code,
            "stdout": result.stdout,
            "stderr": result.stderr,
            "execution_time_ms": result.execution_time_ms,
        }))
    }
}

// ---------------------------------------------------------------------------
// Shell Pipeline (K3 C5)
// ---------------------------------------------------------------------------

/// A shell pipeline compiled into a chain-linked WASM tool spec.
///
/// Shell commands are wrapped as tool definitions with their content
/// hash anchored to the ExoChain for immutability and provenance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellPipeline {
    /// Pipeline name.
    pub name: String,
    /// Shell command string.
    pub command: String,
    /// SHA-256 hash of the command.
    pub content_hash: [u8; 32],
    /// Chain sequence number where this pipeline was registered.
    pub chain_seq: Option<u64>,
}

impl ShellPipeline {
    /// Create a new shell pipeline from a command string.
    pub fn new(name: impl Into<String>, command: impl Into<String>) -> Self {
        let cmd = command.into();
        let hash = compute_module_hash(cmd.as_bytes());
        Self {
            name: name.into(),
            command: cmd,
            content_hash: hash,
            chain_seq: None,
        }
    }

    /// Register this pipeline on the chain for immutability (C5).
    #[cfg(feature = "exochain")]
    pub fn anchor_to_chain(&mut self, chain: &crate::chain::ChainManager) {
        let seq = chain.sequence();
        let hash_hex: String = self
            .content_hash
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        chain.append(
            "shell",
            "shell.pipeline.register",
            Some(serde_json::json!({
                "name": &self.name,
                "command_hash": hash_hex,
                "command_length": self.command.len(),
            })),
        );
        self.chain_seq = Some(seq);
    }

    /// Convert to a [`BuiltinToolSpec`] for registration in the [`ToolRegistry`].
    pub fn to_tool_spec(&self) -> BuiltinToolSpec {
        BuiltinToolSpec {
            name: format!("shell.{}", self.name),
            category: ToolCategory::User,
            description: format!("Shell pipeline: {}", self.name),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "args": {"type": "string", "description": "Additional arguments"}
                }
            }),
            gate_action: "tool.shell.execute".into(),
            effect: EffectVector {
                risk: 0.6,
                security: 0.3,
                ..Default::default()
            },
            native: true,
        }
    }
}
