//! Unified persistence coordinator for kernel state.
//!
//! Provides a single entry point to save and restore all kernel
//! subsystems (CausalGraph, HNSW index, ExoChain) to a data directory.
//! Uses file-based JSON persistence — no external database required.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::causal::CausalGraph;
use crate::hnsw_service::{HnswService, HnswServiceConfig};

/// Configuration for the persistence coordinator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistenceConfig {
    /// Root directory for all persisted state.
    pub data_dir: PathBuf,
    /// If set, auto-save interval in seconds (for future use with a
    /// background timer).
    pub auto_save_interval_secs: Option<u64>,
}

impl Default for PersistenceConfig {
    fn default() -> Self {
        Self {
            data_dir: PathBuf::from(".weftos/state"),
            auto_save_interval_secs: None,
        }
    }
}

impl PersistenceConfig {
    /// Path for the causal graph snapshot.
    pub fn causal_graph_path(&self) -> PathBuf {
        self.data_dir.join("causal_graph.json")
    }

    /// Path for the HNSW index snapshot.
    pub fn hnsw_index_path(&self) -> PathBuf {
        self.data_dir.join("hnsw_index.json")
    }

    /// Path for the ExoChain snapshot.
    pub fn chain_path(&self) -> PathBuf {
        self.data_dir.join("exochain.jsonl")
    }
}

/// Save the causal graph to the configured data directory.
pub fn save_causal_graph(
    config: &PersistenceConfig,
    graph: &CausalGraph,
) -> Result<(), std::io::Error> {
    graph.save_to_file(&config.causal_graph_path())
}

/// Load a causal graph from the configured data directory.
///
/// Returns a new empty graph if the file does not exist.
pub fn load_causal_graph(config: &PersistenceConfig) -> Result<CausalGraph, std::io::Error> {
    let path = config.causal_graph_path();
    if !path.exists() {
        return Ok(CausalGraph::new());
    }
    CausalGraph::load_from_file(&path)
}

/// Save the HNSW service state to the configured data directory.
pub fn save_hnsw(config: &PersistenceConfig, service: &HnswService) -> Result<(), std::io::Error> {
    service.save_to_file(&config.hnsw_index_path())
}

/// Load an HNSW service from the configured data directory.
///
/// Returns a new empty service if the file does not exist.
pub fn load_hnsw(config: &PersistenceConfig) -> Result<HnswService, std::io::Error> {
    let path = config.hnsw_index_path();
    if !path.exists() {
        return Ok(HnswService::new(HnswServiceConfig::default()));
    }
    HnswService::load_from_file(&path)
}

/// Save all kernel state to the configured data directory.
pub fn save_all(
    config: &PersistenceConfig,
    graph: &CausalGraph,
    hnsw: &HnswService,
) -> Result<(), std::io::Error> {
    std::fs::create_dir_all(&config.data_dir)?;
    save_causal_graph(config, graph)?;
    save_hnsw(config, hnsw)?;
    Ok(())
}

/// Save all kernel state, logging the event to the exochain.
#[cfg(feature = "exochain")]
pub fn save_all_with_chain(
    config: &PersistenceConfig,
    graph: &CausalGraph,
    hnsw: &HnswService,
    cm: &crate::chain::ChainManager,
) -> Result<(), std::io::Error> {
    cm.append(
        "persistence",
        crate::chain::EVENT_KIND_KERNEL_SAVE,
        Some(serde_json::json!({
            "data_dir": config.data_dir.display().to_string(),
            "node_count": graph.node_count(),
            "hnsw_count": hnsw.len(),
        })),
    );
    save_all(config, graph, hnsw)
}

/// Restore all kernel state from the configured data directory.
///
/// Components that have no saved state are returned as fresh instances.
pub fn load_all(config: &PersistenceConfig) -> Result<(CausalGraph, HnswService), std::io::Error> {
    let graph = load_causal_graph(config)?;
    let hnsw = load_hnsw(config)?;
    Ok((graph, hnsw))
}

