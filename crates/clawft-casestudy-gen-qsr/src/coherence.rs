//! Phase 3 — store coherence + composite ops health scoring.
//!
//! Computes algebraic connectivity λ₂ of each store's org subgraph via power
//! iteration on `(σI − L)` with deflation against the constant vector. For
//! the small org subgraphs this runs in microseconds; the same approach
//! generalises to the kernel's Lanczos path in production.

use crate::dimensions::{Dimensions, PersonStatus, Position, Store};
use crate::events::DailyRollup;
use crate::gaps::{GapReport, GapSeverity};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreCoherence {
    pub store_ref: String,
    pub brand: String,
    pub metro: String,
    pub region: String,
    /// Algebraic connectivity of the store's org subgraph (0 = disconnected).
    pub org_lambda_2: f64,
    /// Standard deviation of daily revenue variance vs. budget, relative to
    /// mean budget (0 = perfectly tracking budget).
    pub rollup_variance: f64,
    /// Fraction of the period where shifts were adequately staffed.
    pub shift_adequacy_ratio: f64,
    /// Gap count weighted by severity.
    pub gap_weight: f64,
    /// Composite ops health in [0, 1]. Higher = healthier.
    pub ops_health: f64,
}

pub fn score_all_stores(
    dims: &Dimensions,
    events: &[DailyRollup],
    ledger: &crate::ops_events::OpsEventLedger,
    gaps: &GapReport,
) -> Vec<StoreCoherence> {
    let variance_by_store = rollup_variance_by_store(events);
    let adequacy_by_store = shift_adequacy_ratio(ledger);

    let mut out = Vec::with_capacity(dims.stores.len());
    for store in &dims.stores {
        let lambda_2 = org_subgraph_lambda_2(store, dims);
        let variance = variance_by_store.get(&store.label).copied().unwrap_or(0.0);
        let adequacy = adequacy_by_store.get(&store.label).copied().unwrap_or(1.0);
        let gap_weight = gap_weight_for_store(gaps, &store.label);

        let lambda_component = lambda_2.clamp(0.0, 1.0);
        let variance_component = (1.0 - variance.min(0.25) / 0.25).clamp(0.0, 1.0);
        let adequacy_component = adequacy.clamp(0.0, 1.0);
        let gap_component = (1.0 - gap_weight.min(5.0) / 5.0).clamp(0.0, 1.0);

        let ops_health = 0.30 * lambda_component
            + 0.25 * variance_component
            + 0.20 * adequacy_component
            + 0.25 * gap_component;

        out.push(StoreCoherence {
            store_ref: store.label.clone(),
            brand: store.brand.clone(),
            metro: store.metro_code.clone(),
            region: store.region_code.clone(),
            org_lambda_2: round3(lambda_2),
            rollup_variance: round3(variance),
            shift_adequacy_ratio: round3(adequacy),
            gap_weight: round3(gap_weight),
            ops_health: round3(ops_health),
        });
    }
    out
}

/// Rank stores by ops_health ascending (worst first).
pub fn rank_by_health(scores: &mut [StoreCoherence]) {
    scores.sort_by(|a, b| a.ops_health.partial_cmp(&b.ops_health).unwrap());
}

fn rollup_variance_by_store(events: &[DailyRollup]) -> BTreeMap<String, f64> {
    let mut by_store: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for ev in events {
        by_store
            .entry(ev.store_ref.clone())
            .or_default()
            .push(ev.budget_variance_pct);
    }
    by_store
        .into_iter()
        .map(|(k, v)| {
            let mean = v.iter().sum::<f64>() / v.len() as f64;
            let var = v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / v.len() as f64;
            (k, var.sqrt())
        })
        .collect()
}

fn shift_adequacy_ratio(ledger: &crate::ops_events::OpsEventLedger) -> BTreeMap<String, f64> {
    let mut by_store: BTreeMap<String, (u32, u32)> = BTreeMap::new();
    for s in &ledger.shift_adequacy {
        let entry = by_store.entry(s.store_ref.clone()).or_default();
        entry.0 += 1;
        if s.adequate {
            entry.1 += 1;
        }
    }
    by_store
        .into_iter()
        .map(|(k, (total, adeq))| {
            let ratio = if total > 0 {
                adeq as f64 / total as f64
            } else {
                1.0
            };
            (k, ratio)
        })
        .collect()
}

fn gap_weight_for_store(gaps: &GapReport, store_ref: &str) -> f64 {
    gaps.for_store(store_ref)
        .iter()
        .map(|g| match g.severity {
            GapSeverity::Critical => 1.0,
            GapSeverity::High => 0.5,
            GapSeverity::Medium => 0.2,
            GapSeverity::Low => 0.05,
        })
        .sum()
}

// ---------------------------------------------------------------------------
// λ₂ via power iteration
// ---------------------------------------------------------------------------

