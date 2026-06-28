//! M1.5 integration test 2: binding-expression evaluation against a
//! synthetic ontology snapshot.

use clawft_surface::eval::eval_binding;
use clawft_surface::parse::parse_surface_toml;
use clawft_surface::substrate::OntologySnapshot;
use clawft_surface::tree::Binding;
use serde_json::json;

const FIXTURE: &str = include_str!("../fixtures/weftos-admin-desktop.toml");

fn healthy_kernel_snapshot() -> OntologySnapshot {
    let mut s = OntologySnapshot::empty();
    s.put(
        "substrate/kernel/status",
        json!({"state": "healthy", "uptime_ms": 3_600_000u64}),
    );
    s.put(
        "substrate/kernel/services",
        json!([
            {"name": "mesh-listener", "status": "healthy", "cpu_percent": 42.0},
            {"name": "rpc-gateway",   "status": "healthy", "cpu_percent": 9.0},
            {"name": "audit-sink",    "status": "at_risk", "cpu_percent": 88.0},
        ]),
    );
    s.put(
        "substrate/kernel/processes",
        json!([
            {"pid": 101, "name": "weaver", "cpu": 4.2},
            {"pid": 222, "name": "claude", "cpu": 21.7},
        ]),
    );
    s.put(
        "substrate/kernel/logs",
        json!(["[t+0s] boot ok", "[t+1s] 3 services ready"]),
    );
    s
}

#[test]
fn count_filter_healthy_services() {
    let tree = parse_surface_toml(FIXTURE).expect("parse");
    let snap = healthy_kernel_snapshot();

    let overview = &tree.root.children[0];
    let services_chip = &overview.children[1];
    let b = services_chip.bindings.get("label").expect("label binding");
    let v = eval_binding(b, &snap).expect("eval");
    // Two services are healthy in the snapshot above.
    assert_eq!(v.as_i64(), Some(2));
}

#[test]
fn status_field_access_returns_healthy() {
    let tree = parse_surface_toml(FIXTURE).expect("parse");
    let snap = healthy_kernel_snapshot();
    let overview = &tree.root.children[0];
    let status_chip = &overview.children[0];
    let b = status_chip.bindings.get("label").expect("label binding");
    let v = eval_binding(b, &snap).expect("eval");
    assert_eq!(v.to_display_string(), "healthy");
}

#[test]
fn missing_topic_returns_null_without_panic() {
    let tree = parse_surface_toml(FIXTURE).expect("parse");
    let empty = OntologySnapshot::empty();
    let overview = &tree.root.children[0];
    let status_chip = &overview.children[0];
    let b = status_chip.bindings.get("label").expect("label binding");
    let v = eval_binding(b, &empty).expect("eval");
    assert_eq!(v.to_display_string(), "");
}

#[test]
fn literal_binding_short_circuits() {
    use clawft_surface::tree::AttrValue;
    let b = Binding::Literal(AttrValue::Int(7));
    let snap = OntologySnapshot::empty();
    let v = eval_binding(&b, &snap).unwrap();
    assert_eq!(v.as_i64(), Some(7));
}

/// Finding 6: a `count()` applied to a scalar ontology topic (instead
/// of a list) must surface a clean `TypeMismatch`, not panic and not
/// silently return 0. Guards the wrong-shape-binding boundary.
#[test]
fn count_on_scalar_topic_errors_with_type_mismatch() {
    use clawft_surface::eval::{EvalError, eval};
    use clawft_surface::parse::expr::parse;

    let mut snap = OntologySnapshot::empty();
    // A scalar topic — shaped wrong for list combinators.
    snap.put("substrate/scalar_metric", json!(42));

    let e = parse("count($substrate/scalar_metric, s -> true)").expect("parse");
    let err = eval(&e, &snap, None).expect_err("count on scalar must error");
    match err {
        EvalError::TypeMismatch(msg) => {
            assert!(
                msg.contains("count"),
                "TypeMismatch message should mention count(), got: {msg}"
            );
        }
        other => panic!("expected TypeMismatch, got {other:?}"),
    }
}