/// Restore all kernel state, logging the event to the exochain.
#[cfg(feature = "exochain")]
pub fn load_all_with_chain(
    config: &PersistenceConfig,
    cm: &crate::chain::ChainManager,
) -> Result<(CausalGraph, HnswService), std::io::Error> {
    let result = load_all(config)?;
    cm.append(
        "persistence",
        crate::chain::EVENT_KIND_KERNEL_LOAD,
        Some(serde_json::json!({
            "data_dir": config.data_dir.display().to_string(),
            "node_count": result.0.node_count(),
            "hnsw_count": result.1.len(),
        })),
    );
    Ok(result)
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_config() -> PersistenceConfig {
        let dir = std::env::temp_dir().join(format!(
            "weftos_persist_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        PersistenceConfig {
            data_dir: dir,
            auto_save_interval_secs: None,
        }
    }

    #[test]
    fn config_paths() {
        let cfg = PersistenceConfig {
            data_dir: PathBuf::from("/tmp/test"),
            auto_save_interval_secs: None,
        };
        assert_eq!(
            cfg.causal_graph_path(),
            PathBuf::from("/tmp/test/causal_graph.json")
        );
        assert_eq!(
            cfg.hnsw_index_path(),
            PathBuf::from("/tmp/test/hnsw_index.json")
        );
        assert_eq!(cfg.chain_path(), PathBuf::from("/tmp/test/exochain.jsonl"));
    }

    #[test]
    fn load_missing_returns_defaults() {
        let cfg = tmp_config();
        let graph = load_causal_graph(&cfg).unwrap();
        assert_eq!(graph.node_count(), 0);
        let hnsw = load_hnsw(&cfg).unwrap();
        assert!(hnsw.is_empty());
    }

    #[test]
    fn save_and_load_all_roundtrip() {
        let cfg = tmp_config();

        let graph = CausalGraph::new();
        let a = graph.add_node("A".into(), serde_json::json!({"x": 1}));
        let b = graph.add_node("B".into(), serde_json::json!({}));
        graph.link(a, b, crate::causal::CausalEdgeType::Causes, 0.9, 100, 1);

        let hnsw = HnswService::new(HnswServiceConfig::default());
        hnsw.insert(
            "v1".into(),
            vec![1.0, 0.0, 0.0],
            serde_json::json!({"tag": "first"}),
        );

        save_all(&cfg, &graph, &hnsw).unwrap();

        let (loaded_graph, loaded_hnsw) = load_all(&cfg).unwrap();
        assert_eq!(loaded_graph.node_count(), 2);
        assert_eq!(loaded_graph.edge_count(), 1);
        assert_eq!(loaded_hnsw.len(), 1);

        // Cleanup.
        let _ = std::fs::remove_dir_all(&cfg.data_dir);
    }

    // ── Corrupt file recovery ───────────────────────────────────────

    #[test]
    fn corrupt_causal_graph_file_returns_error() {
        let cfg = tmp_config();
        std::fs::create_dir_all(&cfg.data_dir).unwrap();

        // Write garbage bytes to the causal graph file.
        std::fs::write(cfg.causal_graph_path(), b"{{{{not json at all!").unwrap();

        let result = load_causal_graph(&cfg);
        assert!(result.is_err(), "loading corrupt causal graph should fail");
        let err = result.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);

        let _ = std::fs::remove_dir_all(&cfg.data_dir);
    }

    #[test]
    fn corrupt_hnsw_file_returns_error() {
        let cfg = tmp_config();
        std::fs::create_dir_all(&cfg.data_dir).unwrap();

        // Write garbage bytes to the HNSW file.
        std::fs::write(cfg.hnsw_index_path(), b"\x00\x01\x02binary garbage").unwrap();

        let result = load_hnsw(&cfg);
        assert!(result.is_err(), "loading corrupt HNSW index should fail");
        let err = result.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);

        let _ = std::fs::remove_dir_all(&cfg.data_dir);
    }

    #[test]
    fn truncated_json_returns_error() {
        let cfg = tmp_config();
        std::fs::create_dir_all(&cfg.data_dir).unwrap();

        // Write truncated but plausible JSON.
        std::fs::write(cfg.causal_graph_path(), b"{\"next_node_id\":5,\"nodes\":").unwrap();

        let result = load_causal_graph(&cfg);
        assert!(result.is_err(), "loading truncated JSON should fail");

        let _ = std::fs::remove_dir_all(&cfg.data_dir);
    }

    #[test]
    fn empty_file_returns_error() {
        let cfg = tmp_config();
        std::fs::create_dir_all(&cfg.data_dir).unwrap();

        // Write zero-length file.
        std::fs::write(cfg.causal_graph_path(), b"").unwrap();

        let result = load_causal_graph(&cfg);
        assert!(result.is_err(), "loading empty file should fail");

        let _ = std::fs::remove_dir_all(&cfg.data_dir);
    }

    // ── Concurrent write handling ───────────────────────────────────

    #[test]
    fn concurrent_saves_do_not_corrupt() {
        let cfg = tmp_config();
        std::fs::create_dir_all(&cfg.data_dir).unwrap();

        let graph = CausalGraph::new();
        for i in 0..100 {
            graph.add_node(format!("node-{i}"), serde_json::json!({"i": i}));
        }
        let hnsw = HnswService::new(HnswServiceConfig::default());
        for i in 0..50 {
            hnsw.insert(
                format!("v{i}"),
                vec![i as f32, 0.0, 0.0],
                serde_json::json!({}),
            );
        }

        // Save from two threads simultaneously.
        let cfg1 = cfg.clone();
        let cfg2 = cfg.clone();
        let graph_ref = &graph;
        let hnsw_ref = &hnsw;

        std::thread::scope(|s| {
            let h1 = s.spawn(|| save_all(&cfg1, graph_ref, hnsw_ref));
            let h2 = s.spawn(|| save_all(&cfg2, graph_ref, hnsw_ref));

            // Both saves should succeed (last-writer-wins on file I/O).
            h1.join().unwrap().unwrap();
            h2.join().unwrap().unwrap();
        });

        // The resulting files should be loadable.
        let (loaded_graph, loaded_hnsw) = load_all(&cfg).unwrap();
        assert_eq!(loaded_graph.node_count(), 100);
        assert_eq!(loaded_hnsw.len(), 50);

        let _ = std::fs::remove_dir_all(&cfg.data_dir);
    }

    // ── Disk-full / read-only simulation ────────────────────────────

    #[test]
    fn save_to_nonexistent_deep_path_creates_dirs() {
        let cfg = PersistenceConfig {
            data_dir: std::env::temp_dir()
                .join(format!(
                    "weftos_deep_{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ))
                .join("a")
                .join("b")
                .join("c"),
            auto_save_interval_secs: None,
        };

        let graph = CausalGraph::new();
        let hnsw = HnswService::new(HnswServiceConfig::default());

        // save_all creates intermediate directories.
        save_all(&cfg, &graph, &hnsw).unwrap();
        assert!(cfg.causal_graph_path().exists());

        let _ = std::fs::remove_dir_all(
            cfg.data_dir
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .parent()
                .unwrap(),
        );
    }

    #[test]
    fn save_to_readonly_dir_fails() {
        // Create a directory, then make it read-only.
        let base = std::env::temp_dir().join(format!(
            "weftos_ro_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&base).unwrap();

        // Set directory permissions to read-only.
        let mut perms = std::fs::metadata(&base).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(true);
        std::fs::set_permissions(&base, perms.clone()).unwrap();

        let cfg = PersistenceConfig {
            data_dir: base.join("state"),
            auto_save_interval_secs: None,
        };

        let graph = CausalGraph::new();
        let hnsw = HnswService::new(HnswServiceConfig::default());

        let result = save_all(&cfg, &graph, &hnsw);
        assert!(result.is_err(), "saving to read-only dir should fail");

        // Restore permissions for cleanup.
        perms.set_readonly(false);
        let _ = std::fs::set_permissions(&base, perms);
        let _ = std::fs::remove_dir_all(&base);
    }

    // ── Large data roundtrip ────────────────────────────────────────

    #[test]
    fn save_load_roundtrip_large_graph() {
        let cfg = tmp_config();

        let graph = CausalGraph::new();
        let mut node_ids = Vec::with_capacity(1000);
        for i in 0..1000 {
            let nid = graph.add_node(
                format!("node-{i}"),
                serde_json::json!({"index": i, "data": "x".repeat(50)}),
            );
            node_ids.push(nid);
        }

        // Create edges between consecutive nodes.
        for window in node_ids.windows(2) {
            graph.link(
                window[0],
                window[1],
                crate::causal::CausalEdgeType::Follows,
                0.8,
                window[0],
                0,
            );
        }

        let hnsw = HnswService::new(HnswServiceConfig::default());
        for i in 0..1000 {
            hnsw.insert(
                format!("vec-{i}"),
                vec![i as f32, (i * 2) as f32, (i * 3) as f32],
                serde_json::json!({"i": i}),
            );
        }

        save_all(&cfg, &graph, &hnsw).unwrap();

        let (loaded_graph, loaded_hnsw) = load_all(&cfg).unwrap();
        assert_eq!(loaded_graph.node_count(), 1000);
        assert_eq!(loaded_graph.edge_count(), 999);
        assert_eq!(loaded_hnsw.len(), 1000);

        let _ = std::fs::remove_dir_all(&cfg.data_dir);
    }

    #[test]
    fn save_load_roundtrip_preserves_node_data() {
        let cfg = tmp_config();

        let graph = CausalGraph::new();
        let nid = graph.add_node(
            "important-node".into(),
            serde_json::json!({"key": "value", "nested": {"a": [1,2,3]}}),
        );

        let hnsw = HnswService::new(HnswServiceConfig::default());

        save_all(&cfg, &graph, &hnsw).unwrap();

        let (loaded_graph, _) = load_all(&cfg).unwrap();
        assert_eq!(loaded_graph.node_count(), 1);
        // Verify the node data is intact by checking we can retrieve by ID.
        let nodes = loaded_graph.get_node(nid);
        assert!(
            nodes.is_some(),
            "loaded graph should contain the saved node"
        );
        assert_eq!(nodes.unwrap().label, "important-node");

        let _ = std::fs::remove_dir_all(&cfg.data_dir);
    }

    #[test]
    fn double_save_load_is_idempotent() {
        let cfg = tmp_config();

        let graph = CausalGraph::new();
        graph.add_node("A".into(), serde_json::json!({}));
        let hnsw = HnswService::new(HnswServiceConfig::default());

        save_all(&cfg, &graph, &hnsw).unwrap();
        save_all(&cfg, &graph, &hnsw).unwrap();

        let (loaded, _) = load_all(&cfg).unwrap();
        assert_eq!(loaded.node_count(), 1);

        let _ = std::fs::remove_dir_all(&cfg.data_dir);
    }
}
