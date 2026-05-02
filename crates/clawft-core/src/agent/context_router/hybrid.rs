//! v2.5 [`ContextRouter`] composition: chain a primary + fallback.
//!
//! Phase E3 of `docs/plans/agent-core-v1.md`. Ships **plumbing only**.
//! The real v2.5 design layers a sona-backed rerank step on top of the
//! primary's top-K — that lands once `sona` clears the ruv-ecosystem
//! stability gate
//! (`.planning/development_notes/ruv-ecosystem-analysis-20260414.md`).
//! Until then, [`HybridRouter`] is a deterministic chain: try the
//! stronger router (typically the v2
//! [`EmbeddingRouter`](super::EmbeddingRouter)) first; on a structurally
//! empty decision (no skills, no archetype, no `tool_subset`,
//! `complexity_hint == 0.0`) fall through to the cheaper fallback
//! (typically the v1
//! [`LlmClassifierRouter`](super::LlmClassifierRouter)).
//!
//! ## Hard contract (do not break)
//!
//! Per `docs/research/rvf-context-router.md`: the chain NEVER picks a
//! model and NEVER escalates a tier. Every member of the chain emits a
//! [`ContextDecision`] in the same shape; [`HybridRouter`] just selects
//! between them. The B1 [`clamp_complexity`](super::clamp_complexity)
//! invariant on `complexity_hint ∈ [-0.3, +0.3]` is preserved at every
//! boundary — `HybridRouter` never reads or rewrites the hint, so the
//! producing router's clamp survives intact.
//!
//! ## Why "empty decision" instead of a numeric threshold
//!
//! E3 is plumbing only: we don't have a sona-backed reranker yet, so we
//! can't compare confidences across heterogeneous primaries (an
//! [`EmbeddingRouter`](super::EmbeddingRouter) cosine score and an
//! [`LlmClassifierRouter`](super::LlmClassifierRouter) archetype label
//! aren't comparable on the same axis). The v2 router already collapses
//! to [`ContextDecision::default()`] when its top-1 cosine falls below
//! `confidence_threshold`, so "primary returned default-shape" is
//! *exactly* "primary had low confidence" without any extra signaling.
//! That's the cheap-correct seam for v2.5's plumbing pass.

use std::sync::Arc;

use async_trait::async_trait;

use super::{ContextDecision, ContextRequest, ContextRouter};

// TODO(agent-core-v1 phase E3+): wire MicroLoraRouter (v3) once
// ruvllm-wasm lifts the 11-pattern HNSW cap
// (docs/research/rvf-context-router.md:118-128). The 35+-skill clawft
// catalog overruns ruvllm-wasm v2.0.1's documented per-index ceiling,
// which is why v3 is held until upstream lands a larger cap. The v2.5
// rerank step (sona-backed) is also deferred until ruv-ecosystem
// stability clears — see ruv-ecosystem-analysis-20260414.md.

/// Chains two [`ContextRouter`]s. The primary runs first; if its
/// [`ContextDecision`] is structurally empty (no skills, no archetype,
/// no `tool_subset`, `complexity_hint == 0.0`) the fallback runs and
/// its decision is returned.
///
/// E3 ships the plumbing only. The real v2.5 design layers a sona-
/// backed rerank step on top of the primary's top-K — that lands once
/// `sona` clears the ruv-ecosystem stability gate. Until then,
/// `HybridRouter` is a deterministic chain: try the stronger router
/// first ([`EmbeddingRouter`](super::EmbeddingRouter), v2), fall back
/// to the cheaper one ([`LlmClassifierRouter`](super::LlmClassifierRouter),
/// v1) when retrieval is weak.
pub struct HybridRouter {
    primary: Arc<dyn ContextRouter>,
    fallback: Arc<dyn ContextRouter>,
}

impl HybridRouter {
    /// Compose a primary + fallback chain. Both arms are read-only on
    /// the route hot path; the wrapper itself is `Arc`-friendly to
    /// match the `Arc<dyn ContextRouter>` shape the daemon hands to
    /// [`build_daemon_agent_loop`](crate::bootstrap::build_daemon_agent_loop).
    pub fn new(primary: Arc<dyn ContextRouter>, fallback: Arc<dyn ContextRouter>) -> Self {
        Self { primary, fallback }
    }
}

