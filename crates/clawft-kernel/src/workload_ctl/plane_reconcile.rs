//! Reconciling a `place` whose answer was lost (ADR-099 section 7).
//!
//! A signed `place` that was sent but got no valid answer (timeout,
//! dropped connection) may have been carried out. Before any other
//! candidate is tried, the controller asks the target for its instances
//! and in-flight decisions:
//!
//! - the decision is still in flight: unsettled, stop;
//! - it produced instances: unload each (signed, carrying the decision
//!   id), then settled;
//! - neither: settled (the target fetches the payload over the call's own
//!   connection, which is closed by now, so it cannot load it later).
//!
//! An unreachable target is unsettled: placement stops rather than risk a
//! second, unmanaged instance.

use serde_json::{Value, json};

use super::msg::method;
use super::plane::PlacementControlPlane;

impl PlacementControlPlane {
    /// `(settled, note)` for `decision_id` on `node`.
    pub(super) async fn reconcile(&self, node: &str, decision_id: &str) -> (bool, String) {
        let rows = match self.call(node, method::STATUS, None, json!({})).await {
            Ok(Value::Array(rows)) => rows,
            Ok(_) => return (false, "not reconciled: unexpected status answer".into()),
            Err(e) => return (false, format!("not reconciled: {e}")),
        };
        let mine: Vec<&Value> = rows
            .iter()
            .filter(|r| r["decision_id"].as_str() == Some(decision_id))
            .collect();
        if mine.iter().any(|r| r["in_flight"] == true) {
            return (
                false,
                "not reconciled: the target is still handling this decision".into(),
            );
        }
        let mut unloaded = 0usize;
        for iid in mine.iter().filter_map(|r| r["instance_id"].as_str()) {
            let body = json!({ "instance_id": iid });
            if let Err(e) = self
                .call(node, method::UNLOAD, Some(decision_id.to_string()), body)
                .await
            {
                return (false, format!("not reconciled: unload of {iid} failed ({e})"));
            }
            unloaded += 1;
        }
        (
            true,
            format!("reconciled: unloaded {unloaded} instance(s) left by the lost answer"),
        )
    }
}
