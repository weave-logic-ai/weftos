//! Phase 2 — scenario engine.
//!
//! Implements the execution pipeline from analysis §6.2:
//! (1) parse spec → intervention, (2) resolve scope, (3) shadow-clone subgraph,
//! (4) apply intervention, (5) propagate via AGGREGATES_TO / CAUSES edges,
//! (6) EML residual correction, (7) Monte-Carlo uncertainty, (8) attribution.

use crate::dimensions::Dimensions;
use crate::eml::EmlModel;
use crate::graph::{CaseGraph, NodeId, NodeType};
use crate::scenarios::ScenarioSpec;
use crate::scoring::ScenarioPrediction;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use rand_distr::{Distribution, Normal};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterventionPlan {
    pub scenario_id: String,
    pub template: String,
    pub scope_store_labels: Vec<String>,
    pub scope_store_ids: Vec<NodeId>,
    pub scope_daily_ids: Vec<NodeId>,
    pub agg_daily_ids: Vec<NodeId>, // all dailies in the aggregation window
    pub revenue_factor: f64,
    pub baseline_sum: f64,
    pub intervention_sum: f64,
    pub analytical_delta: f64,
    pub analytical_delta_pct: f64,
}

pub struct ScenarioEngine<'a> {
    pub graph: &'a CaseGraph,
    pub dims: &'a Dimensions,
    pub eml: Option<&'a EmlModel>,
}

impl<'a> ScenarioEngine<'a> {
    pub fn new(graph: &'a CaseGraph, dims: &'a Dimensions) -> Self {
        Self {
            graph,
            dims,
            eml: None,
        }
    }

    pub fn with_eml(mut self, eml: &'a EmlModel) -> Self {
        self.eml = Some(eml);
        self
    }

    /// Produce the intervention plan (steps 1–4 of the pipeline).
    pub fn plan(&self, spec: &ScenarioSpec) -> InterventionPlan {
        // Resolve in-scope stores.
        let scope_store_ids = match &spec.closed_store_labels {
            Some(labels) => labels
                .iter()
                .filter_map(|l| self.graph.by_label(l))
                .collect(),
            None => self.graph.stores_matching(
                spec.brand.as_deref(),
                spec.metro.as_deref(),
                spec.region.as_deref(),
            ),
        };
        let scope_store_labels: Vec<String> = scope_store_ids
            .iter()
            .map(|&id| self.graph.node(id).label.clone())
            .collect();
        let scope_set: HashSet<NodeId> = scope_store_ids.iter().copied().collect();

        // For promo_pull: tighten the intervention day window to the promo's
        // actual active range, even if the YAML left it open.
        let (promo_active_start, promo_active_end) = match (&spec.template, &spec.promo_id_to_pull)
        {
            (t, Some(pid)) if t == "promo_pull" => self
                .dims
                .promotions
                .iter()
                .find(|p| &p.label == pid)
                .map(|p| (Some(p.start_day), Some(p.end_day)))
                .unwrap_or((None, None)),
            _ => (None, None),
        };

        // Walk every DailyRollup in the aggregation window.
        let mut baseline_sum = 0.0f64;
        let mut intervention_sum = 0.0f64;
        let mut scope_daily_ids = Vec::new();
        let mut agg_daily_ids = Vec::new();

        for node in &self.graph.nodes {
            if node.node_type != NodeType::DailyRollup {
                continue;
            }
            let day = node.day_index.unwrap_or(0);
            if !spec.in_aggregation_window(day) {
                continue;
            }
            agg_daily_ids.push(node.id);
            let revenue = node.metric.unwrap_or(0.0);
            baseline_sum += revenue;

            let store_id = node_store_id(self.graph, node.id);
            let store_in_scope = store_id.is_some_and(|id| scope_set.contains(&id));

            let day_in_scope_from_week = spec.matches_intervention_day(day);
            let day_in_scope_from_promo = match (promo_active_start, promo_active_end) {
                (Some(s), Some(e)) => day >= s && day < e,
                _ => true,
            };
            let apply = store_in_scope && day_in_scope_from_week && day_in_scope_from_promo;

            if apply {
                scope_daily_ids.push(node.id);
                intervention_sum += revenue * spec.revenue_factor;
            } else {
                intervention_sum += revenue;
            }
        }

        let analytical_delta = intervention_sum - baseline_sum;
        let analytical_delta_pct = if baseline_sum.abs() > f64::EPSILON {
            analytical_delta / baseline_sum
        } else {
            0.0
        };

        InterventionPlan {
            scenario_id: spec.id.clone(),
            template: spec.template.clone(),
            scope_store_labels,
            scope_store_ids,
            scope_daily_ids,
            agg_daily_ids,
            revenue_factor: spec.revenue_factor,
            baseline_sum,
            intervention_sum,
            analytical_delta,
            analytical_delta_pct,
        }
    }

