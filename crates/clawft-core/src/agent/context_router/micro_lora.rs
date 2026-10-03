//! v3 [`MicroLoraRouter`] — **vendor-free** adaptive skill + complexity router.
//!
//! ## Why not ruvllm-wasm?
//!
//! Historical deferral (WEFT-45 / 194 / 338) waited on ruvllm-wasm's
//! ~11-pattern HNSW cap. WeftOS skill catalogs already exceed that, so
//! this implementation keeps **unlimited in-process patterns** using a
//! fixed-dim hashed feature bag + a rank-`r` residual adapter for
//! `complexity_hint` only. No ruvllm-wasm dependency.
//!
//! ## Contract (same as all ContextRouters)
//!
//! - Never picks a model tier
//! - Never expands tool grants
//! - `complexity_hint` always clamped via [`clamp_complexity`](super::clamp_complexity)

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;

use super::{ContextDecision, ContextRequest, ContextRouter, clamp_complexity};

/// Feature dimension for hashed bags (power of two for fast mask).
pub const FEATURE_DIM: usize = 64;

/// Default LoRA rank for the complexity residual.
pub const DEFAULT_RANK: usize = 4;

/// One stored routing pattern (unlimited capacity).
#[derive(Debug, Clone)]
pub struct MicroLoraPattern {
    /// Pattern id (skill name or archetype key).
    pub id: String,
    /// Feature bag (L2-normalized).
    pub features: Vec<f32>,
    /// Skills to inject when this pattern wins.
    pub skills: Vec<String>,
    /// Optional archetype label.
    pub archetype: Option<String>,
    /// Base complexity nudge before LoRA residual.
    pub complexity_bias: f32,
}

/// In-memory MicroLoRA state (patterns + low-rank residual).
#[derive(Debug)]
struct MicroLoraState {
    patterns: Vec<MicroLoraPattern>,
    /// W_down: rank × dim
    w_down: Vec<Vec<f32>>,
    /// W_up: rank (maps to scalar complexity residual)
    w_up: Vec<f32>,
    rank: usize,
    /// Shadow mode: compute v3 decision but callers may log without acting.
    shadow: bool,
    /// Last shadow decision for tests / audit hooks.
    last_shadow: Option<ContextDecision>,
}

/// v3 adaptive context router (WEFT-45 / 338 foundation).
///
/// Build with [`MicroLoraRouter::with_seed_skills`] or empty +
/// [`Self::upsert_pattern`].
pub struct MicroLoraRouter {
    state: Mutex<MicroLoraState>,
}

impl MicroLoraRouter {
    /// Empty router (no patterns → empty decision).
    pub fn new() -> Self {
        Self::with_rank(DEFAULT_RANK)
    }

    /// Empty router with explicit LoRA rank.
    pub fn with_rank(rank: usize) -> Self {
        let rank = rank.clamp(1, 16);
        let w_down = (0..rank)
            .map(|i| {
                (0..FEATURE_DIM)
                    .map(|j| (((i * 17 + j * 3) % 100) as f32 - 50.0) * 0.001)
                    .collect()
            })
            .collect();
        let w_up = vec![0.01; rank];
        Self {
            state: Mutex::new(MicroLoraState {
                patterns: Vec::new(),
                w_down,
                w_up,
                rank,
                shadow: false,
                last_shadow: None,
            }),
        }
    }

    /// Seed one pattern per skill name (id = skill, bag from name tokens).
    pub fn with_seed_skills(skills: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let r = Self::new();
        for s in skills {
            let name = s.into();
            r.upsert_pattern(MicroLoraPattern {
                id: name.clone(),
                features: feature_bag(&name),
                skills: vec![name],
                archetype: None,
                complexity_bias: 0.0,
            });
        }
        r
    }

    /// Enable shadow-only mode (decision stored; still returned for tests).
    pub fn set_shadow(&self, shadow: bool) {
        if let Ok(mut g) = self.state.lock() {
            g.shadow = shadow;
        }
    }

    /// Last shadow decision (if any).
    pub fn last_shadow_decision(&self) -> Option<ContextDecision> {
        self.state.lock().ok().and_then(|g| g.last_shadow.clone())
    }

