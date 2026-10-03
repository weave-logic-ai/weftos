//! `weaver doctor` findings for the Seed licence path (ADR-106): component
//! `runtime`, ids `licence.*`.
//!
//! [`findings`] is pure over the `workload.node.binding` reply; [`gather`]
//! asks the running daemon (a read-only call, 3 s). No daemon, no findings:
//! the daemon checks already say so.

use clawft_rpc::doctor::{Component, Finding, Severity};
use serde_json::Value;

fn f(id: &str, sev: Severity, msg: impl Into<String>) -> Finding {
    Finding::new(Component::Runtime, format!("licence.{id}"), sev, msg)
}

const SET_NONCE: &str = "weaver mesh nonce generate, then set kernel.mesh.mesh_nonce (and \
                         genesis_hash) on every node of the mesh and restart the daemon";

/// Turn a `workload.node.binding` result into findings.
pub fn findings(status: &Value) -> Vec<Finding> {
    let mut out = Vec::new();
    let binding = status.get("binding").filter(|b| !b.is_null());
    let mesh_id = status.get("mesh_id").and_then(Value::as_str);
    if let Some(why) = status.get("poisoned").and_then(Value::as_str) {
        out.push(
            f("store", Severity::Fail, format!("the licence store could not be read, so checkout is off: {why}"))
                .remedy("inspect <runtime>/licence/ (the file is left untouched); restore it or move it aside"),
        );
    }
    if let Some(why) = status.get("config_error").and_then(Value::as_str) {
        out.push(
            f("mesh_nonce", Severity::Fail, format!("{why}; the mesh id cannot be derived, so Seed checkout is off"))
                .remedy(SET_NONCE),
        );
        return out;
    }
    let state = binding.and_then(|b| b.get("state")).and_then(Value::as_str);
    match (mesh_id, binding) {
        (None, None) => out.push(f(
            "mesh_nonce",
            Severity::Ok,
            if status["genesis_pinned"].as_bool() == Some(true) {
                "no kernel.mesh.mesh_nonce is configured: the Seed licence path is inert (no mesh id, no binding)"
            } else {
                "no kernel.mesh.genesis_hash and mesh_nonce are configured: the Seed licence path is inert"
            },
        )),
        (None, Some(_)) => out.push(
            f(
                "mesh_id_unset",
                Severity::Fail,
                "a Seed binding exists but this node has no mesh id (kernel.mesh.genesis_hash or mesh_nonce is missing), so checkout is off",
            )
            .remedy(SET_NONCE),
        ),
        (Some(id), None) => out.push(f("mesh_id", Severity::Ok, format!("mesh id {}, no Seed bound", short(id)))),
        (Some(id), Some(b)) if b["orphaned"].as_bool() == Some(true) => out.push(
            f(
                "binding_orphaned",
                Severity::Fail,
                format!(
                    "the Seed binding is for mesh {} but this node now computes {} (the mesh_nonce or genesis pin changed), so checkout is off",
                    short(b["mesh_id"].as_str().unwrap_or("?")),
                    short(id)
                ),
            )
            .remedy("restore the old mesh_nonce, or: weaver workload node unbind, then weaver workload node bind <seed> for the new mesh id"),
        ),
        (Some(id), Some(b)) => {
            let what = if state == Some("unbound") { "unbound" } else { "bound" };
            out.push(f(
                "binding",
                Severity::Ok,
                format!(
                    "Seed {} {what} to mesh {} (seq {}, grant key {})",
                    b["device_id"].as_str().unwrap_or("?"),
                    short(id),
                    b["seq"].as_u64().unwrap_or(0),
                    b["grant_fingerprint"].as_str().unwrap_or("?"),
                ),
            ));
        }
    }
    out
}

fn short(id: &str) -> &str {
    id.get(..12).unwrap_or(id)
}

/// Ask the daemon for its binding status (`None`: no answer within 3 s).
pub async fn gather() -> Option<Value> {
    let ask = async {
        let mut client = crate::client::DaemonClient::connect().await?;
        let resp = client.simple_call("workload.node.binding").await.ok()?;
        if resp.ok { resp.result } else { None }
    };
    tokio::time::timeout(std::time::Duration::from_secs(3), ask).await.ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ids(v: &[Finding]) -> Vec<(String, Severity)> {
        v.iter().map(|x| (x.id.clone(), x.severity)).collect()
    }

    #[test]
    fn a_node_without_a_nonce_and_without_a_binding_is_inert_not_broken() {
        let st = json!({"mesh_id": null, "genesis_pinned": true, "nonce_set": false, "binding": null});
        let out = findings(&st);
        assert_eq!(ids(&out), [("licence.mesh_nonce".into(), Severity::Ok)]);
        assert!(out[0].message.contains("mesh_nonce"), "{}", out[0].message);
    }

    #[test]
    fn a_binding_without_a_mesh_id_fails() {
        let st = json!({"mesh_id": null, "binding": {"state": "bound", "mesh_id": "aa", "orphaned": false}});
        assert_eq!(ids(&findings(&st)), [("licence.mesh_id_unset".into(), Severity::Fail)]);
    }

    #[test]
    fn an_orphaned_binding_fails_with_the_unbind_remedy() {
        let st = json!({"mesh_id": "b".repeat(64), "binding":
            {"state": "bound", "mesh_id": "a".repeat(64), "orphaned": true, "seq": 3}});
        let out = findings(&st);
        assert_eq!(ids(&out), [("licence.binding_orphaned".into(), Severity::Fail)]);
        assert!(out[0].remedy.as_deref().unwrap().contains("unbind"));
    }

    #[test]
    fn a_healthy_binding_passes_and_a_bad_config_or_store_fails() {
        let st = json!({"mesh_id": "b".repeat(64), "binding": {"state": "bound", "mesh_id": "b".repeat(64),
            "orphaned": false, "seq": 2, "device_id": "seed-1", "grant_fingerprint": "ed25519:0123456789abcdef"}});
        let out = findings(&st);
        assert_eq!(ids(&out), [("licence.binding".into(), Severity::Ok)]);
        let st = json!({"config_error": "kernel.mesh.mesh_nonce must be 64 hex characters", "binding": null});
        assert_eq!(ids(&findings(&st)), [("licence.mesh_nonce".into(), Severity::Fail)]);
        let st = json!({"poisoned": "bad file", "mesh_id": null, "binding": null});
        assert_eq!(findings(&st)[0].id, "licence.store");
    }
}
