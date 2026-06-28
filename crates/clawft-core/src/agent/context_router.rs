//! Pre-LLM context routing seam.
//!
//! `ContextRouter` is invoked inside [`AgentLoop::handle_turn`](super::loop_core::AgentLoop::handle_turn)
//! **before** the LLM request is dispatched. Implementations can:
//!
//! 1. Select skills the loop should inject into the system context.
//! 2. Restrict the tool subset the LLM is allowed to see.
//! 3. Write a clamped `complexity_hint ∈ [-0.3, +0.3]` into
//!    [`ChatRequest::complexity_boost`](crate::pipeline::traits::ChatRequest::complexity_boost),
//!    consumed by the tier classifier at
//!    `crates/clawft-core/src/pipeline/tiered_router.rs:585`.
//!
//! ## Hard contract
//!
//! Per `docs/research/rvf-context-router.md`, ContextRouter is **not** a
//! model-picker. It NEVER chooses a model and NEVER escalates a tier —
//! that's [`TieredRouter`](crate::pipeline::tiered_router) downstream.
//! The hint nudges classification; the router still has the final say.
//!
//! The hint range is enforced via [`clamp_complexity`] which saturates
//! out-of-range values to ±0.3 and `debug_assert!`s in debug builds.
//!
//! ## Phasing
//!
//! agent-core-v1 lands [`NullRouter`] (v0) and, in Phase E1,
//! [`LlmClassifierRouter`] (v1). Phase E2 promotes to `EmbeddingRouter`
//! (v2) once the 7-day fallback metric is below 25%. See
//! `docs/plans/agent-core-v1.md` Phase E and
//! `docs/research/rvf-context-router.md` for the full v0 → v3 sequence.

use async_trait::async_trait;

/// Maximum absolute value for [`ContextDecision::complexity_hint`].
///
/// Per `docs/research/rvf-context-router.md:579-582`: the router can
/// nudge classification within ±0.3. Anything outside this band is
/// saturated by [`clamp_complexity`] (with a `debug_assert!` to catch
/// the misuse in tests).
pub const COMPLEXITY_HINT_LIMIT: f32 = 0.3;

/// Saturate `value` into `[-0.3, +0.3]` and `debug_assert!` it was in
/// range. The assert fires only in debug builds; in release the clamp
/// is silent so a buggy router cannot crash the loop.
pub fn clamp_complexity(value: f32) -> f32 {
    debug_assert!(
        value.is_finite() && value.abs() <= COMPLEXITY_HINT_LIMIT,
        "ContextRouter complexity_hint must be in [-{lim}, +{lim}], got {value}",
        lim = COMPLEXITY_HINT_LIMIT,
    );
    if !value.is_finite() {
        0.0
    } else {
        value.clamp(-COMPLEXITY_HINT_LIMIT, COMPLEXITY_HINT_LIMIT)
    }
}

/// Inputs surfaced to a [`ContextRouter`] before the LLM call.
///
/// Kept deliberately minimal in v0; the schema is allowed to grow as
/// future routers (v1 LLM classifier, v2 embedding retrieval, v2.5
/// hybrid) need richer signals. Today we surface what's natural inside
/// `handle_turn`: the user's message, the channel, and the inbound
/// metadata (which already carries skill activation + allow_from etc.).
#[derive(Debug, Clone)]
pub struct ContextRequest {
    /// Raw user message text.
    pub content: String,
    /// Channel the message arrived on (e.g. "telegram", "panel", "cli").
    pub channel: String,
    /// Conversation / chat identifier within the channel.
    pub chat_id: String,
    /// Pass-through inbound metadata (skill activation, allow_from, …).
    pub metadata: std::collections::HashMap<String, serde_json::Value>,
}

/// Routing decision returned by a [`ContextRouter`].
///
/// All fields are optional in effect — empty `skills`, `None`
/// `tool_subset`, `0.0` hint, and `None` `archetype` mean
/// "no opinion, proceed normally".
#[derive(Debug, Clone)]
pub struct ContextDecision {
    /// Skills to inject into the system context for this turn.
    /// Names match `clawft-core/src/agent/skills*` registry entries.
    pub skills: Vec<String>,

    /// Optional restriction on which tools the LLM may see.
    ///
    /// Three-state semantics for plugin authors:
    /// - `None` — no opinion. Full tool registry is exposed. This is
    ///   the default and what every router should return when it has
    ///   nothing to say about tool scope.
    /// - `Some(list)` — narrow the registry to exactly those tool names
    ///   (intersected with the user's grant matrix downstream — the
    ///   router can only restrict, never expand). Names match the
    ///   `Tool::name()` returned by `clawft-tools::register_all`.
    /// - `Some(vec![])` — empty allowlist, deliberately. The LLM sees
    ///   ZERO tools this turn; tool-call iterations are short-circuited.
    ///   Use for pure-chat skills (e.g. summarisation) that explicitly
    ///   want to forbid tool use even when the user has granted access.
    ///
    /// `Some(vec![])` is NOT equivalent to `None` and NOT a no-op.
    /// Plugin authors writing a `ContextRouter` impl should default to
    /// `None` unless they have a specific reason to restrict.
    pub tool_subset: Option<Vec<String>>,

