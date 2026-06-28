//! Graph-based sensor fusion for distributed buoy arrays.
//!
//! Inspired by K-STEMIT (arXiv:2604.09922). Models a sonobuoy array as a
//! graph where buoys are nodes (with physics features) and inter-buoy
//! connections are edges (weighted by distance and propagation delay).
//!
//! The architecture provides:
//! - **GraphSAGE-style neighborhood aggregation**: learned beamforming for
//!   irregular spatial arrays.
//! - **Temporal convolution**: feature extraction from acoustic time-series
//!   buffers per node.
//! - **Spatio-temporal fusion**: combined feature vector for detection and
//!   classification tasks.

use std::collections::HashSet;

/// A buoy node in the sensor graph.
#[derive(Debug, Clone)]
pub struct SensorNode {
    /// Unique identifier for this buoy.
    pub id: String,
    /// Position as (latitude, longitude, depth_m).
    pub position: (f64, f64, f64),
    /// Physics features: e.g. sound_speed, temperature, salinity.
    pub features: Vec<f32>,
}

/// An edge connecting two buoys.
#[derive(Debug, Clone)]
pub struct SensorEdge {
    /// Index of the source node.
    pub from: usize,
    /// Index of the target node.
    pub to: usize,
    /// Euclidean distance between buoys in meters.
    pub distance_m: f64,
    /// Estimated acoustic propagation delay in milliseconds.
    pub propagation_delay_ms: f64,
}

/// Graph-based sensor fusion for distributed buoy arrays.
///
/// Nodes represent buoys with physics features; edges represent
/// inter-buoy connections weighted by distance and propagation delay.
/// Temporal buffers store time-series data per node for temporal
/// convolution.
#[derive(Debug, Clone)]
pub struct SensorGraph {
    /// Buoy positions (node features).
    pub nodes: Vec<SensorNode>,
    /// Inter-buoy distances/connections (edges).
    pub edges: Vec<SensorEdge>,
    /// Temporal feature buffer per node (ring buffer of feature snapshots).
    pub temporal_buffers: Vec<Vec<f32>>,
}

