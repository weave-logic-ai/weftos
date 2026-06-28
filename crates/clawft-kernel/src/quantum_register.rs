//! Graph → 2D atom register layout, shared across quantum backends.
//!
//! EXPERIMENTAL (0.6.x). Both Pasqal Fresnel and QuEra Aquila consume a list
//! of named atoms at 2D coordinates in micrometers. This module produces a
//! deterministic layout from an adjacency list so the same graph maps to the
//! same register across backends and across runs.
//!
//! The initial layout is force-directed (Fruchterman–Reingold) for simplicity.
//! Spectral embedding is a TODO — it gives better Laplacian-structure
//! preservation but needs an eigensolver. Force-directed is good enough for
//! the interface + stub-backend milestone.

use crate::quantum_backend::QuantumError;

/// Device-independent register constraints used during layout.
#[derive(Debug, Clone, Copy)]
pub struct RegisterConstraints {
    /// Minimum inter-atom distance in micrometers.
    pub min_distance_um: f64,
    /// Maximum extent of the trap area, per axis, in micrometers.
    pub max_extent_um: f64,
    /// Maximum atoms supported by the target device.
    pub max_atoms: usize,
}

impl RegisterConstraints {
    /// Defaults that satisfy both Fresnel (~4um min spacing) and Aquila.
    pub fn neutral_atom_default() -> Self {
        Self {
            min_distance_um: 4.0,
            max_extent_um: 75.0,
            max_atoms: 100,
        }
    }
}

/// Build a named 2D register from a graph adjacency list.
///
/// `adjacency[i]` is the list of `(neighbor_index, weight)` pairs for node i.
/// Returned positions are in micrometers, scaled to fit within the constraints.
///
/// The layout is deterministic for a given adjacency input (fixed seed for
/// the force-directed iteration).
pub fn build_register(
    adjacency: &[Vec<(usize, f64)>],
    constraints: RegisterConstraints,
) -> Result<Vec<(String, [f64; 2])>, QuantumError> {
    let n = adjacency.len();
    if n == 0 {
        return Ok(Vec::new());
    }
    if n > constraints.max_atoms {
        return Err(QuantumError::GraphTooLarge {
            nodes: n,
            max: constraints.max_atoms,
        });
    }

    let positions = force_directed_layout(adjacency, 200);
    let scaled = scale_to_constraints(&positions, constraints)?;

    Ok(scaled
        .into_iter()
        .enumerate()
        .map(|(i, p)| (format!("q{}", i), p))
        .collect())
}

fn force_directed_layout(adjacency: &[Vec<(usize, f64)>], iterations: u32) -> Vec<[f64; 2]> {
    let n = adjacency.len();
    // Deterministic initial positions on a unit circle.
    let mut pos: Vec<[f64; 2]> = (0..n)
        .map(|i| {
            let theta = 2.0 * std::f64::consts::PI * (i as f64) / (n as f64);
            [theta.cos(), theta.sin()]
        })
        .collect();

    let k = (1.0 / (n as f64).max(1.0)).sqrt();
    let k2 = k * k;

    for iter in 0..iterations {
        let temperature = 0.1 * (1.0 - iter as f64 / iterations as f64).max(0.01);
        let mut disp = vec![[0.0_f64; 2]; n];

        // Repulsion (all pairs).
        for i in 0..n {
            for j in 0..n {
                if i == j {
                    continue;
                }
                let dx = pos[i][0] - pos[j][0];
                let dy = pos[i][1] - pos[j][1];
                let dist2 = (dx * dx + dy * dy).max(1e-9);
                let force = k2 / dist2;
                disp[i][0] += dx * force;
                disp[i][1] += dy * force;
            }
        }

        // Attraction along edges, weighted.
        for (i, edges) in adjacency.iter().enumerate() {
            for &(j, w) in edges {
                if j >= n {
                    continue;
                }
                let dx = pos[i][0] - pos[j][0];
                let dy = pos[i][1] - pos[j][1];
                let dist = (dx * dx + dy * dy).sqrt().max(1e-9);
                let force = w * dist / k;
                disp[i][0] -= dx / dist * force;
                disp[i][1] -= dy / dist * force;
            }
        }

        for i in 0..n {
            let mag = (disp[i][0].powi(2) + disp[i][1].powi(2)).sqrt().max(1e-9);
            let step = mag.min(temperature);
            pos[i][0] += disp[i][0] / mag * step;
            pos[i][1] += disp[i][1] / mag * step;
        }
    }
    pos
}

