//! Latent planners: CEM default, MPPI-warm, gradient shooting (WEFT-529).
//!
//! Runs on the **10 Hz** background path (never inline on the 1 ms DEMOCRITUS
//! servo). Uses a [`Predictor`] (`pred_φ`) to roll out candidate action
//! sequences and score them against an optional goal latent.

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

use weftos_worldmodel_core::{
    zero_latent, Action, ActionPlan, Latent, LatentPlanner, PlanStep, PlannerKind, Predictor,
    WorldModelResult, LATENT_DIM,
};

use crate::math_no_std::{cosf, lnf, sqrtf};
use crate::predictor::{IdentityPredictor, LinearPredPhi, NullPredictor};

/// Stub planner: emits `horizon` null actions.
///
/// When `echo_latent` is true, predicted latents copy `z_t` (identity
/// dynamics); otherwise they are the zero prior.
#[derive(Debug, Clone, Copy)]
pub struct NullPlanner {
    /// Reported algorithm kind (default CEM).
    pub kind: PlannerKind,
    /// If true, fill `z_hat` with `z_t`; else zero latent.
    pub echo_latent: bool,
}

impl Default for NullPlanner {
    fn default() -> Self {
        Self {
            kind: PlannerKind::Cem,
            echo_latent: true,
        }
    }
}

impl LatentPlanner for NullPlanner {
    fn plan(&self, z_t: &Latent, horizon: usize) -> WorldModelResult<ActionPlan> {
        let mut steps = Vec::with_capacity(horizon);
        let action = Action::null();
        for _ in 0..horizon {
            let z_hat = if self.echo_latent {
                IdentityPredictor.predict(z_t, &action)?
            } else {
                NullPredictor.predict(z_t, &action)?
            };
            steps.push(PlanStep { action, z_hat });
        }
        Ok(ActionPlan {
            steps,
            kind: self.kind,
        })
    }

    fn kind(&self) -> PlannerKind {
        self.kind
    }
}

/// Cross-entropy method planner (default production path).
///
/// Samples action sequences from a diagonal Gaussian over the action code,
/// rolls out with [`LinearPredPhi`] (or a caller-supplied predictor via
/// [`CemPlanner::plan_with`]), keeps the elite fraction, and refits mean/std.
///
/// # Scoring
///
/// Default cost is squared L2 distance of the final predicted latent to
/// [`CemPlanner::goal`] (zeros ⇒ prefer returning toward the prior origin).
/// Lower is better.
#[derive(Debug, Clone)]
pub struct CemPlanner {
    /// Reported kind (usually [`PlannerKind::Cem`]).
    pub kind: PlannerKind,
    /// Built-in residual predictor used by [`LatentPlanner::plan`].
    pub predictor: LinearPredPhi,
    /// Population size per CEM iteration.
    pub population: usize,
    /// Number of elite samples retained each iteration.
    pub elites: usize,
    /// CEM outer iterations.
    pub iterations: usize,
    /// Initial action-code standard deviation.
    pub init_std: f32,
    /// Goal latent for the default cost (defaults to origin).
    pub goal: Latent,
    /// Deterministic PRNG state (xorshift64).
    seed: u64,
}

impl Default for CemPlanner {
    fn default() -> Self {
        Self {
            kind: PlannerKind::Cem,
            predictor: LinearPredPhi::default(),
            population: 32,
            elites: 8,
            iterations: 4,
            init_std: 0.5,
            goal: zero_latent(),
            seed: 0xC0FFEE_u64,
        }
    }
}

impl CemPlanner {
    /// Construct a CEM planner with an explicit RNG seed (tests).
    pub fn with_seed(seed: u64) -> Self {
        Self {
            seed,
            ..Self::default()
        }
    }

    /// Set the planning goal latent.
    pub fn with_goal(mut self, goal: Latent) -> Self {
        self.goal = goal;
        self
    }