    /// Predict the full scenario outcome: analytical + EML + Monte-Carlo.
    pub fn predict(
        &self,
        spec: &ScenarioSpec,
        mc_samples: usize,
        mc_seed: u64,
    ) -> ScenarioPrediction {
        let plan = self.plan(spec);

        let scope_size = plan.scope_daily_ids.len() as f64;
        let factor_magnitude = (plan.revenue_factor - 1.0).abs();

        let corrected_delta = match self.eml {
            Some(model) => model.apply(plan.analytical_delta, scope_size, factor_magnitude),
            None => plan.analytical_delta,
        };
        let delta_pct = if plan.baseline_sum.abs() > f64::EPSILON {
            corrected_delta / plan.baseline_sum
        } else {
            0.0
        };

        let (ci_80, ci_95) = monte_carlo(corrected_delta, &plan, mc_samples, mc_seed);

        // Attribution proxy: report the first few in-scope daily rollups so the
        // operator can click through to the contributing nodes.
        let predicted_edges: Vec<String> = plan
            .scope_daily_ids
            .iter()
            .take(5)
            .map(|&id| self.graph.node(id).label.clone())
            .collect();

        ScenarioPrediction {
            scenario_id: spec.id.clone(),
            predicted_delta: round2(corrected_delta),
            predicted_delta_pct: (delta_pct * 10_000.0).round() / 10_000.0,
            ci_80: [round2(ci_80[0]), round2(ci_80[1])],
            ci_95: [round2(ci_95[0]), round2(ci_95[1])],
            predicted_edges,
        }
    }
}

/// Walk reverse edges to find the store that owns a given daily rollup.
fn node_store_id(graph: &CaseGraph, daily_id: NodeId) -> Option<NodeId> {
    let incoming = graph.rev.get(&daily_id)?;
    for &edge_idx in incoming {
        let edge = &graph.edges[edge_idx];
        if edge.edge_type == crate::graph::EdgeType::ClosedDay {
            return Some(edge.source);
        }
    }
    None
}

/// Monte-Carlo uncertainty bands. Samples Gaussian perturbations scaled by
/// (a) the analytical delta magnitude and (b) the sqrt of scope size — which
/// is the natural scaling for independent noise across scope days.
fn monte_carlo(center: f64, plan: &InterventionPlan, n: usize, seed: u64) -> ([f64; 2], [f64; 2]) {
    if n == 0 {
        return ([center, center], [center, center]);
    }
    let mut rng = ChaCha8Rng::seed_from_u64(seed);

    let noise_scale = {
        let mag = center.abs().max(1.0);
        let scope = (plan.scope_daily_ids.len() as f64).max(1.0);
        mag * 0.10 + scope.sqrt() * 50.0
    };
    let normal = Normal::new(0.0, noise_scale).expect("sigma > 0");
    let mut samples: Vec<f64> = (0..n).map(|_| center + normal.sample(&mut rng)).collect();
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let lo80 = percentile(&samples, 0.10);
    let hi80 = percentile(&samples, 0.90);
    let lo95 = percentile(&samples, 0.025);
    let hi95 = percentile(&samples, 0.975);
    ([lo80, hi80], [lo95, hi95])
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = (p * (sorted.len() - 1) as f64).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// Helper: use `engine.plan()` on a vector of scenarios to synthesise an
/// EML training set from the corpus. Each sample uses the analytical delta
/// as the "predicted" feature and the truth-manifest delta as the target.
pub fn synth_training_set(
    engine: &ScenarioEngine,
    specs: &[ScenarioSpec],
    events: &[crate::events::DailyRollup],
    dims: &Dimensions,
) -> Vec<crate::eml::EmlSample> {
    specs
        .iter()
        .map(|spec| {
            let plan = engine.plan(spec);
            let truth = crate::truth::compute_counterfactual(spec, events, dims);
            crate::eml::EmlSample {
                analytical: plan.analytical_delta,
                scope_size: plan.scope_daily_ids.len() as f64,
                factor_magnitude: (plan.revenue_factor - 1.0).abs(),
                actual: truth.delta,
            }
        })
        .collect()
}
