//! The service adapters over [`ParentLink`] (ADR-103 Phase 2 F): the
//! remote embedder and the LLM backend of a project kernel. Re-exported from
//! [`crate::parent_link`].

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use clawft_service_llm::{ChatRequest, ChatResponse, LlmBackend, LlmError};
use serde_json::{Value, json};
use tracing::warn;

use crate::parent_link::{
    DEFAULT_EMBED_DIMENSION, PARENT_UNAVAILABLE_KIND, ParentError, ParentLink, Service,
};

/// An embedder that calls the parent's `shared.embed`.
pub struct RemoteEmbedder {
    link: Arc<ParentLink>,
    dimension: AtomicUsize,
}

impl RemoteEmbedder {
    /// An embedder with the default width until the parent reports its own.
    pub fn new(link: Arc<ParentLink>) -> Self {
        Self {
            link,
            dimension: AtomicUsize::new(DEFAULT_EMBED_DIMENSION),
        }
    }

    /// [`new`](Self::new) plus one best-effort `shared.embed` with no texts to
    /// learn the parent's width. A down parent leaves the default and logs.
    pub async fn connect(link: Arc<ParentLink>) -> Arc<Self> {
        let this = Arc::new(Self::new(link));
        if let Err(e) = this.embed_texts(&[]).await {
            warn!(error = %e, "shared embeddings: parent did not answer; assuming {DEFAULT_EMBED_DIMENSION}-d until it does");
        }
        this
    }

    /// Embed `texts` through the parent.
    pub async fn embed_texts(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, ParentError> {
        let v = self
            .link
            .call(Service::Embeddings, "shared.embed", json!({ "texts": texts }))
            .await?;
        let dim = v.get("dimension").and_then(Value::as_u64).unwrap_or(0) as usize;
        if dim > 0 {
            self.dimension.store(dim, Ordering::Relaxed);
        }
        let rows: Vec<Vec<f32>> = serde_json::from_value(v.get("embeddings").cloned().unwrap_or(json!([])))
            .map_err(|e| ParentError::Refused {
                kind: "parent_malformed".into(),
                message: format!("shared.embed reply: {e}"),
            })?;
        if rows.len() != texts.len() {
            return Err(ParentError::Refused {
                kind: "parent_malformed".into(),
                message: format!("shared.embed returned {} rows for {} texts", rows.len(), texts.len()),
            });
        }
        Ok(rows)
    }

    fn width(&self) -> usize {
        self.dimension.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl clawft_core::embeddings::Embedder for RemoteEmbedder {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, clawft_core::embeddings::EmbeddingError> {
        let mut rows = self
            .embed_texts(&[text.to_owned()])
            .await
            .map_err(|e| clawft_core::embeddings::EmbeddingError::Internal(e.to_string()))?;
        rows.pop()
            .ok_or_else(|| clawft_core::embeddings::EmbeddingError::Internal("empty reply".into()))
    }

    async fn embed_batch(
        &self,
        texts: &[String],
    ) -> Result<Vec<Vec<f32>>, clawft_core::embeddings::EmbeddingError> {
        self.embed_texts(texts)
            .await
            .map_err(|e| clawft_core::embeddings::EmbeddingError::Internal(e.to_string()))
    }

    fn dimension(&self) -> usize {
        self.width()
    }

    fn name(&self) -> &str {
        "parent:shared.embed"
    }
}

#[async_trait]
impl clawft_kernel::embedding::EmbeddingProvider for RemoteEmbedder {
    async fn embed(
        &self,
        text: &str,
    ) -> Result<Vec<f32>, clawft_kernel::embedding::EmbeddingError> {
        let mut rows = self
            .embed_texts(&[text.to_owned()])
            .await
            .map_err(|e| clawft_kernel::embedding::EmbeddingError::BackendError(e.to_string()))?;
        rows.pop().ok_or_else(|| {
            clawft_kernel::embedding::EmbeddingError::BackendError("empty reply".into())
        })
    }

    async fn embed_batch(
        &self,
        texts: &[&str],
    ) -> Result<Vec<Vec<f32>>, clawft_kernel::embedding::EmbeddingError> {
        let owned: Vec<String> = texts.iter().map(|t| (*t).to_owned()).collect();
        self.embed_texts(&owned)
            .await
            .map_err(|e| clawft_kernel::embedding::EmbeddingError::BackendError(e.to_string()))
    }

    fn dimensions(&self) -> usize {
        self.width()
    }

    fn model_name(&self) -> &str {
        "parent:shared.embed"
    }
}

// ── LLM ──────────────────────────────────────────────────────────────────

/// The [`LlmBackend`] of a project kernel's `LlmClient`.
///
/// Request/response, not streaming: the daemon's agent path
/// (`ServiceLlmAdapter`) has no streaming today, so `shared.llm.chat` has
/// none either.
#[derive(Debug)]
pub struct ParentLlmBackend {
    link: Arc<ParentLink>,
}

impl ParentLlmBackend {
    pub fn new(link: Arc<ParentLink>) -> Self {
        Self { link }
    }
}

fn llm_error(e: ParentError) -> LlmError {
    match e {
        ParentError::Unavailable(m) => {
            LlmError::Transport(format!("{PARENT_UNAVAILABLE_KIND}: {m}"))
        }
        ParentError::Refused { kind, message } => {
            let status = if kind == "rate_limited" || kind == "budget_exceeded" { 429 } else { 400 };
            LlmError::ClientError {
                status,
                body: format!("{kind}: {message}"),
            }
        }
    }
}

#[async_trait]
impl LlmBackend for ParentLlmBackend {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let params = serde_json::to_value(&request)
            .map_err(|e| LlmError::Malformed(format!("request: {e}")))?;
        let v = self
            .link
            .call(Service::Llm, "shared.llm.chat", params)
            .await
            .map_err(llm_error)?;
        serde_json::from_value(v)
            .map_err(|e| LlmError::Malformed(format!("shared.llm.chat reply: {e}")))
    }

    async fn list_models(&self) -> Result<Vec<String>, LlmError> {
        let v = self
            .link
            .call(Service::Llm, "shared.llm.models", Value::Null)
            .await
            .map_err(llm_error)?;
        Ok(v.get("models")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|m| m.as_str().map(str::to_owned)).collect())
            .unwrap_or_default())
    }

    async fn health(&self) -> Result<bool, LlmError> {
        Ok(self.link.probe().await)
    }
}
