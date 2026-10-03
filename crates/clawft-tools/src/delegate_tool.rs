//! Delegate task tool.
//!
//! Provides a `delegate_task` tool that routes work either to:
//! 1. A **specialist agent profile** loaded from `agents/<name>.{toml,yaml,json}`
//!    (WEFT-349) — preferred when the caller passes `agent`, or
//! 2. A Claude sub-agent via the [`ClaudeDelegator`] for multi-step
//!    orchestration when no specialist is named.
//!
//! Specialist execution order:
//! 1. Injected [`SpecialistRunner`] (tests / mock LLM)
//! 2. Injected [`SubagentSpawner`] (daemon chat path — local child loop)
//! 3. Claude with the profile system prompt prepended
//!
//! The tool checks the [`DelegationEngine`] for the Claude-only path to
//! decide whether to delegate, then hands off for execution.
//!
//! # Feature gate
//!
//! This module is gated behind `feature = "delegate"`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use clawft_core::agent::agents::{AgentDefinition, AgentRegistry};
use clawft_core::agent::spawn::{SpawnSpec, SubagentSpawner, TaskStatus};
use clawft_core::tools::registry::{Tool, ToolError, ToolRegistry};
use serde_json::{Value, json};
use tracing::{debug, info, warn};

use clawft_services::delegation::DelegationEngine;
use clawft_services::delegation::claude::ClaudeDelegator;
use clawft_types::delegation::DelegationTarget;

/// Executes a specialist profile against a task.
///
/// Injected at construction so unit tests can supply a mock LLM / canned
/// review without standing up Claude or a daemon spawner (WEFT-349 AC).
#[async_trait]
pub trait SpecialistRunner: Send + Sync {
    /// Run `task` under the given specialist profile; return the text result.
    async fn run(&self, profile: &AgentDefinition, task: &str) -> Result<String, ToolError>;
}

/// Tool that delegates tasks to a specialist agent profile or Claude.
///
/// When invoked with `agent`, the tool:
/// 1. Loads the specialist profile from the [`AgentRegistry`].
/// 2. Runs it via the specialist runner / spawner / Claude path.
/// 3. Returns the specialist output to the chat-agent tool loop.
///
/// Without `agent`, falls back to the Claude delegation engine path.
pub struct DelegateTaskTool {
    /// Optional Claude API client (None when no API key / feature path).
    delegator: Option<Arc<ClaudeDelegator>>,
    engine: Arc<DelegationEngine>,
    /// Snapshot of tool schemas at registration time.
    tool_schemas: Vec<Value>,
    /// Shared registry for executing tool calls from the Claude delegate.
    registry: Arc<ToolRegistry>,
    /// Discovered specialist profiles (`agents/` layout).
    agents: Arc<AgentRegistry>,
    /// Optional local subagent spawner (daemon chat path).
    spawner: Option<Arc<dyn SubagentSpawner>>,
    /// Optional specialist runner override (tests / mock LLM).
    specialist_runner: Option<Arc<dyn SpecialistRunner>>,
}

impl DelegateTaskTool {
    /// Create a new delegate task tool with a Claude delegator (legacy ctor).
    ///
    /// # Arguments
    ///
    /// * `delegator` - The Claude API client for delegation.
    /// * `engine` - The decision engine for routing.
    /// * `tool_schemas` - OpenAI-format tool schemas (from `registry.schemas()`).
    /// * `registry` - The tool registry for executing sub-agent tool calls.
    pub fn new(
        delegator: Arc<ClaudeDelegator>,
        engine: Arc<DelegationEngine>,
        tool_schemas: Vec<Value>,
        registry: Arc<ToolRegistry>,
    ) -> Self {
        Self {
            delegator: Some(delegator),
            engine,
            tool_schemas,
            registry,
            agents: Arc::new(AgentRegistry::empty()),
            spawner: None,
            specialist_runner: None,
        }
    }

    /// Create a tool that may run specialists without a Claude client.
    pub fn new_with_options(
        delegator: Option<Arc<ClaudeDelegator>>,
        engine: Arc<DelegationEngine>,
        tool_schemas: Vec<Value>,
        registry: Arc<ToolRegistry>,
        agents: Arc<AgentRegistry>,
        spawner: Option<Arc<dyn SubagentSpawner>>,
    ) -> Self {
        Self {
            delegator,
            engine,
            tool_schemas,
            registry,
            agents,
            spawner,
            specialist_runner: None,
        }
    }