    /// Clamped `[-0.3, +0.3]` nudge on the classifier's complexity
    /// score. Wires straight into [`ChatRequest::complexity_boost`].
    /// **Never** picks a tier; downstream `TieredRouter` still owns
    /// that decision.
    pub complexity_hint: f32,

    /// Optional archetype label produced by a classifier router (Phase
    /// E1's [`LlmClassifierRouter`] populates this with values like
    /// `"Reasoning"`, `"CodeGen"`, `"Analysis"`, `"Conversational"`,
    /// or `"Creative"` — see `docs/research/rvf-context-router.md` §6).
    ///
    /// Today this is metadata only: the agent loop logs it for
    /// diagnostics but doesn't otherwise act on it. v2 / v2.5 routers
    /// (`EmbeddingRouter`, `HybridRouter`) will layer skill retrieval
    /// on top of the archetype to bias recall toward the right
    /// shelf. Surfaced here so the downstream wiring can land in a
    /// later commit without reshaping the trait.
    pub archetype: Option<String>,
}

impl ContextDecision {
    /// Construct a decision, clamping `complexity_hint` to the legal
    /// range. All in-trait construction should funnel through here.
    ///
    /// The `archetype` field is left `None`; classifier routers that
    /// produce an archetype label set it after construction.
    pub fn new(
        skills: Vec<String>,
        tool_subset: Option<Vec<String>>,
        complexity_hint: f32,
    ) -> Self {
        Self {
            skills,
            tool_subset,
            complexity_hint: clamp_complexity(complexity_hint),
            archetype: None,
        }
    }
}

impl Default for ContextDecision {
    fn default() -> Self {
        Self {
            skills: Vec::new(),
            tool_subset: None,
            complexity_hint: 0.0,
            archetype: None,
        }
    }
}

/// Pre-LLM router seam. Implementations decide skills + tool subset +
/// complexity hint without ever picking a model.
#[async_trait]
pub trait ContextRouter: Send + Sync + 'static {
    /// Inspect a [`ContextRequest`] and return the decision the agent
    /// loop should apply before assembling the LLM request.
    async fn route(&self, request: &ContextRequest) -> ContextDecision;
}

/// v0 router: returns the empty decision for every request.
///
/// This is the default attached to [`AgentLoop`](super::loop_core::AgentLoop)
/// so existing callers see exactly the same behaviour they had before
/// the seam landed. Phase E1 [`LlmClassifierRouter`] is the v1
/// successor; the swap is a config flip, not a code change.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullRouter;

#[async_trait]
impl ContextRouter for NullRouter {
    async fn route(&self, _request: &ContextRequest) -> ContextDecision {
        ContextDecision::default()
    }
}

// ── v1: LlmClassifierRouter ──────────────────────────────────────────────

// Phase E1: the LLM-classifier router lives in the `llm_classifier`
// submodule and is re-exported here so existing imports continue to
// see `agent::context_router::LlmClassifierRouter` etc. The split
// keeps this file under the 500-line CLAUDE.md cap.
pub mod llm_classifier;

pub use llm_classifier::{
    CLASSIFIER_SYSTEM_PROMPT, Classifier, ClassifierOutput, DEFAULT_CLASSIFIER_MAX_TOKENS,
    LlmClassifierRouter,
};

// ── v2: EmbeddingRouter ──────────────────────────────────────────────────

// Phase E2: the embedding-similarity router lives in the `embedding`
// submodule and is re-exported here under the `vector-memory` feature
// gate (it depends on the crate-local [`Embedder`](crate::embeddings)
// trait, which itself is `vector-memory`-only). The split keeps this
// file under the 500-line CLAUDE.md cap and lets the offline build
// (no `vector-memory`) skip the heavyweight Embedder + index plumbing.
#[cfg(feature = "vector-memory")]
pub mod embedding;

#[cfg(feature = "vector-memory")]
pub use embedding::{
    DEFAULT_CONFIDENCE_THRESHOLD, DEFAULT_TOP_K, EmbeddingRouter, EmbeddingRouterError,
    FALLBACK_TRACING_TARGET,
};

// ── v2.5: HybridRouter (plumbing only) ───────────────────────────────────

