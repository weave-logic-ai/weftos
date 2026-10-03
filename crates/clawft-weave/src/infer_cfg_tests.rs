//! Config rules for the managed-role fields, and the start/stop verbs'
//! classification. Fakes only.

use clawft_kernel::workload_runtime::infer::{RosterOverlay, import_roster};

use crate::infer_wire::*;
use crate::infer_wire_tests::{fake_on, parts, write_cfg};

const ROSTER: &str = include_str!("../../clawft-kernel/src/workload_runtime/infer/fixtures/model-lab-roster.yaml");

#[test]
fn the_new_config_fields_are_validated() {
    let dir = tempfile::tempdir().unwrap();
    let bad = |v: serde_json::Value| {
        write_cfg(dir.path(), v);
        crate::infer_cfg::load_config(dir.path()).unwrap_err()
    };
    let base = || serde_json::json!({"role": "a", "flavor": "llamacpp", "instance_port": 9});
    let with = |k: &str, v: serde_json::Value| {
        let mut r = base();
        r[k] = v;
        serde_json::json!({"roles": [r]})
    };
    bad(with("mode", "weird".into()));
    bad(with("memory_gb", (-1).into()));
    bad(with("memory_gb", 99999.into()));
    bad(with("excludes", serde_json::json!(["bad name!"])));
    bad(with("roster_id", "x".into())); // no roster
    bad(serde_json::json!({"roles": [{"role": "a", "instance_port": 9}]})); // no flavor, no roster_id
    bad(serde_json::json!({"roles": [base()], "budget_gb": 0}));
    bad(serde_json::json!({"roles": [base()], "serve_programs": {"llamacpp": "relative/serve"}}));
    bad(serde_json::json!({"roles": [base()], "serve_programs": {"ollama": "/bin/true"}}));
    bad(serde_json::json!({"roles": [base()], "serve_programs": {"vllm": "/bin/true"}}));
    write_cfg(
        dir.path(),
        serde_json::json!({"roles": [base()], "serve_programs": {"llamacpp": "/opt/x/serve-llamacpp"}, "budget_gb": 96}),
    );
    assert!(crate::infer_cfg::load_config(dir.path()).is_ok());
    // Resolution rules the file check cannot see: managed needs a model, and
    // memory when a budget is set; the proxy port may not be the server's.
    let cfg = |roles: serde_json::Value, budget: Option<f64>| -> crate::infer_cfg::FileCfg {
        let mut v = serde_json::json!({"roles": roles});
        if let Some(b) = budget {
            v["budget_gb"] = b.into();
        }
        serde_json::from_value(v).unwrap()
    };
    let e = |c| crate::infer_cfg::resolve_roles(&c, None).unwrap_err();
    assert!(e(cfg(serde_json::json!([{"role":"a","mode":"managed","flavor":"llamacpp","instance_port":9}]), None)).contains("needs a model"));
    assert!(e(cfg(serde_json::json!([{"role":"a","mode":"managed","flavor":"llamacpp","model":"M","instance_port":9}]), Some(10.0))).contains("needs its memory"));
    assert!(e(cfg(serde_json::json!([{"role":"a","flavor":"llamacpp"}]), None)).contains("no instance_port"));
}

#[tokio::test]
async fn the_start_and_stop_verbs_and_their_classification() {
    use crate::capability::CallerCapabilities;
    let admin = CallerCapabilities::from_scopes(["admin"]);
    let write = CallerCapabilities::from_scopes(["write"]);
    for m in ["infer.start", "infer.stop"] {
        assert!(admin.allows_method(m), "{m}");
        assert!(!write.allows_method(m), "{m}");
        assert!(!CallerCapabilities::anonymous().allows_method(m), "{m}");
    }
    // An adopted role cannot be started or stopped by WeftOS.
    let up = fake_on(0).await;
    let dir = tempfile::tempdir().unwrap();
    write_cfg(dir.path(), serde_json::json!({"roles": [crate::infer_wire_tests::role(up.addr.port(), None)]}));
    let (st, _) = build(parts(dir.path(), None, false, None)).await.unwrap().unwrap();
    assert!(st.start_role("hermes").await.unwrap_err().contains("never starts it"));
    assert!(st.stop_role("hermes").await.unwrap_err().contains("never stops"));
    assert!(st.start_role("nope").await.unwrap_err().contains("unknown role"));
    assert_eq!(up.seen.lock().unwrap().iter().filter(|l| !l.starts_with("GET")).count(), 0, "an adopted server only ever receives reads");
}

