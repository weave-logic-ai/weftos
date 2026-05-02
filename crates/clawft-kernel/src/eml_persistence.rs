//! EML model persistence — save/load all 12+ EML models to `.weftos/eml-models/`.
//!
//! Provides [`save_all`] and [`load_all`] functions used during kernel
//! shutdown and boot respectively. Models are stored as JSON files for
//! portability and debuggability.
//!
//! # Model Inventory
//!
//! | File | Source Crate | Model |
//! |------|-------------|-------|
//! | `coherence.json` | clawft-kernel | EmlCoherenceModel |
//! | `governance_scorer.json` | clawft-kernel | GovernanceScorerModel |
//! | `restart_strategy.json` | clawft-kernel | RestartStrategyModel |
//! | `health_threshold.json` | clawft-kernel | HealthThresholdModel |
//! | `dead_letter.json` | clawft-kernel | DeadLetterModel |
//! | `gossip_timing.json` | clawft-kernel | GossipTimingModel |
//! | `complexity.json` | clawft-kernel | ComplexityModel |
//! | `hnsw_distance.json` | clawft-kernel | HnswEmlManager (distance) |
//! | `hnsw_ef.json` | clawft-kernel | HnswEmlManager (ef) |
//! | `hnsw_path.json` | clawft-kernel | HnswEmlManager (path) |
//! | `hnsw_rebuild.json` | clawft-kernel | HnswEmlManager (rebuild) |

use std::path::{Path, PathBuf};

use tracing::{debug, info, warn};

use crate::eml_coherence::EmlCoherenceModel;
use crate::eml_kernel::{
    ComplexityModel, DeadLetterModel, GovernanceScorerModel, GossipTimingModel,
    HealthThresholdModel, RestartStrategyModel,
};

// ---------------------------------------------------------------------------
// Directory
// ---------------------------------------------------------------------------

/// Default persistence directory: `.weftos/eml-models/`
///
/// Located relative to the runtime directory (typically the project root
/// or `$WEFTOS_RUNTIME_DIR`).
pub fn eml_models_dir(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join(".weftos").join("eml-models")
}

// ---------------------------------------------------------------------------
// Kernel EML Models Bundle
// ---------------------------------------------------------------------------

/// Bundle of all kernel-level EML models for save/load.
///
/// This does NOT include graphify, LLM, or benchmark models -- those
/// are persisted by their respective crates. This bundle covers the 7
/// models owned by `clawft-kernel`.
#[derive(Default)]
pub struct KernelEmlModels {
    pub coherence: EmlCoherenceModel,
    pub governance_scorer: GovernanceScorerModel,
    pub restart_strategy: RestartStrategyModel,
    pub health_threshold: HealthThresholdModel,
    pub dead_letter: DeadLetterModel,
    pub gossip_timing: GossipTimingModel,
    pub complexity: ComplexityModel,
}


// ---------------------------------------------------------------------------
// Save/Load helpers
// ---------------------------------------------------------------------------

/// Save a single serde-serializable model to a JSON file.
fn save_model<T: serde::Serialize>(dir: &Path, filename: &str, model: &T) -> std::io::Result<()> {
    let path = dir.join(filename);
    let json = serde_json::to_string_pretty(model)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&path, json)?;
    debug!(path = %path.display(), "saved EML model");
    Ok(())
}

/// Load a single serde-deserializable model from a JSON file.
///
/// Returns `None` if the file doesn't exist or is malformed.
fn load_model<T: serde::de::DeserializeOwned>(dir: &Path, filename: &str) -> Option<T> {
    let path = dir.join(filename);
    if !path.exists() {
        return None;
    }
    match std::fs::read_to_string(&path) {
        Ok(data) => match serde_json::from_str::<T>(&data) {
            Ok(model) => {
                debug!(path = %path.display(), "loaded EML model");
                Some(model)
            }
            Err(e) => {
                warn!(path = %path.display(), error = %e, "failed to deserialize EML model, using defaults");
                None
            }
        },
        Err(e) => {
            warn!(path = %path.display(), error = %e, "failed to read EML model file");
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Save all kernel EML models to `dir`.
///
/// Creates the directory if it doesn't exist.
pub fn save_all(dir: &Path, models: &KernelEmlModels) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;

    save_model(dir, "coherence.json", &models.coherence)?;
    save_model(dir, "governance_scorer.json", &models.governance_scorer)?;
    save_model(dir, "restart_strategy.json", &models.restart_strategy)?;
    save_model(dir, "health_threshold.json", &models.health_threshold)?;
    save_model(dir, "dead_letter.json", &models.dead_letter)?;
    save_model(dir, "gossip_timing.json", &models.gossip_timing)?;
    save_model(dir, "complexity.json", &models.complexity)?;

    info!(dir = %dir.display(), "saved all kernel EML models");
    Ok(())
}

/// Load all kernel EML models from `dir`.
///
/// Falls back to defaults for any model that can't be loaded.
pub fn load_all(dir: &Path) -> KernelEmlModels {
    if !dir.exists() {
        info!(dir = %dir.display(), "EML model directory does not exist, using defaults");
        return KernelEmlModels::default();
    }

    let models = KernelEmlModels {
        coherence: load_model(dir, "coherence.json").unwrap_or_default(),
        governance_scorer: load_model(dir, "governance_scorer.json").unwrap_or_default(),
        restart_strategy: load_model(dir, "restart_strategy.json").unwrap_or_default(),
        health_threshold: load_model(dir, "health_threshold.json").unwrap_or_default(),
        dead_letter: load_model(dir, "dead_letter.json").unwrap_or_default(),
        gossip_timing: load_model(dir, "gossip_timing.json").unwrap_or_default(),
        complexity: load_model(dir, "complexity.json").unwrap_or_default(),
    };

    info!(dir = %dir.display(), "loaded all kernel EML models");
    models
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_and_load_roundtrip() {
        let dir = std::env::temp_dir().join("eml_persistence_test");
        let _ = std::fs::remove_dir_all(&dir);

        let models = KernelEmlModels::default();
        save_all(&dir, &models).expect("save should succeed");

        let loaded = load_all(&dir);

        // Verify all models loaded with correct trained status
        assert!(!loaded.coherence.is_trained());
        assert!(!loaded.governance_scorer.is_trained());
        assert!(!loaded.restart_strategy.is_trained());
        assert!(!loaded.health_threshold.is_trained());
        assert!(!loaded.dead_letter.is_trained());
        assert!(!loaded.gossip_timing.is_trained());
        assert!(!loaded.complexity.is_trained());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_missing_dir_returns_defaults() {
        let dir = std::env::temp_dir().join("eml_persistence_missing");
        let _ = std::fs::remove_dir_all(&dir);

        let models = load_all(&dir);
        assert!(!models.coherence.is_trained());
    }

    #[test]
    fn load_corrupt_file_returns_default() {
        let dir = std::env::temp_dir().join("eml_persistence_corrupt");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("coherence.json"), "not valid json").unwrap();

        let models = load_all(&dir);
        assert!(!models.coherence.is_trained());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
