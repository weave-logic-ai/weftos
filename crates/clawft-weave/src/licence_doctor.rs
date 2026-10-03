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
    if status.get("installed").and_then(Value::as_bool) == Some(false) {
        out.push(
            f(
                "runtime",
                Severity::Fail,
                format!(
                    "kernel.mesh.mesh_nonce is configured but the Seed licence runtime is not installed ({}), so checkout is off",
                    status["reason"].as_str().unwrap_or("unknown reason")
                ),
            )
            .remedy("see the daemon log at boot; the licence runtime needs the kernel chain and the node signing key"),
        );
        return out;
    }
    let binding = status.get("binding").filter(|b| !b.is_null());
    let mesh_id = status.get("mesh_id").and_then(Value::as_str);
    if status.get("poisoned").and_then(Value::as_bool) == Some(true) {
        out.push(
            f("store", Severity::Fail, "the licence store could not be read, so checkout is off")
                .remedy("inspect <runtime>/licence/ (the file is left untouched; the daemon log names the reason); restore it or move it aside"),
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

/// How close to its expiry a valid grant gets a warning (the steward renews
/// every 12 h, so a grant this close has missed at least one renewal).
pub const EXPIRY_HORIZON_SECS: u64 = 24 * 3600;

/// Turn a `workload.cog.checkout.status` result into findings (ADR-106
/// phase 3): a held grant with no approval, a grant expiring soon, orphaned
/// approvals, and no steward reachable.
pub fn checkout_findings(st: &Value) -> Vec<Finding> {
    let mut out = Vec::new();
    let now = st["now"].as_u64().unwrap_or(0);
    for g in st["grants"].as_array().into_iter().flatten() {
        let r = &g["grant"];
        if r["valid"].as_bool() != Some(true) {
            continue;
        }
        let name = format!("{}@{}", r["cog_id"].as_str().unwrap_or("?"), r["version"].as_str().unwrap_or("?"));
        let missing: Vec<&str> = g["run_gate"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|a| a["run_gate"]["verdict"] == "no_approval")
            .filter_map(|a| a["arch"].as_str())
            .collect();
        if !missing.is_empty() {
            out.push(
                f("approval_missing", Severity::Warn, format!(
                    "{name} has a valid checkout grant but no operator approval for {}: it cannot run on this node",
                    missing.join(", ")
                ))
                .remedy(format!("weaver cog checkout approve {name} --operator-key <file> (check the hashes first)")),
            );
        }
        let left = r["expires_at"].as_u64().unwrap_or(0).saturating_sub(now);
        if left < EXPIRY_HORIZON_SECS {
            out.push(
                f("grant_expiring", Severity::Warn, format!(
                    "the checkout grant for {name} expires in {} h; new starts stop when it lapses",
                    left / 3600
                ))
                .remedy("the steward renews grants every 12 h: check that it is up and can reach weft-licence (weaver cog checkout status)"),
            );
        }
    }
    let orphaned = st["approvals"].as_array().into_iter().flatten().filter(|a| a["active"] == false).count();
    if orphaned > 0 {
        out.push(
            f("approvals_orphaned", Severity::Warn, format!("{orphaned} operator approval(s) name an earlier mesh id (mesh_nonce changed)"))
                .remedy("weaver cog checkout approve --reapprove-orphaned --operator-key <file>"),
        );
    }
    if let Some(steward) = st["steward"].as_str() {
        let started = st["relay"]["started"].as_bool() == Some(true);
        match (st["is_steward"].as_bool() == Some(true), st["steward_reachable"].as_bool()) {
            (true, _) if !started => {}
            (true, Some(false)) => out.push(
                f("no_steward", Severity::Warn, format!(
                    "this node is the bound steward but runs no checkout relay{}",
                    st["relay"]["error"].as_str().map(|e| format!(" ({e})")).unwrap_or_default()
                ))
                .remedy("configure licence-link.json in the runtime dir (url, and a pin or allow_unpinned_lab_link) and restart the daemon"),
            ),
            (false, Some(false)) => out.push(
                f("no_steward", Severity::Warn, format!("the steward {} is not reachable as a licensed peer: new checkouts fail", short(steward)))
                    .remedy("check the steward node is up and admitted (weaver mesh status)"),
            ),
            _ => out.push(f("steward", Severity::Ok, format!("steward {} reachable", short(steward)))),
        }
    }
    out
}

/// Ask the daemon for its checkout status (`None`: no answer within 3 s).
pub async fn gather_checkout() -> Option<Value> {
    let ask = async {
        let mut client = crate::client::DaemonClient::connect().await?;
        let resp = client.simple_call("workload.cog.checkout.status").await.ok()?;
        if resp.ok { resp.result } else { None }
    };
    tokio::time::timeout(std::time::Duration::from_secs(3), ask).await.ok().flatten()
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
    fn a_configured_nonce_with_no_runtime_fails() {
        let st = json!({"installed": false, "nonce_configured": true, "reason": "no chain manager"});
        let out = findings(&st);
        assert_eq!(ids(&out), [("licence.runtime".into(), Severity::Fail)]);
        assert!(out[0].message.contains("no chain manager"));
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
    fn checkout_findings_name_a_missing_approval_an_expiry_and_an_unreachable_steward() {
        let st = json!({
            "now": 1000, "steward": "node-a", "is_steward": false, "steward_reachable": false,
            "relay": {"started": true},
            "grants": [{"grant": {"cog_id": "fall-detect", "version": "1.2.0", "valid": true, "expires_at": 1000 + 3600},
                        "run_gate": [{"arch": "aarch64", "run_gate": {"verdict": "no_approval"}}]},
                       {"grant": {"cog_id": "old", "version": "1", "valid": false, "expires_at": 0}, "run_gate": []}],
            "approvals": [{"active": false}, {"active": true}],
        });
        let out = checkout_findings(&st);
        let got = ids(&out);
        for id in ["licence.approval_missing", "licence.grant_expiring", "licence.approvals_orphaned", "licence.no_steward"] {
            assert!(got.contains(&(id.to_string(), Severity::Warn)), "{id} in {got:?}");
        }
        assert!(out[0].remedy.as_deref().unwrap().contains("checkout approve fall-detect@1.2.0"));
        // A lapsed grant is not reported as missing an approval.
        assert_eq!(got.len(), 4, "{got:?}");
    }

    #[test]
    fn a_steward_without_a_relay_is_reported_only_once_placement_started() {
        let base = |started: bool| json!({"now": 0, "steward": "me", "is_steward": true, "steward_reachable": false,
            "relay": {"started": started, "error": "no governance gate"}, "grants": [], "approvals": []});
        assert!(checkout_findings(&base(false)).is_empty());
        let out = checkout_findings(&base(true));
        assert_eq!(ids(&out), [("licence.no_steward".into(), Severity::Warn)]);
        assert!(out[0].message.contains("no governance gate"));
        let ok = json!({"now": 0, "steward": "node-a", "is_steward": false, "steward_reachable": true, "relay": {}, "grants": [], "approvals": []});
        assert_eq!(ids(&checkout_findings(&ok)), [("licence.steward".into(), Severity::Ok)]);
        assert!(checkout_findings(&json!({"grants": [], "approvals": []})).is_empty(), "no binding, nothing to say");
    }

    #[test]
    fn a_healthy_binding_passes_and_a_bad_config_or_store_fails() {
        let st = json!({"mesh_id": "b".repeat(64), "binding": {"state": "bound", "mesh_id": "b".repeat(64),
            "orphaned": false, "seq": 2, "device_id": "seed-1", "grant_fingerprint": "ed25519:0123456789abcdef"}});
        let out = findings(&st);
        assert_eq!(ids(&out), [("licence.binding".into(), Severity::Ok)]);
        let st = json!({"config_error": "kernel.mesh.mesh_nonce must be 64 hex characters", "binding": null});
        assert_eq!(ids(&findings(&st)), [("licence.mesh_nonce".into(), Severity::Fail)]);
        let st = json!({"poisoned": true, "mesh_id": null, "binding": null});
        assert_eq!(findings(&st)[0].id, "licence.store");
    }
}