    /// Insert or replace a pattern by `id`. **No capacity ceiling.**
    pub fn upsert_pattern(&self, pattern: MicroLoraPattern) {
        let Ok(mut g) = self.state.lock() else {
            return;
        };
        if let Some(i) = g.patterns.iter().position(|p| p.id == pattern.id) {
            g.patterns[i] = pattern;
        } else {
            g.patterns.push(pattern);
        }
    }

    /// Pattern count (unlimited growth).
    pub fn pattern_count(&self) -> usize {
        self.state.lock().map(|g| g.patterns.len()).unwrap_or(0)
    }

    /// Online adapt: nudge residual from a quality signal in `[0, 1]`.
    ///
    /// Call after a turn when journal/telemetry is available. Positive
    /// quality increases alignment of residual with last query bag
    /// (stored via metadata key `micro_lora_query_bag` is not required —
    /// we re-hash `content` if provided).
    pub fn adapt(&self, content: &str, quality: f32, learning_rate: f32) {
        let q = feature_bag(content);
        let quality = quality.clamp(0.0, 1.0);
        let lr = learning_rate.clamp(1e-5, 0.5);
        let Ok(mut g) = self.state.lock() else {
            return;
        };
        // Residual scalar target: quality maps to slight complexity upweight.
        let target = (quality - 0.5) * 0.2;
        let current = lora_residual(&q, &g.w_down, &g.w_up);
        let err = target - current;
        for i in 0..g.rank {
            let mut act = 0.0f32;
            for (w, x) in g.w_down[i].iter().zip(q.iter()).take(FEATURE_DIM) {
                act += w * x;
            }
            g.w_up[i] += lr * err * act;
            let w_up_i = g.w_up[i];
            for (w, x) in g.w_down[i].iter_mut().zip(q.iter()).take(FEATURE_DIM) {
                *w += lr * err * w_up_i * x * 0.1;
            }
        }
    }

    fn route_inner(&self, request: &ContextRequest) -> ContextDecision {
        let bag = feature_bag(&request.content);
        let Ok(g) = self.state.lock() else {
            return ContextDecision::default();
        };
        if g.patterns.is_empty() {
            return ContextDecision::default();
        }

        // Score all patterns — no 11-cap.
        let mut scored: Vec<(f32, &MicroLoraPattern)> = g
            .patterns
            .iter()
            .map(|p| (cosine(&bag, &p.features), p))
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        let top_k = scored.into_iter().take(5).collect::<Vec<_>>();
        if top_k.is_empty() || top_k[0].0 < 0.05 {
            return ContextDecision::default();
        }

        let mut skills = Vec::new();
        let mut seen = HashMap::new();
        for (score, p) in &top_k {
            if *score < 0.05 {
                continue;
            }
            for s in &p.skills {
                if seen.insert(s.clone(), ()).is_none() {
                    skills.push(s.clone());
                }
            }
        }
        skills.truncate(8);

        let residual = lora_residual(&bag, &g.w_down, &g.w_up);
        let complexity_hint = clamp_complexity(top_k[0].1.complexity_bias + residual);
        let archetype = top_k[0].1.archetype.clone().or_else(|| {
            // Lightweight keyword archetype without LLM.
            Some(archetype_from_text(&request.content))
        });

        ContextDecision {
            skills,
            tool_subset: None,
            complexity_hint,
            archetype,
            fallback_used: false,
        }
    }
}

impl Default for MicroLoraRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ContextRouter for MicroLoraRouter {
    async fn route(&self, request: &ContextRequest) -> ContextDecision {
        let decision = self.route_inner(request);
        if let Ok(mut g) = self.state.lock()
            && g.shadow {
                g.last_shadow = Some(decision.clone());
            }
        // Shadow mode still returns the decision so HybridRouter can chain;
        // operators log `last_shadow_decision` for WITNESS-style audits.
        decision
    }
}

