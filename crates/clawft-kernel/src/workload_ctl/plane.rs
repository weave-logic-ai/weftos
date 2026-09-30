//! The controller side of placement (ADR-099 sections 3 and 7).
//!
//! [`PlacementControlPlane`] learns targets over signed `describe` round
//! trips (their signed facts go into a [`NodeFactsCache`] under the trust
//! tier the operator assigned, and their `workload-host` advertisement
//! into a [`ClusterServiceRegistry`]), runs the pure engine on the cached
//! facts, chains the decision, and dispatches signed `workload.ctl`
//! requests. Placement itself is in [`super::plane_place`].

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use clawft_types::placement::TrustTier as FactsTier;
use clawft_types::placement::engine::{Liveness, ScoringWeights};
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::chain::ChainManager;
use crate::cluster::ClusterMembership;
use crate::gate::GateBackend;
use crate::mesh_artifact::ArtifactExchange;
use crate::mesh_service_adv::{ClusterServiceRegistry, ServiceAdvertisement};
use crate::node_facts::NodeFactsCache;
use crate::node_facts_advert::SignedNodeFacts;
use crate::node_registry::node_id_from_pubkey;
use crate::workload_pkg::TrustAnchors;
use crate::workload_runtime::{InstanceHandle, WorkloadHost};

use super::facts::{LiveNodeFacts, placement_view};
use super::msg::{ANY_TARGET, CtlOutcome, CtlRequest, Refusal, method, verify_response};
use super::session::CtlConnection;
use super::transport::CtlConnector;

/// Chain source for controller-side placement records.
pub const PLANE_CHAIN_SOURCE: &str = "workload.placement";

/// Tunables.
#[derive(Debug, Clone)]
pub struct PlaneConfig {
    /// Timeout for read-only calls.
    pub call_timeout: Duration,
    /// Timeout for `place` (includes the payload fetch).
    pub place_timeout: Duration,
    /// Request lifetime.
    pub request_ttl_ms: u64,
    /// Scoring weights (governance config).
    pub weights: ScoringWeights,
}

impl Default for PlaneConfig {
    fn default() -> Self {
        Self {
            call_timeout: Duration::from_secs(20),
            place_timeout: Duration::from_secs(280),
            request_ttl_ms: 290_000,
            weights: ScoringWeights::default(),
        }
    }
}

/// Why a call to a target failed.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CallFailure {
    /// The target answered with a signed refusal.
    #[error("{0}")]
    Refused(Refusal),
    /// No valid answer (transport, timeout, bad signature).
    #[error("unreachable: {0}")]
    Unreachable(String),
}

/// Controller-level errors.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PlaneError {
    /// Bad input.
    #[error("invalid: {0}")]
    Invalid(String),
    /// Package could not be verified or seeded.
    #[error("package: {0}")]
    Package(String),
    /// Unknown node or instance.
    #[error("unknown: {0}")]
    Unknown(String),
    /// A call failed.
    #[error(transparent)]
    Call(#[from] CallFailure),
    /// Governance refused.
    #[error("governance: {0}")]
    Governance(String),
    /// The engine refused the request (bad pin, invalid spec).
    #[error("placement: {0}")]
    Placement(String),
}

/// A known target.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetInfo {
    /// Node id.
    pub node_id: String,
    /// Where its `workload-host` listens.
    pub addr: String,
    /// Operator-assigned trust tier.
    pub tier: FactsTier,
    /// Signing key (hex) from its signed facts.
    pub public_key: String,
    /// Last `describe` succeeded.
    pub reachable: bool,
}

/// One placed instance, as the controller knows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacementRecord {
    /// Instance id on the target.
    pub instance_id: String,
    /// Target node.
    pub node_id: String,
    /// Workload kind.
    pub kind: String,
    /// Workload name.
    pub workload: String,
    /// Chosen variant.
    pub variant: String,
    /// Chained decision.
    pub decision_id: String,
    /// Package manifest hash.
    pub manifest_hash: String,
}

/// One signed round trip.
pub(super) struct Call<'a> {
    pub addr: &'a str,
    pub target: &'a str,
    pub expect: Option<[u8; 32]>,
    pub method: &'a str,
    pub decision_id: Option<String>,
    pub body: Value,
    /// Serve the payload to the target during the call.
    pub serve: bool,
}

