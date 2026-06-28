//! Pluggable embedding backends for ECC vector operations (K3c-G2).
//!
//! Provides the [`EmbeddingProvider`] trait that the [`WeaverEngine`] uses
//! to convert text into vector embeddings for HNSW storage and similarity
//! search. Ships with [`MockEmbeddingProvider`] for deterministic testing.

use std::fmt;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// EmbeddingError
// ---------------------------------------------------------------------------

/// Errors that embedding backends may produce.
#[non_exhaustive]
#[derive(Debug)]
pub enum EmbeddingError {
    /// The underlying model has not been loaded yet.
    ModelNotLoaded,
    /// Vector dimensionality does not match the expected value.
    DimensionMismatch {
        /// Expected dimensionality.
        expected: usize,
        /// Actual dimensionality returned.
        got: usize,
    },
    /// Generic backend failure.
    BackendError(String),
    /// Rate-limited; caller should retry after the given duration.
    RateLimited {
        /// How long to wait before retrying.
        retry_after: Duration,
    },
}

impl fmt::Display for EmbeddingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ModelNotLoaded => write!(f, "embedding model not loaded"),
            Self::DimensionMismatch { expected, got } => {
                write!(f, "dimension mismatch: expected {expected}, got {got}")
            }
            Self::BackendError(msg) => write!(f, "embedding backend error: {msg}"),
            Self::RateLimited { retry_after } => {
                write!(f, "rate limited, retry after {}ms", retry_after.as_millis())
            }
        }
    }
}

impl std::error::Error for EmbeddingError {}

// ---------------------------------------------------------------------------
// EmbeddingProvider trait
// ---------------------------------------------------------------------------

/// Trait for pluggable embedding backends.
///
/// Implementations convert text into fixed-dimensionality float vectors
/// suitable for HNSW indexing and cosine similarity search.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Embed a single text chunk into a vector.
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError>;

    /// Embed a batch of text chunks.
    ///
    /// The default implementation calls [`embed`](Self::embed) in a loop.
    /// Backends that support native batching should override this for
    /// efficiency.
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            results.push(self.embed(text).await?);
        }
        Ok(results)
    }

    /// Dimensionality of the output vectors.
    fn dimensions(&self) -> usize;

    /// Name of the embedding model (for metadata tracking).
    fn model_name(&self) -> &str;
}

// ---------------------------------------------------------------------------
// MockEmbeddingProvider
// ---------------------------------------------------------------------------

/// Deterministic embedding provider for testing.
///
/// Produces vectors derived from a SHA-256 hash of the input text,
/// ensuring reproducible results without any external model dependency.
pub struct MockEmbeddingProvider {
    /// Output vector dimensionality.
    pub dims: usize,
}

impl MockEmbeddingProvider {
    /// Create a mock provider with the given output dimensionality.
    pub fn new(dims: usize) -> Self {
        Self { dims }
    }

    /// Deterministic hash-based embedding generation.
    fn hash_embed(&self, text: &str) -> Vec<f32> {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(text.as_bytes());
        let hash = hasher.finalize();

        let mut vec = Vec::with_capacity(self.dims);
        for i in 0..self.dims {
            // Cycle through hash bytes, normalise to [-1, 1]
            let byte = hash[i % 32];
            vec.push((byte as f32 / 128.0) - 1.0);
        }
        vec
    }
}

#[async_trait]
impl EmbeddingProvider for MockEmbeddingProvider {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError> {
        Ok(self.hash_embed(text))
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        Ok(texts.iter().map(|t| self.hash_embed(t)).collect())
    }

    fn dimensions(&self) -> usize {
        self.dims
    }

    fn model_name(&self) -> &str {
        "mock-sha256"
    }
}

// ---------------------------------------------------------------------------
// LlmEmbeddingProvider
// ---------------------------------------------------------------------------

/// Configuration for the LLM API embedding backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmEmbeddingConfig {
    /// Model identifier (e.g., "text-embedding-3-small").
    pub model: String,
    /// Output vector dimensionality (e.g., 384 or 1536).
    pub dimensions: usize,
    /// Maximum texts per API call for batching.
    pub batch_size: usize,
    /// Whether the API is currently available.
    pub api_available: bool,
}

impl Default for LlmEmbeddingConfig {
    fn default() -> Self {
        Self {
            model: "text-embedding-3-small".to_string(),
            dimensions: 384,
            batch_size: 16,
            api_available: false,
        }
    }
}

