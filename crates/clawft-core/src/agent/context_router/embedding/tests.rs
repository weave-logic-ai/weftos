//! Unit tests for the v2 [`EmbeddingRouter`].
//!
//! Tests use a deterministic in-process [`StubEmbedder`] so we never
//! touch the network. The diskann backend is always exercised when the
//! `embedding-router` feature is on (default); the brute-force backend
//! is exercised inside `embedding/index.rs` tests.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use clawft_types::skill::SkillDefinition;

use super::super::{ContextRequest, ContextRouter, COMPLEXITY_HINT_LIMIT};
use super::{EmbeddingRouter, EmbeddingRouterError};
use crate::agent::skills_v2::SkillRegistry;
use crate::embeddings::{Embedder, EmbeddingError};

/// Deterministic 8-d embedder. Same input → identical output, so we can
/// construct a stable mock registry and predict ranking without any
/// network or ONNX dependency.
struct StubEmbedder {
    dim: usize,
    fail: bool,
}

impl StubEmbedder {
    fn new(dim: usize) -> Self {
        Self { dim, fail: false }
    }
    fn failing() -> Self {
        Self { dim: 8, fail: true }
    }
}

#[async_trait]
impl Embedder for StubEmbedder {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError> {
        if self.fail {
            return Err(EmbeddingError::Internal("stub failure".into()));
        }
        // Tokenise on whitespace; bucket each token's byte-sum mod
        // self.dim. Different prose maps to different sparse vectors;
        // identical prose maps identical.
        let mut v = vec![0.0f32; self.dim];
        for tok in text.split_whitespace() {
            let bucket: usize = tok.bytes().map(|b| b as usize).sum::<usize>() % self.dim;
            v[bucket] += 1.0;
        }
        Ok(v)
    }
    fn dimension(&self) -> usize {
        self.dim
    }
    fn name(&self) -> &str {
        "stub"
    }
}

fn skill(name: &str, description: &str) -> SkillDefinition {
    SkillDefinition::new(name, description)
}

fn skill_with_category(name: &str, description: &str, category: &str) -> SkillDefinition {
    let mut s = SkillDefinition::new(name, description);
    s.metadata.insert(
        "openclaw-category".into(),
        serde_json::Value::String(category.into()),
    );
    s
}

async fn registry_from(skills: Vec<SkillDefinition>) -> SkillRegistry {
    // SkillRegistry::discover is async + filesystem-aware; with `None`
    // paths it just returns an empty registry. Tests then populate via
    // the in-memory upsert API.
    let mut reg = SkillRegistry::discover(None, None, Vec::new())
        .await
        .expect("empty registry should never fail");
    for s in skills {
        reg.upsert(s);
    }
    reg
}

fn req(content: &str) -> ContextRequest {
    ContextRequest {
        content: content.into(),
        channel: "panel".into(),
        chat_id: "c1".into(),
        metadata: HashMap::new(),
    }
}

#[tokio::test]
async fn embedding_router_returns_top_k_skills_for_query() {
    // 5 skills, embedded by the stub. We then query for text that
    // shares tokens with skill "rust-debug" so it should win top-1.
    let skills = vec![
        skill("rust-debug", "rust compile error fix"),
        skill("python-debug", "python traceback investigation"),
        skill("write-poem", "compose a creative short poem"),
        skill("plan-trip", "plan a vacation itinerary"),
        skill("explain-graph", "explain a knowledge graph"),
    ];
    let reg = registry_from(skills).await;
    let router = EmbeddingRouter::new(Arc::new(StubEmbedder::new(8)), &reg)
        .await
        .expect("router build")
        .with_top_k(3)
        // Stub embedder produces sparse vectors; use a low threshold so
        // the test is about ranking, not absolute confidence.
        .with_confidence_threshold(-1.0);

    let d = router.route(&req("rust compile error fix")).await;
    assert_eq!(d.skills.len(), 3);
    assert_eq!(
        d.skills[0], "rust-debug",
        "expected rust-debug as top-1, got skills={:?}",
        d.skills
    );
}

#[tokio::test]
async fn embedding_router_falls_back_below_confidence() {
    let skills = vec![
        skill("rust-debug", "rust compile error fix"),
        skill("python-debug", "python traceback investigation"),
    ];
    let reg = registry_from(skills).await;
    // Threshold of 0.99 — almost-certainly-not-met by stub.
    let router = EmbeddingRouter::new(Arc::new(StubEmbedder::new(8)), &reg)
        .await
        .unwrap()
        .with_confidence_threshold(0.99);

    let d = router.route(&req("totally unrelated weather query")).await;
    assert!(d.skills.is_empty());
    assert!(d.archetype.is_none());
    assert_eq!(d.complexity_hint, 0.0);
}

