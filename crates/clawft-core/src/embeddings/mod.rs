//! Embedding trait definitions and implementations.
//!
//! Provides the [`Embedder`] trait for generating vector embeddings from text,
//! plus a [`hash_embedder::HashEmbedder`] that uses SimHash for local,
//! deterministic embeddings with no API calls required.
//!
//! All types in this module are gated behind the `vector-memory` feature flag.

#[cfg(feature = "vector-memory")]
pub mod hash_embedder;
#[cfg(feature = "vector-memory")]
pub mod hnsw_store;
#[cfg(feature = "vector-memory")]
pub mod micro_hnsw;

#[cfg(feature = "rvf")]
pub mod api_embedder;
#[cfg(feature = "rvf")]
pub mod progressive;
#[cfg(feature = "rvf")]
pub mod quantization;
#[cfg(feature = "rvf")]
pub mod rvf_stub;
#[cfg(feature = "rvf")]
pub mod witness;

use async_trait::async_trait;
use std::fmt;

/// Errors that can occur during embedding generation.
#[non_exhaustive]
#[derive(Debug)]
pub enum EmbeddingError {
    /// The input text could not be processed.
    InvalidInput(String),
    /// An internal error occurred in the embedder.
    Internal(String),
}

impl fmt::Display for EmbeddingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EmbeddingError::InvalidInput(msg) => write!(f, "invalid input: {msg}"),
            EmbeddingError::Internal(msg) => write!(f, "internal error: {msg}"),
        }
    }
}

impl std::error::Error for EmbeddingError {}

/// Trait for generating vector embeddings from text.
///
/// Implementations can be local (e.g. SimHash) or remote (e.g. OpenAI embeddings API).
/// All implementations must be `Send + Sync` for use across async tasks.
#[async_trait]
pub trait Embedder: Send + Sync {
    /// Generate a vector embedding for the given text.
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError>;

    /// Batch embed multiple texts.
    ///
    /// Default implementation calls [`embed`](Embedder::embed) for each text sequentially.
    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            results.push(self.embed(text).await?);
        }
        Ok(results)
    }

    /// Return the dimensionality of embeddings produced by this embedder.
    fn dimension(&self) -> usize;

    /// Return the name/identifier of this embedder (e.g. "hash", "openai-text-embedding-3-small").
    ///
    /// Used for logging and configuration. Default returns "unknown".
    fn name(&self) -> &str {
        "unknown"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedding_error_display_invalid_input() {
        let err = EmbeddingError::InvalidInput("bad text".into());
        assert_eq!(format!("{err}"), "invalid input: bad text");
    }

    #[test]
    fn embedding_error_display_internal() {
        let err = EmbeddingError::Internal("something broke".into());
        assert_eq!(format!("{err}"), "internal error: something broke");
    }

    #[test]
    fn embedding_error_is_error_trait() {
        let err: Box<dyn std::error::Error> = Box::new(EmbeddingError::Internal("test".into()));
        assert!(err.to_string().contains("test"));
    }
}
