//! Seed nodes (COG-001 section 5, option b): a Cognitum Seed does not run
//! WeftOS, so there is no `workload-host` to message. The control plane
//! addresses card 09's `remote.api` Seed adapter, held on this node, by the
//! operator-assigned node id. Only operator-pinned store cogs go there
//! (our signed packages go to native / container nodes).
//!
//! The same governance and audit apply: `workload.place` is gated and the
//! decision chained here; the adapter's `WorkloadHost` gates and chains
//! install / load / start / stop / unload.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::chain;
use crate::gate::GateDecision;
use crate::workload_pkg::codec::hex_encode;
use crate::workload_runtime::{
    HostContract, InstanceHandle, VerifiedWorkload, WorkloadConfig, WorkloadHost,
};

use clawft_types::placement::TrustTier;

use super::facts::governance_tier;
use super::host_service::CtlConfig;
use super::msg::method;
use super::plane::{PLANE_CHAIN_SOURCE, PlacementControlPlane, PlacementRecord, PlaneError};

/// Route name recorded for Seed placements.
pub const SEED_ROUTE: &str = "remote.api";

/// An operator store pin to place on a Seed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorePinOrder {
    /// Operator-assigned Seed node id.
    pub node_id: String,
    /// Store registry (`cognitum`).
    pub registry: String,
    /// Cog id.
    pub id: String,
    /// Pinned version.
    pub version: String,
    /// Optional expected SHA-256 of the store binary.
    #[serde(default)]
    pub sha256: Option<String>,
    /// Instance configuration.
    pub config: CtlConfig,
    /// Start after installing (Seeds auto-start; the adapter stops first).
    #[serde(default)]
    pub start: bool,
}

impl PlacementControlPlane {
    /// Register a Seed adapter under its operator-assigned node id and
    /// trust tier (governance sees that tier, not a default).
    pub fn add_seed(
        &self,
        node_id: &str,
        host: Arc<WorkloadHost>,
        tier: TrustTier,
    ) -> Result<(), PlaneError> {
        if !crate::workload_pkg::manifest::valid_token(node_id, 64) {
            return Err(PlaneError::Invalid(
                "seed node id must be a plain token".into(),
            ));
        }
        if let Ok(mut s) = self.seeds.write() {
            s.insert(node_id.to_string(), (host, tier));
        }
        Ok(())
    }

    fn seed(&self, node_id: &str) -> Option<(Arc<WorkloadHost>, TrustTier)> {
        self.seeds.read().ok().and_then(|s| s.get(node_id).cloned())
    }

    fn chain_here(&self, kind: &str, payload: Value) -> String {
        hex_encode(
            &self
                .chain
                .append(PLANE_CHAIN_SOURCE, kind, Some(payload))
                .hash,
        )
    }

