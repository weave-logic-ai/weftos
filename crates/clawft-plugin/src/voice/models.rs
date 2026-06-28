//! Model download and cache management.
//!
//! Downloads STT/TTS/VAD models, verifies SHA-256 integrity,
//! and caches them in the local filesystem.
//!
//! # Integrity check (WEFT-209 / SC-7)
//!
//! Model bundles MUST ship with a signed manifest
//! (`model.manifest.json` + `model.manifest.sig`) verified by
//! `clawft-service-whisper::manifest::verify_model_dir` against a
//! trusted public key under `~/.clawft/trust-roots/voice/`. The
//! manifest carries the per-file SHA-256 hashes — they are NOT baked
//! into the source tree any more (the previous "PLACEHOLDER_SHA256_*"
//! strings were never actually checked at runtime and gave a false
//! sense of integrity).
//!
//! [`ModelInfo::sha256_hint`] is therefore an `Option<String>`:
//! `None` for entries where the manifest is the source of truth (the
//! sherpa-onnx archives, which are sub-bundles), `Some(hash)` only
//! for single-file artifacts where the hash is known statically.
//! In-process voice is deferred per ADR-053; the canonical 0.7.0
//! integrity path runs on the substrate node next to the whisper.cpp
//! daemon.

use std::path::PathBuf;

/// Manages voice model downloads, caching, and integrity verification.
pub struct ModelDownloadManager {
    cache_dir: PathBuf,
}

/// Information about a downloadable model.
#[derive(Debug, Clone)]
pub struct ModelInfo {
    /// Model identifier (e.g., "sherpa-onnx-streaming-zipformer-en-20M").
    pub id: String,
    /// Download URL.
    pub url: String,
    /// Optional static SHA-256 hint. `None` means "trust the signed
    /// manifest in the bundle"; `Some(<64-hex>)` means "hard-pin the
    /// hash in the source tree." See module docs (SC-7).
    pub sha256_hint: Option<String>,
    /// File size in bytes (for progress reporting).
    pub size_bytes: u64,
}

impl ModelDownloadManager {
    /// Create a new manager with the given cache directory.
    pub fn new(cache_dir: PathBuf) -> Self {
        Self { cache_dir }
    }

    /// Get the cache directory path.
    pub fn cache_dir(&self) -> &PathBuf {
        &self.cache_dir
    }

    /// Check if a model is already cached and valid.
    pub fn is_cached(&self, model: &ModelInfo) -> bool {
        let model_path = self.cache_dir.join(&model.id);
        model_path.exists()
    }

    /// Get the local path where a model would be cached.
    pub fn model_path(&self, model_id: &str) -> PathBuf {
        self.cache_dir.join(model_id)
    }

    /// List available STT models. Hashes are deferred to the per-bundle
    /// signed manifest (see module docs); this list is metadata only.
    pub fn available_stt_models() -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: "sherpa-onnx-streaming-zipformer-en-20M".into(),
                url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-zipformer-en-20M.tar.bz2".into(),
                sha256_hint: None,
                size_bytes: 20_000_000,
            },
        ]
    }

    /// List available TTS models. See [`Self::available_stt_models`]
    /// re: hash sourcing.
    pub fn available_tts_models() -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: "vits-piper-en_US-amy-medium".into(),
                url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-piper-en_US-amy-medium.tar.bz2".into(),
                sha256_hint: None,
                size_bytes: 40_000_000,
            },
        ]
    }

    /// List available VAD models. See [`Self::available_stt_models`]
    /// re: hash sourcing.
    pub fn available_vad_models() -> Vec<ModelInfo> {
        vec![ModelInfo {
            id: "silero-vad-v5".into(),
            url: "https://github.com/snakers4/silero-vad/raw/master/files/silero_vad.onnx".into(),
            sha256_hint: None,
            size_bytes: 2_000_000,
        }]
    }
}