#[test]
fn ports_are_unique_across_all_roles() {
    let dir = tempfile::tempdir().unwrap();
    let bad = |roles: serde_json::Value| {
        write_cfg(dir.path(), serde_json::json!({"roles": roles}));
        crate::infer_cfg::load_config(dir.path()).unwrap_err()
    };
    let r = |name: &str, flavor: &str, inst: u16, proxy: Option<u16>| {
        let mut v = serde_json::json!({"role": name, "flavor": flavor, "instance_port": inst});
        if let Some(p) = proxy {
            v["proxy_port"] = p.into();
        }
        v
    };
    // Two roles on one proxy port.
    assert!(bad(serde_json::json!([r("a", "llamacpp", 1, Some(10)), r("b", "llamacpp", 2, Some(10))])).contains("more than one"));
    // An exposed port that is another role's proxy port, or another's exposed port.
    let mut a = r("a", "llamacpp", 1, Some(10));
    a["expose"] = serde_json::json!({"listen": "0.0.0.0", "port": 11, "token_file": "t"});
    let mut b = r("b", "llamacpp", 2, Some(12));
    b["expose"] = serde_json::json!({"listen": "0.0.0.0", "port": 10, "token_file": "t"});
    assert!(bad(serde_json::json!([a.clone(), b])).contains("more than one"));
    let mut c = r("c", "llamacpp", 3, Some(13));
    c["expose"] = serde_json::json!({"listen": "0.0.0.0", "port": 11, "token_file": "t"});
    assert!(bad(serde_json::json!([a.clone(), c])).contains("more than one"));
    // A server on a listener's port.
    assert!(bad(serde_json::json!([a.clone(), r("d", "llamacpp", 11, None)])).contains("both a server port"));
    // Two non-Ollama roles on one server port; Ollama roles may share.
    assert!(bad(serde_json::json!([r("a", "llamacpp", 5, None), r("b", "mlx-lm", 5, None)])).contains("instance_port 5"));
    assert!(bad(serde_json::json!([r("a", "ollama", 5, None), r("b", "llamacpp", 5, None)])).contains("instance_port 5"));
    write_cfg(dir.path(), serde_json::json!({"roles": [r("a", "ollama", 11434, None), r("b", "ollama", 11434, None)]}));
    assert!(crate::infer_cfg::load_config(dir.path()).is_ok(), "one Ollama serves several models");
}

#[test]
fn roster_supplied_ports_are_checked_too() {
    let roster = import_roster(ROSTER, &RosterOverlay::default()).unwrap();
    let cfg = |roles: serde_json::Value| -> crate::infer_cfg::FileCfg {
        let mut v = serde_json::json!({"roles": roles});
        v["roster"] = serde_json::json!({"file": "unused"});
        serde_json::from_value(v).unwrap()
    };
    let e = |roles| crate::infer_cfg::resolve_roles(&cfg(roles), Some(&roster)).unwrap_err();
    // coder-daily is on 8081 in the roster: another server there is refused,
    // and so is another role's proxy listener.
    assert!(e(serde_json::json!([
        {"role": "a", "roster_id": "coder-daily"},
        {"role": "b", "flavor": "llamacpp", "instance_port": 8081}
    ])).contains("8081"));
    assert!(e(serde_json::json!([
        {"role": "a", "roster_id": "coder-daily"},
        {"role": "b", "flavor": "llamacpp", "instance_port": 9, "proxy_port": 8081}
    ])).contains("listener"));
    // Distinct ports resolve.
    assert!(crate::infer_cfg::resolve_roles(
        &cfg(serde_json::json!([{"role": "a", "roster_id": "coder-daily"}, {"role": "b", "roster_id": "planner"}])),
        Some(&roster)
    )
    .is_ok());
}
