//! Phase B: preference scoring among phase A survivors (ADR-099 section 3).
//!
//! Components: execution tier weight (the strict tier order itself is
//! applied by ranking, see [`Tier::rank`]), preferences (data locality),
//! accelerator fit, measured performance (with a budget filter), load and
//! stickiness. Every component is computed from
//! capabilities and policy, never from the workload kind. Relative
//! components (accelerator fit, performance) are normalised against the
//! best survivor, so they depend on the candidate set but not on its order.

use super::super::capability::{AttrValue, Capability, Provenance};
use super::super::perf::VALUE_ATTR;
use super::constraints::Eligible;
use super::decision::{Constraint, Flag, Rejection, ScoreBreakdown, Tier};
use super::facts::PlacementFacts;
use super::request::PlacementRequest;
use super::spec::{Better, LatencyClass, PerfTarget};

/// Accelerator capability family.
const ACCEL_FAMILY: &str = "accel";
/// Accelerator capacity attribute.
const ACCEL_MEM_ATTR: &str = "mem_bytes";

/// A survivor's score, or its phase B rejection.
pub(super) type Scored = Result<(ScoreBreakdown, Vec<Flag>), Rejection>;

/// A measurement the engine may score: finite and non-negative, the same
/// rule the [`perf`](super::super::perf) constructors enforce. Facts built
/// or deserialized any other way are checked here, so a NaN, infinite or
/// negative value cannot earn credit (NaN compares false against every
/// guard in [`ratio`] and would otherwise score as the best node).
fn valid_measurement(v: f64) -> bool {
    v.is_finite() && v >= 0.0
}

/// Best valid measured value of `t` on a node (`measured` provenance only).
/// `None` when the node has no valid measurement: it is then scored as
/// unmeasured (conservatively, and flagged).
pub(super) fn measured(caps: &[Capability], t: &PerfTarget) -> Option<f64> {
    let want = AttrValue::Str(t.param_value.clone());
    let vals = caps.iter().filter(|c| {
        c.id == t.id
            && c.provenance == Provenance::Measured
            && c.attrs.get(&t.param).is_some_and(|v| v.loose_eq(&want))
    });
    let nums = vals
        .filter_map(|c| c.attrs.get(VALUE_ATTR).and_then(AttrValue::as_f64))
        .filter(|v| valid_measurement(*v));
    match t.better {
        Better::Lower => nums.reduce(f64::min),
        Better::Higher => nums.reduce(f64::max),
    }
}

fn meets(t: &PerfTarget, v: f64) -> bool {
    match (t.budget, t.better) {
        (None, _) => true,
        (Some(b), Better::Lower) => v <= b,
        (Some(b), Better::Higher) => v >= b,
    }
}

/// Assigned accelerator capabilities of a survivor.
fn assigned_accels<'a>(caps: &'a [Capability], e: &Eligible) -> Vec<&'a Capability> {
    let mut idx: Vec<usize> = e.assignment.iter().flatten().copied().collect();
    idx.sort_unstable();
    idx.dedup();
    idx.into_iter()
        .map(|i| &caps[i])
        .filter(|c| c.id.family() == ACCEL_FAMILY)
        .collect()
}

fn accel_capacity(accels: &[&Capability]) -> Option<f64> {
    let v: Vec<f64> = accels
        .iter()
        .filter_map(|c| c.attrs.get(ACCEL_MEM_ATTR).and_then(AttrValue::as_f64))
        .collect();
    (!v.is_empty()).then(|| v.iter().sum())
}

fn ratio(best: f64, this: f64, better: Better) -> f64 {
    let r = match better {
        Better::Lower if this > 0.0 => best / this,
        Better::Lower => 1.0,
        Better::Higher if best > 0.0 => this / best,
        Better::Higher => 1.0,
    };
    r.clamp(0.0, 1.0)
}

/// Score every survivor. `survivors[i]` pairs with `nodes[i]`.
pub(super) fn score_all<F: PlacementFacts>(
    req: &PlacementRequest,
    nodes: &[&F],
    survivors: &[Eligible],
) -> Vec<Scored> {
    let w = &req.weights;
    let policy = &req.spec.policy;
    let perf = policy.perf.as_ref();
    let values: Vec<Option<f64>> = nodes
        .iter()
        .map(|n| perf.and_then(|t| measured(n.capabilities(), t)))
        .collect();
    let passing = |i: usize| perf.is_some_and(|t| values[i].is_some_and(|v| meets(t, v)));
    let best_perf = perf.and_then(|t| {
        (0..nodes.len())
            .filter(|&i| passing(i))
            .filter_map(|i| values[i])
            .reduce(|a, b| match t.better {
                Better::Lower => a.min(b),
                Better::Higher => a.max(b),
            })
    });
    let caps_of = |i: usize| assigned_accels(nodes[i].capabilities(), &survivors[i]);
    let smallest_accel = (0..nodes.len())
        .filter_map(|i| accel_capacity(&caps_of(i)))
        .reduce(f64::min);
    let perf_weight = match policy.latency_class {
        LatencyClass::Interactive => w.perf * w.interactive_perf_multiplier,
        LatencyClass::Batch => w.perf,
    };

    (0..nodes.len())
        .map(|i| {
            let node = nodes[i];
            let e = &survivors[i];
            let caps = node.capabilities();
            let mut flags = Vec::new();
            let mut s = ScoreBreakdown {
                execution: match e.tier {
                    Tier::Native => w.native,
                    Tier::DevFallback => w.dev_fallback,
                    Tier::Emulated => w.emulated,
                },
                ..ScoreBreakdown::default()
            };
            if e.tier == Tier::Emulated {
                flags.push(Flag::Emulated);
            }
            s.locality = policy
                .preferences
                .iter()
                .filter(|p| p.requirement.matches(caps))
                .map(|p| p.weight)
                // Not `sum()`: an empty f64 sum is -0.0.
                .fold(0.0, |a, b| a + b);

            let accels = caps_of(i);
            s.accel_fit = if accels.is_empty() {
                // Workload needs no accelerator: prefer nodes that would not
                // waste one on it.
                if caps.iter().any(|c| c.id.family() == ACCEL_FAMILY) {
                    0.0
                } else {
                    w.accel_fit
                }
            } else {
                match (smallest_accel, accel_capacity(&accels)) {
                    (Some(min), Some(this)) => w.accel_fit * ratio(min, this, Better::Lower),
                    _ => w.accel_fit,
                }
            };

            if let Some(t) = perf {
                match values[i] {
                    Some(v) if !meets(t, v) => {
                        return Err(Rejection {
                            constraint: Constraint::PerfBudget,
                            detail: format!(
                                "measured {} {v} misses budget {}",
                                t.id,
                                t.budget.unwrap_or_default()
                            ),
                        });
                    }
                    Some(v) => {
                        let best = best_perf.unwrap_or(v);
                        s.perf = perf_weight * ratio(best, v, t.better);
                    }
                    None => flags.push(Flag::Unmeasured),
                }
            }

            match node.load() {
                Some(l) if l.is_finite() => s.load = w.load * (1.0 - l.clamp(0.0, 1.0)),
                _ => flags.push(Flag::LoadUnknown),
            }
            if req.current_node.as_deref() == Some(node.node_id()) {
                s.stickiness = if policy.sticky {
                    w.sticky
                } else {
                    w.stickiness
                };
            }
            flags.sort_unstable();
            Ok((s, flags))
        })
        .collect()
}
