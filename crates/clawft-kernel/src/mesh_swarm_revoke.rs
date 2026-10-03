//! Revocation across the mesh (ADR-099 section 6, card mesh-placement-25).
//!
//! A revocation of a package id, signer key or artifact hash has to stop
//! seeding and empty caches on every node, not only where the operator
//! typed it. [`RevocationExchange`] carries it as a signed notice on the
//! `mesh.artifact.revoke` control topic:
//!
//! - **Signed.** The notice is signed with a pinned trust-anchor key whose
//!   origin is `Operator` or `Weftos`; a Cognitum release key, an unpinned
//!   key or a bad signature is ignored. The domain tag keeps it from being
//!   replayed as anything else.
//! - **Applied.** A valid notice is recorded in the local
//!   [`RevocationList`] and applied at once: grants dropped, bytes evicted,
//!   `artifact.revoke` and `artifact.evict` chained
//!   ([`ArtifactExchange::apply_revocations`]), the revocation itself chained
//!   as `workload.revoke` (revoked by `mesh:<signer>`), and the
//!   [`RevocationExchange::set_on_applied`] hook run so placement can stop
//!   what is already running from it.
//! - **Limited.** A signer key that is itself revoked can no longer issue
//!   revocations and signatures are checked strictly. Cheap checks (size,
//!   pinned key, revoked signer, already applied) come first and are free; a
//!   notice that passes them spends a token from its own connection's bucket
//!   before the signature verify, so junk on one connection cannot starve
//!   notices from another, and the operator's own `issue` is exempt.
//! - **Flooded.** A notice that was new here is forwarded to every other
//!   peer; one that was already known is not, so a notice crosses a mesh
//!   once and cannot loop. Revocations are only ever added by notices, so a
//!   replayed old notice changes nothing.

use std::sync::Arc;

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::ipc::{KernelMessage, MessagePayload, MessageTarget};
use crate::mesh_artifact::ArtifactExchange;
use crate::mesh_delivery::PeerCtx;
use crate::mesh_runtime::{MeshRuntime, PeerControlSink, REVOKE_TOPIC};
use crate::revocation::{RevocationKind, RevocationList};
use crate::workload_pkg::{KeyOrigin, TrustAnchors};

/// Domain tag signed before a revocation payload.
pub const REVOKE_DOMAIN: &[u8] = b"weftos.artifact_revocation.v1\0";
/// Largest notice payload accepted, in bytes.
pub const MAX_NOTICE_BYTES: usize = 4096;

/// What is revoked and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevocationNotice {
    /// Package id, signer key or artifact hash.
    pub kind: RevocationKind,
    /// Canonical id for that kind.
    pub id: String,
    /// Human-readable reason.
    pub reason: String,
    /// Issue time, unix seconds.
    pub issued_at: u64,
}

/// A notice as it travels: the exact signed JSON, the key and the signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedRevocation {
    /// JSON of the [`RevocationNotice`], exactly as signed.
    pub payload: String,
    /// Ed25519 public key (32 bytes).
    pub public_key: Vec<u8>,
    /// Ed25519 signature (64 bytes) over [`REVOKE_DOMAIN`] || payload.
    pub signature: Vec<u8>,
}

/// Why a notice was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NoticeError {
    /// Malformed, oversize or invalid id.
    #[error("malformed revocation notice: {0}")]
    Malformed(String),
    /// Signature does not verify.
    #[error("revocation signature does not verify")]
    BadSignature,
    /// Signer is not a pinned operator or WeftOS key.
    #[error("revocation signer is not a pinned operator key")]
    UnauthorizedSigner,
    /// The signing key has itself been revoked.
    #[error("revocation signer key is revoked")]
    SignerRevoked,
    /// Too many notices arrived too fast.
    #[error("revocation notices are arriving too fast")]
    RateLimited,
}

fn signed_bytes(payload: &str) -> Vec<u8> {
    let mut v = REVOKE_DOMAIN.to_vec();
    v.extend_from_slice(payload.as_bytes());
    v
}