/// The controller.
pub struct PlacementControlPlane {
    pub(super) node_id: String,
    pub(super) key: SigningKey,
    pub(super) gate: Arc<dyn GateBackend>,
    pub(super) chain: Arc<ChainManager>,
    pub(super) exchange: Arc<ArtifactExchange>,
    pub(super) anchors: TrustAnchors,
    pub(super) connector: Arc<dyn CtlConnector>,
    pub(super) facts: NodeFactsCache,
    pub(super) targets: RwLock<BTreeMap<String, TargetInfo>>,
    pub(super) services: Mutex<ClusterServiceRegistry>,
    pub(super) membership: Option<Arc<ClusterMembership>>,
    pub(super) placements: Mutex<BTreeMap<String, PlacementRecord>>,
    /// Seed adapters by operator-assigned node id (card 09's `remote.api`).
    pub(super) seeds: RwLock<BTreeMap<String, Arc<WorkloadHost>>>,
    /// Handles of instances placed on Seeds.
    pub(super) seed_handles: tokio::sync::Mutex<BTreeMap<String, InstanceHandle>>,
    pub(super) cfg: PlaneConfig,
}

pub(super) fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}

impl PlacementControlPlane {
    /// Controller for the node owning `key`. Every decision is chained.
    pub fn new(
        key: SigningKey,
        gate: Arc<dyn GateBackend>,
        chain: Arc<ChainManager>,
        exchange: Arc<ArtifactExchange>,
        anchors: TrustAnchors,
        connector: Arc<dyn CtlConnector>,
    ) -> Self {
        Self {
            node_id: node_id_from_pubkey(&key.verifying_key().to_bytes()),
            key,
            gate,
            chain,
            exchange,
            anchors,
            connector,
            facts: NodeFactsCache::new(),
            targets: RwLock::new(BTreeMap::new()),
            services: Mutex::new(ClusterServiceRegistry::new()),
            membership: None,
            placements: Mutex::new(BTreeMap::new()),
            seeds: RwLock::new(BTreeMap::new()),
            seed_handles: tokio::sync::Mutex::new(BTreeMap::new()),
            cfg: PlaneConfig::default(),
        }
    }

    /// Use membership peer state for liveness.
    pub fn with_membership(mut self, m: Arc<ClusterMembership>) -> Self {
        self.membership = Some(m);
        self
    }

    /// Replace the tunables.
    pub fn with_config(mut self, cfg: PlaneConfig) -> Self {
        self.cfg = cfg;
        self
    }

