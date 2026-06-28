//! v1 [`ContextRouter`] backed by an LLM classifier.
//!
//! Phase E1 of `docs/plans/agent-core-v1.md`. The router round-trips
//! the user's message against the daemon's existing `LlmClient` (so
//! local llama-server and OpenRouter both work, A1 is preserved),
//! reads back a tiny JSON envelope of `{archetype, complexity}`, and
//! writes the clamped `complexity_hint` into
//! [`ChatRequest::complexity_boost`](crate::pipeline::traits::ChatRequest::complexity_boost)
//! so the downstream
//! [`TieredRouter`](crate::pipeline::tiered_router::TieredRouter) at
//! `crates/clawft-core/src/pipeline/tiered_router.rs:585` can pick the
//! right tier.
//!
//! ## Hard contract (do not break)
//!
//! From `docs/research/rvf-context-router.md`: the router NEVER picks
//! a model and NEVER escalates a tier. The LLM only classifies; the
//! TieredRouter still owns the actual decision. This module respects
//! that — we only emit `complexity_hint` and `archetype`.
//!
//! ## Robustness
//!
//! Every failure mode collapses to [`ContextDecision::default()`] with
//! a `tracing::warn!`:
//!
//! - Backend transport / 5xx / loading.
//! - Empty body (no `choices` content).
//! - Malformed JSON.
//! - Code-fenced JSON (` ```json ... ``` `) — fences are stripped.
//! - Out-of-range or non-finite `complexity` — pre-clamped before
//!   feeding [`ContextDecision::new`] so the debug-build assert in
//!   [`clamp_complexity`](super::clamp_complexity) doesn't trip on a
//!   misaligned model. The model is upstream of our contract.

use std::sync::Arc;

use async_trait::async_trait;

use super::{
    COMPLEXITY_HINT_LIMIT, ContextDecision, ContextRequest, ContextRouter, clamp_complexity,
};

/// System prompt the v1 classifier sees on every classification turn.
///
/// Kept short on purpose — every classification round-trips against
/// the daemon's existing `LlmClient`, so prompt length directly trades
/// against latency and (on hosted backends like OpenRouter) cost.
///
/// The few-shot examples are deliberately at the boundaries of the
/// `[-0.3, +0.3]` band so the model anchors on the right scale; the
/// in-band cases collapse to ~0.0 by interpolation.
pub const CLASSIFIER_SYSTEM_PROMPT: &str = "\
You classify user requests for an OS-level chat agent. Reply with JSON only:\n\
{ \"archetype\": \"Reasoning\" | \"CodeGen\" | \"Analysis\" | \"Conversational\" | \"Creative\",\n\
  \"complexity\": <number in [-0.3, 0.3]> }\n\
\n\
The `complexity` value adjusts model-tier selection downstream:\n\
  -0.3 → strongly cheap/fast\n\
  +0.3 → strongly capable/expensive\n\
   0.0 → no opinion\n\
\n\
Examples:\n\
  \"what's 2+2?\"          → {\"archetype\":\"Conversational\",\"complexity\":-0.3}\n\
  \"refactor this Rust\"   → {\"archetype\":\"CodeGen\",\"complexity\":0.1}\n\
  \"prove ZF + AC\"        → {\"archetype\":\"Reasoning\",\"complexity\":0.3}\n\
";

/// Default upper bound on classifier `max_tokens`.
///
/// ~64 tokens of JSON output is plenty for the
/// `{archetype, complexity}` envelope; a hard cap keeps a malformed
/// response from burning the budget on a runaway generation.
pub const DEFAULT_CLASSIFIER_MAX_TOKENS: u32 = 64;

/// Result of the v1 classifier turn — the bare JSON envelope, parsed.
///
/// Pulled out as a tiny struct so the [`Classifier`] backend trait can
/// hand back something typed instead of a `serde_json::Value`. Errors
/// from the backend collapse to [`ContextDecision::default()`] in
/// [`LlmClassifierRouter::route`]; the router never propagates a
/// classifier failure to the caller's turn.
#[derive(Debug, Clone)]
pub struct ClassifierOutput {
    /// Archetype label as the LLM emitted it. Empty string when the
    /// model omitted the field.
    pub archetype: String,
    /// Complexity hint. Pre-clamp; [`ContextDecision::new`] is the
    /// single chokepoint that funnels through
    /// [`clamp_complexity`](super::clamp_complexity).
    pub complexity: f32,
}