/// Sign a revocation notice.
pub fn sign_revocation(
    kind: RevocationKind,
    id: &str,
    reason: &str,
    now: u64,
    key: &SigningKey,
) -> Result<SignedRevocation, NoticeError> {
    let id = kind
        .normalize(id)
        .map_err(|e| NoticeError::Malformed(e.to_string()))?;
    let payload = serde_json::to_string(&RevocationNotice {
        kind,
        id,
        reason: reason.chars().take(256).collect(),
        issued_at: now,
    })
    .map_err(|e| NoticeError::Malformed(e.to_string()))?;
    let sig = key.sign(&signed_bytes(&payload));
    Ok(SignedRevocation {
        payload,
        public_key: key.verifying_key().to_bytes().to_vec(),
        signature: sig.to_bytes().to_vec(),
    })
}

/// Verify `signed` against `anchors` and return the notice.
pub fn verify_revocation(
    signed: &SignedRevocation,
    anchors: &TrustAnchors,
) -> Result<RevocationNotice, NoticeError> {
    if signed.payload.len() > MAX_NOTICE_BYTES {
        return Err(NoticeError::Malformed("payload too large".into()));
    }
    let pk: [u8; 32] = signed
        .public_key
        .as_slice()
        .try_into()
        .map_err(|_| NoticeError::Malformed("public key length".into()))?;
    let sig: [u8; 64] = signed
        .signature
        .as_slice()
        .try_into()
        .map_err(|_| NoticeError::Malformed("signature length".into()))?;
    let vk = VerifyingKey::from_bytes(&pk).map_err(|_| NoticeError::BadSignature)?;
    vk.verify_strict(&signed_bytes(&signed.payload), &Signature::from_bytes(&sig))
        .map_err(|_| NoticeError::BadSignature)?;
    let pinned = anchors.signer(&pk).ok_or(NoticeError::UnauthorizedSigner)?;
    if !matches!(pinned.origin, KeyOrigin::Operator | KeyOrigin::Weftos) {
        return Err(NoticeError::UnauthorizedSigner);
    }
    let notice: RevocationNotice = serde_json::from_str(&signed.payload)
        .map_err(|e| NoticeError::Malformed(e.to_string()))?;
    let canonical = notice
        .kind
        .normalize(&notice.id)
        .map_err(|e| NoticeError::Malformed(e.to_string()))?;
    if canonical != notice.id {
        return Err(NoticeError::Malformed("id is not canonical".into()));
    }
    Ok(notice)
}

/// Receives, applies and forwards revocation notices for one node.
pub struct RevocationExchange {
    ex: Arc<ArtifactExchange>,
    list: Arc<RevocationList>,
    anchors: TrustAnchors,
    runtime: Arc<MeshRuntime>,
    me: std::sync::Weak<Self>,
    /// Token buckets for notices that reach signature verification, one per
    /// connection (0 = callers with no connection): `(tokens, last refill)`.
    buckets: dashmap::DashMap<u64, (f64, std::time::Instant)>,
    /// Notices already applied (hash of payload, key, signature), oldest first.
    seen: std::sync::Mutex<Seen>,
    /// Run after a notice that was new here has been applied.
    on_applied: std::sync::OnceLock<AppliedHook>,
    /// Verified notices kept for replay to peers that rejoin.
    log: std::sync::Mutex<log::NoticeLog>,
    /// Replay bookkeeping per peer (in flight, cooldown, cancel).
    replays: std::sync::Mutex<log::Replays>,
    replays_started: std::sync::atomic::AtomicU64,
}

/// Called with each notice that was new here, after it was recorded and the
/// held artifacts were swept. Runs on the receiving task: it must not block
/// (spawn what needs time).
pub type AppliedHook = Arc<dyn Fn(&RevocationNotice) + Send + Sync>;

/// Applied-notice keys: a set for lookup, a queue for oldest-first eviction.
type Seen = (std::collections::HashSet<[u8; 32]>, std::collections::VecDeque<[u8; 32]>);

/// Connections whose buckets are remembered at once.
const MAX_TRACKED_CONNS: usize = 1024;
/// Applied notices remembered for de-duplication.
const MAX_SEEN: usize = 4096;

