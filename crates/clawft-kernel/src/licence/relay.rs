//! The steward's checkout relay (ADR-106 section 4, steps 2, 3 and 5).
//!
//! A `mesh.cog.checkout` request is accepted only from the steward's own
//! kernel or a verified `node` peer (the service-stamped `AdmittedPeer`),
//! after the governance gate permits `cog.checkout`. Concurrent requests for
//! one (cog, version, arch) share a single relay call. The steward forwards
//! to `weft-licence`, verifies the returned grant against the bound key and
//! the mesh id, checks the bytes against the signed sha256 and BLAKE3, seeds
//! them, registers the grant (which makes the bytes shareable through
//! [`crate::mesh_artifact::ArtifactExchange::grant_checkout`]), chains the
//! outcome and floods the grant through a [`GrantFlood`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::{OnceCell, Semaphore};

use super::client::{CheckoutWire, LicenceClient, LicenceClientError};
use super::{
    CheckoutGrant, CheckoutGrantStore, LicenceError, Outcome, SignedGrant, sha256_hex, verify_grant,
};
use crate::chain::ChainManager;
use crate::gate::{GateBackend, GateDecision};
use crate::mesh_admit::PeerClass;
use crate::mesh_artifact::ArtifactExchange;
use crate::mesh_artifact_types::ArtifactKey;
use crate::mesh_delivery::PeerCtx;
use crate::workload_pkg::codec::{hex_decode_exact, hex_encode};

/// Gate action asked for every checkout request.
pub const GATE_ACTION: &str = "cog.checkout";
/// Chain event: a grant was obtained and seeded.
pub const EVENT_KIND_CHECKOUT_GRANTED: &str = "cog.checkout.granted";
/// Chain event: a checkout request was refused or failed.
pub const EVENT_KIND_CHECKOUT_REFUSED: &str = "cog.checkout.refused";

/// Who is asking.
#[derive(Debug, Clone, Copy)]
pub enum CheckoutCaller<'a> {
    /// This node's own kernel.
    Kernel,
    /// A delivery from a mesh peer; the context is built from the service
    /// origin, so `node_verified` means `AdmittedPeer`.
    Peer(&'a PeerCtx),
}

/// Why a checkout was refused or failed. [`CheckoutRefusal::code`] is the
/// stable code a requester sees.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CheckoutRefusal {
    /// The caller is not the steward's kernel or a verified node peer
    /// (`LocalTenant`, `Unadmitted`, a leaf, a legacy peer).
    #[error("caller is not an admitted node")]
    NotAdmitted,
    /// The governance gate did not permit `cog.checkout`.
    #[error("governance gate refused: {0}")]
    GateDenied(String),
    /// The request is not well formed.
    #[error("bad request: {0}")]
    BadRequest(String),
    /// `weft-licence` refused with a stable code.
    #[error("licence refused: {0}")]
    Licence(String),
    /// The link to `weft-licence` failed.
    #[error("licence link failed: {0}")]
    LicenceLink(String),
    /// The returned grant did not verify.
    #[error("returned grant did not verify: {0}")]
    BadGrant(String),
    /// The Seed's clock is ahead of this node's by more than the skew allowance.
    #[error("the Seed's clock is ahead of this node's")]
    SeedClockSkew,
    /// The returned bytes do not match the signed sizes or hashes.
    #[error("artifact does not match the grant: {0}")]
    ArtifactMismatch(String),
    /// This member asked too often (the steward's per-member budget).
    #[error("too many checkout requests from this member")]
    RateLimited,
    /// The steward has too many checkouts in flight.
    #[error("the steward is busy")]
    Busy,
    /// The node could not store the bytes.
    #[error("could not store the artifact: {0}")]
    Storage(String),
}

impl CheckoutRefusal {
    /// The stable wire code.
    pub fn code(&self) -> String {
        match self {
            Self::NotAdmitted => "not_admitted".into(),
            Self::GateDenied(_) => "gate_denied".into(),
            Self::BadRequest(_) => "bad_request".into(),
            Self::Licence(c) => c.clone(),
            Self::LicenceLink(_) => "licence_unreachable".into(),
            Self::BadGrant(_) => "bad_grant".into(),
            Self::SeedClockSkew => "seed_clock_skew".into(),
            Self::ArtifactMismatch(_) => "artifact_mismatch".into(),
            Self::Storage(_) => "storage_failed".into(),
            Self::RateLimited => "rate_limited".into(),
            Self::Busy => "busy".into(),
        }
    }
}