    /// This controller's node id.
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    /// Known targets.
    pub fn targets(&self) -> Vec<TargetInfo> {
        self.targets
            .read()
            .map(|t| t.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Nodes advertising `workload-host`.
    pub fn hosts(&self) -> Vec<ServiceAdvertisement> {
        self.services
            .lock()
            .map(|s| {
                s.resolve(super::msg::WORKLOAD_HOST_SERVICE)
                    .into_iter()
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Placed instances.
    pub fn placements(&self) -> Vec<PlacementRecord> {
        self.placements
            .lock()
            .map(|p| p.values().cloned().collect())
            .unwrap_or_default()
    }

    /// The verified facts the engine will see, with liveness.
    pub fn view(&self) -> Vec<LiveNodeFacts> {
        let targets = self.targets.read().map(|t| t.clone()).unwrap_or_default();
        let contact = |id: &str| {
            targets.get(id).map(|t| {
                if t.reachable {
                    Liveness::Alive
                } else {
                    Liveness::Suspect
                }
            })
        };
        placement_view(
            &self.facts,
            now_ms() / 1000,
            None,
            self.membership.as_deref(),
            &contact,
        )
    }

    /// One signed round trip to `addr`. `expect` is the target's key when
    /// known. `serve` lets the target fetch the payload over the call.
    pub(super) async fn round_trip(&self, c: Call<'_>) -> Result<(Value, [u8; 32]), CallFailure> {
        let Call {
            addr,
            target,
            expect,
            method: m,
            decision_id,
            body,
            serve,
        } = c;
        let timeout = if m == method::PLACE || m == method::LOAD {
            self.cfg.place_timeout
        } else {
            self.cfg.call_timeout
        };
        let req = CtlRequest::new(
            &self.key,
            m,
            target,
            now_ms(),
            self.cfg.request_ttl_ms,
            decision_id,
            body,
        );
        let signed = req.sign(&self.key).map_err(CallFailure::Refused)?;
        let stream = self
            .connector
            .connect(addr)
            .await
            .map_err(|e| CallFailure::Unreachable(e.to_string()))?;
        let mut conn = CtlConnection::new(stream, self.node_id.clone());
        let exchange = serve.then_some(self.exchange.as_ref());
        let r = conn.call(target, m, &signed, exchange, timeout).await;
        conn.close().await;
        let resp_signed = r.map_err(|e| CallFailure::Unreachable(e.to_string()))?;
        let (resp, pk) = verify_response(&resp_signed, &req, expect.as_ref())
            .map_err(|e| CallFailure::Unreachable(format!("bad response: {e}")))?;
        match resp.outcome {
            CtlOutcome::Ok { result } => Ok((result, pk)),
            CtlOutcome::Refused { refusal } => Err(CallFailure::Refused(refusal)),
        }
    }

    /// Learn (or refresh) the target at `addr`: signed `describe`, verify
    /// and cache its signed facts under `tier`, merge its advertisement.
    /// Returns its node id.
    pub async fn add_target(&self, addr: &str, tier: FactsTier) -> Result<String, PlaneError> {
        let (result, pk) = self
            .round_trip(Call {
                addr,
                target: ANY_TARGET,
                expect: None,
                method: method::DESCRIBE,
                decision_id: None,
                body: json!({}),
                serve: false,
            })
            .await?;
        let node_id = node_id_from_pubkey(&pk);
        let facts: SignedNodeFacts = serde_json::from_value(result["facts"].clone())
            .map_err(|e| PlaneError::Invalid(format!("{node_id} sent no signed facts: {e}")))?;
        if facts.public_key != pk.to_vec() {
            return Err(PlaneError::Invalid(format!(
                "{node_id}: facts signed by another key"
            )));
        }
        let now = now_ms() / 1000;
        match self.facts.insert(facts, tier, now) {
            Ok(_) | Err(crate::node_facts::CacheError::Stale { .. }) => {}
            Err(e) => return Err(PlaneError::Invalid(format!("{node_id} facts refused: {e}"))),
        }
        if let Ok(adv) =
            serde_json::from_value::<ServiceAdvertisement>(result["advertisement"].clone())
            && adv.node_id == node_id
            && let Ok(mut s) = self.services.lock()
        {
            s.merge(adv);
        }
        if let Ok(mut t) = self.targets.write() {
            t.insert(
                node_id.clone(),
                TargetInfo {
                    node_id: node_id.clone(),
                    addr: addr.to_string(),
                    tier,
                    public_key: crate::workload_pkg::codec::hex_encode(&pk),
                    reachable: true,
                },
            );
        }
        Ok(node_id)
    }

    /// Re-describe every known target; unreachable ones are marked so the
    /// engine sees them as not alive.
    /// A node whose address now answers as another node (it was replaced)
    /// is marked unreachable too.
    pub async fn refresh(&self) {
        for t in self.targets() {
            let same = matches!(self.add_target(&t.addr, t.tier).await, Ok(id) if id == t.node_id);
            if !same
                && let Ok(mut map) = self.targets.write()
                && let Some(e) = map.get_mut(&t.node_id)
            {
                e.reachable = false;
            }
        }
    }

    pub(super) fn target(&self, node_id: &str) -> Result<(TargetInfo, [u8; 32]), PlaneError> {
        let t = self
            .targets
            .read()
            .ok()
            .and_then(|m| m.get(node_id).cloned())
            .ok_or_else(|| PlaneError::Unknown(format!("no workload-host known for {node_id}")))?;
        let pk = crate::workload_pkg::codec::hex_decode_exact::<32>(&t.public_key)
            .ok_or_else(|| PlaneError::Invalid("bad stored key".into()))?;
        Ok((t, pk))
    }

    /// A signed call to a known node (no payload serving).
    pub async fn call(
        &self,
        node_id: &str,
        m: &str,
        decision_id: Option<String>,
        body: Value,
    ) -> Result<Value, PlaneError> {
        let (t, pk) = self.target(node_id)?;
        Ok(self
            .round_trip(Call {
                addr: &t.addr,
                target: node_id,
                expect: Some(pk),
                method: m,
                decision_id,
                body,
                serve: false,
            })
            .await?
            .0)
    }
}