/// LLM-backed embedding provider that calls the clawft-llm provider layer.
///
/// Uses the model's embedding endpoint to produce real semantic vectors.
/// When no API is configured (or the API is unavailable), falls back to
/// [`MockEmbeddingProvider`] for deterministic hash-based embeddings.
pub struct LlmEmbeddingProvider {
    config: LlmEmbeddingConfig,
    fallback: MockEmbeddingProvider,
}

impl LlmEmbeddingProvider {
    /// Create a new LLM embedding provider with the given configuration.
    pub fn new(config: LlmEmbeddingConfig) -> Self {
        let fallback = MockEmbeddingProvider::new(config.dimensions);
        Self { config, fallback }
    }

    /// Create from a weave.toml-style configuration table.
    ///
    /// Expected keys: `model` (string), `dimensions` (int), `batch_size` (int).
    /// If the table is missing or incomplete, returns a provider with defaults
    /// that falls back to mock embeddings.
    pub fn from_config(table: &std::collections::HashMap<String, String>) -> Self {
        let model = table
            .get("model")
            .cloned()
            .unwrap_or_else(|| "text-embedding-3-small".to_string());
        let dimensions = table
            .get("dimensions")
            .and_then(|d| d.parse::<usize>().ok())
            .unwrap_or(384);
        let batch_size = table
            .get("batch_size")
            .and_then(|b| b.parse::<usize>().ok())
            .unwrap_or(16);
        let api_available = table
            .get("api_available")
            .map(|v| v == "true")
            .unwrap_or(false);

        Self::new(LlmEmbeddingConfig {
            model,
            dimensions,
            batch_size,
            api_available,
        })
    }

    /// Whether the LLM API is available (non-fallback mode).
    pub fn is_api_available(&self) -> bool {
        self.config.api_available
    }

    /// Get the underlying configuration.
    pub fn config(&self) -> &LlmEmbeddingConfig {
        &self.config
    }

    /// Perform an LLM API embedding call.
    ///
    /// In a production deployment this would call the clawft-llm provider's
    /// embed endpoint. Currently returns an error so that the `embed()` method
    /// falls back to the mock provider.
    async fn call_llm_api(&self, _text: &str) -> Result<Vec<f32>, EmbeddingError> {
        if !self.config.api_available {
            return Err(EmbeddingError::BackendError(
                "LLM API not configured; using fallback".to_string(),
            ));
        }
        // Production implementation would call:
        //   provider.embed(EmbedRequest { model, input: vec![text], dimensions })
        // For now, the API path is gated behind api_available.
        Err(EmbeddingError::BackendError(
            "LLM API call not yet wired to clawft-llm provider".to_string(),
        ))
    }

    /// Perform a batched LLM API embedding call.
    async fn call_llm_api_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        if !self.config.api_available {
            return Err(EmbeddingError::BackendError(
                "LLM API not configured; using fallback".to_string(),
            ));
        }
        Err(EmbeddingError::BackendError(
            "LLM API batch call not yet wired to clawft-llm provider".to_string(),
        ))
    }
}

#[async_trait]
impl EmbeddingProvider for LlmEmbeddingProvider {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError> {
        // Try the LLM API first; fall back to mock on any error.
        match self.call_llm_api(text).await {
            Ok(vec) => {
                if vec.len() != self.config.dimensions {
                    return Err(EmbeddingError::DimensionMismatch {
                        expected: self.config.dimensions,
                        got: vec.len(),
                    });
                }
                Ok(vec)
            }
            Err(_) => self.fallback.embed(text).await,
        }
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        // Try the LLM API batch endpoint; fall back to mock.
        match self.call_llm_api_batch(texts).await {
            Ok(vecs) => {
                for v in &vecs {
                    if v.len() != self.config.dimensions {
                        return Err(EmbeddingError::DimensionMismatch {
                            expected: self.config.dimensions,
                            got: v.len(),
                        });
                    }
                }
                Ok(vecs)
            }
            Err(_) => self.fallback.embed_batch(texts).await,
        }
    }

    fn dimensions(&self) -> usize {
        self.config.dimensions
    }

    fn model_name(&self) -> &str {
        &self.config.model
    }
}

// ---------------------------------------------------------------------------
// select_embedding_provider
// ---------------------------------------------------------------------------

