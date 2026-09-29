//! Phase A: each hard constraint, table-driven, plus the synthetic fleet
//! matrix (which nodes can take which workload).

use super::fixtures::*;
use super::{cluster, placed_on, rejected_by, run};
use crate::placement::capability::CapabilityState;
use crate::placement::engine::{
    ClusterState, Constraint, GateInput, GateVerdict, InstanceRecord, Liveness, PlacementRequest,
    TrustTier, WorkloadSpec, place,
};
use crate::placement::memory::MemoryDemand;

/// Which fleet nodes are eligible for each workload, and why the rest fail.
#[test]
fn fleet_matrix_eligibility() {
    struct Case {
        spec: WorkloadSpec,
        allow_emulated: bool,
        eligible: &'static [&'static str],
        placed: Option<&'static str>,
    }
    let cases = [
        Case {
            spec: fall_detect(),
            allow_emulated: false,
            eligible: &["pi5", "seed"],
            placed: Some("pi5"),
        },
        Case {
            spec: sensor_spec("cog", "anomaly-detect", &["aarch64"], None),
            allow_emulated: false,
            eligible: &["pi5", "coral", "mac-dev"],
            placed: Some("pi5"),
        },
        Case {
            spec: sensor_spec("cog", "anomaly-detect", &["aarch64"], None),
            allow_emulated: true,
            eligible: &["pi5", "coral", "mac-dev", "x86"],
            placed: Some("pi5"),
        },
        Case {
            spec: gguf_server("coder", 20),
            allow_emulated: false,
            eligible: &["gpu-box", "mac-dev"],
            placed: Some("gpu-box"),
        },
        Case {
            spec: coral_job(),
            allow_emulated: false,
            eligible: &["coral"],
            placed: Some("coral"),
        },
        Case {
            spec: tsu_job(),
            allow_emulated: false,
            eligible: &[],
            placed: None,
        },
    ];
    for c in cases {
        let mut req = PlacementRequest::new(c.spec.clone());
        req.allow_emulated = c.allow_emulated;
        let d = run(&req, &fleet());
        let mut got: Vec<&str> = d
            .candidates
            .iter()
            .filter(|n| n.eligible())
            .map(|n| n.node_id.as_str())
            .collect();
        let mut want = c.eligible.to_vec();
        got.sort_unstable();
        want.sort_unstable();
        assert_eq!(got, want, "{} (emu={})", c.spec.name, c.allow_emulated);
        assert_eq!(placed_on(&d), c.placed, "{}", c.spec.name);
        assert_eq!(d.candidates.len(), fleet().len(), "every node reported");
    }
}

/// One base node that passes everything, and one mutation per constraint.
#[test]
fn each_hard_constraint_rejects_with_its_name() {
    type Mutate = fn(&mut PlacementRequest, &mut TestNode, &mut ClusterState);
    let cases: Vec<(&str, Mutate, Constraint)> = vec![
        (
            "dead node",
            |_, n, _| n.liveness = Liveness::Dead,
            Constraint::Liveness,
        ),
        (
            "suspect node",
            |_, n, _| n.liveness = Liveness::Suspect,
            Constraint::Liveness,
        ),
        (
            "expired facts",
            |_, n, _| n.expires = Some(500),
            Constraint::FactsExpired,
        ),
        (
            "discovered tier",
            |_, n, _| n.tier = TrustTier::Discovered,
            Constraint::Trust,
        ),
        (
            "pinned tier required",
            |r, _, _| r.spec.policy.min_trust = TrustTier::Pinned,
            Constraint::Trust,
        ),
        (
            "node revoked",
            |_, _, c| {
                c.revoked_nodes.insert("pi5".into());
            },
            Constraint::NodeRevoked,
        ),
        (
            "package revoked",
            |r, _, c| {
                r.spec.policy.revocable_refs = vec!["pkg:anomaly@1".into()];
                c.revoked_refs.insert("pkg:anomaly@1".into());
            },
            Constraint::WorkloadRevoked,
        ),
        (
            "we exclude a resident",
            |r, _, c| {
                r.spec.policy.excludes = vec!["role:planner".into()];
                c.instances.push(InstanceRecord {
                    node_id: "pi5".into(),
                    labels: vec!["role:planner".into()],
                    ..Default::default()
                });
            },
            Constraint::CoResidency,
        ),
        (
            "a resident excludes us",
            |r, _, c| {
                r.spec.policy.labels = vec!["role:coder".into()];
                c.instances.push(InstanceRecord {
                    node_id: "pi5".into(),
                    excludes: vec!["role:coder".into()],
                    ..Default::default()
                });
            },
            Constraint::CoResidency,
        ),
        (
            "memory short",
            |r, _, _| r.spec.requirements.memory.host_bytes = 7 * GIB as u64,
            Constraint::Memory,
        ),
        (
            "pending reservation fills memory",
            |_, _, c| {
                c.instances.push(InstanceRecord {
                    node_id: "pi5".into(),
                    pending: MemoryDemand {
                        host_bytes: 6 * GIB as u64,
                        accel_bytes: 0,
                    },
                    ..Default::default()
                });
            },
            Constraint::Memory,
        ),
        (
            "missing capability",
            |_, n, _| n.caps.retain(|c| c.id.as_str() != "runtime.native"),
            Constraint::Requirements,
        ),
        (
            "provenance floor",
            |r, _, _| r.spec.policy.min_provenance = Some(crate::placement::Provenance::Measured),
            Constraint::Provenance,
        ),
        (
            "gate denies",
            |r, _, _| {
                r.gate = GateInput::Uniform {
                    verdict: GateVerdict::Deny {
                        reason: "default deny".into(),
                    },
                }
            },
            Constraint::Gate,
        ),
        (
            "gate has no verdict for node",
            |r, _, _| {
                r.gate = GateInput::PerNode {
                    verdicts: Default::default(),
                }
            },
            Constraint::Gate,
        ),
    ];
    for (name, mutate, want) in cases {
        let mut req =
            PlacementRequest::new(sensor_spec("cog", "anomaly-detect", &["aarch64"], None));
        let mut node = pi5();
        let mut state = cluster();
        // Base case passes.
        assert!(
            place(&req, std::slice::from_ref(&node), &state)
                .unwrap()
                .placement
                .is_some()
        );
        mutate(&mut req, &mut node, &mut state);
        let d = place(&req, std::slice::from_ref(&node), &state).unwrap();
        assert!(d.is_unplaceable(), "{name}: should be unplaceable");
        let got: std::collections::BTreeSet<Constraint> =
            rejected_by(&d, "pi5").into_iter().collect();
        assert!(got.contains(&want), "{name}: {got:?}");
        // Only the provenance floor also leaves variants with a missing id.
        let extra: Vec<_> = got
            .iter()
            .filter(|c| **c != want && **c != Constraint::Requirements)
            .collect();
        assert!(extra.is_empty(), "{name}: unexpected {extra:?}");
        if want != Constraint::Requirements && want != Constraint::Provenance {
            assert_eq!(got.len(), 1, "{name}: {got:?}");
        }
    }
}

