//! Forced unload on revocation: a node stops and unloads every instance it
//! hosts whose package, signer key or artifact hash was revoked (ADR-099
//! section 7, card mesh-placement-revoke).
//!
//! Revoking a subject drops its grants and evicts its bytes, but a cog that
//! is already running keeps running from what it loaded. This closes that:
//! when a revocation is applied (the operator's verb, a signed mesh notice,
//! or startup after one was applied while the node was down) the host walks
//! its instances and tears down each one that now names a revoked subject.
//! The teardown is not gated: the applied revocation is the authority, so
//! enforcement does not depend on the operator having written a stop permit.
//! Every step is chained (`workload.stop`, `workload.unload`, with
//! `forced_by_revocation` naming the subject).

use std::time::Duration;

use serde::Serialize;
use serde_json::json;

use super::host_service::WorkloadHostService;
use crate::revocation::{RevocationList, RevokedSubject};

/// Grace period given to a revoked instance to exit before it is killed.
const FORCED_GRACE: Duration = Duration::from_secs(2);

/// One instance a revocation took down (or failed to).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ForcedTeardown {
    /// Instance id on this node.
    pub instance_id: String,
    /// Workload name (cog id).
    pub workload: String,
    /// The revoked subject that matched.
    pub subject: RevokedSubject,
    /// Whether the instance is gone. `false`: it is still held and the next
    /// sweep tries again.
    pub unloaded: bool,
    /// Why it is not gone, when it is not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl WorkloadHostService {
    /// Stop and unload every hosted instance that `list` now revokes.
    /// Idempotent: instances already gone are not touched. Instances that
    /// cannot be torn down stay held and are reported with `unloaded:
    /// false`, so the next call retries them.
    pub async fn enforce_revocations(&self, list: &RevocationList) -> Vec<ForcedTeardown> {
        let mut out = Vec::new();
        let mut map = self.instances.lock().await;
        let ids: Vec<String> = map.keys().cloned().collect();
        for id in ids {
            let Some(p) = map.get(&id) else { continue };
            let Some(host) = self.routes.get(&p.route).cloned() else {
                continue;
            };
            let Some(w) = host.workload_for(&p.handle).await else {
                continue;
            };
            let (package_id, keys, hashes) = w.revocation_refs();
            let Some(subject) = list.first_revoked(Some(&package_id), &keys, &hashes) else {
                continue;
            };
            // The token goes first: a revoked cog must not keep a credential
            // this node no longer vouches for.
            if let (Some(hk), Some(l)) = (&self.ingest, &p.ingest) {
                hk.deactivate(l);
            }
            let handle = p.handle.clone();
            let r = host
                .revoke_teardown(&handle, FORCED_GRACE, json!(subject))
                .await;
            if r.is_ok() {
                map.remove(&id);
            }
            out.push(ForcedTeardown {
                instance_id: id,
                workload: w.id.clone(),
                subject,
                unloaded: r.is_ok(),
                error: r.err().map(|e| e.to_string()),
            });
        }
        out
    }
}
