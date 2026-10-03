//! `workload.revoke {package | signer | hash, reason?}` (ADR-099 section 7).
//!
//! The operator's revocation verb, `weaver workload revoke`. Admin
//! capability only (see `capability.rs`); it is not gated by a workload
//! permit, because revoking is the safety action and must work in an
//! incident, before anyone has written a permit for it.
//!
//! What one call does, in order:
//!
//! 1. records the revocation in this node's list (persisted, chained as
//!    `workload.revoke` by the list itself, revoked by `operator`);
//! 2. sweeps what this node holds (grants dropped, bytes evicted,
//!    `artifact.revoke` / `artifact.evict` chained);
//! 3. stops and unloads every instance this node hosts that the revocation
//!    names, directly or through its signer, and drops the controller's
//!    records of them (each step chained, `forced_by_revocation`);
//! 4. signs a revocation notice and floods it to every peer
//!    (`RevocationExchange::issue`), where the same sweep and unload run,
//!    when this node's key is a pinned operator key. Otherwise the answer
//!    says no notice was issued and why: the revocation then holds on this
//!    node only.

use std::sync::{Arc, OnceLock, RwLock};

use clawft_kernel::mesh_artifact::ArtifactExchange;
use clawft_kernel::mesh_swarm_revoke::{RevocationExchange, sign_revocation};
use clawft_kernel::revocation::{RevocationKind, RevocationList};
use clawft_kernel::workload_ctl::{ForcedTeardown, PlacementControlPlane, WorkloadHostService};
use ed25519_dalek::SigningKey;
use serde::Deserialize;
use serde_json::{Value, json};

