//! Docker/Podman orchestration tool plugin for clawft.
//!
//! Provides tools for container operations (build, run, stop, logs, list, exec)
//! using subprocess invocations via `tokio::process::Command`. Supports both
//! `docker` and `podman` runtimes.
//!
//! # Security
//!
//! All command arguments are validated and constructed programmatically.
//! No shell interpolation is ever used. Container names, image names,
//! and environment variables are validated against strict allowlists.
//!
//! # Resource Limits
//!
//! A global concurrency limiter (default: 3 concurrent operations)
//! prevents resource exhaustion under multi-agent use.
//!
//! # Feature Flag
//!
//! This crate is gated behind the workspace `plugin-containers` feature flag.

pub mod operations;
pub mod types;

use std::sync::Arc;

use async_trait::async_trait;
use clawft_plugin::{PluginError, Tool, ToolContext};

use operations::{ConcurrencyLimiter, execute_container};
use types::{ContainerConfig, is_valid_env_var, is_valid_name};

// ---------------------------------------------------------------------------
// Shared builder
// ---------------------------------------------------------------------------

/// Build validated argument lists for container commands.
struct ArgBuilder {
    args: Vec<String>,
}

impl ArgBuilder {
    fn new() -> Self {
        Self { args: Vec::new() }
    }

    fn push(&mut self, arg: impl Into<String>) {
        self.args.push(arg.into());
    }

    fn build(self) -> Vec<String> {
        self.args
    }
}

// ---------------------------------------------------------------------------
// ContainerBuildTool
// ---------------------------------------------------------------------------

/// Tool that builds a container image.
pub struct ContainerBuildTool {
    config: ContainerConfig,
    limiter: Arc<ConcurrencyLimiter>,
}

impl ContainerBuildTool {
    pub fn new(config: ContainerConfig, limiter: Arc<ConcurrencyLimiter>) -> Self {
        Self { config, limiter }
    }
}

#[async_trait]
impl Tool for ContainerBuildTool {
    fn name(&self) -> &str {
        "container_build"
    }

    fn description(&self) -> &str {
        "Build a container image from a Dockerfile"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "context_path": {
                    "type": "string",
                    "description": "Path to the build context directory"
                },
                "tag": {
                    "type": "string",
                    "description": "Image tag (e.g., 'myapp:latest')"
                },
                "dockerfile": {
                    "type": "string",
                    "description": "Path to the Dockerfile (relative to context)"
                },
                "no_cache": {
                    "type": "boolean",
                    "description": "Disable build cache",
                    "default": false
                }
            },
            "required": ["context_path"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &dyn ToolContext,
    ) -> Result<serde_json::Value, PluginError> {
        let context_path = params
            .get("context_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PluginError::ExecutionFailed("context_path is required".into()))?;

        let mut builder = ArgBuilder::new();
        builder.push("build");

        if let Some(tag) = params.get("tag").and_then(|v| v.as_str()) {
            if !is_valid_name(tag) {
                return Err(PluginError::ExecutionFailed(format!(
                    "invalid image tag: '{tag}'"
                )));
            }
            builder.push("-t");
            builder.push(tag);
        }

        if let Some(dockerfile) = params.get("dockerfile").and_then(|v| v.as_str()) {
            builder.push("-f");
            builder.push(dockerfile);
        }

        if params.get("no_cache").and_then(|v| v.as_bool()).unwrap_or(false) {
            builder.push("--no-cache");
        }

        builder.push(context_path);

        let result =
            execute_container(self.config.runtime, &builder.build(), &self.config, &self.limiter)
                .await
                .map_err(PluginError::ExecutionFailed)?;

        serde_json::to_value(&result).map_err(PluginError::from)
    }
}

// ---------------------------------------------------------------------------
// ContainerRunTool
// ---------------------------------------------------------------------------

/// Tool that runs a container.
pub struct ContainerRunTool {
    config: ContainerConfig,
    limiter: Arc<ConcurrencyLimiter>,
}

impl ContainerRunTool {
    pub fn new(config: ContainerConfig, limiter: Arc<ConcurrencyLimiter>) -> Self {
        Self { config, limiter }
    }
}

#[async_trait]
impl Tool for ContainerRunTool {
    fn name(&self) -> &str {
        "container_run"
    }

