//! Daemon-wide sources for `fleet.snapshot`: the placement controller view,
//! the licence binding summary and inference placement status. Each degrades
//! to `None` in builds without placement.

use serde_json::{Value, json};

use super::field;
use super::provenance::SIGNED_FACT;

#[cfg(all(feature = "placement", unix))]
pub(super) fn controller_view() -> Option<Value> {
    crate::workload_place_rpc::controller_view()
}
#[cfg(not(all(feature = "placement", unix)))]
pub(super) fn controller_view() -> Option<Value> {
    None
}

#[cfg(all(feature = "placement", unix))]
pub(super) fn licence_status() -> Option<Value> {
    crate::licence_boot::runtime().map(|rt| licence_summary(&crate::licence_boot::status(&rt)))
}

/// The binding facts a fleet view needs, from `workload.node.binding`: no
/// device id, no grant key, no steward detail, and not the signed record.
/// The `binding` comes from an operator-signed record, so it is a `signed_fact`.
pub(crate) fn licence_summary(status: &Value) -> Value {
    let binding = match status["binding"].as_object() {
        Some(b) => field(
            json!({
                "state": b.get("state"),
                "mesh_id": b.get("mesh_id"),
                "seq": b.get("seq"),
                "grant_fingerprint": b.get("grant_fingerprint"),
                "orphaned": b.get("orphaned"),
            }),
            SIGNED_FACT,
        ),
        None => field(Value::Null, SIGNED_FACT),
    };
    json!({ "mesh_id": status["mesh_id"], "genesis_pinned": status["genesis_pinned"], "binding": binding })
}
#[cfg(not(all(feature = "placement", unix)))]
pub(super) fn licence_status() -> Option<Value> {
    None
}

#[cfg(all(feature = "placement", unix))]
pub(super) async fn infer_status() -> (Option<Value>, String) {
    let r = crate::infer_rpc::handle("infer.status", Value::Null).await;
    match (r.ok, r.result) {
        (true, Some(v)) => (Some(v), String::new()),
        _ => (None, "inference placement is off".into()),
    }
}
#[cfg(not(all(feature = "placement", unix)))]
pub(super) async fn infer_status() -> (Option<Value>, String) {
    (None, "built without placement".into())
}
