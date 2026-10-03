//! The placement table: which node serves `infer.<role>` right now.
//!
//! Local instances come from the card-18 adapter ([`InferRuntime`]:
//! `reconcile`, `health`, `endpoint`); remote ones from `infer.<role>`
//! [`ServiceAdvertisement`]s of admitted peers. Resolution prefers the
//! local instance (sticky: warm KV), then the first admitted peer in node-id
//! order. Every change bumps a generation so caches (the `ProviderRouter`
//! TTL cache) can be invalidated.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, RwLock};

use tokio::sync::watch;

use super::types::{MeshDialer, ProxyAudit, ProxyError, Target};
use super::upstream::parse_loopback_base;
use super::wire::MAX_ROLE;
use crate::ipc::GlobalPid;
use crate::mesh_service_adv::ServiceAdvertisement;
use crate::workload_pkg::manifest::valid_token;
use crate::workload_runtime::infer::{Health, InferRuntime, Reconcile};
use crate::workload_runtime::types::InstanceHandle;

/// Prefix of the advertised service name.
pub const SERVICE_PREFIX: &str = "infer.";

#[derive(Debug, Clone)]
struct LocalEntry {
    base: String,
    port: u16,
    model: Option<String>,
    api: String,
    runtime: String,
}

#[derive(Default)]
struct Inner {
    local: HashMap<String, LocalEntry>,
    remote: HashMap<String, BTreeMap<String, ServiceAdvertisement>>,
    exposed: HashSet<String>,
    proxy_ports: HashMap<String, u16>,
}

/// What [`PlacementTable::sync_local`] found.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncOutcome {
    /// Whether the role is now served locally.
    pub registered: bool,
    /// What reconcile did (restart, give up, ...), for the caller to chain.
    pub reconcile: Option<Reconcile>,
    /// Health seen.
    pub health: Option<Health>,
}

/// Role to serving node.
pub struct PlacementTable {
    node_id: String,
    dialer: Option<Arc<dyn MeshDialer>>,
    audit: Option<Arc<dyn ProxyAudit>>,
    inner: RwLock<Inner>,
    gen_tx: watch::Sender<u64>,
}

fn valid_role(role: &str) -> bool {
    valid_token(role, MAX_ROLE) && !role.contains('/')
}

impl PlacementTable {
    /// Table for node `node_id`. Without a dialer there is no mesh: only
    /// local instances resolve.
    pub fn new(
        node_id: impl Into<String>,
        dialer: Option<Arc<dyn MeshDialer>>,
        audit: Option<Arc<dyn ProxyAudit>>,
    ) -> Self {
        Self {
            node_id: node_id.into(),
            dialer,
            audit,
            inner: RwLock::new(Inner::default()),
            gen_tx: watch::channel(0).0,
        }
    }

    /// This node's id.
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    /// The mesh dialer, when there is one.
    pub fn dialer(&self) -> Option<&Arc<dyn MeshDialer>> {
        self.dialer.as_ref()
    }

    /// Changes so far. A router cache keyed on this is never stale.
    pub fn generation(&self) -> u64 {
        *self.gen_tx.borrow()
    }