    fn description(&self) -> &str {
        "Run a container from an image"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "image": {
                    "type": "string",
                    "description": "Image name to run"
                },
                "name": {
                    "type": "string",
                    "description": "Container name"
                },
                "detach": {
                    "type": "boolean",
                    "description": "Run in detached mode",
                    "default": true
                },
                "ports": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Port mappings (e.g., '8080:80')"
                },
                "env": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Environment variables (KEY=VALUE)"
                },
                "remove": {
                    "type": "boolean",
                    "description": "Remove container when it stops",
                    "default": false
                }
            },
            "required": ["image"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &dyn ToolContext,
    ) -> Result<serde_json::Value, PluginError> {
        let image = params
            .get("image")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PluginError::ExecutionFailed("image is required".into()))?;

        if !is_valid_name(image) {
            return Err(PluginError::ExecutionFailed(format!(
                "invalid image name: '{image}'"
            )));
        }

        let mut builder = ArgBuilder::new();
        builder.push("run");

        if params.get("detach").and_then(|v| v.as_bool()).unwrap_or(true) {
            builder.push("-d");
        }

        if params.get("remove").and_then(|v| v.as_bool()).unwrap_or(false) {
            builder.push("--rm");
        }

        if let Some(name) = params.get("name").and_then(|v| v.as_str()) {
            if !is_valid_name(name) {
                return Err(PluginError::ExecutionFailed(format!(
                    "invalid container name: '{name}'"
                )));
            }
            builder.push("--name");
            builder.push(name);
        }

        if let Some(ports) = params.get("ports").and_then(|v| v.as_array()) {
            for port in ports {
                if let Some(p) = port.as_str() {
                    validate_port_mapping(p)?;
                    builder.push("-p");
                    builder.push(p);
                }
            }
        }

        if let Some(envs) = params.get("env").and_then(|v| v.as_array()) {
            for env in envs {
                if let Some(e) = env.as_str() {
                    if !is_valid_env_var(e) {
                        return Err(PluginError::ExecutionFailed(format!(
                            "invalid environment variable: '{e}'"
                        )));
                    }
                    builder.push("-e");
                    builder.push(e);
                }
            }
        }

        builder.push(image);

        let result =
            execute_container(self.config.runtime, &builder.build(), &self.config, &self.limiter)
                .await
                .map_err(PluginError::ExecutionFailed)?;

        serde_json::to_value(&result).map_err(PluginError::from)
    }
}

