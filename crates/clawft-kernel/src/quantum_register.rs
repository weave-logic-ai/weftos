//! Graph → 2D atom register layout, shared across quantum backends.
//!
//! EXPERIMENTAL (0.6.x). Both Pasqal Fresnel and QuEra Aquila consume a list
//! of named atoms at 2D coordinates in micrometers. This module produces a
//! deterministic layout from an adjacency list so the same graph maps to the
//! same register across backends and across runs.
//!
//! ## Layout backends (WEFT-129)
//!
//! - **Spectral embedding** (default): places atoms using the 2nd and 3rd
//!   eigenvectors of the (combinatorial) graph Laplacian. Preserves
//!   Laplacian structure, which is the same operator used as the quantum
//!   Hamiltonian in [`crate::quantum_state`]. Dense Jacobi eigensolve is
//!   O(n³) — fine for device limits (≤100 atoms). Pure Rust; no Wasmtime.
//! - **Force-directed** (Fruchterman–Reingold): retained as an explicit
//!   option and as an automatic fallback when spectral positions collapse
//!   (e.g. highly symmetric or degenerate graphs).
//!
//! Note: the 0.7.0 audit row titled this "Wasmtime backend for
//! spectral_embedding". That was a misnomer — spectral embedding is
//! classical linear algebra on n≤100, not a WASM sandbox concern. The
//! production surface is this pure-Rust path.

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

/// Algorithm used to map a graph adjacency list to 2D atom positions.
///
/// Default is [`LayoutMethod::Spectral`] (WEFT-129). Force-directed remains
/// available for A/B and as spectral fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayoutMethod {
    /// Laplacian spectral embedding (eigenvectors 2 and 3). Default.
    #[default]
    Spectral,
    /// Classical force-directed (Fruchterman–Reingold).
    ForceDirected,
}

/// Build a named 2D register from a graph adjacency list.
///
/// Uses [`LayoutMethod::Spectral`] by default. See
/// [`build_register_with`] to pick the algorithm explicitly.
///
/// `adjacency[i]` is the list of `(neighbor_index, weight)` pairs for node i.
/// Returned positions are in micrometers, scaled to fit within the constraints.
///
/// The layout is deterministic for a given adjacency input.
pub fn build_register(
    adjacency: &[Vec<(usize, f64)>],
    constraints: RegisterConstraints,
) -> Result<Vec<(String, [f64; 2])>, QuantumError> {
    build_register_with(adjacency, constraints, LayoutMethod::Spectral)
}

/// Build a named 2D register with an explicit [`LayoutMethod`].
pub fn build_register_with(
    adjacency: &[Vec<(usize, f64)>],
    constraints: RegisterConstraints,
    method: LayoutMethod,
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

    let positions = match method {
        LayoutMethod::Spectral => {
            match spectral_embedding_layout(adjacency) {
                Some(pos) if layout_min_pairwise(&pos) > 1e-9 => pos,
                // Degenerate / collapsed spectral layout → force-directed.
                _ => force_directed_layout(adjacency, 200),
            }
        }
        LayoutMethod::ForceDirected => force_directed_layout(adjacency, 200),
    };

    let scaled = scale_to_constraints(&positions, constraints)?;

    Ok(scaled
        .into_iter()
        .enumerate()
        .map(|(i, p)| (format!("q{}", i), p))
        .collect())
}

// ---------------------------------------------------------------------------
// Spectral embedding (WEFT-129)
// ---------------------------------------------------------------------------

