//! Merged config loading with 3-level precedence.
//!
//! Implements the config merge strategy:
//! 1. Start with [`Config::default()`]
//! 2. Merge `~/.clawft/config.json` (global overrides)
//! 3. Merge `<workspace>/.clawft/config.json` (workspace overrides)
//!
//! Both `camelCase` and `snake_case` keys are supported via normalization.

use std::path::Path;

use clawft_types::config::Config;
use clawft_types::{ClawftError, Result};

use crate::config_merge::{deep_merge, normalize_keys};

/// Load a config file as a raw JSON [`serde_json::Value`].
///
/// Returns `None` if the file does not exist.
fn load_config_file(path: &Path) -> Option<serde_json::Value> {
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Load config with 3-level merge: defaults < global < workspace.
///
/// 1. Start with [`Config::default()`] serialized to JSON.
/// 2. Merge `~/.clawft/config.json` (global overrides).
/// 3. Merge `<workspace>/.clawft/config.json` (workspace overrides).
/// 4. Deserialize the merged JSON back into [`Config`].
pub fn load_merged_config(workspace_path: Option<&Path>) -> Result<Config> {
    #[cfg(feature = "native")]
    let global_config = dirs::home_dir().map(|h| h.join(".clawft").join("config.json"));
    #[cfg(not(feature = "native"))]
    let global_config: Option<std::path::PathBuf> = None;
    load_merged_config_from(global_config.as_deref(), workspace_path)
}

/// Load config with 3-level merge from explicit paths.
///
/// This is the internal implementation that accepts explicit file paths
/// for both global and workspace config, enabling deterministic testing.
pub fn load_merged_config_from(
    global_config_path: Option<&Path>,
    workspace_path: Option<&Path>,
) -> Result<Config> {
    let defaults = Config::default();
    let mut merged = serde_json::to_value(&defaults).map_err(|e| ClawftError::ConfigInvalid {
        reason: format!("failed to serialize defaults: {e}"),
    })?;

    // Global config
    if let Some(gp) = global_config_path
        && let Some(mut global) = load_config_file(gp)
    {
        normalize_keys(&mut global);
        deep_merge(&mut merged, &global);
    }

    // Workspace config: <workspace>/.clawft/config.json
    if let Some(ws_path) = workspace_path
        && let Some(mut ws_config) = load_config_file(&ws_path.join(".clawft").join("config.json"))
    {
        normalize_keys(&mut ws_config);
        deep_merge(&mut merged, &ws_config);
    }

    let config: Config = serde_json::from_value(merged).map_err(ClawftError::Json)?;

    // Chain event marker for workspace config load/merge.
    crate::chain_event!(
        "workspace",
        crate::chain_event::EVENT_KIND_WORKSPACE_CONFIG,
        {
            "global_path": global_config_path.map(|p| p.display().to_string()).unwrap_or_default(),
            "workspace_path": workspace_path.map(|p| p.display().to_string()).unwrap_or_default()
        }
    );

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn load_merged_config_defaults_only() {
        let config = load_merged_config_from(None, None).unwrap();
        assert_eq!(config.agents.defaults.model, "deepseek/deepseek-chat");
        assert_eq!(config.agents.defaults.max_tokens, 8192);
    }

    #[test]
    fn load_merged_config_workspace_overrides() {
        let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("clawft-test-merge-ws-{n}"));
        let _ = std::fs::remove_dir_all(&dir);
        let dot_clawft = dir.join(".clawft");
        std::fs::create_dir_all(&dot_clawft).unwrap();

        let ws_config = r#"{"agents": {"defaults": {"max_tokens": 4096}}}"#;
        std::fs::write(dot_clawft.join("config.json"), ws_config).unwrap();

        let config = load_merged_config_from(None, Some(&dir)).unwrap();
        assert_eq!(config.agents.defaults.max_tokens, 4096);
        assert_eq!(config.agents.defaults.model, "deepseek/deepseek-chat");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_merged_config_global_overrides() {
        let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("clawft-test-merge-global-{n}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let global_config = r#"{"agents": {"defaults": {"model": "custom/model"}}}"#;
        let global_path = dir.join("global-config.json");
        std::fs::write(&global_path, global_config).unwrap();

        let config = load_merged_config_from(Some(&global_path), None).unwrap();
        assert_eq!(config.agents.defaults.model, "custom/model");
        assert_eq!(config.agents.defaults.max_tokens, 8192);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_merged_config_workspace_over_global() {
        let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("clawft-test-merge-both-{n}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let global_config =
            r#"{"agents": {"defaults": {"model": "global/model", "max_tokens": 2048}}}"#;
        let global_path = dir.join("global-config.json");
        std::fs::write(&global_path, global_config).unwrap();

        let ws_dir = dir.join("workspace");
        let dot_clawft = ws_dir.join(".clawft");
        std::fs::create_dir_all(&dot_clawft).unwrap();
        let ws_config = r#"{"agents": {"defaults": {"max_tokens": 4096}}}"#;
        std::fs::write(dot_clawft.join("config.json"), ws_config).unwrap();

        let config = load_merged_config_from(Some(&global_path), Some(&ws_dir)).unwrap();
        assert_eq!(config.agents.defaults.model, "global/model");
        assert_eq!(config.agents.defaults.max_tokens, 4096);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_merged_config_missing_workspace_config() {
        let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("clawft-test-merge-missing-{n}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let config = load_merged_config_from(None, Some(&dir)).unwrap();
        assert_eq!(config.agents.defaults.max_tokens, 8192);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_merged_config_normalizes_keys() {
        let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("clawft-test-merge-normalize-{n}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let global_config = r#"{"agents": {"defaults": {"max_tokens": 1000}}}"#;
        let global_path = dir.join("global-config.json");
        std::fs::write(&global_path, global_config).unwrap();

        let ws_dir = dir.join("workspace");
        let dot_clawft = ws_dir.join(".clawft");
        std::fs::create_dir_all(&dot_clawft).unwrap();
        let ws_config = r#"{"agents": {"defaults": {"maxTokens": 2000}}}"#;
        std::fs::write(dot_clawft.join("config.json"), ws_config).unwrap();

        let config = load_merged_config_from(Some(&global_path), Some(&ws_dir)).unwrap();
        assert_eq!(config.agents.defaults.max_tokens, 2000);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_merged_config_mcp_servers() {
        let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("clawft-test-merge-mcp-{n}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let global_config = r#"{
            "tools": {
                "mcpServers": {
                    "github": {"command": "npx", "args": ["-y", "github-mcp"]},
                    "slack": {"command": "npx", "args": ["-y", "slack-mcp"]}
                }
            }
        }"#;
        let global_path = dir.join("global-config.json");
        std::fs::write(&global_path, global_config).unwrap();

        let ws_dir = dir.join("workspace");
        let dot_clawft = ws_dir.join(".clawft");
        std::fs::create_dir_all(&dot_clawft).unwrap();
        let ws_config = r#"{
            "tools": {
                "mcpServers": {
                    "rvf": {"command": "npx", "args": ["-y", "rvf-mcp"]},
                    "slack": null
                }
            }
        }"#;
        std::fs::write(dot_clawft.join("config.json"), ws_config).unwrap();

        let config = load_merged_config_from(Some(&global_path), Some(&ws_dir)).unwrap();

        assert!(
            config.tools.mcp_servers.contains_key("github"),
            "github server should be preserved"
        );
        assert_eq!(config.tools.mcp_servers["github"].command, "npx");

        assert!(
            config.tools.mcp_servers.contains_key("rvf"),
            "rvf server should be added"
        );
        assert_eq!(config.tools.mcp_servers["rvf"].command, "npx");

        assert!(
            !config.tools.mcp_servers.contains_key("slack"),
            "slack server should be removed by null overlay"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