impl From<LicenceClientError> for CheckoutRefusal {
    fn from(e: LicenceClientError) -> Self {
        match e {
            LicenceClientError::Refused { code, .. } => Self::Licence(code),
            LicenceClientError::Transport(w) => Self::LicenceLink(w),
            LicenceClientError::BadResponse(w) => Self::BadGrant(w),
        }
    }
}

/// Spreads a new grant to the mesh. Phase 1b replaces the implementation with
/// the `mesh.cog.grant` flood and sync; the relay only calls this.
#[async_trait]
pub trait GrantFlood: Send + Sync + 'static {
    /// Flood `grant` to the admitted peers. Best effort: the requester also
    /// gets the grant in its reply, and sync catches the rest up.
    async fn flood(&self, grant: &SignedGrant);
}

/// Floods nothing.
#[derive(Debug, Default)]
pub struct NoFlood;

#[async_trait]
impl GrantFlood for NoFlood {
    async fn flood(&self, _: &SignedGrant) {}
}

/// Register `signed` in `store` and, when accepted, make the bytes of every
/// currently valid grant for the cog version shareable in `exchange`. This is
/// the member side of a grant arriving (reply, flood or sync).
pub fn install_grant(
    store: &CheckoutGrantStore,
    exchange: &ArtifactExchange,
    signed: &SignedGrant,
) -> Result<Outcome, LicenceError> {
    let out = store.accept_grant(signed)?;
    // Accepted, so the payload verified and is canonical JSON.
    let g: CheckoutGrant = serde_json::from_str(&signed.payload)
        .map_err(|e| LicenceError::Malformed(e.to_string()))?;
    for v in store.verified_grants() {
        if v.grant().cog_id == g.cog_id && v.grant().version == g.version {
            exchange.grant_checkout(&v);
        }
    }
    Ok(out)
}

/// Limits on what members can cost the steward (ADR-106 section 6): a request
/// budget per member, and a bound on checkouts running at once. Over-limit
/// refusals are not chained, so they cannot grow the chain either.
#[derive(Debug, Clone)]
pub struct RelayLimits {
    /// Requests per member per window.
    pub per_peer: u32,
    /// The window.
    pub window: Duration,
    /// Checkouts in flight at once.
    pub concurrent: usize,
    /// Members remembered at once; the oldest windows are pruned.
    pub max_peers: usize,
}

impl Default for RelayLimits {
    fn default() -> Self {
        Self { per_peer: 5, window: Duration::from_secs(60), concurrent: 8, max_peers: 1024 }
    }
}

type Key = (String, String, String);
type Shared = Arc<OnceCell<Result<SignedGrant, CheckoutRefusal>>>;

/// The relay.
pub struct CheckoutRelay {
    store: Arc<CheckoutGrantStore>,
    exchange: Arc<ArtifactExchange>,
    client: Arc<dyn LicenceClient>,
    gate: Arc<dyn GateBackend>,
    flood: Arc<dyn GrantFlood>,
    chain: Option<Arc<ChainManager>>,
    inflight: Mutex<HashMap<Key, Shared>>,
    limits: RelayLimits,
    buckets: Mutex<HashMap<String, (Instant, u32)>>,
    slots: Semaphore,
}

impl CheckoutRelay {
    /// A relay. The gate is required: with no gate nothing is permitted.
    pub fn new(
        store: Arc<CheckoutGrantStore>,
        exchange: Arc<ArtifactExchange>,
        client: Arc<dyn LicenceClient>,
        gate: Arc<dyn GateBackend>,
        flood: Arc<dyn GrantFlood>,
        chain: Option<Arc<ChainManager>>,
    ) -> Self {
        let limits = RelayLimits::default();
        Self {
            store,
            exchange,
            client,
            gate,
            flood,
            chain,
            inflight: Mutex::default(),
            slots: Semaphore::new(limits.concurrent),
            buckets: Mutex::default(),
            limits,
        }
    }