/// Tiny abstraction so the v1 router can be unit-tested without
/// spinning up an actual HTTP client.
///
/// Production wiring uses the `Arc<LlmClient>` blanket impl below
/// (native-only); tests provide an in-process implementation that
/// returns canned bodies for the malformed / fenced / out-of-range
/// shapes the contract has to handle.
///
/// The trait deliberately returns the raw response body string: the
/// router itself owns fence-stripping and JSON-parsing so all of the
/// "robustness vs. malformed input" surface lives in one place and
/// the test surface stays narrow.
#[async_trait]
pub trait Classifier: Send + Sync + 'static {
    /// Classify `user_content`, capped at `max_tokens` of output, with
    /// the supplied `model_override` (when `Some`).
    ///
    /// Returns the raw response body the model emitted, or `Err(_)` on
    /// any backend failure (transport, 5xx, empty body, …). Errors are
    /// formatted as plain strings — the router only logs them, never
    /// returns them upstream.
    async fn classify(
        &self,
        user_content: &str,
        max_tokens: u32,
        model_override: Option<&str>,
    ) -> Result<String, String>;
}

/// v1 router: round-trips the user's message against a cheap LLM and
/// reads back a JSON envelope of `{archetype, complexity}`.
///
/// The hard contract from `docs/research/rvf-context-router.md`
/// stands: the router NEVER picks a model and NEVER escalates a tier.
/// The LLM only classifies. `TieredRouter` downstream (see
/// `crates/clawft-core/src/pipeline/tiered_router.rs:585`) reads the
/// clamped `complexity_hint` we feed into
/// [`ChatRequest::complexity_boost`](crate::pipeline::traits::ChatRequest::complexity_boost)
/// and decides which tier to use.
///
/// All failure modes — transport errors, malformed JSON, fences, empty
/// content, out-of-range complexity — collapse to
/// [`ContextDecision::default()`] with a `tracing::warn!`. The turn
/// never blocks on a classifier fault.
pub struct LlmClassifierRouter {
    backend: Arc<dyn Classifier>,
    /// Optional override for the classifier model. When `None`, the
    /// underlying [`Classifier`] uses whatever model its `LlmClient` is
    /// pinned to. v1.1 will allow operators to pin a separate cheap
    /// classifier model here without touching the main turn model.
    model_override: Option<String>,
    /// Hard cap on the classifier turn's `max_tokens`. Defaults to
    /// [`DEFAULT_CLASSIFIER_MAX_TOKENS`].
    max_tokens: u32,
}

impl LlmClassifierRouter {
    /// Wrap an existing [`Classifier`] backend (typically
    /// `Arc<LlmClient>` via the native-only blanket impl below).
    pub fn from_backend(backend: Arc<dyn Classifier>) -> Self {
        Self {
            backend,
            model_override: None,
            max_tokens: DEFAULT_CLASSIFIER_MAX_TOKENS,
        }
    }

    /// Build a router from an `Arc<LlmClient>`. Native-only because
    /// `LlmClient` itself is native-only (pulls reqwest).
    #[cfg(feature = "native")]
    pub fn new(llm: Arc<clawft_service_llm::LlmClient>) -> Self {
        Self::from_backend(llm)
    }

    /// Pin a classifier model name (overrides the `LlmClient`'s
    /// configured default). Useful when the daemon's main model is an
    /// expensive reasoning model and you want a cheaper Haiku-class
    /// model on the classifier hot path.
    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model_override = Some(model.into());
        self
    }

    /// Override the classifier `max_tokens` cap. Default is
    /// [`DEFAULT_CLASSIFIER_MAX_TOKENS`] (~64) which fits the JSON
    /// envelope comfortably without burning budget on runaway output.
    #[must_use]
    pub fn with_max_tokens(mut self, tokens: u32) -> Self {
        self.max_tokens = tokens;
        self
    }
}

#[async_trait]
impl ContextRouter for LlmClassifierRouter {
    async fn route(&self, request: &ContextRequest) -> ContextDecision {
        let raw = match self
            .backend
            .classify(
                &request.content,
                self.max_tokens,
                self.model_override.as_deref(),
            )
            .await
        {
            Ok(body) if body.trim().is_empty() => {
                tracing::warn!("LlmClassifierRouter: empty classifier response; falling back");
                return ContextDecision::default();
            }
            Ok(body) => body,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "LlmClassifierRouter: classifier call failed; falling back to empty decision"
                );
                return ContextDecision::default();
            }
        };

        match parse_classifier_envelope(&raw) {
            Some(out) => {
                // ContextDecision::new clamps via clamp_complexity; in
                // release builds out-of-range values saturate to ±0.3,
                // in debug they trip the assert (caller-side bug).
                // We pre-clamp here so debug builds don't panic on a
                // misaligned model — the model is upstream of us.
                let safe = if out.complexity.is_finite() {
                    out.complexity
                        .clamp(-COMPLEXITY_HINT_LIMIT, COMPLEXITY_HINT_LIMIT)
                } else {
                    0.0
                };
                // Funnel through clamp_complexity once more for the
                // single-chokepoint invariant; the pre-clamp above
                // guarantees this is a no-op.
                let _ = clamp_complexity(safe);
                let mut decision = ContextDecision::new(Vec::new(), None, safe);
                if !out.archetype.is_empty() {
                    decision.archetype = Some(out.archetype);
                }
                decision
            }
            None => {
                tracing::warn!(
                    body = %raw,
                    "LlmClassifierRouter: malformed JSON; falling back"
                );
                ContextDecision::default()
            }
        }
    }
}

