//! Command implementations for `weaver`.

pub mod agent_cmd;
pub mod app_cmd;
pub mod bench_cmd;
pub mod bench_eml;
pub mod update_cmd;
pub mod chain_cmd;
pub mod cluster_cmd;
#[cfg(unix)]
pub mod console_cmd;
pub mod custody_cmd;
pub mod ecc_cmd;
pub mod graphify_cmd;
pub mod cron_cmd;
pub mod init_cmd;
pub mod ipc_cmd;
pub mod kernel_cmd;
pub mod leaf_cmd;
pub mod resource_cmd;
pub mod soul_cmd;
pub mod topology_cmd;
pub mod vault_cmd;

use std::path::Path;

use clawft_platform::Platform;
use clawft_types::config::Config;

/// Load configuration from the given path override or via auto-discovery.
pub async fn load_config<P: Platform>(
    platform: &P,
    config_override: Option<&str>,
) -> anyhow::Result<Config> {
    let raw = if let Some(path_str) = config_override {
        let path = Path::new(path_str);
        if !platform.fs().exists(path).await {
            anyhow::bail!("config file not found: {path_str}");
        }
        let contents = platform
            .fs()
            .read_to_string(path)
            .await
            .map_err(|e| anyhow::anyhow!("failed to read config: {e}"))?;
        let value: serde_json::Value = serde_json::from_str(&contents)
            .map_err(|e| anyhow::anyhow!("failed to parse config: {e}"))?;
        clawft_platform::config_loader::normalize_keys(value)
    } else {
        clawft_platform::config_loader::load_config_raw(platform.fs(), platform.env())
            .await
            .map_err(|e| anyhow::anyhow!("failed to load config: {e}"))?
    };

    let config: Config = serde_json::from_value(raw)?;
    Ok(config)
}