    /// Attach specialist agent profiles (WEFT-349).
    pub fn with_agents(mut self, agents: Arc<AgentRegistry>) -> Self {
        self.agents = agents;
        self
    }

    /// Attach a local subagent spawner for specialist execution.
    pub fn with_spawner(mut self, spawner: Arc<dyn SubagentSpawner>) -> Self {
        self.spawner = Some(spawner);
        self
    }

    /// Attach a specialist runner (mock LLM / custom backend).
    pub fn with_specialist_runner(mut self, runner: Arc<dyn SpecialistRunner>) -> Self {
        self.specialist_runner = Some(runner);
        self
    }

    /// Whether any specialist or Claude path is usable.
    pub fn has_backend(&self) -> bool {
        self.specialist_runner.is_some()
            || self.spawner.is_some()
            || self
                .delegator
                .as_ref()
                .map(|d| d.is_available())
                .unwrap_or(false)
            || !self.agents.is_empty()
    }

    /// Build the user-facing task payload that embeds the specialist prompt.
    fn specialist_goal(profile: &AgentDefinition, task: &str) -> String {
        match &profile.system_prompt {
            Some(sp) if !sp.is_empty() => {
                format!(
                    "# Specialist: {}\n\n{}\n\n---\n\n## Task\n{}",
                    profile.name, sp, task
                )
            }
            _ => task.to_string(),
        }
    }

    /// Execute a named specialist profile and return the structured tool result.
    async fn run_specialist(&self, profile: &AgentDefinition, task: &str) -> Result<Value, ToolError> {
        let agent_name = profile.name.as_str();
        info!(agent = %agent_name, "delegate_task spawning specialist");

        // 1. Explicit runner (tests / mock LLM).
        if let Some(runner) = &self.specialist_runner {
            let response = runner.run(profile, task).await?;
            return Ok(json!({
                "status": "delegated",
                "target": "specialist",
                "agent": agent_name,
                "response": response,
                "task": task,
                "profile": {
                    "name": profile.name,
                    "description": profile.description,
                    "skills": profile.skills,
                    "source_path": profile.source_path.as_ref().map(|p| p.display().to_string()),
                },
            }));
        }

        // 2. Local subagent spawner (daemon chat path).
        if let Some(spawner) = &self.spawner {
            let goal = Self::specialist_goal(profile, task);
            let mut spec = SpawnSpec::new(goal);
            spec.persona = Some(profile.name.clone());
            spec.skills = profile.skills.clone();
            spec.await_completion = true;

            match spawner.spawn(spec).await {
                Ok(handle) => {
                    let response = handle
                        .outcome
                        .as_ref()
                        .and_then(|o| o.result.clone())
                        .unwrap_or_else(|| {
                            format!(
                                "Specialist `{}` spawned (task_id={}, status={})",
                                agent_name,
                                handle.task_id,
                                handle.status.as_str()
                            )
                        });
                    let status_label = handle.status.as_str();
                    let ok = matches!(
                        handle.status,
                        TaskStatus::Completed | TaskStatus::Exhausted | TaskStatus::Running
                    );
                    if !ok
                        && let Some(err) = handle.outcome.as_ref().and_then(|o| o.error.as_ref()) {
                            return Err(ToolError::ExecutionFailed(format!(
                                "specialist `{agent_name}` failed: {err}"
                            )));
                        }
                    return Ok(json!({
                        "status": "delegated",
                        "target": "specialist",
                        "agent": agent_name,
                        "response": response,
                        "task": task,
                        "task_id": handle.task_id,
                        "child_conv_id": handle.child_conv_id,
                        "spawn_status": status_label,
                        "profile": {
                            "name": profile.name,
                            "description": profile.description,
                            "skills": profile.skills,
                        },
                    }));
                }
                Err(e) => {
                    return Err(ToolError::ExecutionFailed(format!(
                        "specialist spawn failed for `{agent_name}`: {e}"
                    )));
                }
            }
        }

        // 3. Claude with specialist system prompt.
        if let Some(delegator) = &self.delegator
            && delegator.is_available()
        {
            let goal = Self::specialist_goal(profile, task);
            let response = self.delegate_to_claude(&goal).await?;
            return Ok(json!({
                "status": "delegated",
                "target": "specialist-claude",
                "agent": agent_name,
                "response": response,
                "task": task,
                "profile": {
                    "name": profile.name,
                    "description": profile.description,
                    "skills": profile.skills,
                },
            }));
        }

        Err(ToolError::ExecutionFailed(format!(
            "specialist `{agent_name}` profile loaded but no execution backend is available \
             (inject SpecialistRunner, SubagentSpawner, or configure Claude)"
        )))
    }