/// Strip optional ```` ```json ```` / ```` ``` ```` fences and parse
/// the JSON envelope. Returns `None` on any parse error so the router
/// can collapse to [`ContextDecision::default()`].
fn parse_classifier_envelope(body: &str) -> Option<ClassifierOutput> {
    let trimmed = body.trim();
    // Strip a leading ``` or ```json fence and any matching trailing ```.
    let stripped = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .unwrap_or(trimmed);
    let stripped = stripped.trim_start_matches('\n').trim();
    let stripped = stripped.strip_suffix("```").unwrap_or(stripped).trim();
    let parsed: serde_json::Value = serde_json::from_str(stripped).ok()?;
    let archetype = parsed
        .get("archetype")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let complexity = parsed
        .get("complexity")
        .and_then(|v| v.as_f64())
        .map(|f| f as f32)
        .unwrap_or(0.0);
    Some(ClassifierOutput {
        archetype,
        complexity,
    })
}

/// Native-only blanket impl: an `LlmClient` is a [`Classifier`].
///
/// Uses `LlmClient::complete` (no tools, single-shot) at `temperature
/// = 0.0` for determinism. The `model_override` argument is currently
/// ignored — `LlmClient` doesn't expose a per-call model override
/// today; once it does, this impl will thread it through. Until then
/// the override is a forward-compatible knob on
/// [`LlmClassifierRouter`] that callers can set without API churn.
#[cfg(feature = "native")]
#[async_trait]
impl Classifier for clawft_service_llm::LlmClient {
    async fn classify(
        &self,
        user_content: &str,
        max_tokens: u32,
        _model_override: Option<&str>,
    ) -> Result<String, String> {
        use clawft_service_llm::ChatMessage;
        let messages = vec![
            ChatMessage::system(CLASSIFIER_SYSTEM_PROMPT),
            ChatMessage::user(user_content.to_string()),
        ];
        let resp = self
            .complete(messages, Some(0.0), Some(max_tokens))
            .await
            .map_err(|e| e.to_string())?;
        let body = resp
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .unwrap_or_default();
        Ok(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mock backend used by the v1 router tests. Each instance returns
    /// the same canned response for every call; that's all the v1
    /// surface needs (the router doesn't iterate or retry).
    struct MockClassifier {
        response: Result<String, String>,
    }

    #[async_trait]
    impl Classifier for MockClassifier {
        async fn classify(
            &self,
            _user_content: &str,
            _max_tokens: u32,
            _model_override: Option<&str>,
        ) -> Result<String, String> {
            self.response.clone()
        }
    }

    fn req(content: &str) -> ContextRequest {
        ContextRequest {
            content: content.into(),
            channel: "panel".into(),
            chat_id: "c1".into(),
            metadata: Default::default(),
        }
    }

    fn router_with(response: Result<String, String>) -> LlmClassifierRouter {
        LlmClassifierRouter::from_backend(Arc::new(MockClassifier { response }))
    }

    #[tokio::test]
    async fn llm_classifier_parses_valid_envelope() {
        let r = router_with(Ok(r#"{"archetype":"CodeGen","complexity":0.15}"#.into()));
        let d = r.route(&req("refactor this loop")).await;
        assert!((d.complexity_hint - 0.15).abs() < 1e-5);
        assert_eq!(d.archetype.as_deref(), Some("CodeGen"));
        assert!(d.skills.is_empty());
        assert!(d.tool_subset.is_none());
    }

    #[tokio::test]
    async fn llm_classifier_clamps_out_of_range_complexity() {
        // Model misbehaves and returns 0.9 — well outside the
        // [-0.3, +0.3] band. Must saturate to +0.3 without panicking.
        let r = router_with(Ok(r#"{"archetype":"Reasoning","complexity":0.9}"#.into()));
        let d = r.route(&req("prove ZF + AC")).await;
        assert_eq!(d.complexity_hint, COMPLEXITY_HINT_LIMIT);
        assert_eq!(d.archetype.as_deref(), Some("Reasoning"));
    }

    #[tokio::test]
    async fn llm_classifier_clamps_negative_out_of_range() {
        let r = router_with(Ok(
            r#"{"archetype":"Conversational","complexity":-1.0}"#.into()
        ));
        let d = r.route(&req("hi")).await;
        assert_eq!(d.complexity_hint, -COMPLEXITY_HINT_LIMIT);
        assert_eq!(d.archetype.as_deref(), Some("Conversational"));
    }

    #[tokio::test]
    async fn llm_classifier_strips_code_fences() {
        // Some chat models wrap JSON in ```json ... ``` fences; the
        // router must tolerate that without falling back.
        let body = "```json\n{\"archetype\":\"Analysis\",\"complexity\":0.0}\n```";
        let r = router_with(Ok(body.into()));
        let d = r.route(&req("explain this graph")).await;
        assert_eq!(d.complexity_hint, 0.0);
        assert_eq!(d.archetype.as_deref(), Some("Analysis"));
    }

    #[tokio::test]
    async fn llm_classifier_strips_bare_fences() {
        // Bare ``` (no language tag) is also common.
        let body = "```\n{\"archetype\":\"Creative\",\"complexity\":0.1}\n```";
        let r = router_with(Ok(body.into()));
        let d = r.route(&req("write a poem")).await;
        assert!((d.complexity_hint - 0.1).abs() < 1e-5);
        assert_eq!(d.archetype.as_deref(), Some("Creative"));
    }

    #[tokio::test]
    async fn llm_classifier_falls_back_on_malformed_json() {
        let r = router_with(Ok("not json at all, sorry".into()));
        let d = r.route(&req("x")).await;
        assert_eq!(d.complexity_hint, 0.0);
        assert!(d.archetype.is_none());
        assert!(d.skills.is_empty());
        assert!(d.tool_subset.is_none());
    }

    #[tokio::test]
    async fn llm_classifier_falls_back_on_backend_error() {
        let r = router_with(Err("transport: connection refused".into()));
        let d = r.route(&req("x")).await;
        assert_eq!(d.complexity_hint, 0.0);
        assert!(d.archetype.is_none());
    }

    #[tokio::test]
    async fn llm_classifier_falls_back_on_empty_body() {
        let r = router_with(Ok(String::new()));
        let d = r.route(&req("x")).await;
        assert_eq!(d.complexity_hint, 0.0);
        assert!(d.archetype.is_none());
    }

    #[tokio::test]
    async fn llm_classifier_falls_back_on_whitespace_body() {
        let r = router_with(Ok("   \n\t  ".into()));
        let d = r.route(&req("x")).await;
        assert_eq!(d.complexity_hint, 0.0);
        assert!(d.archetype.is_none());
    }

    #[tokio::test]
    async fn llm_classifier_handles_missing_archetype() {
        // JSON parses but the model omitted the archetype field. Hint
        // is still honored; archetype stays None.
        let r = router_with(Ok(r#"{"complexity":0.2}"#.into()));
        let d = r.route(&req("x")).await;
        assert!((d.complexity_hint - 0.2).abs() < 1e-5);
        assert!(d.archetype.is_none());
    }

    #[tokio::test]
    async fn llm_classifier_handles_missing_complexity() {
        // JSON parses but complexity is omitted — default to 0.0.
        let r = router_with(Ok(r#"{"archetype":"Reasoning"}"#.into()));
        let d = r.route(&req("x")).await;
        assert_eq!(d.complexity_hint, 0.0);
        assert_eq!(d.archetype.as_deref(), Some("Reasoning"));
    }

    #[tokio::test]
    async fn llm_classifier_handles_non_finite_complexity() {
        // Defensive: a model emitting a number too large to fit f32
        // becomes ±inf when narrowed; the pre-clamp coerces to 0.0.
        let r = router_with(Ok(r#"{"archetype":"X","complexity":1e308}"#.into()));
        let d = r.route(&req("x")).await;
        // 1e308 as f32 is +inf → coerced to 0.0 by the pre-clamp
        // path because it's not finite.
        assert_eq!(d.complexity_hint, 0.0);
    }

    #[test]
    fn llm_classifier_builder_methods() {
        let backend: Arc<dyn Classifier> = Arc::new(MockClassifier {
            response: Ok("{}".into()),
        });
        let r = LlmClassifierRouter::from_backend(backend)
            .with_model("haiku-3.5")
            .with_max_tokens(32);
        assert_eq!(r.model_override.as_deref(), Some("haiku-3.5"));
        assert_eq!(r.max_tokens, 32);
    }

    #[test]
    fn llm_classifier_default_max_tokens() {
        let backend: Arc<dyn Classifier> = Arc::new(MockClassifier {
            response: Ok("{}".into()),
        });
        let r = LlmClassifierRouter::from_backend(backend);
        assert_eq!(r.max_tokens, DEFAULT_CLASSIFIER_MAX_TOKENS);
        assert!(r.model_override.is_none());
    }
}
