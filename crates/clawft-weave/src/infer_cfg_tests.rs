//! Config rules for the managed-role fields, and the start/stop verbs'
//! classification. Fakes only.

use crate::infer_wire::*;
use crate::infer_wire_tests::{fake_on, parts, write_cfg};

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
