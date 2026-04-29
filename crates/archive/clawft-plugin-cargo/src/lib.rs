//! Cargo/build integration tool plugin for clawft.
//!
//! Provides tools for running cargo commands (`build`, `test`, `clippy`,
//! `check`, `publish`) as subprocess invocations via `tokio::process::Command`.
//!
//! # Security
//!
//! All command arguments are validated and constructed programmatically.
//! No shell interpolation is ever used. Package names and feature flags
//! are validated against a strict character allowlist.
//!
//! # Feature Flag
//!
//! This crate is gated behind the workspace `plugin-cargo` feature flag.

pub mod operations;
pub mod types;

use async_trait::async_trait;
use clawft_plugin::{PluginError, Tool, ToolContext};

use operations::execute_cargo;
use types::{CargoConfig, CargoFlags, CargoSubcommand};

/// A tool that runs a specific cargo subcommand.
///
/// Each instance wraps one subcommand (build, test, clippy, check, publish).
/// Multiple `CargoTool` instances are registered, one per operation.
pub struct CargoTool {
    subcommand: CargoSubcommand,
    config: CargoConfig,
}

impl CargoTool {
    /// Create a new cargo tool for the given subcommand.
    pub fn new(subcommand: CargoSubcommand, config: CargoConfig) -> Self {
        Self { subcommand, config }
    }

    /// Create all cargo tools with default configuration.
    pub fn all_tools() -> Vec<Self> {
        Self::all_tools_with_config(CargoConfig::default())
    }

    /// Create all cargo tools with the given configuration.
    pub fn all_tools_with_config(config: CargoConfig) -> Vec<Self> {
        vec![
            Self::new(CargoSubcommand::Build, config.clone()),
            Self::new(CargoSubcommand::Test, config.clone()),
            Self::new(CargoSubcommand::Clippy, config.clone()),
            Self::new(CargoSubcommand::Check, config.clone()),
            Self::new(CargoSubcommand::Publish, config),
        ]
    }
}

#[async_trait]
impl Tool for CargoTool {
    fn name(&self) -> &str {
        match self.subcommand {
            CargoSubcommand::Build => "cargo_build",
            CargoSubcommand::Test => "cargo_test",
            CargoSubcommand::Clippy => "cargo_clippy",
            CargoSubcommand::Check => "cargo_check",
            CargoSubcommand::Publish => "cargo_publish",
        }
    }

    fn description(&self) -> &str {
        match self.subcommand {
            CargoSubcommand::Build => "Build a Rust project using cargo build",
            CargoSubcommand::Test => "Run tests using cargo test",
            CargoSubcommand::Clippy => "Run clippy lints using cargo clippy",
            CargoSubcommand::Check => "Type-check a Rust project using cargo check",
            CargoSubcommand::Publish => "Publish a crate to crates.io using cargo publish",
        }
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "working_dir": {
                    "type": "string",
                    "description": "Working directory for the cargo command"
                },
                "release": {
                    "type": "boolean",
                    "description": "Build in release mode",
                    "default": false
                },
                "workspace": {
                    "type": "boolean",
                    "description": "Apply to the entire workspace",
                    "default": false
                },
                "package": {
                    "type": "string",
                    "description": "Target a specific package (must be alphanumeric, -, _)"
                },
                "json_output": {
                    "type": "boolean",
                    "description": "Use JSON message format for structured output (build, check, clippy only)",
                    "default": false
                },
                "features": {
                    "type": "string",
                    "description": "Comma-separated feature list to enable"
                },
                "no_default_features": {
                    "type": "boolean",
                    "description": "Disable default features",
                    "default": false
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
        // Parse and validate flags from parameters
        let mut flags = CargoFlags::from_params(&params)
            .map_err(|e| PluginError::ExecutionFailed(format!("invalid parameters: {e}")))?;

        // Only enable JSON output if the subcommand supports it
        if flags.json_output && !self.subcommand.supports_json_output() {
            flags.json_output = false;
        }

        // Allow overriding working_dir per-invocation
        let mut config = self.config.clone();
        if let Some(dir) = params.get("working_dir").and_then(|v| v.as_str()) {
            config.working_dir = Some(dir.to_string());
        }

        let result = execute_cargo(self.subcommand, &flags, &config)
            .await
            .map_err(PluginError::ExecutionFailed)?;

        serde_json::to_value(&result).map_err(PluginError::from)
    }
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
            "clawft-plugin-cargo"
        }
        fn agent_id(&self) -> &str {
            "test-agent"
        }
    }

    #[test]
    fn all_tools_returns_five() {
        let tools = CargoTool::all_tools();
        assert_eq!(tools.len(), 5);

        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"cargo_build"));
        assert!(names.contains(&"cargo_test"));
        assert!(names.contains(&"cargo_clippy"));
        assert!(names.contains(&"cargo_check"));
        assert!(names.contains(&"cargo_publish"));
    }

    #[test]
    fn tool_descriptions_non_empty() {
        let tools = CargoTool::all_tools();
        for tool in &tools {
            assert!(!tool.description().is_empty(), "empty description for {}", tool.name());
        }
    }

    #[test]
    fn tool_schemas_are_objects() {
        let tools = CargoTool::all_tools();
        for tool in &tools {
            let schema = tool.parameters_schema();
            assert!(schema.is_object(), "schema not object for {}", tool.name());
            assert_eq!(schema["type"], "object");
        }
    }

    #[tokio::test]
    async fn cargo_check_runs_successfully() {
        // Integration test: only runs if cargo is available
        let tool = CargoTool::new(CargoSubcommand::Check, CargoConfig::default());
        let ctx = MockToolContext;

        // Run cargo check on the workspace root
        let params = serde_json::json!({
            "working_dir": env!("CARGO_MANIFEST_DIR")
        });

        let result = tool.execute(params, &ctx).await.unwrap();
        // cargo check on this crate should succeed
        assert!(result["success"].as_bool().unwrap_or(false));
    }

    #[tokio::test]
    async fn rejects_invalid_package_name() {
        let tool = CargoTool::new(CargoSubcommand::Build, CargoConfig::default());
        let ctx = MockToolContext;

        let params = serde_json::json!({
            "package": "evil; rm -rf /"
        });

        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("invalid"), "unexpected error: {err}");
    }
}