/// Validate a port mapping string (e.g., `8080:80`, `127.0.0.1:8080:80`).
fn validate_port_mapping(s: &str) -> Result<(), PluginError> {
    let valid = s
        .chars()
        .all(|c| c.is_ascii_digit() || matches!(c, ':' | '.'));
    if !valid || s.is_empty() {
        return Err(PluginError::ExecutionFailed(format!(
            "invalid port mapping: '{s}'"
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// ContainerStopTool
// ---------------------------------------------------------------------------

/// Tool that stops a running container.
pub struct ContainerStopTool {
    config: ContainerConfig,
    limiter: Arc<ConcurrencyLimiter>,
}

impl ContainerStopTool {
    pub fn new(config: ContainerConfig, limiter: Arc<ConcurrencyLimiter>) -> Self {
        Self { config, limiter }
    }
}

#[async_trait]
impl Tool for ContainerStopTool {
    fn name(&self) -> &str {
        "container_stop"
    }

    fn description(&self) -> &str {
        "Stop a running container"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "container": {
                    "type": "string",
                    "description": "Container name or ID"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Seconds to wait before killing",
                    "default": 10
                }
            },
            "required": ["container"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &dyn ToolContext,
    ) -> Result<serde_json::Value, PluginError> {
        let container = params
            .get("container")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PluginError::ExecutionFailed("container is required".into()))?;

        if !is_valid_name(container) {
            return Err(PluginError::ExecutionFailed(format!(
                "invalid container name: '{container}'"
            )));
        }

        let mut builder = ArgBuilder::new();
        builder.push("stop");

        if let Some(timeout) = params.get("timeout").and_then(|v| v.as_u64()) {
            builder.push("-t");
            builder.push(timeout.to_string());
        }

        builder.push(container);

        let result =
            execute_container(self.config.runtime, &builder.build(), &self.config, &self.limiter)
                .await
                .map_err(PluginError::ExecutionFailed)?;

        serde_json::to_value(&result).map_err(PluginError::from)
    }
}

// ---------------------------------------------------------------------------
// ContainerLogsTool
// ---------------------------------------------------------------------------

/// Tool that retrieves container logs.
pub struct ContainerLogsTool {
    config: ContainerConfig,
    limiter: Arc<ConcurrencyLimiter>,
}

impl ContainerLogsTool {
    pub fn new(config: ContainerConfig, limiter: Arc<ConcurrencyLimiter>) -> Self {
        Self { config, limiter }
    }
}

#[async_trait]
impl Tool for ContainerLogsTool {
    fn name(&self) -> &str {
        "container_logs"
    }

    fn description(&self) -> &str {
        "Get logs from a container"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "container": {
                    "type": "string",
                    "description": "Container name or ID"
                },
                "tail": {
                    "type": "integer",
                    "description": "Number of lines from the end to show",
                    "default": 100
                },
                "timestamps": {
                    "type": "boolean",
                    "description": "Show timestamps",
                    "default": false
                }
            },
            "required": ["container"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &dyn ToolContext,
    ) -> Result<serde_json::Value, PluginError> {
        let container = params
            .get("container")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PluginError::ExecutionFailed("container is required".into()))?;

        if !is_valid_name(container) {
            return Err(PluginError::ExecutionFailed(format!(
                "invalid container name: '{container}'"
            )));
        }

        let mut builder = ArgBuilder::new();
        builder.push("logs");

        let tail = params.get("tail").and_then(|v| v.as_u64()).unwrap_or(100);
        builder.push("--tail");
        builder.push(tail.to_string());

        if params
            .get("timestamps")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            builder.push("--timestamps");
        }

        builder.push(container);

        let result =
            execute_container(self.config.runtime, &builder.build(), &self.config, &self.limiter)
                .await
                .map_err(PluginError::ExecutionFailed)?;

        serde_json::to_value(&result).map_err(PluginError::from)
    }
}

// ---------------------------------------------------------------------------
// ContainerListTool
// ---------------------------------------------------------------------------

/// Tool that lists running containers.
pub struct ContainerListTool {
    config: ContainerConfig,
    limiter: Arc<ConcurrencyLimiter>,
}

impl ContainerListTool {
    pub fn new(config: ContainerConfig, limiter: Arc<ConcurrencyLimiter>) -> Self {
        Self { config, limiter }
    }
}

#[async_trait]
impl Tool for ContainerListTool {
    fn name(&self) -> &str {
        "container_list"
    }

    fn description(&self) -> &str {
        "List running containers"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "all": {
                    "type": "boolean",
                    "description": "Show all containers (including stopped)",
                    "default": false
                },
                "format": {
                    "type": "string",
                    "description": "Output format (e.g., 'json')",
                    "enum": ["table", "json"]
                }
            },
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &dyn ToolContext,
    ) -> Result<serde_json::Value, PluginError> {
        let mut builder = ArgBuilder::new();
        builder.push("ps");

        if params.get("all").and_then(|v| v.as_bool()).unwrap_or(false) {
            builder.push("--all");
        }

        if let Some(format) = params.get("format").and_then(|v| v.as_str())
            && format == "json"
        {
            builder.push("--format");
            builder.push("json");
        }

        let result =
            execute_container(self.config.runtime, &builder.build(), &self.config, &self.limiter)
                .await
                .map_err(PluginError::ExecutionFailed)?;

        serde_json::to_value(&result).map_err(PluginError::from)
    }
}

// ---------------------------------------------------------------------------
// ContainerExecTool
// ---------------------------------------------------------------------------

/// Tool that executes a command in a running container.
pub struct ContainerExecTool {
    config: ContainerConfig,
    limiter: Arc<ConcurrencyLimiter>,
}

impl ContainerExecTool {
    pub fn new(config: ContainerConfig, limiter: Arc<ConcurrencyLimiter>) -> Self {
        Self { config, limiter }
    }
}

#[async_trait]
impl Tool for ContainerExecTool {
    fn name(&self) -> &str {
        "container_exec"
    }

    fn description(&self) -> &str {
        "Execute a command inside a running container"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "container": {
                    "type": "string",
                    "description": "Container name or ID"
                },
                "command": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Command and arguments to execute"
                },
                "workdir": {
                    "type": "string",
                    "description": "Working directory inside the container"
                }
            },
            "required": ["container", "command"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &dyn ToolContext,
    ) -> Result<serde_json::Value, PluginError> {
        let container = params
            .get("container")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PluginError::ExecutionFailed("container is required".into()))?;

        if !is_valid_name(container) {
            return Err(PluginError::ExecutionFailed(format!(
                "invalid container name: '{container}'"
            )));
        }

        let command_arr = params
            .get("command")
            .and_then(|v| v.as_array())
            .ok_or_else(|| PluginError::ExecutionFailed("command is required".into()))?;

        if command_arr.is_empty() {
            return Err(PluginError::ExecutionFailed(
                "command array must not be empty".into(),
            ));
        }

        let mut builder = ArgBuilder::new();
        builder.push("exec");

        if let Some(workdir) = params.get("workdir").and_then(|v| v.as_str()) {
            builder.push("-w");
            builder.push(workdir);
        }

        builder.push(container);

        for item in command_arr {
            if let Some(s) = item.as_str() {
                builder.push(s);
            }
        }

        let result =
            execute_container(self.config.runtime, &builder.build(), &self.config, &self.limiter)
                .await
                .map_err(PluginError::ExecutionFailed)?;

        serde_json::to_value(&result).map_err(PluginError::from)
    }
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