/// Place nodes using the 2nd and 3rd eigenvectors of the combinatorial
/// graph Laplacian L = D − A (symmetric, undirected view of the input).
///
/// Returns `None` when the spectral coordinates are unusable (n < 2 after
/// filtering, or eigensolve produced non-finite values).
fn spectral_embedding_layout(adjacency: &[Vec<(usize, f64)>]) -> Option<Vec<[f64; 2]>> {
    let n = adjacency.len();
    if n == 0 {
        return Some(Vec::new());
    }
    if n == 1 {
        return Some(vec![[0.0, 0.0]]);
    }

    // Symmetric weighted adjacency (max of either direction keeps weights
    // non-negative and deterministic).
    let mut a = vec![vec![0.0f64; n]; n];
    for (i, edges) in adjacency.iter().enumerate() {
        for &(j, w) in edges {
            if j >= n || i == j {
                continue;
            }
            let ww = w.max(0.0);
            if ww > a[i][j] {
                a[i][j] = ww;
                a[j][i] = ww;
            }
        }
    }

    // Combinatorial Laplacian L = D − A.
    let mut l = vec![vec![0.0f64; n]; n];
    for i in 0..n {
        let mut deg = 0.0f64;
        for j in 0..n {
            if i != j {
                deg += a[i][j];
                l[i][j] = -a[i][j];
            }
        }
        l[i][i] = deg;
    }

    let (evals, evecs) = dense_jacobi_eigen(&l, n, 200 * n.max(1));

    // Sort eigenpairs by eigenvalue ascending (λ1 ≈ 0 for connected graphs).
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&i, &j| {
        evals[i]
            .partial_cmp(&evals[j])
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Pick the first two eigenvectors with λ above the numerical null-space
    // floor. For a connected graph these are the Fiedler vector (x) and the
    // next mode (y). For disconnected graphs we still take the two smallest
    // non-null modes so components separate spatially.
    const NULL_FLOOR: f64 = 1e-8;
    let mut axes: Vec<usize> = Vec::with_capacity(2);
    for &idx in &order {
        if evals[idx] > NULL_FLOOR {
            axes.push(idx);
            if axes.len() == 2 {
                break;
            }
        }
    }

    // Pathological: only one non-null mode (e.g. n=2). Use it for x and a
    // deterministic orthogonal for y.
    if axes.is_empty() {
        return None;
    }

    let mut pos = vec![[0.0f64; 2]; n];
    let vx = &evecs[axes[0]];
    for i in 0..n {
        pos[i][0] = vx[i];
    }

    if axes.len() >= 2 {
        let vy = &evecs[axes[1]];
        for i in 0..n {
            pos[i][1] = vy[i];
        }
    } else {
        // Build a deterministic vector orthogonal to vx in R^n, project, use as y.
        let mut y: Vec<f64> = (0..n)
            .map(|i| ((i as f64) + 0.5) - (n as f64) / 2.0)
            .collect();
        let dot: f64 = y.iter().zip(vx.iter()).map(|(a, b)| a * b).sum();
        let norm_x2: f64 = vx.iter().map(|x| x * x).sum::<f64>().max(1e-18);
        for i in 0..n {
            y[i] -= dot / norm_x2 * vx[i];
        }
        let norm_y: f64 = y.iter().map(|v| v * v).sum::<f64>().sqrt().max(1e-18);
        for i in 0..n {
            pos[i][1] = y[i] / norm_y;
        }
    }

    // Sign-canonicalize each axis so the layout is stable across platforms
    // (eigenvector sign is free). Make the first non-near-zero component positive.
    for axis in 0..2 {
        let mut flip = false;
        for p in pos.iter().take(n) {
            if p[axis].abs() > 1e-12 {
                flip = p[axis] < 0.0;
                break;
            }
        }
        if flip {
            for p in pos.iter_mut().take(n) {
                p[axis] = -p[axis];
            }
        }
    }

    // Reject non-finite coordinates.
    if pos.iter().any(|p| !p[0].is_finite() || !p[1].is_finite()) {
        return None;
    }

    Some(pos)
}

/// Dense Jacobi eigensolve for a symmetric matrix (local copy of the
/// algorithm used in `causal.rs` so this module stays free of CausalGraph).
///
/// Returns `(eigenvalues, eigenvectors)` where `eigenvectors[j]` is the
/// j-th eigenvector (length m).
// Index loops over a dense symmetric matrix touch a[i][j], a[r][p] and a[p][r] together; iterator forms would not read as the algorithm.
#[allow(clippy::needless_range_loop)]
fn dense_jacobi_eigen(mat: &[Vec<f64>], m: usize, max_iter: usize) -> (Vec<f64>, Vec<Vec<f64>>) {
    if m == 0 {
        return (Vec::new(), Vec::new());
    }
    if m == 1 {
        return (vec![mat[0][0]], vec![vec![1.0]]);
    }

    let mut a = mat.to_vec();
    let mut v = vec![vec![0.0f64; m]; m];
    for (i, row) in v.iter_mut().enumerate() {
        row[i] = 1.0;
    }

    for _ in 0..max_iter {
        let mut max_off = 0.0f64;
        let mut p = 0usize;
        let mut q = 1usize;
        for i in 0..m {
            for j in (i + 1)..m {
                if a[i][j].abs() > max_off {
                    max_off = a[i][j].abs();
                    p = i;
                    q = j;
                }
            }
        }

        if max_off < 1e-15 {
            break;
        }

        let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
        let t = if theta == 0.0 || !theta.is_finite() {
            1.0
        } else {
            theta.signum() / (theta.abs() + (1.0 + theta * theta).sqrt())
        };
        let c = 1.0 / (1.0 + t * t).sqrt();
        let s = t * c;

        let app = a[p][p];
        let aqq = a[q][q];
        let apq = a[p][q];
        a[p][p] = c * c * app - 2.0 * s * c * apq + s * s * aqq;
        a[q][q] = s * s * app + 2.0 * s * c * apq + c * c * aqq;
        a[p][q] = 0.0;
        a[q][p] = 0.0;

        for r in 0..m {
            if r != p && r != q {
                let arp = a[r][p];
                let arq = a[r][q];
                a[r][p] = c * arp - s * arq;
                a[p][r] = a[r][p];
                a[r][q] = s * arp + c * arq;
                a[q][r] = a[r][q];
            }
        }

        for row in v.iter_mut() {
            let vp = row[p];
            let vq = row[q];
            row[p] = c * vp - s * vq;
            row[q] = s * vp + c * vq;
        }
    }

    let eigenvalues: Vec<f64> = (0..m).map(|i| a[i][i]).collect();
    let eigenvectors: Vec<Vec<f64>> = (0..m).map(|j| (0..m).map(|i| v[i][j]).collect()).collect();
    (eigenvalues, eigenvectors)
}

fn layout_min_pairwise(positions: &[[f64; 2]]) -> f64 {
    if positions.len() < 2 {
        return f64::INFINITY;
    }
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
    min_d
}

// ---------------------------------------------------------------------------
// Force-directed (Fruchterman–Reingold)
// ---------------------------------------------------------------------------

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

    #[test]
    fn spectral_path_graph_places_endpoints_farther() {
        // Path 0—1—2—3: spectral x-coordinate should order nodes along the path.
        let adj = vec![
            vec![(1, 1.0)],
            vec![(0, 1.0), (2, 1.0)],
            vec![(1, 1.0), (3, 1.0)],
            vec![(2, 1.0)],
        ];
        let c = RegisterConstraints::neutral_atom_default();
        let r = build_register_with(&adj, c, LayoutMethod::Spectral).unwrap();
        assert_eq!(r.len(), 4);

        // Endpoints (0,3) should be at least as far apart as any consecutive pair.
        let dist = |i: usize, j: usize| {
            let dx = r[i].1[0] - r[j].1[0];
            let dy = r[i].1[1] - r[j].1[1];
            (dx * dx + dy * dy).sqrt()
        };
        let end = dist(0, 3);
        let mid = dist(1, 2);
        assert!(
            end + 1e-9 >= mid,
            "path endpoints should not be closer than middle edge: end={end} mid={mid}"
        );

        // Min distance still holds.
        for i in 0..4 {
            for j in (i + 1)..4 {
                assert!(dist(i, j) >= c.min_distance_um - 1e-6);
            }
        }
    }

    #[test]
    fn spectral_layout_raw_is_some_for_triangle() {
        let adj = vec![
            vec![(1, 1.0), (2, 1.0)],
            vec![(0, 1.0), (2, 1.0)],
            vec![(0, 1.0), (1, 1.0)],
        ];
        let pos = spectral_embedding_layout(&adj).expect("spectral layout");
        assert_eq!(pos.len(), 3);
        assert!(layout_min_pairwise(&pos) > 1e-9);
    }

    #[test]
    fn force_directed_still_works_explicitly() {
        let adj = vec![vec![(1, 1.0)], vec![(0, 1.0)]];
        let c = RegisterConstraints::neutral_atom_default();
        let r = build_register_with(&adj, c, LayoutMethod::ForceDirected).unwrap();
        assert_eq!(r.len(), 2);
        let dx = r[0].1[0] - r[1].1[0];
        let dy = r[0].1[1] - r[1].1[1];
        let d = (dx * dx + dy * dy).sqrt();
        assert!(d >= c.min_distance_um - 1e-6);
    }

    #[test]
    fn spectral_and_force_both_deterministic() {
        let adj = vec![
            vec![(1, 1.0), (3, 1.0)],
            vec![(0, 1.0), (2, 1.0)],
            vec![(1, 1.0), (3, 1.0)],
            vec![(0, 1.0), (2, 1.0)],
        ];
        let c = RegisterConstraints::neutral_atom_default();
        for method in [LayoutMethod::Spectral, LayoutMethod::ForceDirected] {
            let a = build_register_with(&adj, c, method).unwrap();
            let b = build_register_with(&adj, c, method).unwrap();
            assert_eq!(a, b, "method {method:?} not deterministic");
        }
    }

    #[test]
    fn default_method_is_spectral() {
        assert_eq!(LayoutMethod::default(), LayoutMethod::Spectral);
    }
}