/// Notices accepted per second (burst [`NOTICE_BURST`]).
pub const NOTICES_PER_SEC: f64 = 2.0;
/// Burst of notices accepted at once.
pub const NOTICE_BURST: f64 = 10.0;

impl RevocationExchange {
    /// Exchange applying notices to `ex`, recording them in `list` and
    /// trusting `anchors`. Installs itself on `runtime` and gives `ex` the
    /// list.
    pub fn start(
        ex: Arc<ArtifactExchange>,
        list: Arc<RevocationList>,
        anchors: TrustAnchors,
        runtime: Arc<MeshRuntime>,
    ) -> Arc<Self> {
        ex.set_revocations(list.clone());
        let me = Arc::new_cyclic(|w| Self {
            ex,
            list,
            anchors,
            runtime: runtime.clone(),
            me: w.clone(),
            buckets: dashmap::DashMap::new(),
            seen: Default::default(),
            on_applied: std::sync::OnceLock::new(),
            log: Default::default(),
            replays: Default::default(),
            replays_started: Default::default(),
        });
        runtime.set_control_sink(REVOKE_TOPIC, me.clone());
        Self::spawn_rejoin_replay(&me);
        me
    }

    /// Run `hook` after every notice that was new here is applied (first
    /// call wins; returns whether this one did). Covers a notice from a peer
    /// and the operator's own [`Self::issue`].
    pub fn set_on_applied(&self, hook: AppliedHook) -> bool {
        self.on_applied.set(hook).is_ok()
    }

    /// Spend one token from `conn`'s bucket.
    fn take_token(&self, conn: u64) -> bool {
        if self.buckets.len() >= MAX_TRACKED_CONNS && !self.buckets.contains_key(&conn) {
            let oldest = self.buckets.iter().min_by_key(|b| b.1).map(|b| *b.key());
            if let Some(k) = oldest {
                self.buckets.remove(&k);
            }
        }
        let now = std::time::Instant::now();
        let mut b = self.buckets.entry(conn).or_insert((NOTICE_BURST, now));
        let dt = now.duration_since(b.1).as_secs_f64();
        b.1 = now;
        b.0 = (b.0 + dt * NOTICES_PER_SEC).min(NOTICE_BURST);
        if b.0 < 1.0 {
            return false;
        }
        b.0 -= 1.0;
        true
    }

    fn notice_key(signed: &SignedRevocation) -> [u8; 32] {
        let mut h = blake3::Hasher::new();
        h.update(signed.payload.as_bytes());
        h.update(&signed.public_key);
        h.update(&signed.signature);
        *h.finalize().as_bytes()
    }

    /// Record and apply a notice from a caller with no connection (budget 0).
    /// Returns whether it was new here; a notice already applied is not
    /// swept again.
    pub fn accept(&self, signed: &SignedRevocation) -> Result<bool, NoticeError> {
        self.accept_from(signed, Some(0))
    }