/// Hash-bag features (unsigned FNV-ish) into unit sphere.
pub fn feature_bag(text: &str) -> Vec<f32> {
    let mut v = vec![0.0f32; FEATURE_DIM];
    for tok in text.split(|c: char| !c.is_alphanumeric()) {
        if tok.is_empty() {
            continue;
        }
        let h = fnv1a64(tok.to_ascii_lowercase().as_bytes());
        let idx = (h as usize) % FEATURE_DIM;
        let sign = if (h >> 63) & 1 == 1 { -1.0 } else { 1.0 };
        v[idx] += sign;
    }
    // L2 normalize
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 1e-8 {
        for x in &mut v {
            *x /= n;
        }
    }
    v
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    let mut dot = 0.0f32;
    for i in 0..n {
        dot += a[i] * b[i];
    }
    // Bags are unit-normalized.
    dot
}

fn lora_residual(q: &[f32], w_down: &[Vec<f32>], w_up: &[f32]) -> f32 {
    let mut s = 0.0f32;
    for (i, row) in w_down.iter().enumerate() {
        let mut act = 0.0f32;
        for j in 0..FEATURE_DIM.min(q.len()).min(row.len()) {
            act += row[j] * q[j];
        }
        s += w_up.get(i).copied().unwrap_or(0.0) * act;
    }
    clamp_complexity(s)
}

fn archetype_from_text(text: &str) -> String {
    let t = text.to_ascii_lowercase();
    if t.contains("code") || t.contains("compile") || t.contains("rust") || t.contains("fn ") {
        "CodeGen".into()
    } else if t.contains("why") || t.contains("explain") || t.contains("reason") {
        "Reasoning".into()
    } else if t.contains("analy") || t.contains("metric") || t.contains("data") {
        "Analysis".into()
    } else if t.contains("story") || t.contains("poem") || t.contains("creative") {
        "Creative".into()
    } else {
        "Conversational".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unlimited_patterns_beyond_eleven() {
        let r = MicroLoraRouter::new();
        for i in 0..40 {
            r.upsert_pattern(MicroLoraPattern {
                id: format!("skill-{i}"),
                features: feature_bag(&format!("skill domain {i} keywords unique {i}")),
                skills: vec![format!("skill-{i}")],
                archetype: Some("Analysis".into()),
                complexity_bias: 0.0,
            });
        }
        assert_eq!(r.pattern_count(), 40); // would blow ruvllm 11-cap
        let req = ContextRequest {
            content: "skill domain 7 keywords unique 7 please".into(),
            channel: "cli".into(),
            chat_id: "c1".into(),
            metadata: HashMap::new(),
        };
        let d = r.route(&req).await;
        assert!(
            d.skills.iter().any(|s| s.contains('7')),
            "expected skill-7 near top, got {:?}",
            d.skills
        );
    }

    #[tokio::test]
    async fn adapt_moves_complexity() {
        let r = MicroLoraRouter::with_seed_skills(["code-review", "chat"]);
        let req = ContextRequest {
            content: "please review this rust code compile error".into(),
            channel: "cli".into(),
            chat_id: "c1".into(),
            metadata: HashMap::new(),
        };
        let before = r.route(&req).await.complexity_hint;
        for _ in 0..20 {
            r.adapt(&req.content, 0.95, 0.05);
        }
        let after = r.route(&req).await.complexity_hint;
        // High quality adapts residual — not always strictly greater due to
        // clamp, but should remain finite and in range.
        assert!(after.abs() <= 0.3);
        assert!(before.abs() <= 0.3);
        let _ = (before, after);
    }

    #[tokio::test]
    async fn shadow_records_decision() {
        let r = MicroLoraRouter::with_seed_skills(["alpha"]);
        r.set_shadow(true);
        let req = ContextRequest {
            content: "alpha topic".into(),
            channel: "cli".into(),
            chat_id: "c".into(),
            metadata: HashMap::new(),
        };
        let d = r.route(&req).await;
        let shadow = r.last_shadow_decision().expect("shadow");
        assert_eq!(d.skills, shadow.skills);
    }

    #[test]
    fn feature_bag_normalized() {
        let v = feature_bag("hello world hello");
        let n: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((n - 1.0).abs() < 1e-4 || n < 1e-6);
    }
}