fn scale_to_constraints(
    positions: &[[f64; 2]],
    c: RegisterConstraints,
) -> Result<Vec<[f64; 2]>, QuantumError> {
    if positions.is_empty() {
        return Ok(Vec::new());
    }

    // Find minimum pairwise distance in the raw layout.
    let mut min_d = f64::INFINITY;
    for i in 0..positions.len() {
        for j in (i + 1)..positions.len() {
            let dx = positions[i][0] - positions[j][0];
            let dy = positions[i][1] - positions[j][1];
            let d = (dx * dx + dy * dy).sqrt();
            if d < min_d {
                min_d = d;
            }
        }
    }

    if positions.len() == 1 {
        return Ok(vec![[0.0, 0.0]]);
    }

    if !min_d.is_finite() || min_d <= 0.0 {
        return Err(QuantumError::InvalidRegister(
            "degenerate layout: atoms collapsed to same point".into(),
        ));
    }

    // Scale so min_d -> c.min_distance_um.
    let scale = c.min_distance_um / min_d;
    let mut scaled: Vec<[f64; 2]> = positions
        .iter()
        .map(|p| [p[0] * scale, p[1] * scale])
        .collect();

    // Check extent.
    let (mut xmin, mut xmax, mut ymin, mut ymax) = (
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
    );
    for p in &scaled {
        xmin = xmin.min(p[0]);
        xmax = xmax.max(p[0]);
        ymin = ymin.min(p[1]);
        ymax = ymax.max(p[1]);
    }
    let extent = (xmax - xmin).max(ymax - ymin);
    if extent > c.max_extent_um {
        return Err(QuantumError::InvalidRegister(format!(
            "register extent {:.1}um exceeds device max {:.1}um",
            extent, c.max_extent_um
        )));
    }

    // Recenter on origin.
    let cx = (xmin + xmax) / 2.0;
    let cy = (ymin + ymax) / 2.0;
    for p in &mut scaled {
        p[0] -= cx;
        p[1] -= cy;
    }
    Ok(scaled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_graph_returns_empty_register() {
        let r = build_register(&[], RegisterConstraints::neutral_atom_default()).unwrap();
        assert!(r.is_empty());
    }

    #[test]
    fn single_node_places_at_origin() {
        let adj = vec![vec![]];
        let r = build_register(&adj, RegisterConstraints::neutral_atom_default()).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, "q0");
        assert_eq!(r[0].1, [0.0, 0.0]);
    }

    #[test]
    fn too_large_graph_errors() {
        let adj: Vec<Vec<(usize, f64)>> = (0..5).map(|_| vec![]).collect();
        let constraints = RegisterConstraints {
            min_distance_um: 4.0,
            max_extent_um: 75.0,
            max_atoms: 3,
        };
        let err = build_register(&adj, constraints).unwrap_err();
        matches!(err, QuantumError::GraphTooLarge { nodes: 5, max: 3 });
    }

    #[test]
    fn small_graph_respects_min_distance() {
        let adj = vec![
            vec![(1, 1.0), (2, 1.0)],
            vec![(0, 1.0), (2, 1.0)],
            vec![(0, 1.0), (1, 1.0)],
        ];
        let c = RegisterConstraints::neutral_atom_default();
        let r = build_register(&adj, c).unwrap();
        assert_eq!(r.len(), 3);
        for i in 0..3 {
            for j in (i + 1)..3 {
                let dx = r[i].1[0] - r[j].1[0];
                let dy = r[i].1[1] - r[j].1[1];
                let d = (dx * dx + dy * dy).sqrt();
                assert!(
                    d >= c.min_distance_um - 1e-6,
                    "atoms {} and {} too close: {}",
                    i,
                    j,
                    d
                );
            }
        }
    }

    #[test]
    fn layout_is_deterministic() {
        let adj = vec![vec![(1, 1.0)], vec![(0, 1.0), (2, 1.0)], vec![(1, 1.0)]];
        let c = RegisterConstraints::neutral_atom_default();
        let r1 = build_register(&adj, c).unwrap();
        let r2 = build_register(&adj, c).unwrap();
        assert_eq!(r1, r2);
    }
}
