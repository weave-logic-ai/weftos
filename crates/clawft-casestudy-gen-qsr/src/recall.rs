//! HNSW recall survey on financial / operational feature vectors.
//!
//! Uses `instant-distance` (the same crate the kernel's `HnswService`
//! delegates to) to answer the empirical question deferred in §9 of the
//! analysis: *does HNSW recall hold up on QSR-style feature vectors?*
//!
//! Protocol:
//! 1. Featurize every rollup into a fixed-dim vector (daily / weekly /
//!    monthly variants).
//! 2. Sample N query points from the corpus.
//! 3. Brute-force exact top-k for each query (the oracle).
//! 4. Build HNSW at several `ef_search` settings.
//! 5. Report recall@k = |HNSW_topk ∩ exact_topk| / k, plus latencies.

use crate::dimensions::Dimensions;
use crate::events::DailyRollup;
use crate::rollup::{MonthlyRollup, WeeklyRollup};
use instant_distance::{Builder, HnswMap, Point, Search};
use rand::seq::IteratorRandom;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Feature vector + Point impl
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct FeatureVector(pub Vec<f32>);

impl Point for FeatureVector {
    fn distance(&self, other: &Self) -> f32 {
        let mut acc = 0.0f32;
        for (a, b) in self.0.iter().zip(&other.0) {
            let d = a - b;
            acc += d * d;
        }
        acc.sqrt()
    }
}

// ---------------------------------------------------------------------------
// Featurisation — 28-dim vectors
// ---------------------------------------------------------------------------
//
// Scheme:
// - Normalised financial ratios (5 dims)
// - Operational counts normalised (3 dims)
// - DOW one-hot (7 dims) — identity for weekly/monthly since collapsed
// - Month-of-year sin/cos (2 dims)
// - Day-of-year sin/cos (2 dims)
// - Promo activity (1 dim)
// - Days-covered (for rollups; =1 for daily)
// - Budget variance sign + magnitude (2 dims)
// Total: 23 dims. Deliberately small to match the real-world shape of
// per-store-per-day features (not text embeddings).

const DIM: usize = 23;

pub fn featurize_daily(rollup: &DailyRollup, baseline_daily_sales: f64) -> FeatureVector {
    let baseline = baseline_daily_sales.max(1.0) as f32;
    let rev = rollup.revenue as f32 / baseline;
    let cogs_r = (rollup.cogs / rollup.revenue.max(1.0)) as f32;
    let labor_r = (rollup.labor / rollup.revenue.max(1.0)) as f32;
    let labor_hours = (rollup.labor_hours as f32) / 100.0;
    let avg_ticket = (rollup.avg_ticket as f32) / 20.0;

    let tickets = (rollup.tickets as f32) / 500.0;
    let budget_var = rollup.budget_variance_pct as f32;
    let budget_sign = budget_var.signum();
    let budget_mag = budget_var.abs();

    let mut v = vec![rev, cogs_r, labor_r, labor_hours, avg_ticket, tickets];

    // DOW one-hot
    let dow = (rollup.day_index % 7) as usize;
    for i in 0..7 {
        v.push(if i == dow { 1.0 } else { 0.0 });
    }
    // Month sin/cos
    let month_phase = (rollup.day_index as f32 / 30.0) * std::f32::consts::TAU;
    v.push(month_phase.sin());
    v.push(month_phase.cos());
    // Year sin/cos
    let year_phase = (rollup.day_index as f32 / 365.0) * std::f32::consts::TAU;
    v.push(year_phase.sin());
    v.push(year_phase.cos());

    v.push(rollup.promo_codes_active.len() as f32);
    v.push(1.0); // days_covered
    v.push(budget_sign);
    v.push(budget_mag);

    debug_assert_eq!(v.len(), DIM);
    FeatureVector(v)
}