#[async_trait]
impl ContextRouter for HybridRouter {
    async fn route(&self, request: &ContextRequest) -> ContextDecision {
        let primary_decision = self.primary.route(request).await;
        if is_empty_decision(&primary_decision) {
            tracing::info!(
                target: "context_router.hybrid_fallback",
                "HybridRouter: primary returned empty; falling through to fallback"
            );
            self.fallback.route(request).await
        } else {
            primary_decision
        }
    }
}

/// Structural emptiness check — does this decision carry any signal at
/// all? Mirrors the four user-visible fields on [`ContextDecision`]:
/// `skills`, `archetype`, `tool_subset`, `complexity_hint`. If none of
/// them carry a value, the decision is indistinguishable from
/// [`ContextDecision::default()`] and the chain falls through.
fn is_empty_decision(d: &ContextDecision) -> bool {
    d.skills.is_empty()
        && d.archetype.is_none()
        && d.tool_subset.is_none()
        && d.complexity_hint == 0.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Counting stub: records how many times `route` was called and
    /// returns the same canned [`ContextDecision`] each time. This is
    /// the same pattern B1 / E1 used to keep the router test surface
    /// narrow without spinning up real backends.
    struct StubRouter {
        decision: ContextDecision,
        calls: Mutex<usize>,
    }

    impl StubRouter {
        fn new(decision: ContextDecision) -> Self {
            Self {
                decision,
                calls: Mutex::new(0),
            }
        }

        fn calls(&self) -> usize {
            *self.calls.lock().unwrap()
        }
    }

    #[async_trait]
    impl ContextRouter for StubRouter {
        async fn route(&self, _request: &ContextRequest) -> ContextDecision {
            *self.calls.lock().unwrap() += 1;
            self.decision.clone()
        }
    }

    fn req() -> ContextRequest {
        ContextRequest {
            content: "hello".into(),
            channel: "panel".into(),
            chat_id: "c1".into(),
            metadata: Default::default(),
        }
    }

    #[tokio::test]
    async fn hybrid_returns_primary_when_non_empty() {
        // Primary has a real signal (skills + non-zero hint) — the
        // chain returns it verbatim and never touches the fallback.
        let primary_decision =
            ContextDecision::new(vec!["x".into()], None, 0.1);
        let primary = Arc::new(StubRouter::new(primary_decision));
        let fallback = Arc::new(StubRouter::new(ContextDecision::default()));

        let router = HybridRouter::new(
            primary.clone() as Arc<dyn ContextRouter>,
            fallback.clone() as Arc<dyn ContextRouter>,
        );
        let decision = router.route(&req()).await;

        assert_eq!(decision.skills, vec!["x".to_string()]);
        assert!((decision.complexity_hint - 0.1).abs() < 1e-5);
        assert_eq!(primary.calls(), 1);
        assert_eq!(
            fallback.calls(),
            0,
            "fallback must not run when primary is non-empty"
        );
    }

    #[tokio::test]
    async fn hybrid_falls_back_on_empty_primary() {
        // Primary returns the empty decision; fallback wins. The
        // fallback's archetype-bearing decision must surface unchanged.
        let primary = Arc::new(StubRouter::new(ContextDecision::default()));
        let mut fb = ContextDecision::new(Vec::new(), None, 0.2);
        fb.archetype = Some("X".into());
        let fallback = Arc::new(StubRouter::new(fb));

        let router = HybridRouter::new(
            primary.clone() as Arc<dyn ContextRouter>,
            fallback.clone() as Arc<dyn ContextRouter>,
        );
        let decision = router.route(&req()).await;

        assert_eq!(decision.archetype.as_deref(), Some("X"));
        assert!((decision.complexity_hint - 0.2).abs() < 1e-5);
        assert!(decision.skills.is_empty());
        assert_eq!(primary.calls(), 1);
        assert_eq!(fallback.calls(), 1);
    }

    #[tokio::test]
    async fn hybrid_returns_default_when_both_empty() {
        // Both arms are empty — the chain returns the fallback's
        // (empty) decision. Both arms are called exactly once.
        let primary = Arc::new(StubRouter::new(ContextDecision::default()));
        let fallback = Arc::new(StubRouter::new(ContextDecision::default()));

        let router = HybridRouter::new(
            primary.clone() as Arc<dyn ContextRouter>,
            fallback.clone() as Arc<dyn ContextRouter>,
        );
        let decision = router.route(&req()).await;

        assert!(decision.skills.is_empty());
        assert!(decision.archetype.is_none());
        assert!(decision.tool_subset.is_none());
        assert_eq!(decision.complexity_hint, 0.0);
        assert_eq!(primary.calls(), 1);
        assert_eq!(fallback.calls(), 1);
    }

    #[tokio::test]
    async fn hybrid_does_not_call_fallback_when_primary_has_skills() {
        // Skills-only primary decision counts as non-empty.
        let primary_decision =
            ContextDecision::new(vec!["alpha".into(), "beta".into()], None, 0.0);
        let primary = Arc::new(StubRouter::new(primary_decision));
        let fallback = Arc::new(StubRouter::new(ContextDecision::default()));

        let router = HybridRouter::new(
            primary.clone() as Arc<dyn ContextRouter>,
            fallback.clone() as Arc<dyn ContextRouter>,
        );
        let decision = router.route(&req()).await;

        assert_eq!(
            decision.skills,
            vec!["alpha".to_string(), "beta".to_string()]
        );
        assert_eq!(decision.complexity_hint, 0.0);
        assert_eq!(primary.calls(), 1);
        assert_eq!(fallback.calls(), 0);
    }

    #[tokio::test]
    async fn hybrid_does_not_call_fallback_when_primary_has_archetype() {
        // Archetype-only primary decision counts as non-empty even
        // though `skills` is empty and `complexity_hint == 0.0`.
        let mut d = ContextDecision::new(Vec::new(), None, 0.0);
        d.archetype = Some("Reasoning".into());
        let primary = Arc::new(StubRouter::new(d));
        let fallback = Arc::new(StubRouter::new(ContextDecision::default()));

        let router = HybridRouter::new(
            primary.clone() as Arc<dyn ContextRouter>,
            fallback.clone() as Arc<dyn ContextRouter>,
        );
        let decision = router.route(&req()).await;

        assert_eq!(decision.archetype.as_deref(), Some("Reasoning"));
        assert!(decision.skills.is_empty());
        assert_eq!(primary.calls(), 1);
        assert_eq!(fallback.calls(), 0);
    }

    #[tokio::test]
    async fn hybrid_does_not_call_fallback_when_primary_has_complexity() {
        // Non-zero complexity_hint counts as non-empty even with no
        // skills + no archetype + no tool_subset. This guards the v1
        // classifier's "complexity-only" outputs (it can return just
        // `{archetype: ..., complexity: -0.2}` without skills).
        let primary_decision =
            ContextDecision::new(Vec::new(), None, -0.2);
        let primary = Arc::new(StubRouter::new(primary_decision));
        let fallback = Arc::new(StubRouter::new(ContextDecision::default()));

        let router = HybridRouter::new(
            primary.clone() as Arc<dyn ContextRouter>,
            fallback.clone() as Arc<dyn ContextRouter>,
        );
        let decision = router.route(&req()).await;

        assert!((decision.complexity_hint + 0.2).abs() < 1e-5);
        assert!(decision.skills.is_empty());
        assert!(decision.archetype.is_none());
        assert_eq!(primary.calls(), 1);
        assert_eq!(fallback.calls(), 0);
    }

    #[test]
    fn is_empty_decision_default_is_empty() {
        // Sanity-check the structural predicate against
        // ContextDecision::default(): the chain *must* fall through on
        // it, otherwise the v2.5 plumbing degenerates into "always
        // returns the primary".
        let d = ContextDecision::default();
        assert!(is_empty_decision(&d));
    }

    #[test]
    fn is_empty_decision_tool_subset_some_empty_is_non_empty() {
        // `Some(vec![])` is "no tools at all" — a real, intentional
        // signal (per the trait docs), not the absence of one. Treat
        // it as non-empty so the chain doesn't override an explicit
        // pure-chat skill restriction.
        let d = ContextDecision {
            skills: Vec::new(),
            tool_subset: Some(Vec::new()),
            complexity_hint: 0.0,
            archetype: None,
        };
        assert!(!is_empty_decision(&d));
    }
}
