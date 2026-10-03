//! Per-instance tokens and the placement binding behind them.
//!
//! At place time the node registers the instance's `COGNITUM_COG_TOKEN`
//! together with the project that placed it. The registry keeps only the
//! BLAKE3 hash of each token. At unload the instance is revoked.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::forward::Forwarder;
use super::types::IngestError;
use crate::workload_runtime::HostContract;

/// What the bridge knows about a placed cog instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceBinding {
    /// Instance id on this node.
    pub instance_id: String,
    /// Project that placed the cog (the placement record's project id).
    /// `None`: the placing controller's store takes the vectors.
    pub project_id: Option<String>,
    /// Node id of the placing controller.
    pub controller_node: String,
}

impl InstanceBinding {
    /// Binding for a placement made by `controller_node` on behalf of
    /// `project_id`.
    pub fn new(
        instance_id: impl Into<String>,
        project_id: Option<String>,
        controller_node: impl Into<String>,
    ) -> Self {
        Self {
            instance_id: instance_id.into(),
            project_id,
            controller_node: controller_node.into(),
        }
    }

    /// Binding for a [`PlacementRecord`](crate::workload_ctl::PlacementRecord).
    pub fn from_record(
        rec: &crate::workload_ctl::PlacementRecord,
        project_id: Option<String>,
        controller_node: &str,
    ) -> Self {
        Self::new(rec.instance_id.clone(), project_id, controller_node)
    }
}

/// Registry of live instances, by token hash.
#[derive(Default)]
pub struct TokenRegistry {
    inner: Mutex<RegInner>,
}

#[derive(Default)]
struct RegInner {
    by_hash: HashMap<[u8; 32], InstanceBinding>,
    by_instance: HashMap<String, [u8; 32]>,
}

fn hash(token: &str) -> [u8; 32] {
    *blake3::hash(token.as_bytes()).as_bytes()
}

impl TokenRegistry {
    /// Empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an instance with the token of its host contract.
    pub fn register(
        &self,
        binding: InstanceBinding,
        contract: &HostContract,
    ) -> Result<(), IngestError> {
        contract
            .validate()
            .map_err(|e| IngestError::Malformed(e.to_string()))?;
        let h = hash(contract.token.expose());
        let mut g = self.inner.lock().map_err(|_| poisoned())?;
        if g.by_instance.contains_key(&binding.instance_id) || g.by_hash.contains_key(&h) {
            return Err(IngestError::Malformed(
                "instance or token already registered".into(),
            ));
        }
        g.by_instance.insert(binding.instance_id.clone(), h);
        g.by_hash.insert(h, binding);
        Ok(())
    }

    /// Revoke an instance's token. True if it was registered.
    pub fn revoke(&self, instance_id: &str) -> bool {
        let Ok(mut g) = self.inner.lock() else {
            return false;
        };
        match g.by_instance.remove(instance_id) {
            Some(h) => g.by_hash.remove(&h).is_some(),
            None => false,
        }
    }

    /// The binding a presented token belongs to.
    pub fn lookup(&self, token: &str) -> Option<InstanceBinding> {
        self.inner.lock().ok()?.by_hash.get(&hash(token)).cloned()
    }

    /// True if `instance_id` currently has a token.
    pub fn contains(&self, instance_id: &str) -> bool {
        self.inner
            .lock()
            .map(|g| g.by_instance.contains_key(instance_id))
            .unwrap_or(false)
    }

    /// Live instances.
    pub fn len(&self) -> usize {
        self.inner.lock().map(|g| g.by_instance.len()).unwrap_or(0)
    }

    /// True when no instance is registered.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn poisoned() -> IngestError {
    IngestError::Unavailable("registry lock poisoned".into())
}

/// Per-instance request and vector budgets over a fixed window.
pub struct RateBudget {
    window: Duration,
    max_requests: u32,
    max_vectors: u32,
    used: Mutex<HashMap<String, (Instant, u32, u32)>>,
}

impl RateBudget {
    /// `max_requests` and `max_vectors` per instance per `window`.
    pub fn new(window: Duration, max_requests: u32, max_vectors: u32) -> Self {
        Self {
            window,
            max_requests,
            max_vectors,
            used: Mutex::new(HashMap::new()),
        }
    }

    /// Charge one request (before the body is read).
    pub fn charge_request(&self, instance: &str) -> bool {
        self.charge(instance, 1, 0)
    }

    /// Charge `n` vectors (after the body is validated).
    pub fn charge_vectors(&self, instance: &str, n: usize) -> bool {
        self.charge(instance, 0, u32::try_from(n).unwrap_or(u32::MAX))
    }

    fn charge(&self, instance: &str, reqs: u32, vecs: u32) -> bool {
        let Ok(mut m) = self.used.lock() else {
            return false;
        };
        let now = Instant::now();
        let e = m.entry(instance.to_string()).or_insert((now, 0, 0));
        if now.duration_since(e.0) >= self.window {
            *e = (now, 0, 0);
        }
        if e.1.saturating_add(reqs) > self.max_requests || e.2.saturating_add(vecs) > self.max_vectors
        {
            return false;
        }
        e.1 += reqs;
        e.2 += vecs;
        true
    }

    /// Drop an instance's counters (at unload).
    pub fn forget(&self, instance: &str) {
        if let Ok(mut m) = self.used.lock() {
            m.remove(instance);
        }
    }
}

impl Default for RateBudget {
    /// 20 requests and 2048 vectors per second per instance.
    fn default() -> Self {
        Self::new(Duration::from_secs(1), 20, 2048)
    }
}

/// Decides which forwarder (store owner) takes an instance's batches.
pub trait StoreRouter: Send + Sync {
    /// Forwarder for `b`, or `None` when no store owner is known.
    fn route(&self, b: &InstanceBinding) -> Option<Arc<dyn Forwarder>>;
}

/// Router over fixed maps. A binding with a project routes to that
/// project's owner and to nothing else: when the project has no route the
/// batch is refused, it is never redirected to the controller's store. A
/// binding without a project routes to its placing controller's store.
#[derive(Default)]
pub struct StaticRouter {
    projects: Mutex<HashMap<String, Arc<dyn Forwarder>>>,
    controllers: Mutex<HashMap<String, Arc<dyn Forwarder>>>,
}

impl StaticRouter {
    /// Empty router.
    pub fn new() -> Self {
        Self::default()
    }

    /// Route a project's batches.
    pub fn with_project_route(self, project_id: &str, f: Arc<dyn Forwarder>) -> Self {
        if let Ok(mut m) = self.projects.lock() {
            m.insert(project_id.to_string(), f);
        }
        self
    }

    /// Route project-less batches placed by `controller_node`.
    pub fn with_controller(self, controller_node: &str, f: Arc<dyn Forwarder>) -> Self {
        if let Ok(mut m) = self.controllers.lock() {
            m.insert(controller_node.to_string(), f);
        }
        self
    }
}

impl StoreRouter for StaticRouter {
    fn route(&self, b: &InstanceBinding) -> Option<Arc<dyn Forwarder>> {
        match &b.project_id {
            Some(p) => self.projects.lock().ok()?.get(p).cloned(),
            None => self.controllers.lock().ok()?.get(&b.controller_node).cloned(),
        }
    }
}