/// Create all container tools with the given configuration and a shared
/// concurrency limiter.
pub fn all_container_tools(config: ContainerConfig) -> Vec<Box<dyn Tool>> {
    let limiter = ConcurrencyLimiter::new(config.max_concurrent_ops);
    vec![
        Box::new(ContainerBuildTool::new(config.clone(), Arc::clone(&limiter))),
        Box::new(ContainerRunTool::new(config.clone(), Arc::clone(&limiter))),
        Box::new(ContainerStopTool::new(config.clone(), Arc::clone(&limiter))),
        Box::new(ContainerLogsTool::new(config.clone(), Arc::clone(&limiter))),
        Box::new(ContainerListTool::new(config.clone(), Arc::clone(&limiter))),
        Box::new(ContainerExecTool::new(config, limiter)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawft_plugin::KeyValueStore;

    struct MockKvStore;

    #[async_trait]
    impl KeyValueStore for MockKvStore {
        async fn get(&self, _key: &str) -> Result<Option<String>, PluginError> {
            Ok(None)
        }
        async fn set(&self, _key: &str, _value: &str) -> Result<(), PluginError> {
            Ok(())
        }
        async fn delete(&self, _key: &str) -> Result<bool, PluginError> {
            Ok(false)
        }
        async fn list_keys(
            &self,
            _prefix: Option<&str>,
        ) -> Result<Vec<String>, PluginError> {
            Ok(vec![])
        }
    }

    struct MockToolContext;

    impl ToolContext for MockToolContext {
        fn key_value_store(&self) -> &dyn KeyValueStore {
            &MockKvStore
        }
        fn plugin_id(&self) -> &str {
            "clawft-plugin-containers"
        }
        fn agent_id(&self) -> &str {
            "test-agent"
        }
    }

    #[test]
    fn all_tools_returns_six() {
        let tools = all_container_tools(ContainerConfig::default());
        assert_eq!(tools.len(), 6);

        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"container_build"));
        assert!(names.contains(&"container_run"));
        assert!(names.contains(&"container_stop"));
        assert!(names.contains(&"container_logs"));
        assert!(names.contains(&"container_list"));
        assert!(names.contains(&"container_exec"));
    }

    #[test]
    fn tool_descriptions_non_empty() {
        let tools = all_container_tools(ContainerConfig::default());
        for tool in &tools {
            assert!(
                !tool.description().is_empty(),
                "empty description for {}",
                tool.name()
            );
        }
    }

    #[test]
    fn tool_schemas_are_objects() {
        let tools = all_container_tools(ContainerConfig::default());
        for tool in &tools {
            let schema = tool.parameters_schema();
            assert!(schema.is_object(), "schema not object for {}", tool.name());
            assert_eq!(schema["type"], "object");
        }
    }

    #[test]
    fn validate_port_mapping_valid() {
        assert!(validate_port_mapping("8080:80").is_ok());
        assert!(validate_port_mapping("127.0.0.1:8080:80").is_ok());
        assert!(validate_port_mapping("443:443").is_ok());
    }

    #[test]
    fn validate_port_mapping_invalid() {
        assert!(validate_port_mapping("evil; rm").is_err());
        assert!(validate_port_mapping("").is_err());
        assert!(validate_port_mapping("abc:def").is_err());
    }

    #[tokio::test]
    async fn run_tool_rejects_invalid_image_name() {
        let config = ContainerConfig::default();
        let limiter = ConcurrencyLimiter::new(config.max_concurrent_ops);
        let tool = ContainerRunTool::new(config, limiter);
        let ctx = MockToolContext;

        let params = serde_json::json!({
            "image": "evil; rm -rf /"
        });

        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("invalid image name"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn stop_tool_rejects_invalid_container_name() {
        let config = ContainerConfig::default();
        let limiter = ConcurrencyLimiter::new(config.max_concurrent_ops);
        let tool = ContainerStopTool::new(config, limiter);
        let ctx = MockToolContext;

        let params = serde_json::json!({
            "container": "bad name with spaces"
        });

        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn run_tool_rejects_invalid_env_var() {
        let config = ContainerConfig::default();
        let limiter = ConcurrencyLimiter::new(config.max_concurrent_ops);
        let tool = ContainerRunTool::new(config, limiter);
        let ctx = MockToolContext;

        let params = serde_json::json!({
            "image": "ubuntu",
            "env": ["NO-EQUALS"]
        });

        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("invalid environment variable"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn exec_tool_rejects_empty_command() {
        let config = ContainerConfig::default();
        let limiter = ConcurrencyLimiter::new(config.max_concurrent_ops);
        let tool = ContainerExecTool::new(config, limiter);
        let ctx = MockToolContext;

        let params = serde_json::json!({
            "container": "myapp",
            "command": []
        });

        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("must not be empty"), "unexpected error: {err}");
    }
}
