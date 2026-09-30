//! The target side: a node's `workload-host` service (ADR-099 section 7).
//!
//! It verifies each signed request, then for `place` / `load`:
//!
//! 1. picks the runtime adapter for the chosen route (`native`,
//!    `container`, ...); no adapter is an admission refusal;
//! 2. fetches the package before loading, over the same connection, with
//!    the artifact piece protocol (card 11) and verifies its signatures
//!    against this node's own trust anchors;
//! 3. rebuilds the workload from the fetched bytes and hands it to the
//!    adapter through [`WorkloadHost`], whose admission self-check (binary
//!    arch, OS, runtime) runs before governance and load (card 09).
//!
//! Every refusal is chained as `workload.refuse` (source
//! [`HOST_CHAIN_SOURCE`]); a placement is chained as `workload.place`.
//! Unknown `workload.*` methods go to the governance gate, which denies
//! and chains them (default deny, ADR-099 section 4).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::chain::{self, ChainManager};
use crate::gate::{GateBackend, GateDecision};
use crate::ipc::GlobalPid;
use crate::mesh_artifact::ArtifactExchange;
use crate::mesh_artifact_pkg::PackageExchangeError;
use crate::mesh_artifact_transfer::PeerSet;
use crate::mesh_service_adv::ServiceAdvertisement;
use crate::node_facts_advert::SignedNodeFacts;
use crate::node_registry::node_id_from_pubkey;
use crate::workload_pkg::TrustAnchors;
use crate::workload_pkg::manifest::valid_token;
use crate::workload_pkg::store::StoreSource;
use crate::workload_runtime::{
    HostContract, InstanceHandle, RunEvidence, RunMode, RuntimeError, VerifiedWorkload,
    WorkloadConfig, WorkloadHost,
};

use super::cog_kind::route_of;
use super::host_instances::InFlight;
use super::msg::{
    CTL_VERSION, ControllerPolicy, CtlOutcome, CtlRequest, CtlResponse, NonceGuard, Refusal,
    RefusalCode, SignedCtl, WORKLOAD_HOST_SERVICE, method, verify_request,
};
use super::refusal_budget::RefusalBudget;

/// Chain source for target-side placement records.
pub const HOST_CHAIN_SOURCE: &str = "workload.host";

/// Place-time configuration sent with `place` / `load`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CtlConfig {
    /// Run mode (`once`, `interval`, `listener`).
    pub mode: RunMode,
    /// Extra cog arguments (validated by the adapter).
    #[serde(default)]
    pub args: Vec<String>,
    /// UDP port for the sensor feed (0.0.0.0:<port>).
    #[serde(default = "default_csi_port")]
    pub csi_port: u16,
}

fn default_csi_port() -> u16 {
    crate::workload_runtime::host_contract::DEFAULT_CSI_PORT
}

/// Body of `place` / `load`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaceBody {
    /// Workload name (cog id).
    pub name: String,
    /// BLAKE3 of the signed manifest (fetch key).
    pub manifest_hash: String,
    /// Chosen variant (`aarch64-native`).
    pub variant: String,
    /// Instance configuration.
    pub config: CtlConfig,
    /// Start after loading (place only).
    #[serde(default)]
    pub start: bool,
}

/// Body of the instance verbs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceBody {
    /// Instance id (`status` without one lists every instance).
    #[serde(default)]
    pub instance_id: Option<String>,
    /// Stop grace period.
    #[serde(default)]
    pub grace_ms: Option<u64>,
}

pub(super) struct Placed {
    pub(super) handle: InstanceHandle,
    pub(super) route: String,
    pub(super) name: String,
    pub(super) variant: String,
    pub(super) decision_id: Option<String>,
    pub(super) last: Option<RunEvidence>,
}

/// Fresh signed facts on demand (a daemon re-probes before the TTL ends).
pub type FactsSource = Arc<dyn Fn() -> Option<SignedNodeFacts> + Send + Sync>;