    /// Wakes on every change (`invalidate_all_placement` on it).
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.gen_tx.subscribe()
    }

    fn bump(&self) {
        self.gen_tx.send_modify(|g| *g += 1);
    }

    fn audit(&self, kind: &str, payload: serde_json::Value) {
        if let Some(a) = &self.audit {
            a.record(kind, payload);
        }
    }

    /// Serve `role` from the loopback server at `base`. Refused when the
    /// endpoint is not `http://<loopback ip>:<port>`.
    pub fn register_local(
        &self,
        role: &str,
        base: &str,
        model: Option<String>,
        api: &str,
        runtime: &str,
    ) -> Result<(), ProxyError> {
        if !valid_role(role) {
            return Err(ProxyError::BadRequest(format!("bad role '{role}'")));
        }
        let (_, port) = parse_loopback_base(base)?;
        let entry = LocalEntry {
            base: base.trim_end_matches('/').to_string(),
            port,
            model,
            api: api.to_string(),
            runtime: runtime.to_string(),
        };
        let changed = {
            let mut g = self.inner.write().unwrap();
            let prev = g.local.insert(role.to_string(), entry.clone());
            prev.is_none_or(|p| p.base != entry.base)
        };
        if changed {
            self.bump();
        }
        Ok(())
    }

    /// Stop serving `role` locally.
    pub fn deregister_local(&self, role: &str) {
        let had = self.inner.write().unwrap().local.remove(role).is_some();
        if had {
            self.bump();
        }
    }

    /// Re-evaluate `role` against its adapter instance: reconcile (a dead
    /// managed server is restarted within its policy), probe, and serve it
    /// only while it answers.
    pub async fn sync_local(
        &self,
        role: &str,
        rt: &InferRuntime,
        h: &InstanceHandle,
    ) -> SyncOutcome {
        let reconcile = rt.reconcile(h).await.ok();
        let health = rt.health(h).await.ok().map(|r| r.health);
        let endpoint = rt.endpoint(h).await;
        let spec = rt.spec_of(h).await;
        let up = matches!(health, Some(Health::Up));
        let mut registered = false;
        if let (true, Some(base), Some(spec)) = (up, endpoint, spec) {
            registered = self
                .register_local(
                    role,
                    &base,
                    spec.model.clone(),
                    &spec.api,
                    &format!("{:?}", spec.runtime),
                )
                .is_ok();
        }
        if !registered {
            self.deregister_local(role);
        }
        SyncOutcome {
            registered,
            reconcile,
            health,
        }
    }

    /// Allow or stop serving `role` to mesh peers. Beyond loopback this is
    /// a governed decision (ADR-101 section 7): the caller decides, this
    /// records it.
    pub fn expose_to_mesh(&self, role: &str, on: bool) {
        let changed = {
            let mut g = self.inner.write().unwrap();
            if on {
                g.exposed.insert(role.to_string())
            } else {
                g.exposed.remove(role)
            }
        };
        if changed {
            self.audit(
                "infer.mesh.expose",
                serde_json::json!({"role": role, "node_id": self.node_id, "exposed": on}),
            );
            self.bump();
        }
    }

    /// The loopback proxy port that fronts `role` on this node.
    pub fn set_proxy_port(&self, role: &str, port: u16) {
        self.inner
            .write()
            .unwrap()
            .proxy_ports
            .insert(role.to_string(), port);
    }

    /// Take a peer's `infer.<role>` advertisement. Only admitted, verified
    /// peers are heard; anything else is dropped.
    pub fn ingest_advertisement(&self, ad: &ServiceAdvertisement) -> bool {
        let Some(role) = ad.name.strip_prefix(SERVICE_PREFIX) else {
            return false;
        };
        if !valid_role(role) || ad.node_id == self.node_id {
            return false;
        }
        let admitted = self.dialer.as_ref().is_some_and(|d| d.is_admitted(&ad.node_id));
        if !admitted {
            self.audit(
                "infer.advert.refused",
                serde_json::json!({"role": role, "node_id": ad.node_id, "why": "peer not admitted"}),
            );
            return false;
        }
        let changed = {
            let mut g = self.inner.write().unwrap();
            let per = g.remote.entry(role.to_string()).or_default();
            match per.get(&ad.node_id) {
                Some(old) if old.last_updated >= ad.last_updated => false,
                _ => {
                    per.insert(ad.node_id.clone(), ad.clone());
                    true
                }
            }
        };
        if changed {
            self.bump();
        }
        changed
    }

    /// A peer left or was revoked: forget what it advertised.
    pub fn remove_node(&self, node_id: &str) {
        let changed = {
            let mut g = self.inner.write().unwrap();
            let mut any = false;
            for per in g.remote.values_mut() {
                any |= per.remove(node_id).is_some();
            }
            g.remote.retain(|_, per| !per.is_empty());
            any
        };
        if changed {
            self.bump();
        }
    }

    /// Where `role` is served: this node first, then an admitted peer.
    /// Admission is re-checked here, so a revoked peer stops resolving at
    /// once even before its entry is removed.
    pub fn resolve(&self, role: &str) -> Option<Target> {
        let g = self.inner.read().unwrap();
        if let Some(l) = g.local.get(role) {
            return Some(Target::Local {
                base: l.base.clone(),
            });
        }
        let dialer = self.dialer.as_ref()?;
        g.remote
            .get(role)?
            .keys()
            .find(|n| dialer.is_admitted(n))
            .map(|n| Target::Remote { node_id: n.clone() })
    }

    /// The local instance of `role`, only when it is exposed to the mesh:
    /// what a peer's forwarded request may reach. Never a remote target, so
    /// a request cannot be bounced from node to node.
    pub fn local_for_mesh(&self, role: &str) -> Option<String> {
        let g = self.inner.read().unwrap();
        g.exposed
            .contains(role)
            .then(|| g.local.get(role).map(|l| l.base.clone()))
            .flatten()
    }

    /// Base URL for in-process consumers (`PlacementResolver`): the
    /// instance itself when it is local, this node's loopback proxy when it
    /// is remote (the proxy speaks the mesh). `None` falls back.
    pub fn base_url_for_role(&self, role: &str) -> Option<String> {
        match self.resolve(role)? {
            Target::Local { base } => Some(format!("{base}/v1")),
            Target::Remote { .. } => {
                let port = *self.inner.read().unwrap().proxy_ports.get(role)?;
                Some(format!("http://127.0.0.1:{port}/v1"))
            }
        }
    }

    /// `infer.<role>` advertisement for a locally served role that is
    /// exposed to the mesh.
    pub fn advertisement(&self, role: &str, now_secs: u64) -> Option<ServiceAdvertisement> {
        let g = self.inner.read().unwrap();
        let l = g.local.get(role).filter(|_| g.exposed.contains(role))?;
        let mut metadata = HashMap::new();
        metadata.insert("role".to_string(), role.to_string());
        metadata.insert("node_id".to_string(), self.node_id.clone());
        metadata.insert("port".to_string(), l.port.to_string());
        metadata.insert("api".to_string(), l.api.clone());
        metadata.insert("runtime".to_string(), l.runtime.clone());
        metadata.insert("load".to_string(), "0".to_string());
        if let Some(m) = &l.model {
            metadata.insert("model".to_string(), m.clone());
        }
        Some(ServiceAdvertisement {
            name: format!("{SERVICE_PREFIX}{role}"),
            methods: ["chat.completions", "completions", "embeddings", "models"]
                .map(String::from)
                .to_vec(),
            node_id: self.node_id.clone(),
            global_pid: GlobalPid::local(0, &self.node_id),
            version: "1".to_string(),
            contract_hash: None,
            metadata,
            last_updated: now_secs,
        })
    }

    /// Advertisements of every exposed local role.
    pub fn advertisements(&self, now_secs: u64) -> Vec<ServiceAdvertisement> {
        let roles: Vec<String> = self.inner.read().unwrap().local.keys().cloned().collect();
        roles
            .iter()
            .filter_map(|r| self.advertisement(r, now_secs))
            .collect()
    }
}