    /// Plan using an arbitrary predictor (for tests / service injection).
    pub fn plan_with<P: Predictor>(
        &self,
        predictor: &P,
        z_t: &Latent,
        horizon: usize,
    ) -> WorldModelResult<ActionPlan> {
        if horizon == 0 {
            return Ok(ActionPlan::empty(self.kind));
        }
        let pop = self.population.max(1);
        let elites = self.elites.clamp(1, pop);
        let iters = self.iterations.max(1);
        let action_dim = 32usize;

        // Per-horizon-step mean/std over action code.
        let mut mean = vec![[0.0f32; 32]; horizon];
        let mut std = vec![[self.init_std; 32]; horizon];
        let mut rng = self.seed ^ (horizon as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);

        let mut best_seq: Vec<Action> = vec![Action::null(); horizon];
        let mut best_cost = f32::INFINITY;
        let mut best_traj: Vec<Latent> = vec![*z_t; horizon];

        for _ in 0..iters {
            // Sample population: each member is horizon × action.
            let mut scored: Vec<(f32, Vec<Action>, Vec<Latent>)> = Vec::with_capacity(pop);
            for _ in 0..pop {
                let mut actions = Vec::with_capacity(horizon);
                for t in 0..horizon {
                    let mut a = Action::null();
                    for d in 0..action_dim {
                        let u = next_gaussian(&mut rng);
                        a.code[d] = mean[t][d] + std[t][d] * u;
                    }
                    actions.push(a);
                }
                let (cost, traj) = rollout_cost(predictor, z_t, &actions, &self.goal)?;
                if cost < best_cost {
                    best_cost = cost;
                    best_seq = actions.clone();
                    best_traj = traj.clone();
                }
                scored.push((cost, actions, traj));
            }
            scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(core::cmp::Ordering::Equal));
            let elite = &scored[..elites];

            // Refit mean/std from elites.
            for t in 0..horizon {
                for d in 0..action_dim {
                    let mut m = 0.0f32;
                    for e in elite {
                        m += e.1[t].code[d];
                    }
                    m /= elites as f32;
                    let mut v = 0.0f32;
                    for e in elite {
                        let dv = e.1[t].code[d] - m;
                        v += dv * dv;
                    }
                    v = sqrtf(v / elites as f32).max(1e-3);
                    mean[t][d] = m;
                    std[t][d] = v;
                }
            }
        }

        let mut steps = Vec::with_capacity(horizon);
        for t in 0..horizon {
            steps.push(PlanStep {
                action: best_seq[t],
                z_hat: best_traj[t],
            });
        }
        Ok(ActionPlan {
            steps,
            kind: self.kind,
        })
    }
}

impl LatentPlanner for CemPlanner {
    fn plan(&self, z_t: &Latent, horizon: usize) -> WorldModelResult<ActionPlan> {
        self.plan_with(&self.predictor, z_t, horizon)
    }

    fn kind(&self) -> PlannerKind {
        self.kind
    }
}

/// MPPI warm-start planner: fewer CEM iterations, wider initial noise.
#[derive(Debug, Clone)]
pub struct MppiWarmPlanner {
    /// Inner CEM engine (kind forced to MppiWarm).
    pub inner: CemPlanner,
}

impl Default for MppiWarmPlanner {
    fn default() -> Self {
        let inner = CemPlanner {
            kind: PlannerKind::MppiWarm,
            iterations: 2,
            init_std: 1.0,
            population: 48,
            ..CemPlanner::default()
        };
        Self { inner }
    }
}

impl LatentPlanner for MppiWarmPlanner {
    fn plan(&self, z_t: &Latent, horizon: usize) -> WorldModelResult<ActionPlan> {
        let mut plan = self.inner.plan(z_t, horizon)?;
        plan.kind = PlannerKind::MppiWarm;
        Ok(plan)
    }

    fn kind(&self) -> PlannerKind {
        PlannerKind::MppiWarm
    }
}

/// Gradient-shooting style planner: iterative residual steps along −∇cost
/// approximated by finite differences on the action code (no autodiff).
#[derive(Debug, Clone)]
pub struct GradientPlanner {
    /// Predictor for rollouts.
    pub predictor: LinearPredPhi,
    /// Gradient steps.
    pub steps: usize,
    /// Step size on the action code.
    pub lr: f32,
    /// Goal latent.
    pub goal: Latent,
    /// Finite-difference epsilon.
    pub eps: f32,
}

impl Default for GradientPlanner {
    fn default() -> Self {
        Self {
            predictor: LinearPredPhi::default(),
            steps: 8,
            lr: 0.1,
            goal: zero_latent(),
            eps: 1e-2,
        }
    }
}

impl LatentPlanner for GradientPlanner {
    fn plan(&self, z_t: &Latent, horizon: usize) -> WorldModelResult<ActionPlan> {
        if horizon == 0 {
            return Ok(ActionPlan::empty(PlannerKind::Gradient));
        }
        let mut actions = vec![Action::null(); horizon];
        for _ in 0..self.steps {
            for t in 0..horizon {
                for d in 0..32 {
                    let mut plus = actions.clone();
                    let mut minus = actions.clone();
                    plus[t].code[d] += self.eps;
                    minus[t].code[d] -= self.eps;
                    let (c_plus, _) = rollout_cost(&self.predictor, z_t, &plus, &self.goal)?;
                    let (c_minus, _) = rollout_cost(&self.predictor, z_t, &minus, &self.goal)?;
                    let g = (c_plus - c_minus) / (2.0 * self.eps);
                    actions[t].code[d] -= self.lr * g;
                }
            }
        }
        let mut z = *z_t;
        let mut steps = Vec::with_capacity(horizon);
        for a in &actions {
            z = self.predictor.predict(&z, a)?;
            steps.push(PlanStep {
                action: *a,
                z_hat: z,
            });
        }
        Ok(ActionPlan {
            steps,
            kind: PlannerKind::Gradient,
        })
    }

