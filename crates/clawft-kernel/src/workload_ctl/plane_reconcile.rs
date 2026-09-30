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
//! second instance. The decision is remembered, and once the target
//! answers ([`PlacementControlPlane::settle_unsettled`], run by status
//! listings and by instance verbs on an unknown id) any instance it
//! produced is adopted as a placement the controller manages.

use serde_json::{Value, json};

use super::msg::method;
use super::plane::{PlacementControlPlane, PlacementRecord};
use crate::chain;

impl PlacementControlPlane {
    pub(super) fn remember_unsettled(&self, template: PlacementRecord) {
        if let Ok(mut u) = self.unsettled.lock() {
            u.insert(template.decision_id.clone(), template);
        }
    }

    /// Decisions still unsettled.
    pub fn unsettled(&self) -> Vec<PlacementRecord> {
        self.unsettled
            .lock()
            .map(|u| u.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Ask each target holding an unsettled decision what became of it:
    /// adopt the instances it produced (chained as `adopted`), forget it if
    /// it produced none, keep it while in flight or unreachable. Returns
    /// the number of instances adopted.
    pub async fn settle_unsettled(&self) -> usize {
        let mut adopted = 0;
        for t in self.unsettled() {
            let rows = match self.call(&t.node_id, method::STATUS, None, json!({})).await {
                Ok(Value::Array(rows)) => rows,
                _ => continue,
            };
            let mine: Vec<&Value> = rows
                .iter()
                .filter(|r| r["decision_id"].as_str() == Some(t.decision_id.as_str()))
                .collect();
            if mine.iter().any(|r| r["in_flight"] == true) {
                continue;
            }
            for iid in mine.iter().filter_map(|r| r["instance_id"].as_str()) {
                let rec = PlacementRecord {
                    instance_id: iid.to_string(),
                    ..t.clone()
                };
                self.chain_event(
                    chain::EVENT_KIND_WORKLOAD_PLACE,
                    json!({ "phase": "adopted", "decision_id": t.decision_id, "node": t.node_id,
                            "variant": t.variant, "instance_id": iid }),
                );
                if let Ok(mut p) = self.placements.lock() {
                    p.insert(iid.to_string(), rec);
                }
                adopted += 1;
            }
            if let Ok(mut u) = self.unsettled.lock() {
                u.remove(&t.decision_id);
            }
        }
        adopted
    }

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
                return (
                    false,
                    format!("not reconciled: unload of {iid} failed ({e})"),
                );
            }
            unloaded += 1;
        }
        (
            true,
            format!("reconciled: unloaded {unloaded} instance(s) left by the lost answer"),
        )
    }
}
