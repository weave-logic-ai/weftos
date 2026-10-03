//! Unit tests for `workload_rpc` (mock gate + recording audit sink).

use super::*;
use crate::rpc_gate::test_support::{FixedGate, Recorder};

const HASH: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn install_params(name: &str) -> Value {
    json!({ "name": name, "kind": "cog", "manifest_hash": HASH, "version": "0.3.1" })
}

#[test]
fn permitted_install_records_and_chains() {
    let reg = WorkloadRegistry::new();
    let gate = FixedGate::permit();
    let rec = Recorder::default();
    let resp = handle_install(&reg, install_params("anomaly-detect"), "n-1", Some(&gate), &rec.sink());
    assert!(resp.ok, "{:?}", resp.error);
    assert_eq!(gate.actions(), [WORKLOAD_INSTALL]);
    assert_eq!(rec.kinds(), [WORKLOAD_INSTALL]);
    let stored = reg.get("anomaly-detect").expect("recorded");
    assert_eq!(stored.node_id, "n-1");
    assert_eq!(stored.kind, "cog");
    // Gate context carries kind + effect so policy can see what it permits.
    let ctx = &gate.seen.lock().unwrap()[0].2;
    assert_eq!(ctx["kind"], "cog");
    assert!(ctx["effect"].is_object());
}

#[test]
fn denied_install_is_refused_chained_and_not_recorded() {
    let reg = WorkloadRegistry::new();
    let gate = FixedGate::deny("default-deny workload.*");
    let rec = Recorder::default();
    let resp = handle_install(&reg, install_params("w"), "n-1", Some(&gate), &rec.sink());
    assert!(!resp.ok);
    assert!(resp.error.unwrap().contains("default-deny"));
    assert!(reg.list().is_empty());
    assert_eq!(rec.kinds(), [WORKLOAD_REFUSE]);
    assert_eq!(rec.0.lock().unwrap()[0].1["action"], WORKLOAD_INSTALL);
}

#[test]
fn deferred_install_is_refused() {
    let reg = WorkloadRegistry::new();
    let rec = Recorder::default();
    let resp = handle_install(&reg, install_params("w"), "n", Some(&FixedGate::defer("review")), &rec.sink());
    assert!(!resp.ok);
    assert!(reg.list().is_empty());
    assert_eq!(rec.kinds(), [WORKLOAD_REFUSE]);
}

#[test]
fn install_without_gate_fails_closed() {
    let reg = WorkloadRegistry::new();
    let rec = Recorder::default();
    let resp = handle_install(&reg, install_params("w"), "n", None, &rec.sink());
    assert!(!resp.ok);
    assert!(reg.list().is_empty());
    assert_eq!(rec.kinds(), [WORKLOAD_REFUSE]);
}

#[test]
fn invalid_params_never_reach_the_gate() {
    let reg = WorkloadRegistry::new();
    let gate = FixedGate::permit();
    let rec = Recorder::default();
    for bad in [
        json!({}),
        json!({"name": "../x", "kind": "cog", "manifest_hash": HASH}),
        json!({"name": "ok", "kind": "cog", "manifest_hash": "sha256:zz"}),
        json!({"name": "ok", "kind": "cog", "manifest_hash": HASH, "token": "t"}),
    ] {
        assert!(!handle_install(&reg, bad, "n", Some(&gate), &rec.sink()).ok);
    }
    assert!(gate.actions().is_empty());
    assert!(rec.kinds().is_empty());
}

#[test]
fn duplicate_install_is_rejected_before_gate() {
    let reg = WorkloadRegistry::new();
    let gate = FixedGate::permit();
    let rec = Recorder::default();
    assert!(handle_install(&reg, install_params("w"), "n", Some(&gate), &rec.sink()).ok);
    let again = handle_install(&reg, install_params("w"), "n", Some(&gate), &rec.sink());
    assert!(!again.ok);
    assert_eq!(gate.actions().len(), 1);
}