    /// Replace the limits (before the relay is shared).
    pub fn with_limits(mut self, limits: RelayLimits) -> Self {
        self.slots = Semaphore::new(limits.concurrent);
        self.limits = limits;
        self
    }

    /// Charge one request to `peer`; false when over its budget.
    fn allow_peer(&self, peer: &str) -> bool {
        let now = Instant::now();
        let mut m = self.buckets.lock().unwrap_or_else(|p| p.into_inner());
        if m.len() >= self.limits.max_peers && !m.contains_key(peer) {
            let w = self.limits.window;
            m.retain(|_, (start, _)| now.duration_since(*start) < w);
            if m.len() >= self.limits.max_peers {
                return false;
            }
        }
        let e = m.entry(peer.to_owned()).or_insert((now, 0));
        if now.duration_since(e.0) >= self.limits.window {
            *e = (now, 0);
        }
        if e.1 >= self.limits.per_peer {
            return false;
        }
        e.1 += 1;
        true
    }

    /// Handle one checkout request. Admission, then the gate, then the merged
    /// relay call.
    pub async fn handle(
        &self,
        caller: CheckoutCaller<'_>,
        req: &CheckoutWire,
    ) -> Result<SignedGrant, CheckoutRefusal> {
        let who = match caller {
            CheckoutCaller::Kernel => "kernel".to_owned(),
            CheckoutCaller::Peer(p) if p.node_verified && p.class == PeerClass::Node => {
                p.peer_id.clone()
            }
            CheckoutCaller::Peer(_) => {
                let e = CheckoutRefusal::NotAdmitted;
                self.chain_refused(req, "unadmitted", &e);
                return Err(e);
            }
        };
        // Limits come before the gate and the chain: a flood of requests
        // costs a counter, not a chain event.
        if matches!(caller, CheckoutCaller::Peer(_)) && !self.allow_peer(&who) {
            return Err(CheckoutRefusal::RateLimited);
        }
        let Ok(_slot) = self.slots.try_acquire() else {
            return Err(CheckoutRefusal::Busy);
        };
        let r = self.checked(&who, req).await;
        if let Err(e) = &r {
            self.chain_refused(req, &who, e);
        }
        r
    }

    async fn checked(&self, who: &str, req: &CheckoutWire) -> Result<SignedGrant, CheckoutRefusal> {
        req.validate().map_err(CheckoutRefusal::BadRequest)?;
        let ctx = serde_json::json!({
            "cog_id": req.cog_id, "version": req.version, "arch": req.arch,
        });
        match self.gate.check(who, GATE_ACTION, &ctx) {
            GateDecision::Permit { .. } => {}
            GateDecision::Defer { reason } | GateDecision::Deny { reason, .. } => {
                return Err(CheckoutRefusal::GateDenied(reason));
            }
        }
        let key = (req.cog_id.clone(), req.version.clone(), req.arch.clone());
        let cell: Shared = {
            let mut m = self.inflight.lock().unwrap_or_else(|p| p.into_inner());
            m.entry(key.clone()).or_default().clone()
        };
        let ran = AtomicBool::new(false);
        let out = cell
            .get_or_init(|| async {
                ran.store(true, Ordering::Relaxed);
                let r = self.relay(req).await;
                // Later requests start a fresh relay call; waiters on this
                // cell still read the stored result.
                let mut m = self.inflight.lock().unwrap_or_else(|p| p.into_inner());
                if m.get(&key).is_some_and(|c| Arc::ptr_eq(c, &cell)) {
                    m.remove(&key);
                }
                r
            })
            .await;
        // Every caller is chained as a grantee, the merged ones too.
        if let Ok(signed) = out
            && let Ok(g) = serde_json::from_str::<CheckoutGrant>(&signed.payload)
        {
            let b3 = g.artifact(&req.arch).map(|a| a.blake3.clone()).unwrap_or_default();
            self.chain_granted(who, req, &g, &b3, !ran.load(Ordering::Relaxed));
        }
        out.clone()
    }