pub fn featurize_weekly(rollup: &WeeklyRollup, baseline_weekly_sales: f64) -> FeatureVector {
    let baseline = baseline_weekly_sales.max(1.0) as f32;
    let rev = rollup.revenue as f32 / baseline;
    let cogs_r = (rollup.cogs / rollup.revenue.max(1.0)) as f32;
    let labor_r = (rollup.labor / rollup.revenue.max(1.0)) as f32;
    let labor_hours = (rollup.labor_hours as f32) / (100.0 * 7.0);
    let avg_ticket = (rollup.avg_ticket as f32) / 20.0;

    let tickets = (rollup.tickets as f32) / (500.0 * 7.0);
    let budget_var = rollup.budget_variance_pct as f32;
    let budget_sign = budget_var.signum();
    let budget_mag = budget_var.abs();

    let mut v = vec![rev, cogs_r, labor_r, labor_hours, avg_ticket, tickets];
    // DOW one-hot is N/A for weekly — leave zero so the vector shape matches.
    v.extend([0.0; 7]);
    let year_phase = (rollup.week_index as f32 / 52.0) * std::f32::consts::TAU;
    v.push(year_phase.sin());
    v.push(year_phase.cos());
    v.push(year_phase.sin());
    v.push(year_phase.cos());

    v.push(rollup.promo_codes_active_union.len() as f32);
    v.push(rollup.days_covered as f32);
    v.push(budget_sign);
    v.push(budget_mag);

    debug_assert_eq!(v.len(), DIM);
    FeatureVector(v)
}

pub fn featurize_monthly(rollup: &MonthlyRollup, baseline_monthly_sales: f64) -> FeatureVector {
    let baseline = baseline_monthly_sales.max(1.0) as f32;
    let rev = rollup.revenue as f32 / baseline;
    let cogs_r = (rollup.cogs / rollup.revenue.max(1.0)) as f32;
    let labor_r = (rollup.labor / rollup.revenue.max(1.0)) as f32;
    let labor_hours = (rollup.labor_hours as f32) / (100.0 * 30.0);
    let avg_ticket = (rollup.avg_ticket as f32) / 20.0;

    let tickets = (rollup.tickets as f32) / (500.0 * 30.0);
    let budget_var = rollup.budget_variance_pct as f32;
    let budget_sign = budget_var.signum();
    let budget_mag = budget_var.abs();

    let mut v = vec![rev, cogs_r, labor_r, labor_hours, avg_ticket, tickets];
    v.extend([0.0; 7]);
    // Parse month index from "YYYY-MM"
    let month_num = rollup
        .year_month
        .split('-')
        .nth(1)
        .and_then(|s| s.parse::<f32>().ok())
        .unwrap_or(1.0);
    let month_phase = ((month_num - 1.0) / 12.0) * std::f32::consts::TAU;
    v.push(month_phase.sin());
    v.push(month_phase.cos());
    v.push(month_phase.sin());
    v.push(month_phase.cos());

    v.push(rollup.promo_codes_active_union.len() as f32);
    v.push(rollup.days_covered as f32);
    v.push(budget_sign);
    v.push(budget_mag);

    debug_assert_eq!(v.len(), DIM);
    FeatureVector(v)
}

