//! Plugin registry types for managing the local plugin index.
//!
//! The registry stores metadata about installed and available plugins
//! in `~/.clawft/plugins/index.json`.
//!
//! Types and helpers here are wired through the `plugin` subcommands
//! in later phases; left in place as the module's intended public
//! surface so the layout doesn't churn when that work lands.

#![allow(dead_code)]

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Plugin metadata for the registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    pub plugin_type: String,
    pub version: String,
    pub description: String,
    pub author: String,
    pub license: String,
    pub weftos_min_version: String,
    pub checksum: Option<String>,
    pub published_at: Option<String>,
}

/// Local plugin index (stored in `~/.clawft/plugins/index.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginIndex {
    pub plugins: Vec<PluginManifest>,
    pub last_updated: String,
}

/// Return the path to the local plugin index file.
fn index_path() -> anyhow::Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("cannot determine home directory"))?;
    Ok(home.join(".clawft").join("plugins").join("index.json"))
}

/// Load the local plugin index, returning an empty index if the file does not
/// exist.
pub fn load_index() -> anyhow::Result<PluginIndex> {
    let path = index_path()?;
    if !path.exists() {
        return Ok(PluginIndex {
            plugins: Vec::new(),
            last_updated: chrono::Utc::now().to_rfc3339(),
        });
    }
    let contents = std::fs::read_to_string(&path)?;
    let index: PluginIndex = serde_json::from_str(&contents)?;
    Ok(index)
}

/// Persist the plugin index to disk.
pub fn save_index(index: &PluginIndex) -> anyhow::Result<()> {
    let path = index_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(index)?;
    std::fs::write(&path, json)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_index_round_trips() {
        let idx = PluginIndex {
            plugins: Vec::new(),
            last_updated: "2026-04-02T00:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&idx).unwrap();
        let parsed: PluginIndex = serde_json::from_str(&json).unwrap();
        assert!(parsed.plugins.is_empty());
    }

    #[test]
    fn manifest_round_trips() {
        let m = PluginManifest {
            name: "test-plugin".into(),
            plugin_type: "analyzer".into(),
            version: "0.1.0".into(),
            description: "A test".into(),
            author: "dev".into(),
            license: "MIT".into(),
            weftos_min_version: "0.4.0".into(),
            checksum: None,
            published_at: None,
        };
        let json = serde_json::to_string(&m).unwrap();
        let parsed: PluginManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "test-plugin");
    }
}