#[test]
fn list_and_inspect_return_records() {
    let reg = WorkloadRegistry::new();
    let gate = FixedGate::permit();
    let rec = Recorder::default();
    for n in ["b", "a"] {
        assert!(handle_install(&reg, install_params(n), "n", Some(&gate), &rec.sink()).ok);
    }
    let list = handle_list(&reg).result.unwrap();
    let names: Vec<_> = list.as_array().unwrap().iter().map(|r| r["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["a", "b"]);
    assert_eq!(list[0]["state"], "installed");
    let one = handle_inspect(&reg, &json!({"name": "b"}));
    assert_eq!(one.result.unwrap()["manifest_hash"], HASH);
    assert!(!handle_inspect(&reg, &json!({"name": "zz"})).ok);
    assert!(!handle_inspect(&reg, &json!({})).ok);
}

#[test]
fn unload_is_gated_and_chained() {
    let reg = WorkloadRegistry::new();
    let rec = Recorder::default();
    assert!(handle_install(&reg, install_params("w"), "n", Some(&FixedGate::permit()), &rec.sink()).ok);

    let deny = FixedGate::deny("no");
    assert!(!handle_unload(&reg, &json!({"name": "w"}), Some(&deny), &rec.sink()).ok);
    assert!(reg.contains("w"), "denied unload must keep the record");

    let permit = FixedGate::permit();
    assert!(handle_unload(&reg, &json!({"name": "w"}), Some(&permit), &rec.sink()).ok);
    assert_eq!(permit.actions(), [WORKLOAD_UNLOAD]);
    assert!(!reg.contains("w"));
    assert_eq!(rec.kinds(), [WORKLOAD_INSTALL, WORKLOAD_REFUSE, WORKLOAD_UNLOAD]);
    assert!(!handle_unload(&reg, &json!({"name": "w"}), Some(&permit), &rec.sink()).ok);
}

#[test]
fn route_dispatches_and_reports_unbuilt_verbs() {
    let reg = WorkloadRegistry::new();
    let rec = Recorder::default();
    assert!(route("workload.list", json!(null), &reg, "n", None, &rec.sink()).ok);
    for m in NOT_YET {
        let r = route(m, json!({}), &reg, "n", None, &rec.sink());
        assert!(r.error.unwrap().contains("not available"), "{m}");
    }
    let unknown = route("workload.bogus", json!({}), &reg, "n", None, &rec.sink());
    assert!(unknown.error.unwrap().contains("denied by default"));
    let refused = rec.0.lock().unwrap().iter().any(|(k, p)| k == WORKLOAD_REFUSE && p["action"] == "workload.bogus");
    assert!(refused, "unknown workload.* refusal is chained");
    let other = route("bogus.method", json!({}), &reg, "n", None, &rec.sink());
    assert!(other.error.unwrap().starts_with("unknown method"));
}

/// Review round 3 (low): an anonymous local caller looping unknown
/// `workload.*` names must not grow the chain without bound, nor put its
/// text on the chain verbatim.
#[test]
fn unknown_method_refusals_are_budgeted_and_the_name_is_cut() {
    let rec = Recorder::default();
    let budget = RefusalBudget::new(4, std::time::Duration::from_secs(3600));
    let long = format!("workload.{}", "x".repeat(10_000));
    for _ in 0..50 {
        let r = deny_unknown(&rec.sink(), &long, &budget);
        assert!(!r.ok);
        assert!(r.error.unwrap().len() < 200, "the reply does not echo 10 KB");
    }
    let chained = rec.0.lock().unwrap().clone();
    assert_eq!(chained.len(), 4, "only the budget is chained");
    for (k, p) in &chained {
        assert_eq!(k, WORKLOAD_REFUSE);
        assert_eq!(p["action"].as_str().unwrap().chars().count(), MAX_SHOWN_METHOD);
        assert_eq!(p["method_bytes"], long.len());
    }
}

/// ADR-106 phase 1d: `workload.node.*` left `NOT_YET`; the plain router (no
/// placement build) says why it cannot serve them, and never denies them as unknown.
#[test]
fn licence_verbs_are_not_in_not_yet_and_need_placement_in_the_plain_router() {
    assert!(!NOT_YET.contains(&"workload.node.bind"));
    let reg = WorkloadRegistry::new();
    let rec = Recorder::default();
    for m in NEEDS_PLACEMENT {
        let r = route(m, json!({}), &reg, "n", None, &rec.sink());
        assert!(r.error.unwrap().contains("placement feature"), "{m}");
    }
    assert!(rec.0.lock().unwrap().is_empty(), "not an unknown-method refusal");
}