// Phase E3: chain a primary + fallback ContextRouter. Plumbing only —
// the sona-backed rerank step is deferred until ruv-ecosystem
// stability clears (see hybrid.rs module docs and
// `docs/research/rvf-context-router.md`). v3 (`MicroLoraRouter`) is
// also deferred until ruvllm-wasm lifts its 11-pattern HNSW cap; see
// the TODO marker on `HybridRouter`.
pub mod hybrid;

pub use hybrid::HybridRouter;

#[cfg(test)]
mod tests {
    use super::*;

    /// Object-safety probe. If the trait stops being object-safe (e.g.
    /// someone adds a `where Self: Sized` requirement to `route`), this
    /// fails to compile.
    #[allow(dead_code)]
    fn _coerce(_router: &dyn ContextRouter) {}

    #[tokio::test]
    async fn null_router_returns_empty_decision() {
        let router = NullRouter;
        let req = ContextRequest {
            content: "hello".into(),
            channel: "panel".into(),
            chat_id: "c1".into(),
            metadata: Default::default(),
        };
        let decision = router.route(&req).await;
        assert!(decision.skills.is_empty());
        assert!(decision.tool_subset.is_none());
        assert_eq!(decision.complexity_hint, 0.0);
    }

    #[test]
    fn default_decision_is_neutral() {
        let d = ContextDecision::default();
        assert!(d.skills.is_empty());
        assert!(d.tool_subset.is_none());
        assert_eq!(d.complexity_hint, 0.0);
    }

    #[test]
    fn clamp_complexity_passes_through_in_range() {
        assert_eq!(clamp_complexity(0.0), 0.0);
        assert_eq!(clamp_complexity(0.1), 0.1);
        assert_eq!(clamp_complexity(-0.25), -0.25);
        assert_eq!(clamp_complexity(0.3), 0.3);
        assert_eq!(clamp_complexity(-0.3), -0.3);
    }

    // The debug_assert! makes clamp_complexity panic on out-of-range
    // input in debug builds — that's the contract enforcement. Tests
    // in `cargo test` (debug profile) run with debug_asserts on, so
    // we cover the saturating path with std::panic::catch_unwind to
    // keep both invariants verifiable from one place.
    #[test]
    fn clamp_complexity_saturates_out_of_range() {
        // Out-of-range values panic in debug (the assert), saturate in
        // release. Verify both behaviours from a single test:
        //   - debug builds: catch the panic, that's the contract.
        //   - release builds: assert the saturation directly.
        let high = std::panic::catch_unwind(|| clamp_complexity(0.5));
        let low = std::panic::catch_unwind(|| clamp_complexity(-1.0));

        if cfg!(debug_assertions) {
            assert!(high.is_err(), "0.5 should panic via debug_assert");
            assert!(low.is_err(), "-1.0 should panic via debug_assert");
        } else {
            assert_eq!(high.unwrap(), COMPLEXITY_HINT_LIMIT);
            assert_eq!(low.unwrap(), -COMPLEXITY_HINT_LIMIT);
        }
    }

    #[test]
    fn clamp_complexity_handles_non_finite() {
        // NaN / infinities should never end up in complexity_boost.
        // debug_assert! fires on them; in release we coerce to 0.0.
        let nan = std::panic::catch_unwind(|| clamp_complexity(f32::NAN));
        let inf = std::panic::catch_unwind(|| clamp_complexity(f32::INFINITY));

        if cfg!(debug_assertions) {
            assert!(nan.is_err());
            assert!(inf.is_err());
        } else {
            assert_eq!(nan.unwrap(), 0.0);
            assert_eq!(inf.unwrap(), 0.0);
        }
    }

    #[test]
    fn context_decision_new_clamps_hint() {
        // In debug we'd panic on out-of-range; assert the in-range
        // round-trip plus the boundary.
        let d = ContextDecision::new(vec!["a".into()], Some(vec!["t".into()]), 0.2);
        assert_eq!(d.skills, vec!["a"]);
        assert_eq!(d.tool_subset.as_deref(), Some(&["t".to_string()][..]));
        assert!((d.complexity_hint - 0.2).abs() < f32::EPSILON);

        let edge = ContextDecision::new(vec![], None, COMPLEXITY_HINT_LIMIT);
        assert_eq!(edge.complexity_hint, COMPLEXITY_HINT_LIMIT);
    }

    #[test]
    fn context_decision_default_archetype_is_none() {
        // E1 contract: archetype defaults to None so v0 NullRouter
        // and any pre-E1 caller stays archetype-free.
        let d = ContextDecision::default();
        assert!(d.archetype.is_none());

        let n = ContextDecision::new(vec![], None, 0.0);
        assert!(n.archetype.is_none());
    }
}