/// A node's `workload-host`.
pub struct WorkloadHostService {
    node_id: String,
    key: SigningKey,
    anchors: TrustAnchors,
    controllers: Box<dyn ControllerPolicy>,
    nonces: NonceGuard,
    pub(super) routes: BTreeMap<String, Arc<WorkloadHost>>,
    exchange: Arc<ArtifactExchange>,
    gate: Arc<dyn GateBackend>,
    chain: Option<Arc<ChainManager>>,
    facts: Mutex<Option<SignedNodeFacts>>,
    facts_source: Option<FactsSource>,
    address: Option<String>,
    pub(super) instances: tokio::sync::Mutex<HashMap<String, Placed>>,
    /// Decision ids of `place` / `load` requests being handled. A
    /// controller that lost a response reconciles against this and the
    /// instance list (see `plane_reconcile`).
    pub(super) in_flight: Mutex<HashSet<String>>,
    /// Bounds chain writes for requests that failed verification.
    verify_budget: RefusalBudget,
}

fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}

pub(super) fn refuse(code: RefusalCode, reason: impl Into<String>) -> Refusal {
    Refusal::new(code, reason)
}

pub(super) fn runtime_refusal(e: &RuntimeError) -> Refusal {
    let code = match e {
        RuntimeError::AdmissionRefused(_) => RefusalCode::Admission,
        RuntimeError::Governance(_) => RefusalCode::Governance,
        RuntimeError::UnknownInstance(_) => RefusalCode::UnknownInstance,
        RuntimeError::InvalidConfig(_) => RefusalCode::InvalidRequest,
        _ => RefusalCode::Runtime,
    };
    Refusal::new(code, e.to_string())
}

impl WorkloadHostService {
    /// Host for the node owning `key`. Serves nobody until controllers are
    /// added, and loads nothing until a route adapter is added.
    pub fn new(
        key: SigningKey,
        exchange: Arc<ArtifactExchange>,
        anchors: TrustAnchors,
        gate: Arc<dyn GateBackend>,
    ) -> Self {
        Self {
            node_id: node_id_from_pubkey(&key.verifying_key().to_bytes()),
            key,
            anchors,
            controllers: Box::new(Vec::<[u8; 32]>::new()),
            nonces: NonceGuard::new(),
            routes: BTreeMap::new(),
            exchange,
            gate,
            chain: None,
            facts: Mutex::new(None),
            facts_source: None,
            address: None,
            instances: tokio::sync::Mutex::new(HashMap::new()),
            in_flight: Mutex::new(HashSet::new()),
            verify_budget: RefusalBudget::default(),
        }
    }

    /// Replace the bound on chained refusals of unverified requests.
    pub fn with_refusal_budget(mut self, b: RefusalBudget) -> Self {
        self.verify_budget = b;
        self
    }

    /// Adapter (under governance) for one route kind (`native`, ...).
    pub fn with_route(mut self, route: &str, host: Arc<WorkloadHost>) -> Self {
        self.routes.insert(route.to_string(), host);
        self
    }

    /// Who may send requests.
    pub fn with_controllers(mut self, c: impl ControllerPolicy + 'static) -> Self {
        self.controllers = Box::new(c);
        self
    }

    /// Chain for placement and refusal records.
    pub fn with_chain(mut self, cm: Arc<ChainManager>) -> Self {
        self.chain = Some(cm);
        self
    }

    /// Mesh address advertised for this service.
    pub fn with_address(mut self, addr: impl Into<String>) -> Self {
        self.address = Some(addr.into());
        self
    }

    /// Replace the signed facts `describe` returns.
    pub fn set_facts(&self, facts: SignedNodeFacts) {
        if let Ok(mut f) = self.facts.lock() {
            *f = Some(facts);
        }
    }

    /// Read the signed facts from `source` on every `describe` (falling
    /// back to [`Self::set_facts`] when it has none).
    pub fn with_facts_source(mut self, source: FactsSource) -> Self {
        self.facts_source = Some(source);
        self
    }

    fn current_facts(&self) -> Option<SignedNodeFacts> {
        self.facts_source
            .as_ref()
            .and_then(|f| f())
            .or_else(|| self.facts.lock().ok().and_then(|f| f.clone()))
    }

    /// This node's id.
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    /// The `workload-host` advertisement (ADR-099 section 7).
    pub fn advertisement(&self) -> ServiceAdvertisement {
        let mut metadata = HashMap::new();
        let routes: Vec<&str> = self.routes.keys().map(String::as_str).collect();
        metadata.insert("routes".to_string(), routes.join(","));
        if let Some(a) = &self.address {
            metadata.insert("addr".to_string(), a.clone());
        }
        ServiceAdvertisement {
            name: WORKLOAD_HOST_SERVICE.to_string(),
            methods: method::ALL.iter().map(|m| m.to_string()).collect(),
            node_id: self.node_id.clone(),
            global_pid: GlobalPid::new(self.node_id.clone(), 0),
            version: CTL_VERSION.to_string(),
            contract_hash: None,
            metadata,
            last_updated: chrono::Utc::now().timestamp().max(0) as u64,
        }
    }