    /// Claude tool-use loop (shared by specialist-claude and plain Claude paths).
    async fn delegate_to_claude(&self, task: &str) -> Result<String, ToolError> {
        let delegator = self.delegator.as_ref().ok_or_else(|| {
            ToolError::ExecutionFailed("Claude delegator is not configured".into())
        })?;

        let registry = self.registry.clone();
        let tool_executor =
            move |name: &str,
                  input: Value|
                  -> Pin<Box<dyn Future<Output = Result<String, String>> + Send>> {
                let registry = registry.clone();
                let name = name.to_string();
                Box::pin(async move {
                    match registry.execute(&name, input, None).await {
                        Ok(result) => Ok(serde_json::to_string(&result).unwrap_or_default()),
                        Err(e) => Err(e.to_string()),
                    }
                })
            };

        delegator
            .delegate(task, &self.tool_schemas, &tool_executor)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("delegation failed: {e}")))
    }
}

#[async_trait]
impl Tool for DelegateTaskTool {
    fn name(&self) -> &str {
        "delegate_task"
    }

    fn description(&self) -> &str {
        "Delegate a complex task to a specialist agent profile (from agents/) \
         or to Claude for multi-step orchestration. Pass `agent` to load a \
         named specialist (e.g. code-reviewer); omit it to use the Claude path."
    }

    fn parameters(&self) -> Value {
        let agent_names = self.agents.names();
        let agent_desc = if agent_names.is_empty() {
            "Specialist agent profile name (loaded from agents/<name>.toml or agents/<name>/agent.*)"
                .to_string()
        } else {
            format!(
                "Specialist agent profile name. Available: {}",
                agent_names.join(", ")
            )
        };
        json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "The task description to delegate"
                },
                "agent": {
                    "type": "string",
                    "description": agent_desc
                },
                "model": {
                    "type": "string",
                    "description": "Optional model override (Claude path; defaults to config)"
                }
            },
            "required": ["task"]
        })
    }

    async fn execute(&self, args: Value) -> Result<Value, ToolError> {
        let task = args
            .get("task")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("missing required field: task".into()))?;

        debug!(task = %task, "delegate_task invoked");

        // ── WEFT-349: named specialist path ─────────────────────────────
        if let Some(agent_name) = args
            .get("agent")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let profile = self.agents.get(agent_name).cloned().ok_or_else(|| {
                let available = self.agents.names();
                let hint = if available.is_empty() {
                    "no specialist profiles are registered".to_string()
                } else {
                    format!("available: {}", available.join(", "))
                };
                ToolError::InvalidArgs(format!(
                    "unknown specialist agent `{agent_name}` ({hint})"
                ))
            })?;
            return self.run_specialist(&profile, task).await;
        }

        // ── Claude-only path (engine decide) ────────────────────────────
        let claude_available = self
            .delegator
            .as_ref()
            .map(|d| d.is_available())
            .unwrap_or(false);
        let decision = self.engine.decide(task, claude_available);

        if decision == DelegationTarget::Local {
            info!(
                task = %task,
                claude_available,
                "delegation engine decided to handle locally"
            );
            return Ok(json!({
                "status": "local",
                "message": if claude_available {
                    "Task does not require delegation; handle locally."
                } else {
                    "Claude delegator is not available; handle locally."
                },
                "task": task,
                "claude_available": claude_available,
            }));
        }

        if !claude_available {
            // Engine wanted Claude but nothing is available. If we have a
            // single specialist or a runner, surface a useful hint.
            if !self.agents.is_empty() {
                warn!(
                    task = %task,
                    agents = ?self.agents.names(),
                    "Claude unavailable; pass agent=<name> to use a specialist profile"
                );
            }
            return Ok(json!({
                "status": "local",
                "message": "Claude delegator is not available; handle locally (or pass agent= for a specialist).",
                "task": task,
                "claude_available": false,
            }));
        }

        info!(
            task = %task,
            target = ?decision,
            claude_available,
            "delegating task to Claude"
        );

        let response = self.delegate_to_claude(task).await?;
        Ok(json!({
            "status": "delegated",
            "target": "claude",
            "response": response,
            "task": task,
        }))
    }
}