/// Build the org subgraph for one store: nodes = {store, positions,
/// active people at that store}, edges = position↔store, person↔position
/// (if person fills it), person↔store (employed-at).
fn org_subgraph_lambda_2(store: &Store, dims: &Dimensions) -> f64 {
    let mut labels: Vec<String> = Vec::new();
    let store_idx = labels.len();
    labels.push(store.label.clone());

    let positions: Vec<&Position> = dims
        .positions
        .iter()
        .filter(|p| p.store_ref == store.label)
        .collect();
    let mut pos_idx = BTreeMap::new();
    for p in &positions {
        pos_idx.insert(p.label.clone(), labels.len());
        labels.push(p.label.clone());
    }

    let mut person_idx = BTreeMap::new();
    for person in dims
        .people
        .iter()
        .filter(|p| p.home_store_ref == store.label && p.status == PersonStatus::Active)
    {
        person_idx.insert(person.label.clone(), labels.len());
        labels.push(person.label.clone());
    }

    let n = labels.len();
    if n < 3 {
        return 0.0;
    }

    // Adjacency matrix
    let mut adj = vec![vec![0.0f64; n]; n];
    let link = |adj: &mut Vec<Vec<f64>>, i: usize, j: usize, w: f64| {
        adj[i][j] += w;
        adj[j][i] += w;
    };

    // Position ↔ store
    for p in &positions {
        let pi = pos_idx[&p.label];
        link(&mut adj, pi, store_idx, 1.0);
        // Position ↔ filler
        if let Some(filler) = &p.filled_by_ref
            && let Some(&hi) = person_idx.get(filler)
        {
            link(&mut adj, pi, hi, 1.5);
        }
    }
    // Person ↔ store (employs_at) for anyone not already linked via a position
    for (person_label, &hi) in &person_idx {
        let already_linked_via_pos = positions
            .iter()
            .any(|p| p.filled_by_ref.as_deref() == Some(person_label));
        if !already_linked_via_pos {
            link(&mut adj, hi, store_idx, 0.6);
        }
    }

    // Laplacian L = D - A
    let mut laplacian = vec![vec![0.0f64; n]; n];
    for i in 0..n {
        let mut deg = 0.0;
        for j in 0..n {
            if i != j {
                laplacian[i][j] = -adj[i][j];
                deg += adj[i][j];
            }
        }
        laplacian[i][i] = deg;
    }

    // Power iteration on (σI − L) with deflation against the all-ones vector
    // (eigenvalue 0 of L). Resulting dominant eigenvalue of the shifted
    // operator = σ − λ₂ of L. Choose σ conservatively.
    let sigma = 2.0
        * laplacian
            .iter()
            .enumerate()
            .map(|(i, r)| r[i])
            .fold(0.0f64, f64::max)
            .max(1.0);
    let mut v = seed_vector(n);
    deflate_ones(&mut v);

    let max_iter = 200;
    for _ in 0..max_iter {
        let mut w = vec![0.0f64; n];
        // w = (σI − L) v
        #[allow(clippy::needless_range_loop)] // matrix-vector product is clearer indexed
        for i in 0..n {
            let mut s = sigma * v[i];
            for j in 0..n {
                s -= laplacian[i][j] * v[j];
            }
            w[i] = s;
        }
        deflate_ones(&mut w);
        let norm = vec_norm(&w);
        if norm < 1e-15 {
            return 0.0;
        }
        for x in &mut w {
            *x /= norm;
        }
        let diff = {
            let mut d = 0.0f64;
            for i in 0..n {
                d += (v[i] - w[i]).powi(2);
            }
            d.sqrt()
        };
        v = w;
        if diff < 1e-9 {
            break;
        }
    }

    // Rayleigh quotient of L on the converged vector gives λ₂.
    let mut num = 0.0f64;
    #[allow(clippy::needless_range_loop)] // matrix-vector product is clearer indexed
    for i in 0..n {
        for j in 0..n {
            num += v[i] * laplacian[i][j] * v[j];
        }
    }
    let denom = v.iter().map(|x| x * x).sum::<f64>();
    (num / denom.max(1e-12)).max(0.0)
}

fn seed_vector(n: usize) -> Vec<f64> {
    // Deterministic non-constant seed so we avoid trivial overlap with the
    // all-ones vector.
    (0..n)
        .map(|i| ((i as f64 + 1.0).sin()).abs() + 0.01)
        .collect()
}

fn deflate_ones(v: &mut [f64]) {
    let mean = v.iter().sum::<f64>() / v.len() as f64;
    for x in v.iter_mut() {
        *x -= mean;
    }
}

fn vec_norm(v: &[f64]) -> f64 {
    v.iter().map(|x| x * x).sum::<f64>().sqrt()
}

fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}
