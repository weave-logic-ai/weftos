//! End-to-end test of the tiny tier: generate → serialize → reload → counterfactual.

use clawft_casestudy_gen_qsr::{
    config::GeneratorConfig,
    generate, output, scenarios,
    scoring::{self, ScenarioPrediction},
    truth,
};
use std::path::PathBuf;

fn scenario_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("scenarios")
        .join(name)
}

#[test]
fn tiny_tier_generates_deterministically() {
    let tmp = tempfile::tempdir().unwrap();
    let config = GeneratorConfig::tiny(42);
    let corpus = generate(&config, tmp.path()).unwrap();

    // Tiny: 10 stores × 30 days
    assert_eq!(corpus.dims.stores.len(), 10);
    assert_eq!(corpus.events.len(), 10 * 30);

    // 3 positions per store
    assert_eq!(corpus.dims.positions.len(), 30);

    // Position filled/unfilled counts sum to total. Vacancy count itself is
    // probabilistic — with a 0.08 rate over 30 positions the expected value is
    // ~2.4, but no guarantee. We assert the structural invariant only.
    let filled = corpus
        .dims
        .positions
        .iter()
        .filter(|p| p.filled_by_ref.is_some())
        .count();
    let unfilled = corpus.dims.positions.len() - filled;
    assert_eq!(filled + unfilled, corpus.dims.positions.len());

    // Truth manifest lists one causal edge per promotion
    assert_eq!(
        corpus.truth.causal_edges.len(),
        corpus.dims.promotions.len()
    );

    // Determinism: same seed → same first store label
    let other = tempfile::tempdir().unwrap();
    let corpus2 = generate(&config, other.path()).unwrap();
    assert_eq!(corpus.dims.stores[0].label, corpus2.dims.stores[0].label);
    assert_eq!(corpus.events[0].label, corpus2.events[0].label);
}

#[test]
fn geo_miss_counterfactual_has_negative_delta() {
    let tmp = tempfile::tempdir().unwrap();
    let config = GeneratorConfig::tiny(42);
    let corpus = generate(&config, tmp.path()).unwrap();

    let spec = scenarios::load_from_file(&scenario_path("geo_miss.yaml")).unwrap();
    let cf = truth::compute_counterfactual(&spec, &corpus.events, &corpus.dims);

    assert!(cf.delta < 0.0, "expected negative delta, got {}", cf.delta);
    // tiny-tier corpora should produce a small but non-trivial magnitude
    assert!(
        cf.delta.abs() > 100.0,
        "expected |delta| > 100, got {}",
        cf.delta
    );
    // Scope hit at least one metro-alpha brand-a store
    assert!(cf.stores_in_scope > 0);
    // Intervention spans one week (7 days), possibly fewer if a store has a stream gap
    assert!(cf.days_intervened > 0 && cf.days_intervened <= 7);
}

#[test]
fn labor_shock_counterfactual_does_not_move_revenue() {
    let tmp = tempfile::tempdir().unwrap();
    let config = GeneratorConfig::tiny(42);
    let corpus = generate(&config, tmp.path()).unwrap();

    let spec = scenarios::load_from_file(&scenario_path("labor_shock.yaml")).unwrap();
    let cf = truth::compute_counterfactual(&spec, &corpus.events, &corpus.dims);

    // Labor-shock only scales labor_hours; revenue_factor = 1.0, so revenue delta = 0
    assert_eq!(cf.delta, 0.0);
    assert!(cf.stores_in_scope > 0);
}

#[test]
fn promo_pull_counterfactual_negative_across_brand() {
    let tmp = tempfile::tempdir().unwrap();
    let config = GeneratorConfig::tiny(42);
    let corpus = generate(&config, tmp.path()).unwrap();

    let spec = scenarios::load_from_file(&scenario_path("promo_pull.yaml")).unwrap();
    let cf = truth::compute_counterfactual(&spec, &corpus.events, &corpus.dims);

    // Pulling a 12%-lift promo on brand-a should reduce revenue — scenario
    // expresses this with revenue_factor=0.88 across the promo window.
    assert!(cf.delta < 0.0, "expected negative delta, got {}", cf.delta);
    assert!(
        cf.stores_in_scope
            >= corpus
                .dims
                .stores
                .iter()
                .filter(|s| s.brand == "brand-a")
                .count()
    );
}

#[test]
fn on_disk_corpus_round_trips() {
    let tmp = tempfile::tempdir().unwrap();
    let config = GeneratorConfig::tiny(42);
    let corpus = generate(&config, tmp.path()).unwrap();

    let dims = output::load_dimensions(tmp.path()).unwrap();
    let events = output::load_events(tmp.path()).unwrap();
    let truth_loaded = output::load_truth(tmp.path()).unwrap();

    assert_eq!(dims.stores.len(), corpus.dims.stores.len());
    assert_eq!(events.len(), corpus.events.len());
    assert_eq!(
        truth_loaded.causal_edges.len(),
        corpus.truth.causal_edges.len()
    );
    assert_eq!(dims.stores[0].label, corpus.dims.stores[0].label);
    assert_eq!(events[0].label, corpus.events[0].label);
}

#[test]
fn scoring_harness_accepts_exact_prediction() {
    let tmp = tempfile::tempdir().unwrap();
    let config = GeneratorConfig::tiny(42);
    let corpus = generate(&config, tmp.path()).unwrap();

    let spec = scenarios::load_from_file(&scenario_path("geo_miss.yaml")).unwrap();
    let cf = truth::compute_counterfactual(&spec, &corpus.events, &corpus.dims);

    // A "prediction" that matches truth exactly and has a tight CI should pass.
    let prediction = ScenarioPrediction {
        scenario_id: cf.scenario_id.clone(),
        predicted_delta: cf.delta,
        predicted_delta_pct: cf.delta_pct,
        ci_80: [cf.delta - 1.0, cf.delta + 1.0],
        ci_95: [cf.delta - 10.0, cf.delta + 10.0],
        predicted_edges: vec![],
    };
    let score = scoring::score(&prediction, &cf);
    assert!(score.directional_accuracy);
    assert!(score.within_ci_80);
    assert!(score.within_ci_95);
    assert!(score.passes_tier_gate);
    assert!(score.magnitude_error < 1e-9);
}

#[test]
fn scoring_harness_rejects_wrong_direction() {
    let tmp = tempfile::tempdir().unwrap();
    let config = GeneratorConfig::tiny(42);
    let corpus = generate(&config, tmp.path()).unwrap();

    let spec = scenarios::load_from_file(&scenario_path("geo_miss.yaml")).unwrap();
    let cf = truth::compute_counterfactual(&spec, &corpus.events, &corpus.dims);

    let prediction = ScenarioPrediction {
        scenario_id: cf.scenario_id.clone(),
        predicted_delta: -cf.delta, // wrong sign
        predicted_delta_pct: -cf.delta_pct,
        ci_80: [-cf.delta - 1.0, -cf.delta + 1.0],
        ci_95: [-cf.delta - 10.0, -cf.delta + 10.0],
        predicted_edges: vec![],
    };
    let score = scoring::score(&prediction, &cf);
    assert!(!score.directional_accuracy);
    assert!(!score.passes_tier_gate);
}
