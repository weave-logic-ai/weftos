//! Phase B: tiers, measured performance and its budget filter, accelerator
//! fit, locality preferences, load and stickiness.

use super::fixtures::*;
use super::{placed_on, rejected_by, run};
use crate::placement::engine::{Constraint, Flag, PlacementRequest, Tier};

fn tier_of(d: &crate::placement::engine::Decision, node: &str) -> Option<Tier> {
    d.candidates
        .iter()
        .find(|c| c.node_id == node)
        .and_then(|c| c.tier)
}

fn score_of(d: &crate::placement::engine::Decision, node: &str) -> f64 {
    d.candidates
        .iter()
        .find(|c| c.node_id == node)
        .and_then(|c| c.score.as_ref())
        .map(|s| s.total())
        .unwrap_or_else(|| panic!("{node} has no score"))
}

#[test]
fn native_beats_emulated_beats_dev_fallback() {
    let mut req = PlacementRequest::new(sensor_spec("cog", "anomaly-detect", &["aarch64"], None));
    req.allow_emulated = true;
    let d = run(&req, &[pi5(), mac(), x86()]);
    assert_eq!(tier_of(&d, "pi5"), Some(Tier::Native));
    assert_eq!(tier_of(&d, "mac-dev"), Some(Tier::DevFallback));
    assert_eq!(tier_of(&d, "x86"), Some(Tier::Emulated));
    let exec = |n: &str| {
        d.candidates
            .iter()
            .find(|c| c.node_id == n)
            .unwrap()
            .score
            .as_ref()
            .unwrap()
            .execution
    };
    assert_eq!(
        (exec("pi5"), exec("mac-dev"), exec("x86")),
        (100.0, 20.0, 40.0)
    );
    assert_eq!(placed_on(&d), Some("pi5"));
    // Without real hardware, the opted-in emulated route beats the dev Mac.
    let d = run(&req, &[mac(), x86()]);
    assert_eq!(placed_on(&d), Some("x86"));
}

#[test]
fn cycle_budget_filters_slow_nodes_and_ranks_passers_by_measurement() {
    let d = run(
        &PlacementRequest::new(fall_detect()),
        &[zero(), seed(), pi5()],
    );
    assert_eq!(rejected_by(&d, "zero"), vec![Constraint::PerfBudget]);
    let why = &d
        .candidates
        .iter()
        .find(|c| c.node_id == "zero")
        .unwrap()
        .rejections[0];
    assert!(
        why.detail.contains("6000") && why.detail.contains("2000"),
        "{}",
        why.detail
    );
    let perf = |n: &str| {
        d.candidates
            .iter()
            .find(|c| c.node_id == n)
            .unwrap()
            .score
            .as_ref()
            .unwrap()
            .perf
    };
    // Interactive doubles the 30-point perf weight; pi5 is the fastest.
    assert_eq!(perf("pi5"), 60.0);
    assert!((perf("seed") - 60.0 * 1000.0 / 1100.0).abs() < 1e-9);
    assert_eq!(placed_on(&d), Some("pi5"));
}

#[test]
fn slow_node_wins_without_a_budget_only_if_nothing_else_fits() {
    let mut spec = fall_detect();
    spec.policy.perf.as_mut().unwrap().budget = None;
    let d = run(&PlacementRequest::new(spec), &[zero()]);
    assert_eq!(placed_on(&d), Some("zero"));
}

#[test]
fn unmeasured_node_is_flagged_and_scored_conservatively() {
    let mut blind = pi5();
    blind.id = "pi5-new".into();
    blind.caps.retain(|c| c.id.as_str() != "perf.cog.cycle_ms");
    let d = run(&PlacementRequest::new(fall_detect()), &[blind, seed()]);
    let c = d
        .candidates
        .iter()
        .find(|c| c.node_id == "pi5-new")
        .unwrap();
    assert!(c.eligible(), "unmeasured is not filtered");
    assert!(c.flags.contains(&Flag::Unmeasured));
    assert_eq!(c.score.as_ref().unwrap().perf, 0.0);
    assert_eq!(
        placed_on(&d),
        Some("seed"),
        "measured passer outranks unmeasured"
    );
}