#[test]
fn all_failing_constraints_are_recorded_not_only_the_first() {
    let req = PlacementRequest::new(sensor_spec("cog", "anomaly-detect", &["aarch64"], None));
    let mut n = x86();
    n.liveness = Liveness::Dead;
    n.tier = TrustTier::Discovered;
    let d = run(&req, &[n]);
    let got = rejected_by(&d, "x86");
    assert!(got.contains(&Constraint::Liveness), "{got:?}");
    assert!(got.contains(&Constraint::Trust), "{got:?}");
    assert!(got.contains(&Constraint::EmulationNotAllowed), "{got:?}");
}

#[test]
fn unified_memory_is_one_pool_and_discrete_vram_is_separate() {
    // 80 GiB model: fits the Mac's 90 GiB unified pool, not the 24 GiB card.
    let d = run(&PlacementRequest::new(gguf_server("big", 80)), &fleet());
    assert_eq!(placed_on(&d), Some("mac-dev"));
    let why = &d
        .candidates
        .iter()
        .find(|c| c.node_id == "gpu-box")
        .unwrap()
        .rejections;
    assert_eq!(why[0].constraint, Constraint::Memory);
    assert!(why[0].detail.starts_with("vram pool"), "{}", why[0].detail);
    // 95 GiB (host 1 + accel 94) overflows the unified pool as one sum.
    let d = run(&PlacementRequest::new(gguf_server("huge", 94)), &[mac()]);
    assert!(
        d.candidates[0].rejections[0]
            .detail
            .starts_with("unified pool")
    );
}

#[test]
fn busy_exclusive_accelerator_is_never_stolen() {
    let mut n = coral();
    for c in &mut n.caps {
        if c.id.as_str() == "accel.tpu.coral" {
            c.state = CapabilityState::Busy;
        }
    }
    let d = run(&PlacementRequest::new(coral_job()), &[n]);
    assert!(d.is_unplaceable());
    let r = &d.candidates[0].rejections[0];
    assert_eq!(r.constraint, Constraint::Requirements);
    assert!(r.detail.contains("unavailable (busy)"), "{}", r.detail);
}

#[test]
fn claimed_tsu_fails_provenance_until_measured() {
    let d = run(&PlacementRequest::new(tsu_job()), &[tsu()]);
    assert_eq!(rejected_by(&d, "tsu"), vec![Constraint::Provenance]);
    let mut measured = tsu();
    for c in &mut measured.caps {
        if c.id.as_str() == "accel.tsu.acme" {
            c.provenance = crate::placement::Provenance::Measured;
        }
    }
    let d = run(&PlacementRequest::new(tsu_job()), &[measured]);
    let p = d.placement.expect("measured TSU places");
    assert_eq!(p.accelerator.as_deref(), Some("accel.tsu.acme"));
}

#[test]
fn unplaceable_reports_every_node_with_reasons() {
    let d = run(&PlacementRequest::new(tsu_job()), &fleet());
    assert!(d.is_unplaceable());
    assert_eq!(d.reasons().count(), fleet().len());
    assert!(d.reasons().all(|r| !r.rejections.is_empty()));
}