/// Convenience: featurize an entire daily corpus using dimension metadata
/// for per-store baseline normalisation.
pub fn featurize_daily_corpus(
    events: &[DailyRollup],
    dims: &Dimensions,
) -> Vec<FeatureVector> {
    let baseline: HashMap<&str, f64> = dims
        .stores
        .iter()
        .map(|s| (s.label.as_str(), s.baseline_daily_sales))
        .collect();
    events
        .iter()
        .map(|e| {
            let base = baseline.get(e.store_ref.as_str()).copied().unwrap_or(5000.0);
            featurize_daily(e, base)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Brute-force oracle
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct NeighbourHit {
    pub index: usize,
    pub distance: f32,
}

pub fn brute_force_knn(corpus: &[FeatureVector], query: &FeatureVector, k: usize) -> Vec<NeighbourHit> {
    let mut hits: Vec<NeighbourHit> = corpus
        .iter()
        .enumerate()
        .map(|(index, v)| NeighbourHit { index, distance: v.distance(query) })
        .collect();
    hits.sort_by(|a, b| a.distance.partial_cmp(&b.distance).unwrap());
    hits.truncate(k);
    hits
}

// ---------------------------------------------------------------------------
// HNSW build + query
// ---------------------------------------------------------------------------

pub struct BuiltIndex {
    map: HnswMap<FeatureVector, usize>,
}

pub fn build_hnsw(corpus: &[FeatureVector], ef_construction: usize, ef_search: usize) -> BuiltIndex {
    let values: Vec<usize> = (0..corpus.len()).collect();
    let map = Builder::default()
        .ef_construction(ef_construction)
        .ef_search(ef_search)
        .build(corpus.to_vec(), values);
    BuiltIndex { map }
}

impl BuiltIndex {
    pub fn search(&self, query: &FeatureVector, k: usize) -> Vec<NeighbourHit> {
        let mut state = Search::default();
        let mut out = Vec::with_capacity(k);
        for item in self.map.search(query, &mut state).take(k) {
            out.push(NeighbourHit { index: *item.value, distance: item.distance });
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Recall benchmark
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecallRow {
    pub grain: String,
    pub corpus_size: usize,
    pub ef_construction: usize,
    pub ef_search: usize,
    pub k: usize,
    pub queries: usize,
    pub recall_at_k: f64,
    pub build_ms: u128,
    pub avg_hnsw_query_us: f64,
    pub avg_brute_force_query_us: f64,
}

pub fn benchmark(
    grain: &str,
    corpus: &[FeatureVector],
    k: usize,
    num_queries: usize,
    ef_construction: usize,
    ef_search: usize,
    seed: u64,
) -> RecallRow {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);

    // Sample queries from the corpus itself — classic held-out-point recall
    // protocol: each query is removed from the candidate set so we don't
    // trivially self-match.
    let query_indices: Vec<usize> =
        (0..corpus.len()).choose_multiple(&mut rng, num_queries.min(corpus.len()));

    // Build an HNSW over the full corpus. Self-matches are excluded at score
    // time by filtering out equal indices.
    let t_build = std::time::Instant::now();
    let index = build_hnsw(corpus, ef_construction, ef_search);
    let build_ms = t_build.elapsed().as_millis();

    let mut total_overlap = 0usize;
    let mut t_hnsw_total = 0u128;
    let mut t_brute_total = 0u128;

    for &qi in &query_indices {
        let query = &corpus[qi];

        let t = std::time::Instant::now();
        let exact: std::collections::HashSet<usize> = brute_force_knn(corpus, query, k + 1)
            .into_iter()
            .filter(|h| h.index != qi)
            .take(k)
            .map(|h| h.index)
            .collect();
        t_brute_total += t.elapsed().as_micros();

        let t = std::time::Instant::now();
        let found: std::collections::HashSet<usize> = index
            .search(query, k + 1)
            .into_iter()
            .filter(|h| h.index != qi)
            .take(k)
            .map(|h| h.index)
            .collect();
        t_hnsw_total += t.elapsed().as_micros();

        let overlap = exact.intersection(&found).count();
        total_overlap += overlap;
    }

    let total_k = k * query_indices.len();
    let recall = if total_k > 0 {
        total_overlap as f64 / total_k as f64
    } else {
        0.0
    };

    RecallRow {
        grain: grain.into(),
        corpus_size: corpus.len(),
        ef_construction,
        ef_search,
        k,
        queries: query_indices.len(),
        recall_at_k: (recall * 10_000.0).round() / 10_000.0,
        build_ms,
        avg_hnsw_query_us: if !query_indices.is_empty() {
            t_hnsw_total as f64 / query_indices.len() as f64
        } else {
            0.0
        },
        avg_brute_force_query_us: if !query_indices.is_empty() {
            t_brute_total as f64 / query_indices.len() as f64
        } else {
            0.0
        },
    }
}
