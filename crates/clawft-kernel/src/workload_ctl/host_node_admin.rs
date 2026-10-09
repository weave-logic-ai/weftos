//! Node-admin methods on the `workload.ctl` wire (`dashboard.status`,
//! `dashboard.token.rotate`): the same signed envelope, controller-key check,
//! expiry and nonce replay guard as every other method, answered by a hook the
//! daemon installs instead of by a workload adapter.
//!
//! Trust is the host's own: only a key in this node's controller list reaches
//! the hook at all (`workload-host.json`), and the hook may refuse further.
//! Both ends chain the call (`node_admin.request` / `node_admin.result`) with
//! the method, the requester and the outcome, never the body or any secret.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::host_service::{WorkloadHostService, refuse};
use super::msg::{CtlRequest, Refusal, RefusalCode};

/// Chain kind of an accepted node-admin request on the target.
pub const EVENT_NODE_ADMIN_REQUEST: &str = "node_admin.request";
/// Chain kind of its outcome.
pub const EVENT_NODE_ADMIN_RESULT: &str = "node_admin.result";

/// What a node does when an authorised controller asks it to run a node-admin
/// method. The result is returned to the controller verbatim, so it must not
/// contain secrets.
#[async_trait]
pub trait NodeAdmin: Send + Sync {
    /// Run `method` for `requester` (its verified node id). `Err` is the
    /// reason shown to the controller.
    async fn call(&self, method: &str, requester: &str, body: &Value) -> Result<Value, String>;

    /// [`Self::call`] that may also hand back raw bytes, sent to the
    /// controller as one unsigned frame after the signed response (which
    /// must carry their SHA-256). Only for bodies that asked for it; the
    /// default never does.
    async fn call_raw(&self, method: &str, requester: &str, body: &Value) -> Result<(Value, Option<Vec<u8>>), String> {
        self.call(method, requester, body).await.map(|v| (v, None))
    }
}

impl WorkloadHostService {
    /// Install the node-admin hook (first call wins).
    pub fn set_node_admin(&self, hook: Arc<dyn NodeAdmin>) -> bool {
        self.node_admin.set(hook).is_ok()
    }

    pub(super) async fn node_admin_op(&self, req: &CtlRequest) -> Result<(Value, Option<Vec<u8>>), Refusal> {
        let Some(hook) = self.node_admin.get() else {
            return Err(refuse(
                RefusalCode::UnknownMethod,
                format!("{}: node administration is not enabled on this node", req.method),
            ));
        };
        self.record(
            EVENT_NODE_ADMIN_REQUEST,
            json!({ "node": self.node_id(), "method": req.method, "requester": req.requester,
                    "decision_id": req.decision_id }),
        );
        let out = hook.call_raw(&req.method, &req.requester, &req.body).await;
        self.record(
            EVENT_NODE_ADMIN_RESULT,
            json!({ "node": self.node_id(), "method": req.method, "requester": req.requester,
                    "decision_id": req.decision_id, "ok": out.is_ok(),
                    "error": out.as_ref().err() }),
        );
        out.map_err(|e| refuse(RefusalCode::Runtime, e))
    }
}