    /// As [`Self::accept`] for a notice from connection `conn`. The cheap
    /// checks come first and cost no budget: size, a pinned operator/WeftOS
    /// key, a signer that is not itself revoked, a notice not already
    /// applied. Only then is a token spent (from `conn`'s own bucket, so
    /// junk on one connection cannot starve notices on another), before the
    /// signature verify. `None` skips the budget (the operator's own `issue`).
    fn accept_from(
        &self,
        signed: &SignedRevocation,
        conn: Option<u64>,
    ) -> Result<bool, NoticeError> {
        if signed.payload.len() > MAX_NOTICE_BYTES {
            return Err(NoticeError::Malformed("payload too large".into()));
        }
        let pk: [u8; 32] = signed
            .public_key
            .as_slice()
            .try_into()
            .map_err(|_| NoticeError::Malformed("public key length".into()))?;
        match self.anchors.signer(&pk) {
            Some(k) if matches!(k.origin, KeyOrigin::Operator | KeyOrigin::Weftos) => {}
            _ => return Err(NoticeError::UnauthorizedSigner),
        }
        if self
            .list
            .is_subject_revoked(RevocationKind::SignerKey, &crate::workload_pkg::codec::hex_encode(&pk))
        {
            return Err(NoticeError::SignerRevoked);
        }
        let key = Self::notice_key(signed);
        if self.seen.lock().unwrap_or_else(|p| p.into_inner()).0.contains(&key) {
            return Ok(false);
        }
        if let Some(conn) = conn
            && !self.take_token(conn)
        {
            return Err(NoticeError::RateLimited);
        }
        let n = verify_revocation(signed, &self.anchors)?;
        let by = format!(
            "mesh:{}",
            crate::workload_pkg::codec::hex_encode(&pk[..4])
        );
        let chain = self.ex.chain.clone();
        let sink = chain.as_ref().map(|cm| {
            let cm = cm.clone();
            move |k: &str, p: serde_json::Value| {
                cm.append(crate::workload_governance::gate::CHAIN_SOURCE, k, Some(p));
            }
        });
        let new = match self.list.revoke_audited(
            n.kind,
            &n.id,
            &n.reason,
            &by,
            sink.as_ref().map(|f| f as &dyn Fn(&str, serde_json::Value)),
        ) {
            Ok(new) => new,
            // The entry is held in memory (fail-closed) and was chained: it
            // is in force, so apply and forward it; the disk write is the
            // only thing that failed.
            Err(crate::revocation::RevocationError::Persist(e)) => {
                tracing::error!(error = %e, "revocation applied but not persisted");
                true
            }
            Err(e) => return Err(NoticeError::Malformed(e.to_string())),
        };
        {
            let mut g = self.seen.lock().unwrap_or_else(|p| p.into_inner());
            if g.0.insert(key) {
                g.1.push_back(key);
                if g.1.len() > MAX_SEEN
                    && let Some(old) = g.1.pop_front()
                {
                    g.0.remove(&old);
                }
            }
        }
        self.log_notice(signed);
        if new {
            self.ex.apply_revocations();
            if let Some(hook) = self.on_applied.get() {
                hook(&n);
            }
        }
        Ok(new)
    }

    /// Revoke here (the operator's own node) and send the notice to every
    /// peer. `signed` must verify against this node's anchors. Not rate
    /// limited: peers' traffic cannot starve the operator.
    pub async fn issue(&self, signed: SignedRevocation) -> Result<bool, NoticeError> {
        let new = self.accept_from(&signed, None)?;
        self.flood(&signed, None).await;
        Ok(new)
    }

    async fn flood(&self, signed: &SignedRevocation, except: Option<&str>) {
        let Ok(value) = serde_json::to_value(signed) else {
            return;
        };
        for peer in self.runtime.peer_ids() {
            if Some(peer.as_str()) == except {
                continue;
            }
            let msg = KernelMessage::new(
                0,
                MessageTarget::Topic(REVOKE_TOPIC.to_string()),
                MessagePayload::Json(value.clone()),
            );
            if let Err(e) = self.runtime.route_to_remote(&peer, msg).await {
                tracing::debug!(peer, error = %e, "revocation forward failed");
            }
        }
    }
}

impl PeerControlSink for RevocationExchange {
    fn on_peer_control(
        &self,
        ctx: &PeerCtx,
        conn: u64,
        payload: &serde_json::Value,
    ) -> Vec<serde_json::Value> {
        let signed: SignedRevocation = match serde_json::from_value(payload.clone()) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(peer = %ctx.peer_id, error = %e, "malformed revocation message");
                return Vec::new();
            }
        };
        match self.accept_from(&signed, Some(conn)) {
            Ok(true) => {
                // New here: pass it on. Done on a task: the sink is sync and
                // the runtime is mid-dispatch.
                if let Some(me) = self.me.upgrade() {
                    let from = ctx.peer_id.clone();
                    tokio::spawn(async move { me.flood(&signed, Some(&from)).await });
                }
            }
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(peer = %ctx.peer_id, verified = ctx.node_verified, error = %e,
                    "revocation notice refused");
            }
        }
        Vec::new()
    }
}

#[path = "mesh_swarm_revoke_log.rs"]
mod log;
pub use log::{MAX_LOGGED, REPLAY_COOLDOWN};

#[cfg(test)]
#[path = "mesh_swarm_revoke_tests.rs"]
mod tests;