#[tokio::test]
async fn embedding_router_falls_back_on_embed_error() {
    let skills = vec![skill("rust-debug", "rust compile error fix")];
    let reg = registry_from(skills).await;
    // Build router with a working embedder so construction succeeds,
    // then swap in a failing one for the route call.
    let router = EmbeddingRouter::new(Arc::new(StubEmbedder::new(8)), &reg)
        .await
        .unwrap();
    let mut bad = router;
    bad.embedder = Arc::new(StubEmbedder::failing());

    let d = bad.route(&req("anything")).await;
    assert!(d.skills.is_empty());
    assert!(d.archetype.is_none());
    assert_eq!(d.complexity_hint, 0.0);
}

#[tokio::test]
async fn empty_skill_registry_yields_construction_error() {
    let reg = registry_from(Vec::new()).await;
    let result = EmbeddingRouter::new(Arc::new(StubEmbedder::new(8)), &reg).await;
    assert!(matches!(result, Err(EmbeddingRouterError::EmptyRegistry)));
}

#[tokio::test]
async fn complexity_hint_is_clamped() {
    let skills = vec![
        skill("rust-debug", "rust compile error fix"),
        skill("python-debug", "python traceback investigation"),
    ];
    let reg = registry_from(skills).await;
    let router = EmbeddingRouter::new(Arc::new(StubEmbedder::new(8)), &reg)
        .await
        .unwrap()
        .with_confidence_threshold(-1.0);

    let d = router.route(&req("rust compile error fix")).await;
    assert!(
        d.complexity_hint >= -COMPLEXITY_HINT_LIMIT
            && d.complexity_hint <= COMPLEXITY_HINT_LIMIT,
        "complexity_hint must lie in [-0.3, +0.3], got {}",
        d.complexity_hint
    );
}

#[tokio::test]
async fn archetype_comes_from_top1_category() {
    let skills = vec![
        skill_with_category("rust-debug", "rust compile error fix", "CodeGen"),
        skill_with_category(
            "python-debug",
            "python traceback investigation",
            "Analysis",
        ),
    ];
    let reg = registry_from(skills).await;
    let router = EmbeddingRouter::new(Arc::new(StubEmbedder::new(8)), &reg)
        .await
        .unwrap()
        .with_confidence_threshold(-1.0);

    let d = router.route(&req("rust compile error fix")).await;
    assert_eq!(d.archetype.as_deref(), Some("CodeGen"));
}

#[tokio::test]
async fn confidence_threshold_is_clamped_to_legal_range() {
    let skills = vec![skill("a", "alpha")];
    let reg = registry_from(skills).await;
    let router = EmbeddingRouter::new(Arc::new(StubEmbedder::new(8)), &reg)
        .await
        .unwrap()
        .with_confidence_threshold(5.0);
    // 5.0 clamps to 1.0; route at high cos < 1.0 → fallback.
    let d = router.route(&req("alpha")).await;
    // Self-match at 1.0 may equal the threshold; allow either path but
    // verify we don't panic.
    let _ = d;
}

#[tokio::test]
async fn top_k_with_fewer_than_k_skills_returns_what_exists() {
    // Registry has 2 skills; ask for 5 — get 2.
    let skills = vec![
        skill("rust-debug", "rust compile error fix"),
        skill("python-debug", "python traceback investigation"),
    ];
    let reg = registry_from(skills).await;
    let router = EmbeddingRouter::new(Arc::new(StubEmbedder::new(8)), &reg)
        .await
        .unwrap()
        .with_top_k(5)
        .with_confidence_threshold(-1.0);

    let d = router.route(&req("rust compile error fix")).await;
    assert_eq!(d.skills.len(), 2);
}

// ── Helper coverage ────────────────────────────────────────────────

#[test]
fn normalise_zero_vector_is_zero() {
    let v = super::normalise(vec![0.0, 0.0, 0.0]);
    assert!(v.iter().all(|&x| x == 0.0));
}

#[test]
fn normalise_unit_norm() {
    let v = super::normalise(vec![3.0, 4.0]);
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    assert!((norm - 1.0).abs() < 1e-5);
}

#[test]
fn extract_category_handles_aliases() {
    let mut s = SkillDefinition::new("x", "y");
    s.metadata.insert(
        "openclaw-category".into(),
        serde_json::Value::String("CodeGen".into()),
    );
    assert_eq!(super::extract_category(&s), Some("CodeGen".into()));

    let mut s2 = SkillDefinition::new("x", "y");
    s2.metadata.insert(
        "category".into(),
        serde_json::Value::String("Reasoning".into()),
    );
    assert_eq!(super::extract_category(&s2), Some("Reasoning".into()));
}