#[test]
fn smallest_sufficient_accelerator_wins_and_cpu_work_avoids_accelerators() {
    let d = run(
        &PlacementRequest::new(gguf_server("coder", 20)),
        &[mac(), gpu()],
    );
    let fit = |n: &str| {
        d.candidates
            .iter()
            .find(|c| c.node_id == n)
            .unwrap()
            .score
            .as_ref()
            .unwrap()
            .accel_fit
    };
    assert_eq!(fit("gpu-box"), 10.0);
    assert!((fit("mac-dev") - 10.0 * 24.0 / 128.0).abs() < 1e-9);
    assert_eq!(
        d.placement.unwrap().accelerator.as_deref(),
        Some("accel.gpu.cuda")
    );

    // A CPU-only workload: the plain Pi 5 outranks the Coral node, which
    // would idle its accelerator.
    let d = run(
        &PlacementRequest::new(sensor_spec("cog", "anomaly-detect", &["aarch64"], None)),
        &[coral(), pi5()],
    );
    assert!(score_of(&d, "pi5") > score_of(&d, "coral"));
    assert_eq!(placed_on(&d), Some("pi5"));
}

#[test]
fn locality_preference_moves_the_choice_within_a_tier() {
    let mut other = gpu();
    other.id = "gpu-box-b".into();
    let base = run(
        &PlacementRequest::new(gguf_server("coder", 20)),
        &[other.clone(), gpu()],
    );
    assert_eq!(placed_on(&base), Some("gpu-box"), "tie breaks by node id");
    let mut with_weights = other;
    with_weights
        .caps
        .push(cap("model.present").with_attr("shards", list(&["blake3-model-a"])));
    let mut spec = gguf_server("coder", 20);
    spec.policy.preferences[0].weight = 100.0;
    let d = run(&PlacementRequest::new(spec.clone()), &[with_weights, gpu()]);
    let loc = d
        .candidates
        .iter()
        .find(|c| c.node_id == "gpu-box-b")
        .unwrap();
    assert_eq!(loc.score.as_ref().unwrap().locality, 100.0);
    assert_eq!(placed_on(&d), Some("gpu-box-b"));

    // Locality never lifts the dev Mac over real hardware (strict tiers).
    let mut mac_weights = mac();
    mac_weights
        .caps
        .push(cap("model.present").with_attr("shards", list(&["blake3-model-a"])));
    let d = run(&PlacementRequest::new(spec), &[mac_weights, gpu()]);
    assert_eq!(placed_on(&d), Some("gpu-box"));
}

#[test]
fn load_headroom_breaks_near_ties_and_unknown_load_is_conservative() {
    let spec = sensor_spec("cog", "anomaly-detect", &["aarch64"], None);
    let mut busy = pi5();
    busy.id = "pi5-busy".into();
    busy.load = Some(0.9);
    let mut unknown = pi5();
    unknown.id = "pi5-unknown".into();
    unknown.load = None;
    let d = run(&PlacementRequest::new(spec), &[busy, unknown, pi5()]);
    assert_eq!(placed_on(&d), Some("pi5"));
    let u = d
        .candidates
        .iter()
        .find(|c| c.node_id == "pi5-unknown")
        .unwrap();
    assert!(u.flags.contains(&Flag::LoadUnknown));
    assert_eq!(u.score.as_ref().unwrap().load, 0.0);
}

#[test]
fn stickiness_keeps_the_current_node_and_sticky_resists_better_nodes() {
    // Seed scores below pi5 (slower measured cycle) on its own.
    let mut req = PlacementRequest::new(fall_detect());
    req.current_node = Some("seed".into());
    let d = run(&req, &[pi5(), seed()]);
    assert_eq!(
        placed_on(&d),
        Some("seed"),
        "ordinary stickiness covers a small gap"
    );

    // A large gap: seed loses the budget-free perf edge and is loaded.
    let mut slow = seed();
    slow.load = Some(1.0);
    let mut spec = fall_detect();
    spec.policy.latency_class = Default::default();
    let mut req = PlacementRequest::new(spec.clone());
    req.current_node = Some("seed".into());
    req.weights.stickiness = 1.0;
    assert_eq!(placed_on(&run(&req, &[pi5(), slow.clone()])), Some("pi5"));
    spec.policy.sticky = true;
    req.spec = spec;
    assert_eq!(placed_on(&run(&req, &[pi5(), slow])), Some("seed"));
}

#[test]
fn ranking_is_independent_of_fact_order() {
    let req = PlacementRequest::new(sensor_spec("cog", "anomaly-detect", &["aarch64"], None));
    let mut nodes = fleet();
    let a = run(&req, &nodes);
    nodes.reverse();
    let b = run(&req, &nodes);
    assert_eq!(a, b);
}