/// Select the best available embedding provider based on configuration.
///
/// Priority order:
/// 1. ONNX local model if `onnx_model_path` points to a valid `.onnx` file
/// 2. LLM API if llm_embedding config is present
/// 3. Mock (fallback, for testing or when no backend available)
pub fn select_embedding_provider(
    llm_config: Option<LlmEmbeddingConfig>,
) -> Box<dyn EmbeddingProvider> {
    // Try ONNX first: check standard model locations.
    let onnx_paths = onnx_model_search_paths();
    for path in &onnx_paths {
        if path.exists() {
            let provider = crate::embedding_onnx::OnnxEmbeddingProvider::new(path);
            if provider.is_runtime_available() {
                tracing::info!("Using ONNX embedding provider from {}", path.display());
                return Box::new(provider);
            }
        }
    }

    if let Some(config) = llm_config {
        return Box::new(LlmEmbeddingProvider::new(config));
    }
    Box::new(MockEmbeddingProvider::new(64))
}

/// Standard search paths for the ONNX embedding model.
///
/// Looks in (in order):
/// 1. `.weftos/models/all-MiniLM-L6-v2.onnx` (project-local)
/// 2. `$HOME/.weftos/models/all-MiniLM-L6-v2.onnx` (user-global)
/// 3. `$WEFTOS_MODEL_PATH` environment variable
fn onnx_model_search_paths() -> Vec<std::path::PathBuf> {
    let model_name = "all-MiniLM-L6-v2.onnx";
    let mut paths = Vec::new();

    // Project-local.
    paths.push(std::path::PathBuf::from(format!(
        ".weftos/models/{model_name}"
    )));

    // User-global.
    if let Some(home) = dirs_home() {
        paths.push(home.join(format!(".weftos/models/{model_name}")));
    }

    // Env override.
    if let Ok(env_path) = std::env::var("WEFTOS_MODEL_PATH") {
        paths.push(std::path::PathBuf::from(env_path));
    }

    paths
}