impl Default for SensorGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl SensorGraph {
    /// Create an empty sensor graph.
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            temporal_buffers: Vec::new(),
        }
    }

    /// Add a buoy node to the graph.
    ///
    /// Returns the index of the newly added node. A corresponding empty
    /// temporal buffer is allocated automatically.
    pub fn add_buoy(&mut self, node: SensorNode) -> usize {
        let idx = self.nodes.len();
        self.nodes.push(node);
        self.temporal_buffers.push(Vec::new());
        idx
    }

    /// Connect two buoys with an edge.
    ///
    /// Distance and propagation delay are computed from positions using
    /// a simple Euclidean distance model. For real deployments, replace
    /// with proper geodesic + Bellhop propagation models.
    ///
    /// # Panics
    /// Panics if `from` or `to` are out of bounds.
    pub fn connect(&mut self, from: usize, to: usize) {
        assert!(from < self.nodes.len(), "from index out of bounds");
        assert!(to < self.nodes.len(), "to index out of bounds");

        let (lat1, lon1, d1) = self.nodes[from].position;
        let (lat2, lon2, d2) = self.nodes[to].position;

        // Simple Euclidean distance in the (lat, lon, depth) space.
        // In production, use proper geodesic distance.
        let dlat = lat2 - lat1;
        let dlon = lon2 - lon1;
        let ddep = d2 - d1;
        let distance_m = (dlat * dlat + dlon * dlon + ddep * ddep).sqrt();

        // Approximate propagation delay: ~1500 m/s speed of sound in water.
        let speed_of_sound = 1500.0; // m/s
        let propagation_delay_ms = (distance_m / speed_of_sound) * 1000.0;

        self.edges.push(SensorEdge {
            from,
            to,
            distance_m,
            propagation_delay_ms,
        });
    }

    /// Append a feature snapshot to a node's temporal buffer.
    ///
    /// # Panics
    /// Panics if `node_idx` is out of bounds.
    pub fn push_temporal(&mut self, node_idx: usize, values: &[f32]) {
        assert!(
            node_idx < self.temporal_buffers.len(),
            "node index out of bounds"
        );
        self.temporal_buffers[node_idx].extend_from_slice(values);
    }

    /// Return indices of neighbors for a given node (undirected).
    fn neighbor_indices(&self, node_idx: usize) -> Vec<usize> {
        let mut neighbors = HashSet::new();
        for edge in &self.edges {
            if edge.from == node_idx {
                neighbors.insert(edge.to);
            }
            if edge.to == node_idx {
                neighbors.insert(edge.from);
            }
        }
        let mut v: Vec<usize> = neighbors.into_iter().collect();
        v.sort();
        v
    }

    /// GraphSAGE-style neighborhood aggregation.
    ///
    /// For each node, aggregates features from neighbors within `hop` hops,
    /// weighted by inverse distance. Returns a feature vector that is the
    /// distance-weighted mean of neighbor features.
    ///
    /// For hop > 1, recursively expands the neighborhood. This is a simplified
    /// mean-aggregator; a full GNN would learn the aggregation weights.
    pub fn aggregate_neighbors(&self, node_idx: usize, hop: usize) -> Vec<f32> {
        if node_idx >= self.nodes.len() || hop == 0 {
            return self
                .nodes
                .get(node_idx)
                .map(|n| n.features.clone())
                .unwrap_or_default();
        }

        // Collect k-hop neighborhood via BFS.
        let mut visited = HashSet::new();
        let mut frontier = vec![node_idx];
        visited.insert(node_idx);

        for _ in 0..hop {
            let mut next_frontier = Vec::new();
            for &n in &frontier {
                for &nb in &self.neighbor_indices(n) {
                    if visited.insert(nb) {
                        next_frontier.push(nb);
                    }
                }
            }
            frontier = next_frontier;
        }

        // Collect all neighbors (exclude self).
        let neighbors: Vec<usize> = visited.into_iter().filter(|&n| n != node_idx).collect();
        if neighbors.is_empty() {
            return self.nodes[node_idx].features.clone();
        }

        // Build distance lookup for direct edges from node_idx.
        let edge_distances: std::collections::HashMap<usize, f64> = self
            .edges
            .iter()
            .filter_map(|e| {
                if e.from == node_idx {
                    Some((e.to, e.distance_m))
                } else if e.to == node_idx {
                    Some((e.from, e.distance_m))
                } else {
                    None
                }
            })
            .collect();

        // Weighted aggregation: inverse-distance weighting.
        let feat_dim = self.nodes[node_idx].features.len();
        let mut agg = vec![0.0f32; feat_dim];
        let mut total_weight = 0.0f32;

        for &nb in &neighbors {
            let dist = edge_distances.get(&nb).copied().unwrap_or(1.0);
            let w = 1.0 / (dist as f32 + 1e-6);
            total_weight += w;
            let feats = &self.nodes[nb].features;
            for (i, &f) in feats.iter().enumerate().take(feat_dim) {
                agg[i] += w * f;
            }
        }

        if total_weight > 0.0 {
            for v in &mut agg {
                *v /= total_weight;
            }
        }

        agg
    }

    /// Temporal convolution: extract features from the time-series buffer.
    ///
    /// Returns the last `window` values from the temporal buffer. If the
    /// buffer has fewer values, it is zero-padded on the left.
    ///
    /// In a full implementation, this would apply 1-D convolution kernels
    /// (learned filters) over the time series. This stub returns the raw
    /// windowed signal as features.
    pub fn temporal_features(&self, node_idx: usize, window: usize) -> Vec<f32> {
        if node_idx >= self.temporal_buffers.len() {
            return vec![0.0; window];
        }

        let buf = &self.temporal_buffers[node_idx];
        if buf.len() >= window {
            buf[buf.len() - window..].to_vec()
        } else {
            let mut result = vec![0.0f32; window - buf.len()];
            result.extend_from_slice(buf);
            result
        }
    }

    /// Combined spatio-temporal feature for detection/classification.
    ///
    /// Concatenates:
    /// 1. The node's own features.
    /// 2. 1-hop aggregated neighbor features.
    /// 3. Temporal features (last 16 samples from the buffer).
    ///
    /// The resulting vector can feed into a classification head for
    /// species ID, vessel detection, etc.
    pub fn fused_features(&self, node_idx: usize) -> Vec<f32> {
        let own = self
            .nodes
            .get(node_idx)
            .map(|n| n.features.clone())
            .unwrap_or_default();
        let spatial = self.aggregate_neighbors(node_idx, 1);
        let temporal = self.temporal_features(node_idx, 16);

        let mut fused = Vec::with_capacity(own.len() + spatial.len() + temporal.len());
        fused.extend_from_slice(&own);
        fused.extend_from_slice(&spatial);
        fused.extend_from_slice(&temporal);
        fused
    }

    /// Number of nodes in the graph.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Number of edges in the graph.
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_buoy(id: &str, lat: f64, lon: f64, depth: f64, features: Vec<f32>) -> SensorNode {
        SensorNode {
            id: id.to_string(),
            position: (lat, lon, depth),
            features,
        }
    }

    #[test]
    fn new_graph_is_empty() {
        let g = SensorGraph::new();
        assert_eq!(g.node_count(), 0);
        assert_eq!(g.edge_count(), 0);
    }

    #[test]
    fn add_buoy_increments_count() {
        let mut g = SensorGraph::new();
        let idx = g.add_buoy(make_buoy("B1", 0.0, 0.0, 10.0, vec![1.0, 2.0]));
        assert_eq!(idx, 0);
        assert_eq!(g.node_count(), 1);
        assert_eq!(g.temporal_buffers.len(), 1);
    }

    #[test]
    fn connect_creates_edge_with_distance() {
        let mut g = SensorGraph::new();
        g.add_buoy(make_buoy("B1", 0.0, 0.0, 0.0, vec![1.0]));
        g.add_buoy(make_buoy("B2", 3.0, 4.0, 0.0, vec![2.0]));
        g.connect(0, 1);

        assert_eq!(g.edge_count(), 1);
        let edge = &g.edges[0];
        assert!((edge.distance_m - 5.0).abs() < 1e-6);
        assert!(edge.propagation_delay_ms > 0.0);
    }

    #[test]
    fn aggregate_neighbors_single_hop() {
        let mut g = SensorGraph::new();
        g.add_buoy(make_buoy("B1", 0.0, 0.0, 0.0, vec![1.0, 0.0]));
        g.add_buoy(make_buoy("B2", 1.0, 0.0, 0.0, vec![0.0, 1.0]));
        g.add_buoy(make_buoy("B3", 0.0, 1.0, 0.0, vec![0.0, 1.0]));
        g.connect(0, 1);
        g.connect(0, 2);

        let agg = g.aggregate_neighbors(0, 1);
        assert_eq!(agg.len(), 2);
        // Both neighbors equidistant, so aggregation should be mean.
        assert!((agg[0] - 0.0).abs() < 0.01);
        assert!((agg[1] - 1.0).abs() < 0.01);
    }

    #[test]
    fn aggregate_neighbors_no_neighbors_returns_self() {
        let mut g = SensorGraph::new();
        g.add_buoy(make_buoy("B1", 0.0, 0.0, 0.0, vec![5.0, 3.0]));

        let agg = g.aggregate_neighbors(0, 1);
        assert_eq!(agg, vec![5.0, 3.0]);
    }

    #[test]
    fn aggregate_neighbors_hop_zero_returns_self() {
        let mut g = SensorGraph::new();
        g.add_buoy(make_buoy("B1", 0.0, 0.0, 0.0, vec![5.0]));
        g.add_buoy(make_buoy("B2", 1.0, 0.0, 0.0, vec![9.0]));
        g.connect(0, 1);

        let agg = g.aggregate_neighbors(0, 0);
        assert_eq!(agg, vec![5.0]);
    }

    #[test]
    fn temporal_features_zero_padded() {
        let mut g = SensorGraph::new();
        g.add_buoy(make_buoy("B1", 0.0, 0.0, 0.0, vec![1.0]));
        g.push_temporal(0, &[0.5, 0.6, 0.7]);

        let tf = g.temporal_features(0, 5);
        assert_eq!(tf.len(), 5);
        assert_eq!(tf, vec![0.0, 0.0, 0.5, 0.6, 0.7]);
    }

    #[test]
    fn temporal_features_exact_window() {
        let mut g = SensorGraph::new();
        g.add_buoy(make_buoy("B1", 0.0, 0.0, 0.0, vec![1.0]));
        g.push_temporal(0, &[1.0, 2.0, 3.0]);

        let tf = g.temporal_features(0, 3);
        assert_eq!(tf, vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn temporal_features_larger_buffer_takes_tail() {
        let mut g = SensorGraph::new();
        g.add_buoy(make_buoy("B1", 0.0, 0.0, 0.0, vec![1.0]));
        g.push_temporal(0, &[1.0, 2.0, 3.0, 4.0, 5.0]);

        let tf = g.temporal_features(0, 3);
        assert_eq!(tf, vec![3.0, 4.0, 5.0]);
    }

    #[test]
    fn fused_features_length() {
        let mut g = SensorGraph::new();
        g.add_buoy(make_buoy("B1", 0.0, 0.0, 0.0, vec![1.0, 2.0, 3.0]));
        g.add_buoy(make_buoy("B2", 1.0, 0.0, 0.0, vec![4.0, 5.0, 6.0]));
        g.connect(0, 1);
        g.push_temporal(0, &[0.1; 20]);

        let fused = g.fused_features(0);
        // own(3) + spatial(3) + temporal(16) = 22
        assert_eq!(fused.len(), 22);
    }

    #[test]
    fn fused_features_isolated_node() {
        let mut g = SensorGraph::new();
        g.add_buoy(make_buoy("B1", 0.0, 0.0, 0.0, vec![1.0, 2.0]));

        let fused = g.fused_features(0);
        // own(2) + spatial(2, same as self) + temporal(16, all zeros) = 20
        assert_eq!(fused.len(), 20);
    }

    #[test]
    #[should_panic(expected = "from index out of bounds")]
    fn connect_out_of_bounds_panics() {
        let mut g = SensorGraph::new();
        g.add_buoy(make_buoy("B1", 0.0, 0.0, 0.0, vec![1.0]));
        g.connect(5, 0);
    }

    #[test]
    fn multi_hop_aggregation() {
        let mut g = SensorGraph::new();
        g.add_buoy(make_buoy("A", 0.0, 0.0, 0.0, vec![1.0]));
        g.add_buoy(make_buoy("B", 1.0, 0.0, 0.0, vec![2.0]));
        g.add_buoy(make_buoy("C", 2.0, 0.0, 0.0, vec![3.0]));
        g.connect(0, 1); // A -- B
        g.connect(1, 2); // B -- C

        // 1-hop from A should only include B.
        let agg1 = g.aggregate_neighbors(0, 1);
        assert!((agg1[0] - 2.0).abs() < 0.01);

        // 2-hop from A should include B and C.
        let agg2 = g.aggregate_neighbors(0, 2);
        // B at distance 1.0, C at distance ~2.0 (no direct edge, falls back to 1.0).
        // Both are included, so result is a weighted mean.
        assert!(agg2[0] > 1.0 && agg2[0] < 3.0);
    }
}