    async fn relay(&self, req: &CheckoutWire) -> Result<SignedGrant, CheckoutRefusal> {
        let signed = self.client.checkout(req).await?;
        let binding = self
            .store
            .active_binding()
            .ok_or_else(|| CheckoutRefusal::BadGrant("no binding".into()))?;
        let pk = hex_decode_exact::<32>(&binding.grant_pubkey)
            .ok_or_else(|| CheckoutRefusal::BadGrant("bound grant key".into()))?;
        let mesh = self
            .store
            .local_mesh_id()
            .get()
            .ok_or_else(|| CheckoutRefusal::BadGrant("no local mesh id".into()))?;
        let g = verify_grant(&signed, &pk, &mesh).map_err(|e| CheckoutRefusal::BadGrant(e.to_string()))?;
        if g.cog_id != req.cog_id || (req.version != "latest" && g.version != req.version) {
            return Err(CheckoutRefusal::BadGrant("grant is for another cog or version".into()));
        }
        let art = g
            .artifact(&req.arch)
            .ok_or_else(|| CheckoutRefusal::BadGrant(format!("grant has no {} artifact", req.arch)))?
            .clone();
        let hash = hex_decode_exact::<32>(&art.blake3)
            .ok_or_else(|| CheckoutRefusal::BadGrant("artifact hash".into()))?;
        match self.exchange.resolve(&ArtifactKey::Content(hash)) {
            // Held already: the BLAKE3 was checked on the way in, but the grant
            // also commits to the registry sha256 of these bytes.
            Some(d) => self.check_held_sha256(&d, &art.sha256)?,
            None => self.fetch_and_seed(&art.blake3, &art.sha256, art.size).await?,
        }
        install_grant(&self.store, &self.exchange, &signed).map_err(|e| match e {
            LicenceError::NotYetValid => CheckoutRefusal::SeedClockSkew,
            e => CheckoutRefusal::BadGrant(e.to_string()),
        })?;
        self.flood.flood(&signed).await;
        Ok(signed)
    }

    fn check_held_sha256(
        &self,
        d: &crate::mesh_artifact_types::ArtifactDescriptor,
        sha256: &str,
    ) -> Result<(), CheckoutRefusal> {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        self.exchange
            .read_to(&d.id(), &mut |b| {
                h.update(b);
                Ok(())
            })
            .map_err(|e| CheckoutRefusal::Storage(e.to_string()))?;
        if hex_encode(&h.finalize()) != sha256 {
            return Err(CheckoutRefusal::ArtifactMismatch("sha256 of the held bytes".into()));
        }
        Ok(())
    }

    async fn fetch_and_seed(&self, blake3: &str, sha256: &str, size: u64) -> Result<(), CheckoutRefusal> {
        let limit = self.exchange.config().max_artifact_bytes;
        if size > limit {
            return Err(CheckoutRefusal::ArtifactMismatch(format!("{size} bytes is over the limit")));
        }
        let bytes = self.client.artifact(blake3, size).await?;
        if bytes.len() as u64 != size {
            return Err(CheckoutRefusal::ArtifactMismatch("size".into()));
        }
        if sha256_hex(&bytes) != sha256 {
            return Err(CheckoutRefusal::ArtifactMismatch("sha256".into()));
        }
        if hex_encode(blake3::hash(&bytes).as_bytes()) != blake3 {
            return Err(CheckoutRefusal::ArtifactMismatch("blake3".into()));
        }
        self.exchange.seed_bytes(&bytes).map_err(|e| CheckoutRefusal::Storage(e.to_string()))?;
        Ok(())
    }

    fn chain_granted(&self, who: &str, req: &CheckoutWire, g: &CheckoutGrant, blake3: &str, merged: bool) {
        if let Some(cm) = &self.chain {
            cm.append(
                "licence",
                EVENT_KIND_CHECKOUT_GRANTED,
                Some(serde_json::json!({
                    "cog_id": g.cog_id, "version": g.version, "arch": req.arch,
                    "grant_id": g.grant_id, "seq": g.seq, "blake3": blake3, "requester": who,
                    "merged": merged,
                })),
            );
        }
    }

    fn chain_refused(&self, req: &CheckoutWire, who: &str, e: &CheckoutRefusal) {
        if let Some(cm) = &self.chain {
            cm.append(
                "licence",
                EVENT_KIND_CHECKOUT_REFUSED,
                Some(serde_json::json!({
                    "cog_id": req.cog_id, "version": req.version, "arch": req.arch,
                    "code": e.code(), "requester": who,
                })),
            );
        }
    }
}
