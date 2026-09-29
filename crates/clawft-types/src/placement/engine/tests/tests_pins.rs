//! Operator overrides: pins, affinity, and the explicit emulation opt-in.

use super::fixtures::*;
use super::{cluster, placed_on, run};
use crate::placement::engine::{
    Constraint, Execution, Flag, PlacementError, PlacementRequest, Tier, place,
};

fn aarch64_only() -> PlacementRequest {
    PlacementRequest::new(sensor_spec("cog", "anomaly-detect", &["aarch64"], None))
}

#[test]
fn pin_to_ineligible_node_errors_naming_the_constraint() {
    let cases: Vec<(&str, PlacementRequest, Vec<TestNode>, Constraint)> = vec![
        (
            "wrong arch, no emulation",
            aarch64_only(),
            fleet(),
            Constraint::EmulationNotAllowed,
        ),
        (
            "no route at all",
            aarch64_only(),
            fleet(),
            Constraint::Requirements,
        ),
        (
            "slow node",
            PlacementRequest::new(fall_detect()),
            fleet(),
            Constraint::PerfBudget,
        ),
        (
            "discovered node",
            aarch64_only(),
            {
                let mut p = pi5();
                p.tier = crate::placement::engine::TrustTier::Discovered;
                vec![p]
            },
            Constraint::Trust,
        ),
    ];
    let pins = ["x86", "gpu-box", "zero", "pi5"];
    for ((name, mut req, nodes, want), pin) in cases.into_iter().zip(pins) {
        req.pin = Some(pin.into());
        let err = place(&req, &nodes, &cluster()).unwrap_err();
        assert!(
            matches!(&err, PlacementError::PinIneligible { node, .. } if node == pin),
            "{name}: {err:?}"
        );
        assert!(err.constraints().contains(&want), "{name}: {err:?}");
        let msg = err.to_string();
        assert!(
            msg.contains(&format!("constraint `{}`", want.as_str())),
            "{name}: {msg}"
        );
    }
}

#[test]
fn pin_overrides_score_but_not_constraints() {
    let mut req = aarch64_only();
    req.pin = Some("coral".into());
    let d = run(&req, &fleet());
    let p = d.placement.expect("pinned");
    assert_eq!(p.node_id, "coral");
    assert!(p.flags.contains(&Flag::Pinned));
    // pi5 still ranks first in the report: the pin did not rewrite scores.
    assert_eq!(d.candidates[0].node_id, "pi5");
}

#[test]
fn pin_to_unknown_node_is_an_error() {
    let mut req = aarch64_only();
    req.pin = Some("nowhere".into());
    let err = place(&req, &fleet(), &cluster()).unwrap_err();
    assert_eq!(err, PlacementError::UnknownPin("nowhere".into()));
}

#[test]
fn emulation_only_with_the_flag() {
    let nodes = [x86(), gpu()];
    let d = run(&aarch64_only(), &nodes);
    assert!(d.is_unplaceable(), "never automatic");
    let why = &d
        .candidates
        .iter()
        .find(|c| c.node_id == "x86")
        .unwrap()
        .rejections;
    assert_eq!(why[0].constraint, Constraint::EmulationNotAllowed);
    assert!(
        why[0].detail.contains("aarch64-emulated"),
        "{}",
        why[0].detail
    );

    let mut req = aarch64_only();
    req.allow_emulated = true;
    let d = run(&req, &nodes);
    let p = d.placement.expect("emulated fallback with flag");
    assert_eq!(
        (p.node_id.as_str(), p.execution),
        ("x86", Execution::Emulated)
    );
    assert!(p.emulated() && p.flags.contains(&Flag::Emulated));
    assert_eq!(p.tier, Tier::Emulated);
}

#[test]
fn emulation_is_a_fallback_even_when_it_would_outscore_native() {
    let mut req = aarch64_only();
    req.allow_emulated = true;
    // Give the emulated x86 a locality preference big enough to top the list.
    req.spec.policy.preferences = vec![crate::placement::engine::Preference {
        name: "near-consumer".into(),
        requirement: crate::placement::requirement::Requirement::exact(
            crate::placement::CapabilityId::new("cpu.arch.x86_64").unwrap(),
        ),
        weight: 500.0,
    }];
    let d = run(&req, &[x86(), pi5()]);
    assert_eq!(d.candidates[0].node_id, "x86", "x86 ranks first by score");
    assert_eq!(placed_on(&d), Some("pi5"), "but a native route wins");
}

#[test]
fn affinity_prefers_and_avoids_among_eligible_nodes_only() {
    let mut req = PlacementRequest::new(fall_detect());
    req.affinity.prefer = vec!["seed".into()];
    let d = run(&req, &fleet());
    assert_eq!(placed_on(&d), Some("seed"));
    assert!(d.placement.unwrap().flags.contains(&Flag::Affinity));

    // Preferring an ineligible node changes nothing.
    req.affinity.prefer = vec!["zero".into()];
    assert_eq!(placed_on(&run(&req, &fleet())), Some("pi5"));

    req.affinity.prefer.clear();
    req.affinity.avoid = vec!["pi5".into()];
    assert_eq!(placed_on(&run(&req, &fleet())), Some("seed"));

    // Avoiding every eligible node still places (affinity is not a constraint).
    req.affinity.avoid = vec!["pi5".into(), "seed".into()];
    assert_eq!(placed_on(&run(&req, &fleet())), Some("pi5"));
}

#[test]
fn invalid_requests_are_refused_at_the_boundary() {
    let mut req = aarch64_only();
    req.weights.native = f64::NAN;
    assert!(matches!(
        place(&req, &fleet(), &cluster()),
        Err(PlacementError::Invalid(_))
    ));
    let mut req = aarch64_only();
    req.spec.kind = "Bad Kind".into();
    assert!(matches!(
        place(&req, &fleet(), &cluster()),
        Err(PlacementError::Invalid(_))
    ));
    let dup = [pi5(), pi5()];
    assert_eq!(
        place(&aarch64_only(), &dup, &cluster()).unwrap_err(),
        PlacementError::DuplicateNode("pi5".into())
    );
}
