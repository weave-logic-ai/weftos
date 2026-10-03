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
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::OnceCell;

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
        Self { store, exchange, client, gate, flood, chain, inflight: Mutex::default() }
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
        let out = cell
            .get_or_init(|| async {
                let r = self.relay(who, req).await;
                // Later requests start a fresh relay call; waiters on this
                // cell still read the stored result.
                let mut m = self.inflight.lock().unwrap_or_else(|p| p.into_inner());
                if m.get(&key).is_some_and(|c| Arc::ptr_eq(c, &cell)) {
                    m.remove(&key);
                }
                r
            })
            .await;
        out.clone()
    }

    async fn relay(&self, who: &str, req: &CheckoutWire) -> Result<SignedGrant, CheckoutRefusal> {
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
        let have = self.exchange.resolve(&ArtifactKey::Content(hash)).is_some();
        if !have {
            self.fetch_and_seed(&art.blake3, &art.sha256, art.size).await?;
        }
        install_grant(&self.store, &self.exchange, &signed).map_err(|e| match e {
            LicenceError::NotYetValid => CheckoutRefusal::SeedClockSkew,
            e => CheckoutRefusal::BadGrant(e.to_string()),
        })?;
        self.chain_granted(who, req, &g, &art.blake3);
        self.flood.flood(&signed).await;
        Ok(signed)
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

    fn chain_granted(&self, who: &str, req: &CheckoutWire, g: &CheckoutGrant, blake3: &str) {
        if let Some(cm) = &self.chain {
            cm.append(
                "licence",
                EVENT_KIND_CHECKOUT_GRANTED,
                Some(serde_json::json!({
                    "cog_id": g.cog_id, "version": g.version, "arch": req.arch,
                    "grant_id": g.grant_id, "seq": g.seq, "blake3": blake3, "requester": who,
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
