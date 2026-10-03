//! Search-path prediction via region → entry-node lookup (WEFT-385 / HNSW-EML #4).
//!
//! Clusters store embeddings into K regions. Each region keeps:
//! - a centroid vector
//! - an **entry node** (store id nearest the centroid — the "highway on-ramp")
//! - the member ids assigned to that region
//!
//! At query time the nearest region(s) yield predicted entry nodes and a
//! constrained candidate set, skipping most of the global HNSW traversal.
//! Expected impact: 2–5× faster search on clustered corpora (deep analysis #4).
//!
//! Gated behind `vector-memory` (same as [`super::hnsw_store`]).

use serde::{Deserialize, Serialize};

/// Default number of regions (k-means clusters).
pub const DEFAULT_NUM_REGIONS: usize = 16;

/// Default how many nearest regions to probe at query time.
pub const DEFAULT_REGION_PROBES: usize = 2;

/// One region in the lookup table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegionEntry {
    /// Region index in the table.
    pub region_id: usize,
    /// Centroid of member embeddings (not necessarily L2-normalized).
    pub centroid: Vec<f32>,
    /// Store entry id nearest the centroid (primary on-ramp).
    pub entry_id: String,
    /// Optional secondary on-ramps (next-nearest members to centroid).
    pub entry_ids: Vec<String>,
    /// All store entry ids assigned to this region.
    pub member_ids: Vec<String>,
}

/// Region → entry-node lookup table.
///
/// Built offline (or after bulk ingest) from store embeddings. Lookup is
/// O(K · d) cosine against centroids — cheap compared to full HNSW.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RegionEntryTable {
    /// Regions (length = K).
    pub regions: Vec<RegionEntry>,
    /// How many entry nodes retained per region (primary + secondaries).
    pub entries_per_region: usize,
    /// Number of k-means iterations used when built.
    pub kmeans_iters: usize,
}

/// Prediction for a single query.
#[derive(Debug, Clone)]
pub struct PathPrediction {
    /// Nearest region id (or `None` if the table is empty).
    pub region_id: Option<usize>,
    /// Predicted entry node ids (highway on-ramps), best first.
    pub entry_ids: Vec<String>,
    /// Cosine similarity of the query to the chosen region centroid.
    pub region_score: f32,
    /// All member ids from the probed regions (candidate pool).
    pub candidate_ids: Vec<String>,
    /// Whether the table was non-empty (learned / built).
    pub is_learned: bool,
}

/// Configuration for building / probing the table.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PathPredictConfig {
    /// Number of regions (k). Clamped to `[1, n]`.
    pub num_regions: usize,
    /// Regions to probe at query time.
    pub region_probes: usize,
    /// Entry nodes kept per region (1 = primary only).
    pub entries_per_region: usize,
    /// K-means iterations.
    pub kmeans_iters: usize,
}

impl Default for PathPredictConfig {
    fn default() -> Self {
        Self {
            num_regions: DEFAULT_NUM_REGIONS,
            region_probes: DEFAULT_REGION_PROBES,
            entries_per_region: 3,
            kmeans_iters: 12,
        }
    }
}

