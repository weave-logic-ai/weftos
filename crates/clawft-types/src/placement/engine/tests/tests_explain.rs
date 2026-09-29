//! `--explain` snapshots, kind independence, and the serde boundary.
//!
//! Snapshots live in `tests/snapshots/`. To regenerate after an intended
//! format change: `PLACEMENT_UPDATE_SNAPSHOTS=1` and re-run the tests, then
//! review the diff.

use std::path::PathBuf;

use super::fixtures::*;
use super::run;
use crate::placement::engine::{PlacementRequest, WorkloadSpec, explain};

fn snapshot(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src/placement/engine/tests/snapshots")
        .join(format!("{name}.txt"));
    if std::env::var_os("PLACEMENT_UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("missing snapshot {}: {e}", path.display()));
    assert_eq!(actual, want, "explain snapshot {name} changed");
}

#[test]
fn explain_snapshot_placed_interactive_cog() {
    let d = run(&PlacementRequest::new(fall_detect()), &fleet());
    snapshot("placed_fall_detect", &explain(&d));
}

#[test]
fn explain_snapshot_unplaceable_without_emulation() {
    let req = PlacementRequest::new(sensor_spec("cog", "anomaly-detect", &["aarch64"], None));
    let d = run(&req, &[x86(), gpu(), tsu()]);
    snapshot("unplaceable_no_emulation", &explain(&d));
}

#[test]
fn explain_snapshot_emulated_fallback() {
    let mut req = PlacementRequest::new(sensor_spec("cog", "anomaly-detect", &["aarch64"], None));
    req.allow_emulated = true;
    let d = run(&req, &[x86(), gpu()]);
    snapshot("emulated_fallback", &explain(&d));
}

#[test]
fn explain_snapshot_inference_with_accelerator() {
    let d = run(&PlacementRequest::new(gguf_server("coder", 20)), &fleet());
    snapshot("placed_gguf_server", &explain(&d));
}

#[test]
fn explain_names_the_decision_and_every_node() {
    let d = run(&PlacementRequest::new(fall_detect()), &fleet());
    let text = explain(&d);
    assert!(text.contains("decision: PLACED on pi5"), "{text}");
    for n in fleet() {
        assert!(text.contains(&n.id), "{} missing:\n{text}", n.id);
    }
    assert!(
        text.contains("perf_budget: measured perf.cog.cycle_ms 6000"),
        "{text}"
    );
}

/// The engine must not branch on `kind`: the same requirements under any
/// kind string give the same decision.
#[test]
fn decision_does_not_depend_on_kind() {
    for spec in [
        fall_detect(),
        gguf_server("coder", 20),
        coral_job(),
        tsu_job(),
    ] {
        let a = run(&PlacementRequest::new(spec.clone()), &fleet());
        let mut other = spec.clone();
        other.kind = "zz-unknown-kind".into();
        let mut b = run(&PlacementRequest::new(other), &fleet());
        b.kind = a.kind.clone();
        assert_eq!(a, b, "{}", spec.name);
    }
}

/// Source-level guard: engine code (tests excluded) names no workload kind.
#[test]
fn engine_source_has_no_kind_specific_code() {
    let files = [
        include_str!("../mod.rs"),
        include_str!("../constraints.rs"),
        include_str!("../score.rs"),
        include_str!("../request.rs"),
        include_str!("../decision.rs"),
        include_str!("../explain.rs"),
        include_str!("../spec.rs"),
        include_str!("../facts.rs"),
    ];
    let banned = [
        "\"cog\"",
        "\"inference\"",
        "\"accelerator-job\"",
        "kind ==",
        "kind.as_str()",
        "match kind",
        "kind.starts_with",
        "kind.contains",
        "kind.eq(",
        "fall-detect",
    ];
    for (i, src) in files.iter().enumerate() {
        for line in src.lines().filter(|l| !l.trim_start().starts_with("//")) {
            for b in banned {
                assert!(
                    !line.contains(b),
                    "engine file #{i} has kind-specific `{b}`: {line}"
                );
            }
        }
    }
}

#[test]
fn spec_round_trips_and_rejects_bad_input() {
    let spec = fall_detect();
    let json = serde_json::to_string(&spec).unwrap();
    let back: WorkloadSpec = serde_json::from_str(&json).unwrap();
    assert_eq!(back, spec);
    let req = PlacementRequest::new(spec);
    let back: PlacementRequest =
        serde_json::from_str(&serde_json::to_string(&req).unwrap()).unwrap();
    assert_eq!(back, req);

    for bad in [
        r#"{"kind":"","name":"x"}"#,
        r#"{"kind":"Cog","name":"x"}"#,
        r#"{"kind":"cog","name":""}"#,
        r#"{"kind":"cog","name":"x","requirements":{"common":[{"id":"BAD"}]}}"#,
        r#"{"kind":"cog","name":"x","policy":{"preferences":[{"name":"p","requirement":{"id":"a.b"},"weight":-1}]}}"#,
    ] {
        assert!(
            serde_json::from_str::<WorkloadSpec>(bad).is_err(),
            "accepted {bad}"
        );
    }
    let ok: WorkloadSpec = serde_json::from_str(r#"{"kind":"x-new-kind","name":"n"}"#).unwrap();
    assert_eq!(
        ok.policy.min_trust,
        crate::placement::engine::TrustTier::Paired
    );
}