    fn record(&self, kind: &str, payload: Value) {
        if let Some(cm) = &self.chain {
            cm.append(HOST_CHAIN_SOURCE, kind, Some(payload));
        }
    }

    fn refused(&self, req: Option<&CtlRequest>, phase: &str, r: &Refusal) {
        // Unauthenticated refusals are chained within a budget only.
        let suppressed = match req {
            Some(_) => 0,
            None => match self.verify_budget.take() {
                Some(n) => n,
                None => return,
            },
        };
        self.record(
            chain::EVENT_KIND_WORKLOAD_REFUSE,
            json!({
                "node": self.node_id, "phase": phase, "code": r.code.as_str(),
                "reason": r.reason, "method": req.map(|q| q.method.as_str()),
                "requester": req.map(|q| q.requester.as_str()),
                "decision_id": req.and_then(|q| q.decision_id.as_deref()),
                "suppressed": suppressed,
            }),
        );
    }

    /// Handle one signed request (`fetch` is the connection back to the
    /// controller, used to fetch the payload before loading).
    pub async fn handle(
        &self,
        header_method: &str,
        signed: &SignedCtl,
        fetch: Option<&mut PeerSet>,
    ) -> SignedCtl {
        self.handle_checked(header_method, signed, fetch).await.0
    }

    /// [`Self::handle`], also saying whether the request was authenticated
    /// (a session drops a peer after an unauthenticated request).
    pub async fn handle_checked(
        &self,
        header_method: &str,
        signed: &SignedCtl,
        fetch: Option<&mut PeerSet>,
    ) -> (SignedCtl, bool) {
        let verified = verify_request(
            signed,
            &self.node_id,
            now_ms(),
            self.controllers.as_ref(),
            &self.nonces,
        )
        .and_then(|r| {
            if r.method == header_method {
                Ok(r)
            } else {
                Err(refuse(
                    RefusalCode::InvalidRequest,
                    "envelope method differs from signed method",
                ))
            }
        });
        let authenticated = verified.is_ok();
        let (nonce, method_name, outcome) = match verified {
            Err(r) => {
                self.refused(None, "verify", &r);
                // Unauthenticated nonce, only to bind the refusal.
                let claimed: Option<CtlRequest> = serde_json::from_str(&signed.payload).ok();
                let nonce = claimed.map(|c| c.nonce).unwrap_or_default();
                (nonce, header_method.to_string(), Err(r))
            }
            Ok(req) => {
                let out = self.dispatch(&req, fetch).await;
                if let Err(r) = &out {
                    self.refused(Some(&req), "handle", r);
                }
                (req.nonce, req.method, out)
            }
        };
        let resp = CtlResponse {
            version: CTL_VERSION,
            method: method_name,
            responder: self.node_id.clone(),
            request_nonce: nonce,
            outcome: match outcome {
                Ok(result) => CtlOutcome::Ok { result },
                Err(refusal) => CtlOutcome::Refused { refusal },
            },
        }
        .sign(&self.key);
        (resp, authenticated)
    }

    async fn dispatch(
        &self,
        req: &CtlRequest,
        fetch: Option<&mut PeerSet>,
    ) -> Result<Value, Refusal> {
        // A decision id is the chain hash of the controller's decision.
        let valid_id = req
            .decision_id
            .as_deref()
            .is_some_and(|d| crate::workload_pkg::codec::is_lower_hex(d, 64));
        if method::mutates(&req.method) && !valid_id {
            return Err(refuse(
                RefusalCode::InvalidRequest,
                "mutations carry a decision id (64 hex chain hash)",
            ));
        }
        match req.method.as_str() {
            method::DESCRIBE => {
                Ok(json!({ "facts": self.current_facts(), "advertisement": self.advertisement() }))
            }
            method::PLACE | method::LOAD => self.place(req, fetch).await,
            method::START | method::STOP | method::UNLOAD | method::STATUS | method::LOGS => {
                self.instance_op(req).await
            }
            m if m.starts_with(crate::workload_governance::policy::WORKLOAD_ACTION_PREFIX) => {
                // Not in the message set: ask the gate so the refusal is on
                // its audit trail, and refuse whatever it says.
                let ctx = json!({ "requester": req.requester, "method": m });
                let why = match self.gate.check(&req.requester, m, &ctx) {
                    GateDecision::Deny { reason, .. } => reason,
                    _ => "not a workload.ctl method".to_string(),
                };
                Err(refuse(RefusalCode::UnknownMethod, format!("{m}: {why}")))
            }
            m => Err(refuse(
                RefusalCode::InvalidRequest,
                format!("{m} is not a workload.ctl method"),
            )),
        }
    }