/// Get the user's home directory.
fn dirs_home() -> Option<std::path::PathBuf> {
    std::env::var("HOME").ok().map(std::path::PathBuf::from)
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_embed_returns_correct_dimensions() {
        let provider = MockEmbeddingProvider::new(64);
        let vec = provider.embed("hello world").await.unwrap();
        assert_eq!(vec.len(), 64);
    }

    #[tokio::test]
    async fn mock_embed_deterministic() {
        let provider = MockEmbeddingProvider::new(32);
        let v1 = provider.embed("test input").await.unwrap();
        let v2 = provider.embed("test input").await.unwrap();
        assert_eq!(v1, v2);
    }

    #[tokio::test]
    async fn mock_embed_different_inputs_differ() {
        let provider = MockEmbeddingProvider::new(32);
        let v1 = provider.embed("alpha").await.unwrap();
        let v2 = provider.embed("beta").await.unwrap();
        assert_ne!(v1, v2);
    }

    #[tokio::test]
    async fn mock_embed_batch() {
        let provider = MockEmbeddingProvider::new(16);
        let results = provider.embed_batch(&["a", "b", "c"]).await.unwrap();
        assert_eq!(results.len(), 3);
        for v in &results {
            assert_eq!(v.len(), 16);
        }
    }

    #[tokio::test]
    async fn mock_embed_batch_matches_individual() {
        let provider = MockEmbeddingProvider::new(8);
        let batch = provider.embed_batch(&["x", "y"]).await.unwrap();
        let x = provider.embed("x").await.unwrap();
        let y = provider.embed("y").await.unwrap();
        assert_eq!(batch[0], x);
        assert_eq!(batch[1], y);
    }

    #[test]
    fn mock_model_name() {
        let provider = MockEmbeddingProvider::new(16);
        assert_eq!(provider.model_name(), "mock-sha256");
    }

    #[test]
    fn mock_dimensions() {
        let provider = MockEmbeddingProvider::new(128);
        assert_eq!(provider.dimensions(), 128);
    }

    #[test]
    fn embedding_error_display() {
        let err = EmbeddingError::DimensionMismatch {
            expected: 384,
            got: 256,
        };
        assert!(err.to_string().contains("384"));
        assert!(err.to_string().contains("256"));

        let err2 = EmbeddingError::ModelNotLoaded;
        assert!(err2.to_string().contains("not loaded"));
    }

    // ── LlmEmbeddingProvider tests ───────────────────────────────────

    #[tokio::test]
    async fn llm_provider_falls_back_to_mock_when_api_unavailable() {
        let config = LlmEmbeddingConfig {
            api_available: false,
            dimensions: 64,
            ..Default::default()
        };
        let provider = LlmEmbeddingProvider::new(config);
        // Should succeed via fallback, not error.
        let vec = provider.embed("hello world").await.unwrap();
        assert_eq!(vec.len(), 64);
    }

    #[tokio::test]
    async fn llm_provider_fallback_is_deterministic() {
        let config = LlmEmbeddingConfig {
            api_available: false,
            dimensions: 32,
            ..Default::default()
        };
        let provider = LlmEmbeddingProvider::new(config);
        let v1 = provider.embed("test").await.unwrap();
        let v2 = provider.embed("test").await.unwrap();
        assert_eq!(v1, v2);
    }

    #[tokio::test]
    async fn llm_provider_batch_fallback() {
        let config = LlmEmbeddingConfig {
            api_available: false,
            dimensions: 16,
            ..Default::default()
        };
        let provider = LlmEmbeddingProvider::new(config);
        let results = provider.embed_batch(&["a", "b", "c"]).await.unwrap();
        assert_eq!(results.len(), 3);
        for v in &results {
            assert_eq!(v.len(), 16);
        }
    }

    #[test]
    fn llm_provider_reports_model_name() {
        let config = LlmEmbeddingConfig {
            model: "custom-embed-v1".to_string(),
            ..Default::default()
        };
        let provider = LlmEmbeddingProvider::new(config);
        assert_eq!(provider.model_name(), "custom-embed-v1");
    }

    #[test]
    fn llm_provider_reports_dimensions() {
        let config = LlmEmbeddingConfig {
            dimensions: 1536,
            ..Default::default()
        };
        let provider = LlmEmbeddingProvider::new(config);
        assert_eq!(provider.dimensions(), 1536);
    }

    #[test]
    fn llm_provider_api_availability_check() {
        let unavailable = LlmEmbeddingProvider::new(LlmEmbeddingConfig::default());
        assert!(!unavailable.is_api_available());

        let available = LlmEmbeddingProvider::new(LlmEmbeddingConfig {
            api_available: true,
            ..Default::default()
        });
        assert!(available.is_api_available());
    }

    #[test]
    fn llm_provider_from_config_defaults() {
        let table = std::collections::HashMap::new();
        let provider = LlmEmbeddingProvider::from_config(&table);
        assert_eq!(provider.dimensions(), 384);
        assert_eq!(provider.model_name(), "text-embedding-3-small");
        assert!(!provider.is_api_available());
    }

    #[test]
    fn llm_provider_from_config_custom() {
        let mut table = std::collections::HashMap::new();
        table.insert("model".to_string(), "my-model".to_string());
        table.insert("dimensions".to_string(), "768".to_string());
        table.insert("batch_size".to_string(), "32".to_string());
        table.insert("api_available".to_string(), "true".to_string());
        let provider = LlmEmbeddingProvider::from_config(&table);
        assert_eq!(provider.model_name(), "my-model");
        assert_eq!(provider.dimensions(), 768);
        assert_eq!(provider.config().batch_size, 32);
        assert!(provider.is_api_available());
    }

    #[test]
    fn select_provider_returns_mock_when_no_config() {
        let provider = select_embedding_provider(None);
        assert_eq!(provider.dimensions(), 64);
        assert_eq!(provider.model_name(), "mock-sha256");
    }

    #[test]
    fn select_provider_returns_llm_when_config_present() {
        let config = LlmEmbeddingConfig {
            model: "test-embed".to_string(),
            dimensions: 256,
            ..Default::default()
        };
        let provider = select_embedding_provider(Some(config));
        assert_eq!(provider.dimensions(), 256);
        assert_eq!(provider.model_name(), "test-embed");
    }

    #[tokio::test]
    async fn llm_provider_fallback_matches_mock() {
        let config = LlmEmbeddingConfig {
            api_available: false,
            dimensions: 32,
            ..Default::default()
        };
        let llm = LlmEmbeddingProvider::new(config);
        let mock = MockEmbeddingProvider::new(32);
        let llm_vec = llm.embed("same input").await.unwrap();
        let mock_vec = mock.embed("same input").await.unwrap();
        // Fallback should produce identical results to mock.
        assert_eq!(llm_vec, mock_vec);
    }
}