/// Longest reason kept.
const MAX_REASON: usize = 256;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Params {
    #[serde(default)]
    package: Option<String>,
    #[serde(default)]
    signer: Option<String>,
    #[serde(default)]
    hash: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

/// How often the daemon re-runs [`Revoker::enforce`], so a teardown that
/// failed (an adapter busy, a host not up yet) is retried without waiting
/// for another revocation.
pub const ENFORCE_TICK: std::time::Duration = std::time::Duration::from_secs(60);

/// What the daemon revokes against. Built from the kernel's list alone, so
/// the verb works when the placement plane could not be built (a broken
/// policy file during an incident); the rest is attached as it comes up.
pub struct Revoker {
    /// This node's subject revocation list (the kernel's).
    list: Arc<RevocationList>,
    /// The artifact exchange holding the bytes, once built.
    exchange: RwLock<Option<Arc<ArtifactExchange>>>,
    /// This node's `workload-host`, when it has one.
    host: OnceLock<Arc<WorkloadHostService>>,
    /// The controller, whose records of torn-down instances are dropped.
    plane: OnceLock<Arc<PlacementControlPlane>>,
    /// Mesh revocation notices, when there is a mesh.
    notices: OnceLock<Arc<RevocationExchange>>,
    /// The key notices are signed with, when it is a pinned operator key.
    notice_key: OnceLock<SigningKey>,
}

impl Revoker {
    /// A revoker over `list`, with nothing else attached yet.
    pub fn new(list: Arc<RevocationList>) -> Self {
        Self {
            list,
            exchange: RwLock::new(None),
            host: OnceLock::new(),
            plane: OnceLock::new(),
            notices: OnceLock::new(),
            notice_key: OnceLock::new(),
        }
    }

    /// Sweep this exchange when something is revoked (a later build replaces it).
    pub fn set_exchange(&self, ex: Arc<ArtifactExchange>) {
        if let Ok(mut g) = self.exchange.write() {
            *g = Some(ex);
        }
    }

    /// Attach the pieces that exist once placement is up (first call wins).
    pub fn attach(
        &self,
        host: Option<Arc<WorkloadHostService>>,
        plane: Arc<PlacementControlPlane>,
        notices: Option<Arc<RevocationExchange>>,
        notice_key: Option<SigningKey>,
    ) {
        if let Some(h) = host {
            let _ = self.host.set(h);
        }
        let _ = self.plane.set(plane);
        if let Some(n) = notices {
            let _ = self.notices.set(n);
        }
        if let Some(k) = notice_key {
            let _ = self.notice_key.set(k);
        }
    }

    /// Run [`Self::enforce`] every `period` for as long as the runtime lives.
    pub fn spawn_tick(self: &Arc<Self>, period: std::time::Duration) -> tokio::task::JoinHandle<()> {
        let me = Arc::clone(self);
        tokio::spawn(async move {
            let mut t = tokio::time::interval(period);
            t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            t.tick().await; // the first tick is immediate; the caller sweeps at start
            loop {
                t.tick().await;
                let forced = me.enforce().await;
                if !forced.is_empty() {
                    tracing::warn!(
                        n = forced.len(),
                        left = forced.iter().filter(|f| !f.unloaded).count(),
                        "revocation sweep"
                    );
                }
            }
        })
    }
}

fn target(p: &Params) -> Result<(RevocationKind, String), String> {
    let named: Vec<(RevocationKind, &String)> = [
        (RevocationKind::Package, &p.package),
        (RevocationKind::SignerKey, &p.signer),
        (RevocationKind::ArtifactHash, &p.hash),
    ]
    .into_iter()
    .filter_map(|(k, v)| v.as_ref().map(|v| (k, v)))
    .collect();
    let [(kind, id)] = named.as_slice() else {
        return Err("name exactly one of package, signer, hash".into());
    };
    let id = kind.normalize(id).map_err(|e| e.to_string())?;
    Ok((*kind, id))
}

impl Revoker {
    /// Stop and unload what the list now revokes on this node, then drop
    /// the controller's records of what went. Safe to call repeatedly.
    pub async fn enforce(&self) -> Vec<ForcedTeardown> {
        let Some(host) = self.host.get() else {
            return Vec::new();
        };
        let forced = host.enforce_revocations(&self.list).await;
        if let Some(plane) = self.plane.get() {
            let gone: Vec<String> = forced
                .iter()
                .filter(|f| f.unloaded)
                .map(|f| f.instance_id.clone())
                .collect();
            plane.forget_instances(&gone);
        }
        forced
    }

    /// Serve one `workload.revoke` call.
    pub async fn revoke(&self, params: Value) -> Result<Value, String> {
        let p: Params =
            serde_json::from_value(params).map_err(|e| format!("invalid workload.revoke params: {e}"))?;
        let (kind, id) = target(&p)?;
        let reason: String = p
            .reason
            .as_deref()
            .filter(|r| !r.trim().is_empty())
            .unwrap_or("operator revocation")
            .chars()
            .take(MAX_REASON)
            .collect();
        // The revocation holds in memory even if the write fails, and is
        // chained either way; the error is reported, not swallowed.
        let (newly, persist_error) = match self.list.revoke_subject_by(kind, &id, &reason, "operator") {
            Ok(n) => (n, None),
            Err(clawft_kernel::revocation::RevocationError::Persist(e)) => (true, Some(e)),
            Err(e) => return Err(e.to_string()),
        };
        let ex = self.exchange.read().ok().and_then(|g| g.clone());
        let swept = ex.map_or(0, |ex| ex.apply_revocations().len());
        let forced = self.enforce().await;
        let notice = self.issue(kind, &id, &reason).await;
        Ok(json!({
            "revoked": { "kind": kind, "id": id, "reason": reason },
            "newly_revoked": newly,
            "persisted": persist_error.is_none(),
            "persist_error": persist_error,
            "artifacts_swept": swept,
            "forced": forced,
            "notice": notice,
        }))
    }

    async fn issue(&self, kind: RevocationKind, id: &str, reason: &str) -> String {
        let Some(notices) = self.notices.get() else {
            return "not issued: this node has no mesh (or placement is not up), the revocation \
                    holds here only"
                .into();
        };
        let Some(key) = self.notice_key.get() else {
            return "not issued: this node's key is not a pinned operator key in \
                    workload-trust.json, the revocation holds here only"
                .into();
        };
        let now = chrono::Utc::now().timestamp().max(0) as u64;
        let signed = match sign_revocation(kind, id, reason, now, key) {
            Ok(s) => s,
            Err(e) => return format!("not issued: {e}"),
        };
        match notices.issue(signed).await {
            Ok(_) => "issued to every connected peer".into(),
            Err(e) => format!("not issued: {e}"),
        }
    }
}

#[cfg(test)]
#[path = "workload_revoke_rpc_tests.rs"]
mod tests;