    /// Place an operator-pinned store cog on a Seed. Pinned by
    /// construction: the node is the operator-assigned id.
    pub async fn place_store_pin(&self, o: &StorePinOrder) -> Result<PlacementRecord, PlaneError> {
        let (host, tier) = self
            .seed(&o.node_id)
            .ok_or_else(|| PlaneError::Unknown(format!("no Seed adapter for {}", o.node_id)))?;
        let w = VerifiedWorkload::store_pin(&o.registry, &o.id, &o.version, o.sha256.as_deref())
            .map_err(|e| PlaneError::Invalid(e.to_string()))?;
        let ctx = json!({ "node_id": o.node_id, "workload": {
            "kind": w.kind, "package_trust": "operator_attested", "node_tier": governance_tier(tier),
            "network": host.runtime().network_exposure(), "secrets": false, "emulated": false,
            "resource_cost": 0.25, "package_id": format!("store.{}.{}", w.id, w.version),
        }});
        let verdict = self.gate.check(&self.node_id, "workload.place", &ctx);
        let permitted = matches!(verdict, GateDecision::Permit { .. });
        let decision_id = self.chain_here(
            chain::EVENT_KIND_WORKLOAD_PLACE,
            json!({ "phase": "decision", "route": SEED_ROUTE, "runtime": host.runtime().id(),
                    "pin": o.node_id, "store_pin": { "registry": o.registry, "id": o.id,
                    "version": o.version }, "permitted": permitted }),
        );
        if !permitted {
            let reason = match verdict {
                GateDecision::Deny { reason, .. } | GateDecision::Defer { reason } => reason,
                GateDecision::Permit { .. } => String::new(),
            };
            return Err(PlaneError::Governance(reason));
        }
        let cfg = WorkloadConfig {
            mode: o.config.mode.clone(),
            args: o.config.args.clone(),
            host: HostContract::new(SocketAddr::from(([0, 0, 0, 0], o.config.csi_port))),
            node_id: o.node_id.clone(),
        };
        let refuse = |phase: &str, e: &dyn std::fmt::Display| {
            self.chain_here(
                chain::EVENT_KIND_WORKLOAD_REFUSE,
                json!({ "phase": phase, "decision_id": decision_id, "node": o.node_id,
                        "route": SEED_ROUTE, "reason": e.to_string() }),
            );
        };
        let h = match host.load(&w, &cfg).await {
            Ok(h) => h,
            Err(e) => {
                refuse("dispatch", &e);
                return Err(PlaneError::Call(super::plane::CallFailure::Refused(
                    super::msg::Refusal::new(super::msg::RefusalCode::Admission, e.to_string()),
                )));
            }
        };
        if o.start
            && let Err(e) = host.start(&h).await
        {
            // Not placed: forget the loaded instance so nothing is left
            // that the controller does not track.
            let rolled = host.unload(h.clone()).await;
            let why = match &rolled {
                Ok(()) => format!("start failed: {e}; loaded instance unloaded"),
                Err(u) => format!("start failed: {e}; unload failed too: {u}"),
            };
            refuse("start", &why);
            if rolled.is_err() {
                // Still loaded: keep it addressable (status / unload).
                self.track_seed(self.seed_record(o, &w, &h, &decision_id), h)
                    .await;
            }
            return Err(PlaneError::Call(super::plane::CallFailure::Refused(
                super::msg::Refusal::new(super::msg::RefusalCode::Runtime, why),
            )));
        }
        let rec = self.seed_record(o, &w, &h, &decision_id);
        self.chain_here(
            chain::EVENT_KIND_WORKLOAD_PLACE,
            json!({ "phase": "placed", "decision_id": decision_id, "node": o.node_id,
                    "route": SEED_ROUTE, "instance_id": h.instance_id }),
        );
        self.track_seed(rec.clone(), h).await;
        Ok(rec)
    }

    fn seed_record(
        &self,
        o: &StorePinOrder,
        w: &VerifiedWorkload,
        h: &InstanceHandle,
        decision_id: &str,
    ) -> PlacementRecord {
        PlacementRecord {
            instance_id: h.instance_id.clone(),
            node_id: o.node_id.clone(),
            kind: w.kind.clone(),
            workload: w.id.clone(),
            variant: SEED_ROUTE.to_string(),
            decision_id: decision_id.to_string(),
            manifest_hash: String::new(),
        }
    }

    async fn track_seed(&self, rec: PlacementRecord, h: InstanceHandle) {
        self.seed_handles
            .lock()
            .await
            .insert(h.instance_id.clone(), h);
        if let Ok(mut p) = self.placements.lock() {
            p.insert(rec.instance_id.clone(), rec);
        }
    }

    /// Instance verbs on a Seed placement (gated and chained by its host).
    pub(super) async fn seed_instance(
        &self,
        rec: &PlacementRecord,
        m: &str,
    ) -> Option<Result<Value, PlaneError>> {
        let (host, _) = self.seed(&rec.node_id)?;
        let mut handles = self.seed_handles.lock().await;
        let Some(h) = handles.get(&rec.instance_id).cloned() else {
            return Some(Err(PlaneError::Unknown(rec.instance_id.clone())));
        };
        let err = |e: crate::workload_runtime::RuntimeError| PlaneError::Governance(e.to_string());
        Some(match m {
            method::STATUS => {
                Ok(json!({ "instance_id": h.instance_id, "status": host.status(&h).await }))
            }
            method::START => host
                .start(&h)
                .await
                .map(|_| json!({ "started": h.instance_id }))
                .map_err(err),
            method::STOP => host
                .stop(&h, Duration::from_secs(5))
                .await
                .map(|ev| json!({ "stopped": h.instance_id, "evidence": ev.audit() }))
                .map_err(err),
            method::UNLOAD => {
                let r = host.unload(h.clone()).await;
                if r.is_ok() {
                    handles.remove(&rec.instance_id);
                }
                r.map(|_| json!({ "unloaded": rec.instance_id }))
                    .map_err(err)
            }
            other => Err(PlaneError::Invalid(format!(
                "{other} is not supported on a Seed"
            ))),
        })
    }
}
