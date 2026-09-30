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

use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
use super::msg::{
    CTL_VERSION, ControllerPolicy, CtlOutcome, CtlRequest, CtlResponse, NonceGuard, Refusal,
    RefusalCode, SignedCtl, WORKLOAD_HOST_SERVICE, method, verify_request,
};

/// Chain source for target-side placement records.
pub const HOST_CHAIN_SOURCE: &str = "workload.host";
/// Largest captured output a `logs` answer carries.
const MAX_LOG_BYTES: usize = 64 * 1024;

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

struct Placed {
    handle: InstanceHandle,
    route: String,
    name: String,
    variant: String,
    decision_id: Option<String>,
    last: Option<RunEvidence>,
}

/// A node's `workload-host`.
pub struct WorkloadHostService {
    node_id: String,
    key: SigningKey,
    anchors: TrustAnchors,
    controllers: Box<dyn ControllerPolicy>,
    nonces: NonceGuard,
    routes: BTreeMap<String, Arc<WorkloadHost>>,
    exchange: Arc<ArtifactExchange>,
    gate: Arc<dyn GateBackend>,
    chain: Option<Arc<ChainManager>>,
    facts: Mutex<Option<SignedNodeFacts>>,
    address: Option<String>,
    instances: tokio::sync::Mutex<HashMap<String, Placed>>,
}

fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}

fn refuse(code: RefusalCode, reason: impl Into<String>) -> Refusal {
    Refusal::new(code, reason)
}

fn runtime_refusal(e: &RuntimeError) -> Refusal {
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
            address: None,
            instances: tokio::sync::Mutex::new(HashMap::new()),
        }
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
        self.record(
            chain::EVENT_KIND_WORKLOAD_REFUSE,
            json!({
                "node": self.node_id, "phase": phase, "code": r.code.as_str(),
                "reason": r.reason, "method": req.map(|q| q.method.as_str()),
                "requester": req.map(|q| q.requester.as_str()),
                "decision_id": req.and_then(|q| q.decision_id.as_deref()),
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
        CtlResponse {
            version: CTL_VERSION,
            method: method_name,
            responder: self.node_id.clone(),
            request_nonce: nonce,
            outcome: match outcome {
                Ok(result) => CtlOutcome::Ok { result },
                Err(refusal) => CtlOutcome::Refused { refusal },
            },
        }
        .sign(&self.key)
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
                let facts = self.facts.lock().ok().and_then(|f| f.clone());
                Ok(json!({ "facts": facts, "advertisement": self.advertisement() }))
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
        if started {
            host.start(&h).await.map_err(|e| runtime_refusal(&e))?;
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

    async fn instance_op(&self, req: &CtlRequest) -> Result<Value, Refusal> {
        let b: InstanceBody = serde_json::from_value(req.body.clone())
            .map_err(|e| refuse(RefusalCode::InvalidRequest, format!("instance body: {e}")))?;
        let mut map = self.instances.lock().await;
        let Some(iid) = b.instance_id else {
            if req.method != method::STATUS {
                return Err(refuse(RefusalCode::InvalidRequest, "instance_id required"));
            }
            let mut all = Vec::new();
            for (id, p) in map.iter() {
                let host = &self.routes[&p.route];
                all.push(json!({
                    "instance_id": id, "workload": p.name, "variant": p.variant,
                    "decision_id": p.decision_id, "status": host.status(&p.handle).await,
                }));
            }
            return Ok(Value::Array(all));
        };
        let p = map
            .get_mut(&iid)
            .ok_or_else(|| refuse(RefusalCode::UnknownInstance, format!("no instance {iid}")))?;
        let host = self.routes[&p.route].clone();
        match req.method.as_str() {
            method::START => host.start(&p.handle).await.map(|_| json!({"started": iid})),
            method::STOP => {
                let grace = Duration::from_millis(b.grace_ms.unwrap_or(2_000).min(60_000));
                host.stop(&p.handle, grace).await.map(|ev| {
                    let audit = ev.audit();
                    p.last = Some(ev);
                    json!({ "stopped": iid, "evidence": audit })
                })
            }
            method::UNLOAD => {
                let h = p.handle.clone();
                let r = host.unload(h).await.map(|_| json!({ "unloaded": iid }));
                if r.is_ok() {
                    map.remove(&iid);
                }
                r
            }
            method::STATUS => {
                Ok(json!({ "instance_id": iid, "status": host.status(&p.handle).await }))
            }
            _ => Ok(match &p.last {
                Some(ev) => json!({
                    "instance_id": iid,
                    "stdout": clip(&ev.stdout), "stderr": clip(&ev.stderr),
                    "exit_code": ev.exit_code, "truncated": ev.truncated,
                }),
                None => json!({ "instance_id": iid, "note": "no captured run yet (stop first)" }),
            }),
        }
        .map_err(|e| runtime_refusal(&e))
    }
}

fn clip(s: &str) -> &str {
    let mut end = s.len().min(MAX_LOG_BYTES);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}