    fn kind(&self) -> PlannerKind {
        PlannerKind::Gradient
    }
}

/// Build a planner for the given algorithm kind (factory for service wiring).
pub fn planner_for_kind(kind: PlannerKind) -> Box<dyn LatentPlanner> {
    match kind {
        PlannerKind::Cem => Box::new(CemPlanner::default()),
        PlannerKind::MppiWarm => Box::new(MppiWarmPlanner::default()),
        PlannerKind::Gradient => Box::new(GradientPlanner::default()),
        _ => Box::new(CemPlanner::default()),
    }
}

fn rollout_cost<P: Predictor>(
    predictor: &P,
    z0: &Latent,
    actions: &[Action],
    goal: &Latent,
) -> WorldModelResult<(f32, Vec<Latent>)> {
    let mut z = *z0;
    let mut traj = Vec::with_capacity(actions.len());
    for a in actions {
        z = predictor.predict(&z, a)?;
        traj.push(z);
    }
    let final_z = traj.last().copied().unwrap_or(*z0);
    let mut cost = 0.0f32;
    for i in 0..LATENT_DIM {
        let d = final_z[i] - goal[i];
        cost += d * d;
    }
    // Action effort: penalise unused channels more so CEM concentrates on
    // dimensions that actually reduce goal error under LinearPredPhi.
    for a in actions {
        for (d, &c) in a.code.iter().enumerate() {
            let w = if d == 0 { 1e-5 } else { 5e-2 };
            cost += w * c * c;
        }
    }
    Ok((cost, traj))
}

/// Box-Muller / xorshift gaussian.
fn next_gaussian(state: &mut u64) -> f32 {
    // Two uniform (0,1) via xorshift64.
    let u1 = next_u01(state).max(1e-7);
    let u2 = next_u01(state);
    let r = sqrtf(-2.0 * lnf(u1));
    let theta = 6.283_185_5 * u2;
    r * cosf(theta)
}

fn next_u01(state: &mut u64) -> f32 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    ((*state >> 11) as f32) / ((1u64 << 53) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_horizon_length() {
        let p = NullPlanner::default();
        let plan = p.plan(&zero_latent(), 5).unwrap();
        assert_eq!(plan.steps.len(), 5);
        assert_eq!(plan.kind, PlannerKind::Cem);
    }

    #[test]
    fn cem_toy_task_moves_toward_goal() {
        // Start at origin; goal has +1 on action-aligned dims 0,32,64,...
        // LinearPredPhi with a.code[0]=positive moves those dims toward goal.
        let mut goal = zero_latent();
        for i in 0..LATENT_DIM {
            if i % 32 == 0 {
                goal[i] = 1.0;
            }
        }
        let mut planner = CemPlanner::with_seed(42).with_goal(goal);
        planner.population = 48;
        planner.elites = 12;
        planner.iterations = 6;
        let z0 = zero_latent();
        let plan = planner.plan(&z0, 3).unwrap();
        assert_eq!(plan.steps.len(), 3);
        assert_eq!(plan.kind, PlannerKind::Cem);

        // Score only goal-aligned dims (unused action channels may carry noise).
        let final_z = plan.steps.last().unwrap().z_hat;
        let mut cost_plan = 0.0f32;
        let mut cost_zero = 0.0f32;
        for i in (0..LATENT_DIM).step_by(32) {
            let d = final_z[i] - goal[i];
            cost_plan += d * d;
            cost_zero += goal[i] * goal[i];
        }
        assert!(
            cost_plan < cost_zero,
            "CEM should beat null plan: plan={cost_plan} null={cost_zero}"
        );
        let a0_sum: f32 = plan.steps.iter().map(|s| s.action.code[0]).sum();
        assert!(a0_sum > 0.0, "expected positive net a0, got {a0_sum}");
    }

    #[test]
    fn mppi_and_gradient_kinds() {
        let m = MppiWarmPlanner::default();
        assert_eq!(m.kind(), PlannerKind::MppiWarm);
        let plan = m.plan(&zero_latent(), 2).unwrap();
        assert_eq!(plan.kind, PlannerKind::MppiWarm);
        assert_eq!(plan.steps.len(), 2);

        let g = GradientPlanner::default();
        assert_eq!(g.kind(), PlannerKind::Gradient);
        let plan = g.plan(&zero_latent(), 2).unwrap();
        assert_eq!(plan.kind, PlannerKind::Gradient);
    }

    #[test]
    fn planner_for_kind_factory() {
        for kind in [PlannerKind::Cem, PlannerKind::MppiWarm, PlannerKind::Gradient] {
            let p = planner_for_kind(kind);
            assert_eq!(p.kind(), kind);
            assert_eq!(p.plan(&zero_latent(), 1).unwrap().steps.len(), 1);
        }
    }
}
