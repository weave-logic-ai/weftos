//! Inputs that must not game the engine: a workload's own instance cannot
//! exclude it by co-residency, and a non-finite or negative measured
//! performance value never earns credit.

use super::fixtures::*;
use super::{cluster, placed_on, rejected_by};
use crate::placement::capability::{AttrValue, Capability, CapabilityId, Provenance};
use crate::placement::engine::{
    ClusterState, Constraint, Decision, Flag, InstanceRecord, PlacementRequest, WorkloadRef, place,
};
use crate::placement::perf;

/// A model server that refuses to share a node with another model server.
fn exclusive_server() -> PlacementRequest {
    let mut r = PlacementRequest::new(sensor_spec("cog", "anomaly-detect", &["aarch64"], None));
    r.spec.policy.labels = vec!["role:model-server".into()];
    r.spec.policy.excludes = vec!["role:model-server".into()];
    r
}

fn resident(workload: Option<WorkloadRef>) -> InstanceRecord {
    InstanceRecord {
        node_id: "pi5".into(),
        workload,
        labels: vec!["role:model-server".into()],
        excludes: vec!["role:model-server".into()],
        ..Default::default()
    }
}

fn place_with(req: &PlacementRequest, instances: Vec<InstanceRecord>) -> Decision {
    let c = ClusterState {
        instances,
        ..cluster()
    };
    place(req, &[pi5()], &c).expect("valid request")
}

#[test]
fn own_instance_does_not_exclude_its_re_placement() {
    let req = exclusive_server();
    let own = WorkloadRef::of(&req.spec);
    let d = place_with(&req, vec![resident(Some(own))]);
    assert!(rejected_by(&d, "pi5").is_empty(), "{d:#?}");
    assert_eq!(placed_on(&d), Some("pi5"));
}

#[test]
fn another_workload_with_the_same_labels_still_excludes() {
    let req = exclusive_server();
    let cases = [
        (
            "different name",
            Some(WorkloadRef {
                kind: "cog".into(),
                name: "other-server".into(),
            }),
        ),
        (
            "different kind, same name",
            Some(WorkloadRef {
                kind: "inference".into(),
                name: "anomaly-detect".into(),
            }),
        ),
        ("unattributed instance", None),
    ];
    for (case, workload) in cases {
        let d = place_with(&req, vec![resident(workload)]);
        assert_eq!(
            rejected_by(&d, "pi5"),
            vec![Constraint::CoResidency, Constraint::CoResidency],
            "{case}: both directions of the exclusion apply"
        );
        assert!(d.is_unplaceable(), "{case}");
    }
    // Its own instance is skipped, a real neighbour on the same node is not.
    let d = place_with(
        &req,
        vec![
            resident(Some(WorkloadRef::of(&req.spec))),
            resident(Some(WorkloadRef {
                kind: "cog".into(),
                name: "other-server".into(),
            })),
        ],
    );
    assert!(rejected_by(&d, "pi5").contains(&Constraint::CoResidency));
}

#[test]
fn workload_ref_round_trips_and_is_optional_on_the_wire() {
    let rec = resident(Some(WorkloadRef {
        kind: "cog".into(),
        name: "x".into(),
    }));
    let json = serde_json::to_string(&rec).unwrap();
    assert_eq!(serde_json::from_str::<InstanceRecord>(&json).unwrap(), rec);
    let legacy: InstanceRecord =
        serde_json::from_str(r#"{"node_id":"pi5","labels":["a"]}"#).unwrap();
    assert_eq!(legacy.workload, None);
}

/// A `measured` fall-detect cycle record with a raw value, bypassing the
/// validating `perf` constructor (as a hand-built or foreign fact would).
fn raw_cycle(v: f64) -> Capability {
    Capability::new(
        CapabilityId::new(perf::PERF_COG_CYCLE_MS).unwrap(),
        Provenance::Measured,
    )
    .with_attr("cog_id", "fall-detect")
    .with_attr(perf::VALUE_ATTR, AttrValue::Float(v))
}

fn pi5_measuring(v: f64) -> TestNode {
    let mut n = pi5();
    n.caps.retain(|c| c.id.as_str() != perf::PERF_COG_CYCLE_MS);
    n.caps.push(raw_cycle(v));
    n
}

#[test]
fn invalid_measured_perf_gets_no_credit() {
    for budget in [None, Some(2000.0)] {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -5.0] {
            let mut spec = fall_detect();
            spec.policy.perf.as_mut().unwrap().budget = budget;
            let req = PlacementRequest::new(spec);
            let d = place(&req, &[pi5_measuring(bad), seed()], &cluster()).unwrap();
            let case = format!("value {bad}, budget {budget:?}");
            let pi = d.candidates.iter().find(|c| c.node_id == "pi5").unwrap();
            assert!(pi.eligible(), "{case}: treated as unmeasured, not filtered");
            assert_eq!(pi.score.as_ref().unwrap().perf, 0.0, "{case}");
            assert!(pi.flags.contains(&Flag::Unmeasured), "{case}");
            let sd = d.candidates.iter().find(|c| c.node_id == "seed").unwrap();
            // The seed's valid measurement is the best: full interactive weight.
            assert_eq!(sd.score.as_ref().unwrap().perf, 60.0, "{case}");
            assert_eq!(placed_on(&d), Some("seed"), "{case}");
        }
    }
}

#[test]
fn a_valid_value_beside_an_invalid_one_is_still_used() {
    let mut n = pi5_measuring(f64::NAN);
    n.caps.push(raw_cycle(900.0));
    let req = PlacementRequest::new(fall_detect());
    let d = place(&req, &[n, seed()], &cluster()).unwrap();
    let pi = d.candidates.iter().find(|c| c.node_id == "pi5").unwrap();
    assert_eq!(pi.score.as_ref().unwrap().perf, 60.0);
    assert!(!pi.flags.contains(&Flag::Unmeasured));
    assert_eq!(placed_on(&d), Some("pi5"));
}