/// Options for [`register_delegate_task`] (WEFT-349).
#[derive(Default)]
pub struct RegisterDelegateOpts {
    /// Workspace agents directory (`agents/` or `.clawft/agents`).
    pub workspace_agents: Option<std::path::PathBuf>,
    /// User agents directory (`~/.clawft/agents`).
    pub user_agents: Option<std::path::PathBuf>,
    /// Local subagent spawner for specialist execution (daemon path).
    pub spawner: Option<Arc<dyn SubagentSpawner>>,
    /// Optional Anthropic API key from providers config (env wins).
    pub config_api_key: Option<String>,
}

/// Discover specialist profiles and optionally build a Claude client, then
/// register [`DelegateTaskTool`] when either path is usable.
///
/// Shared by the CLI (`register_core_tools`) and the weave daemon so the
/// chat agent always sees `delegate_task` when profiles exist (WEFT-349).
pub fn register_delegate_task(
    config: &clawft_types::delegation::DelegationConfig,
    registry: &mut ToolRegistry,
    opts: RegisterDelegateOpts,
) {
    let agents = discover_agents(opts.workspace_agents.as_deref(), opts.user_agents.as_deref());

    let mut delegator: Option<Arc<ClaudeDelegator>> = None;
    if config.claude_enabled {
        if let Some(key) = resolve_anthropic_key(opts.config_api_key.as_deref()) {
            match ClaudeDelegator::new(config, key) {
                Some(d) => delegator = Some(Arc::new(d)),
                None => warn!("failed to create ClaudeDelegator"),
            }
        } else {
            debug!(
                "no Anthropic API key; Claude delegation disabled (specialists still available)"
            );
        }
    } else {
        info!("Claude delegation disabled in config");
    }

    let has_claude = delegator
        .as_ref()
        .map(|d| d.is_available())
        .unwrap_or(false);
    if !has_claude && agents.is_empty() {
        info!("delegation not registered (no Claude key and no specialist profiles)");
        return;
    }

    if !agents.is_empty() {
        info!(
            count = agents.len(),
            agents = ?agents.names(),
            "specialist agent profiles discovered for delegate_task"
        );
    }

    let engine = Arc::new(DelegationEngine::new(config.clone()));
    let tool_schemas = registry.schemas();
    let registry_snapshot = Arc::new(registry.snapshot());

    let tool = DelegateTaskTool::new_with_options(
        delegator,
        engine,
        tool_schemas,
        registry_snapshot,
        agents,
        opts.spawner,
    );
    registry.register(Arc::new(tool));
    info!(
        claude = has_claude,
        "delegation tool registered (specialist profiles + optional Claude)"
    );
}

fn resolve_anthropic_key(config_api_key: Option<&str>) -> Option<String> {
    match std::env::var("ANTHROPIC_API_KEY") {
        Ok(key) if !key.is_empty() => Some(key),
        Ok(_) => config_api_key
            .filter(|k| !k.is_empty())
            .map(str::to_string),
        Err(_) => config_api_key
            .filter(|k| !k.is_empty())
            .map(str::to_string),
    }
}

fn discover_agents(
    workspace_override: Option<&std::path::Path>,
    user_override: Option<&std::path::Path>,
) -> Arc<AgentRegistry> {
    use std::path::Path;

    // Callers (CLI/daemon) may pass `~/.clawft/agents` as `user_override`.
    // When unset we only walk upward for workspace agents.
    let user_dir = user_override.map(|p| p.to_path_buf());

    let ws_dir = workspace_override.map(|p| p.to_path_buf()).or_else(|| {
        std::env::current_dir().ok().and_then(|cwd| {
            let mut dir: &Path = cwd.as_path();
            loop {
                let clawft = dir.join(".clawft").join("agents");
                if clawft.is_dir() {
                    return Some(clawft);
                }
                let top = dir.join("agents");
                if top.is_dir() {
                    return Some(top);
                }
                match dir.parent() {
                    Some(parent) => dir = parent,
                    None => return None,
                }
            }
        })
    });

    match AgentRegistry::discover(ws_dir.as_deref(), user_dir.as_deref(), Vec::new()) {
        Ok(r) => Arc::new(r),
        Err(e) => {
            warn!(error = %e, "failed to discover specialist agents");
            Arc::new(AgentRegistry::empty())
        }
    }
}

#[cfg(test)]
#[path = "delegate_tool_tests.rs"]
mod tests;
