//! Phase 2 — EML residual correction.
//!
//! A lightweight learned residual model. Given analytical scenario predictions
//! and the corresponding ground-truth deltas, fits a linear correction:
//!
//!   predicted_actual = β₀ + β₁·analytical + β₂·scope_size + β₃·|factor−1|
//!
//! Trained by ridge-regularised normal equations (closed form). This matches
//! the *shape* of `causal_predict.rs::CausalCollapseModel` — a tiny regressor
//! on top of an analytical predictor — sized appropriately for the test
//! harness. Replacing this with the kernel's EML in production is a
//! drop-in swap at the `engine.rs` call site.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EmlModel {
    /// [β₀, β₁, β₂, β₃]: intercept, analytical_slope, scope_slope, mag_slope.
    pub weights: [f64; 4],
    pub trained: bool,
    pub training_samples: usize,
    /// RMS residual on the training set (diagnostic).
    pub training_rmse: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmlSample {
    pub analytical: f64,
    pub scope_size: f64,
    pub factor_magnitude: f64,
    pub actual: f64,
}

impl EmlModel {
    /// Apply the learned correction to an analytical delta.
    pub fn apply(&self, analytical: f64, scope_size: f64, factor_magnitude: f64) -> f64 {
        if !self.trained {
            return analytical;
        }
        self.weights[0]
            + self.weights[1] * analytical
            + self.weights[2] * scope_size
            + self.weights[3] * factor_magnitude
    }

    /// Fit ridge-regularised OLS. `ridge` typical value: 1e-3.
    #[allow(clippy::needless_range_loop)] // matrix ops are clearer indexed
    pub fn fit(samples: &[EmlSample], ridge: f64) -> Self {
        if samples.is_empty() {
            return Self::default();
        }
        // Design: [1, analytical, scope_size, factor_magnitude].
        let p = 4;
        let n = samples.len();
        let mut xtx = [[0.0f64; 4]; 4];
        let mut xty = [0.0f64; 4];

        for s in samples {
            let x = [1.0, s.analytical, s.scope_size, s.factor_magnitude];
            for i in 0..p {
                for j in 0..p {
                    xtx[i][j] += x[i] * x[j];
                }
                xty[i] += x[i] * s.actual;
            }
        }
        for i in 0..p {
            xtx[i][i] += ridge;
        }

        let weights = solve4(xtx, xty);
        let mut model = Self {
            weights,
            trained: true,
            training_samples: n,
            training_rmse: 0.0,
        };

        let mut ss = 0.0f64;
        for s in samples {
            let pred = model.apply(s.analytical, s.scope_size, s.factor_magnitude);
            let r = pred - s.actual;
            ss += r * r;
        }
        model.training_rmse = (ss / n as f64).sqrt();
        model
    }
}

/// 4×4 linear solve via Gauss–Jordan with partial pivoting. Returns zeros on
/// singularity (with ridge > 0 this should not happen in practice).
#[allow(clippy::needless_range_loop)] // matrix ops are clearer indexed
fn solve4(a: [[f64; 4]; 4], b: [f64; 4]) -> [f64; 4] {
    let n = 4;
    let mut m = [[0.0f64; 5]; 4];
    for i in 0..n {
        for j in 0..n {
            m[i][j] = a[i][j];
        }
        m[i][4] = b[i];
    }
    for i in 0..n {
        let mut pivot_row = i;
        for k in (i + 1)..n {
            if m[k][i].abs() > m[pivot_row][i].abs() {
                pivot_row = k;
            }
        }
        m.swap(i, pivot_row);
        let pivot = m[i][i];
        if pivot.abs() < 1e-12 {
            return [0.0; 4];
        }
        for j in i..=n {
            m[i][j] /= pivot;
        }
        for k in 0..n {
            if k == i {
                continue;
            }
            let factor = m[k][i];
            for j in i..=n {
                m[k][j] -= factor * m[i][j];
            }
        }
    }
    [m[0][4], m[1][4], m[2][4], m[3][4]]
}