impl RegionEntryTable {
    /// Build a region → entry-node table from `(id, embedding)` pairs.
    ///
    /// Uses deterministic k-means++ style seeding (first K spaced samples)
    /// so tests and rebuilds are reproducible without an RNG dependency.
    pub fn build(entries: &[(String, Vec<f32>)], config: PathPredictConfig) -> Self {
        if entries.is_empty() {
            return Self {
                regions: Vec::new(),
                entries_per_region: config.entries_per_region.max(1),
                kmeans_iters: config.kmeans_iters,
            };
        }

        let dims = entries[0].1.len();
        if dims == 0 {
            return Self::default();
        }

        let k = config
            .num_regions
            .clamp(1, entries.len())
            .min(entries.len());
        let entries_per_region = config.entries_per_region.max(1);
        let iters = config.kmeans_iters.max(1);

        // Seed centroids: evenly spaced samples through the corpus.
        let mut centroids: Vec<Vec<f32>> = (0..k)
            .map(|i| {
                let idx = if k == 1 {
                    0
                } else {
                    i * (entries.len() - 1) / (k - 1)
                };
                entries[idx].1.clone()
            })
            .collect();

        let mut assignment = vec![0usize; entries.len()];

        for _ in 0..iters {
            // Assign each point to nearest centroid (cosine distance).
            for (i, (_, emb)) in entries.iter().enumerate() {
                let mut best = 0usize;
                let mut best_sim = f32::NEG_INFINITY;
                for (c, centroid) in centroids.iter().enumerate() {
                    let sim = cosine_similarity(emb, centroid);
                    if sim > best_sim {
                        best_sim = sim;
                        best = c;
                    }
                }
                assignment[i] = best;
            }

            // Recompute centroids as mean of assigned members.
            let mut sums = vec![vec![0.0_f32; dims]; k];
            let mut counts = vec![0usize; k];
            for (i, (_, emb)) in entries.iter().enumerate() {
                let c = assignment[i];
                counts[c] += 1;
                for d in 0..dims.min(emb.len()) {
                    sums[c][d] += emb[d];
                }
            }
            for c in 0..k {
                if counts[c] == 0 {
                    // Keep previous centroid if empty cluster.
                    continue;
                }
                let inv = 1.0 / counts[c] as f32;
                for d in 0..dims {
                    centroids[c][d] = sums[c][d] * inv;
                }
            }
        }

        // Final assignment after last centroid update.
        for (i, (_, emb)) in entries.iter().enumerate() {
            let mut best = 0usize;
            let mut best_sim = f32::NEG_INFINITY;
            for (c, centroid) in centroids.iter().enumerate() {
                let sim = cosine_similarity(emb, centroid);
                if sim > best_sim {
                    best_sim = sim;
                    best = c;
                }
            }
            assignment[i] = best;
        }

        // Build RegionEntry rows: members + entry nodes nearest centroid.
        let mut regions = Vec::with_capacity(k);
        for (c, centroid) in centroids.iter().enumerate().take(k) {
            let mut members: Vec<(String, f32)> = Vec::new();
            for (i, (id, emb)) in entries.iter().enumerate() {
                if assignment[i] == c {
                    let sim = cosine_similarity(emb, centroid);
                    members.push((id.clone(), sim));
                }
            }
            // Sort by proximity to centroid (best entry first).
            members.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

            let member_ids: Vec<String> = members.iter().map(|(id, _)| id.clone()).collect();
            let entry_ids: Vec<String> = members
                .iter()
                .take(entries_per_region)
                .map(|(id, _)| id.clone())
                .collect();
            let entry_id = entry_ids
                .first()
                .cloned()
                .unwrap_or_else(|| format!("region-{c}"));

            regions.push(RegionEntry {
                region_id: c,
                centroid: centroids[c].clone(),
                entry_id,
                entry_ids,
                member_ids,
            });
        }

        Self {
            regions,
            entries_per_region,
            kmeans_iters: iters,
        }
    }

    /// Build with default config (16 regions, 2 probes, 3 entries/region).
    pub fn build_default(entries: &[(String, Vec<f32>)]) -> Self {
        Self::build(entries, PathPredictConfig::default())
    }

    /// Whether the table has at least one region with members.
    pub fn is_ready(&self) -> bool {
        self.regions.iter().any(|r| !r.member_ids.is_empty())
    }

    /// Number of regions.
    pub fn len(&self) -> usize {
        self.regions.len()
    }

    /// Empty table.
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    /// Predict entry nodes and a candidate pool for `query`.
    ///
    /// Probes the `probes` nearest regions by cosine-to-centroid.
    pub fn predict(&self, query: &[f32], probes: usize) -> PathPrediction {
        if !self.is_ready() {
            return PathPrediction {
                region_id: None,
                entry_ids: Vec::new(),
                region_score: 0.0,
                candidate_ids: Vec::new(),
                is_learned: false,
            };
        }

        let probes = probes.max(1).min(self.regions.len());

        // Score all regions.
        let mut scored: Vec<(usize, f32)> = self
            .regions
            .iter()
            .map(|r| (r.region_id, cosine_similarity(query, &r.centroid)))
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let best_region = scored[0].0;
        let best_score = scored[0].1;

        let mut entry_ids = Vec::new();
        let mut candidate_ids = Vec::new();
        let mut seen_entries = std::collections::HashSet::new();
        let mut seen_candidates = std::collections::HashSet::new();

        for &(rid, _) in scored.iter().take(probes) {
            let region = &self.regions[rid];
            for eid in &region.entry_ids {
                if seen_entries.insert(eid.clone()) {
                    entry_ids.push(eid.clone());
                }
            }
            // Always include primary entry_id even if entry_ids was empty.
            if seen_entries.insert(region.entry_id.clone()) {
                entry_ids.push(region.entry_id.clone());
            }
            for mid in &region.member_ids {
                if seen_candidates.insert(mid.clone()) {
                    candidate_ids.push(mid.clone());
                }
            }
        }

        PathPrediction {
            region_id: Some(best_region),
            entry_ids,
            region_score: best_score,
            candidate_ids,
            is_learned: true,
        }
    }