    async fn place(&self, req: &CtlRequest, fetch: Option<&mut PeerSet>) -> Result<Value, Refusal> {
        let _in_flight = InFlight::enter(&self.in_flight, req.decision_id.clone());
        let b: PlaceBody = serde_json::from_value(req.body.clone())
            .map_err(|e| refuse(RefusalCode::InvalidRequest, format!("place body: {e}")))?;
        if !valid_token(&b.name, 64)
            || !crate::workload_pkg::codec::is_lower_hex(&b.manifest_hash, 64)
        {
            return Err(refuse(
                RefusalCode::InvalidRequest,
                "bad name or manifest hash",
            ));
        }
        let route = route_of(&b.variant).to_string();
        let host = self.routes.get(&route).cloned().ok_or_else(|| {
            refuse(
                RefusalCode::Admission,
                format!("no {route} adapter on this node for {}", b.variant),
            )
        })?;
        let peers = fetch.ok_or_else(|| refuse(RefusalCode::Fetch, "no payload source"))?;
        let pkg = self
            .exchange
            .fetch_package(peers, &b.manifest_hash, &self.anchors)
            .await
            .map_err(|e| match e {
                PackageExchangeError::Verify(v) => refuse(RefusalCode::Verify, v.to_string()),
                other => refuse(RefusalCode::Fetch, other.to_string()),
            })?;
        let w =
            VerifiedWorkload::from_package(&pkg.verified, &StoreSource::new(self.exchange.store()))
                .map_err(|e| refuse(RefusalCode::Verify, e.to_string()))?;
        if w.id != b.name {
            return Err(refuse(
                RefusalCode::Verify,
                "package id differs from the placed name",
            ));
        }
        let cfg = WorkloadConfig {
            mode: b.config.mode.clone(),
            args: b.config.args.clone(),
            host: HostContract::new(SocketAddr::from(([0, 0, 0, 0], b.config.csi_port))),
            node_id: self.node_id.clone(),
        };
        let h = host.load(&w, &cfg).await.map_err(|e| runtime_refusal(&e))?;
        let iid = h.instance_id.clone();
        self.instances.lock().await.insert(
            iid.clone(),
            Placed {
                handle: h.clone(),
                route: route.clone(),
                name: b.name.clone(),
                variant: b.variant.clone(),
                decision_id: req.decision_id.clone(),
                last: None,
            },
        );
        let started = req.method == method::PLACE && b.start;
        if started && let Err(e) = host.start(&h).await {
            return Err(self.roll_back(&host, &h, &e).await);
        }
        let status = host.status(&h).await;
        self.record(
            chain::EVENT_KIND_WORKLOAD_PLACE,
            json!({
                "phase": "placed", "node": self.node_id, "requester": req.requester,
                "decision_id": req.decision_id, "workload": b.name, "variant": b.variant,
                "runtime": h.runtime, "instance_id": iid, "package_id": pkg.package_id,
                "started": started,
            }),
        );
        Ok(json!({
            "instance_id": iid, "runtime": h.runtime, "variant": b.variant,
            "package_id": pkg.package_id, "status": status, "node": self.node_id,
        }))
    }

    /// A start that failed after load: unload, so the target keeps no
    /// instance the controller was told was refused. If the unload fails
    /// too, the instance stays listed (and reconcilable) and the refusal
    /// says so.
    async fn roll_back(
        &self,
        host: &WorkloadHost,
        h: &InstanceHandle,
        e: &RuntimeError,
    ) -> Refusal {
        let mut r = runtime_refusal(e);
        let iid = h.instance_id.clone();
        match host.unload(h.clone()).await {
            Ok(()) => {
                self.instances.lock().await.remove(&iid);
                r.reason = format!("start failed: {}; loaded instance {iid} unloaded", r.reason);
            }
            Err(u) => {
                r.reason = format!(
                    "start failed: {}; unload of {iid} failed too ({u}), it stays listed",
                    r.reason
                );
            }
        }
        r
    }
}
