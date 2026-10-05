//! Controller side of the node-admin methods (`dashboard.status`,
//! `dashboard.token.rotate`): one signed call to a known peer, with the
//! request and its outcome chained on this node.
//!
//! A state-changing method (`dashboard.token.rotate`) is sent only to an
//! operator-pinned peer: the target's tier comes from `workload-peers.json`
//! (never from the peer), so a node that is merely discovered or paired cannot
//! be told to rotate its credentials.

use serde_json::{Value, json};

use clawft_types::placement::TrustTier;

use super::msg::method;
use super::plane::{PlacementControlPlane, PlaneError};

/// Chain kind of a node-admin request sent from this controller.
pub const EVENT_NODE_ADMIN_SENT: &str = "node_admin.sent";
/// Chain kind of its outcome.
pub const EVENT_NODE_ADMIN_OUTCOME: &str = "node_admin.outcome";

impl PlacementControlPlane {
    /// Send node-admin method `m` to `node_id` and return its result.
    pub async fn node_admin(&self, node_id: &str, m: &str, body: Value) -> Result<Value, PlaneError> {
        if !method::is_node_admin(m) {
            return Err(PlaneError::Invalid(format!("{m} is not a node-admin method")));
        }
        let (target, _) = self.target(node_id)?;
        if method::mutates(m) && target.tier < TrustTier::Pinned {
            return Err(PlaneError::Governance(format!(
                "{m} needs an operator-pinned peer; {node_id} is {:?} (set its tier to pinned in workload-peers.json)",
                target.tier
            )));
        }
        self.ensure_described(node_id).await;
        let decision_id = method::mutates(m).then(|| {
            self.chain_event(EVENT_NODE_ADMIN_SENT, json!({ "node": node_id, "method": m }))
        });
        let out = self.call(node_id, m, decision_id.clone(), body).await;
        self.chain_event(
            EVENT_NODE_ADMIN_OUTCOME,
            json!({ "node": node_id, "method": m, "decision_id": decision_id,
                    "ok": out.is_ok(), "error": out.as_ref().err().map(|e| e.to_string()) }),
        );
        out
    }
}
