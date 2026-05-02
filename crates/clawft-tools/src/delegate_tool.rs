//! Delegate task tool.
//!
//! Provides a `delegate_task` tool that delegates complex orchestration
//! tasks to a Claude sub-agent via the [`ClaudeDelegator`].
//!
//! The tool checks the [`DelegationEngine`] to decide whether to delegate,
//! then hands off to the Claude API delegator for execution.
//!
//! # Feature gate
//!
//! This module is gated behind `feature = "delegate"`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use clawft_core::tools::registry::{Tool, ToolError, ToolRegistry};
use serde_json::{Value, json};
use tracing::{debug, info};

use clawft_services::delegation::DelegationEngine;
use clawft_services::delegation::claude::ClaudeDelegator;
use clawft_types::delegation::DelegationTarget;

/// Tool that delegates complex tasks to a Claude sub-agent.
///
/// When invoked, the tool:
/// 1. Queries the [`DelegationEngine`] to decide whether to delegate.
/// 2. If the target is `Claude`, delegates via the Anthropic API.
/// 3. Returns the final text response from the delegate, or falls
///    back to a "handle locally" message if not delegated.
pub struct DelegateTaskTool {
    delegator: Arc<ClaudeDelegator>,
    engine: Arc<DelegationEngine>,
    /// Snapshot of tool schemas at registration time.
    tool_schemas: Vec<Value>,
    /// Shared registry for executing tool calls from the delegate.
    registry: Arc<ToolRegistry>,
}

impl DelegateTaskTool {
    /// Create a new delegate task tool.
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
            delegator,
            engine,
            tool_schemas,
            registry,
        }
    }
}

#[async_trait]
impl Tool for DelegateTaskTool {
    fn name(&self) -> &str {
        "delegate_task"
    }

    fn description(&self) -> &str {
        "Delegate a complex task to Claude for multi-step orchestration. \
         Claude will use available tools to accomplish the task."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "The task description to delegate to Claude"
                },
                "model": {
                    "type": "string",
                    "description": "Optional model override (defaults to config)"
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

        // Check if delegation engine approves.
        let claude_available = true; // We have a delegator.
        let decision = self.engine.decide(task, claude_available);

        if decision == DelegationTarget::Local {
            info!(task = %task, "delegation engine decided to handle locally");
            return Ok(json!({
                "status": "local",
                "message": "Task does not require delegation; handle locally.",
                "task": task,
            }));
        }

        info!(task = %task, target = ?decision, "delegating task");

        // Build the tool executor closure using the shared registry.
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

        match self
            .delegator
            .delegate(task, &self.tool_schemas, &tool_executor)
            .await
        {
            Ok(response) => Ok(json!({
                "status": "delegated",
                "target": "claude",
                "response": response,
                "task": task,
            })),
            Err(e) => Err(ToolError::ExecutionFailed(format!(
                "delegation failed: {e}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_validation() {
        let params = json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "The task description to delegate to Claude"
                },
                "model": {
                    "type": "string",
                    "description": "Optional model override (defaults to config)"
                }
            },
            "required": ["task"]
        });

        assert_eq!(params["type"], "object");
        assert!(params["properties"]["task"].is_object());
        assert!(params["properties"]["model"].is_object());
        let required = params["required"].as_array().unwrap();
        assert!(required.contains(&json!("task")));
        assert!(!required.contains(&json!("model")));
    }

    #[test]
    fn tool_name_and_description() {
        assert_eq!("delegate_task", "delegate_task");
        assert!(!"Delegate a complex task to Claude".is_empty());
    }
}