    /// Predict with default probe count.
    pub fn predict_default(&self, query: &[f32]) -> PathPrediction {
        self.predict(query, DEFAULT_REGION_PROBES)
    }
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let n = a.len().min(b.len());
    let mut dot = 0.0_f32;
    let mut na = 0.0_f32;
    let mut nb = 0.0_f32;
    for i in 0..n {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    let denom = na.sqrt() * nb.sqrt();
    if denom == 0.0 {
        0.0
    } else {
        dot / denom
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clustered_corpus() -> Vec<(String, Vec<f32>)> {
        // Three well-separated axis-aligned clusters.
        let mut out = Vec::new();
        for i in 0..10 {
            let n = 0.05 * (i as f32);
            out.push((format!("a{i}"), vec![1.0 + n, n, n]));
            out.push((format!("b{i}"), vec![n, 1.0 + n, n]));
            out.push((format!("c{i}"), vec![n, n, 1.0 + n]));
        }
        out
    }

    #[test]
    fn build_empty_table() {
        let table = RegionEntryTable::build_default(&[]);
        assert!(table.is_empty());
        assert!(!table.is_ready());
        let pred = table.predict_default(&[1.0, 0.0, 0.0]);
        assert!(!pred.is_learned);
        assert!(pred.entry_ids.is_empty());
    }

    #[test]
    fn build_and_predict_entry_in_right_cluster() {
        let corpus = clustered_corpus();
        let table = RegionEntryTable::build(
            &corpus,
            PathPredictConfig {
                num_regions: 3,
                region_probes: 1,
                entries_per_region: 2,
                kmeans_iters: 15,
            },
        );
        assert_eq!(table.len(), 3);
        assert!(table.is_ready());

        // Query near cluster A → entry should be an "a*" id.
        let pred = table.predict(&[1.0, 0.0, 0.0], 1);
        assert!(pred.is_learned);
        assert!(pred.region_id.is_some());
        assert!(!pred.entry_ids.is_empty());
        assert!(
            pred.entry_ids[0].starts_with('a'),
            "expected a* entry, got {:?}",
            pred.entry_ids
        );
        assert!(
            pred.candidate_ids.iter().all(|id| id.starts_with('a')),
            "single-probe candidates should stay in cluster A: {:?}",
            pred.candidate_ids
        );
    }

    #[test]
    fn multi_probe_expands_candidates() {
        let corpus = clustered_corpus();
        let table = RegionEntryTable::build(
            &corpus,
            PathPredictConfig {
                num_regions: 3,
                region_probes: 2,
                entries_per_region: 1,
                kmeans_iters: 15,
            },
        );
        let pred = table.predict(&[1.0, 0.0, 0.0], 2);
        // Two regions → more candidates than one cluster alone.
        assert!(pred.candidate_ids.len() > 10);
        assert!(pred.entry_ids.len() >= 2);
    }

    #[test]
    fn serialization_roundtrip() {
        let corpus = clustered_corpus();
        let table = RegionEntryTable::build_default(&corpus);
        let json = serde_json::to_string(&table).unwrap();
        let restored: RegionEntryTable = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.len(), table.len());
        assert_eq!(restored.regions[0].entry_id, table.regions[0].entry_id);
    }

    #[test]
    fn single_point_corpus() {
        let corpus = vec![("only".into(), vec![0.5, 0.5])];
        let table = RegionEntryTable::build_default(&corpus);
        assert_eq!(table.len(), 1);
        let pred = table.predict_default(&[0.5, 0.5]);
        assert_eq!(pred.entry_ids[0], "only");
        assert_eq!(pred.candidate_ids, vec!["only".to_string()]);
    }
}
