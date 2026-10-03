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
//!   ([`ArtifactExchange::apply_revocations`]).
//! - **Flooded.** A notice that was new here is forwarded to every other
//!   peer; one that was already known is not, so a notice crosses a mesh
//!   once and cannot loop. Revocations are only ever added by notices, so a
//!   replayed old notice changes nothing.

use std::sync::Arc;

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
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
    vk.verify(&signed_bytes(&signed.payload), &Signature::from_bytes(&sig))
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
}

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
        });
        runtime.set_control_sink(REVOKE_TOPIC, me.clone());
        me
    }

    /// Record and apply a notice. Returns whether it was new here.
    pub fn accept(&self, signed: &SignedRevocation) -> Result<bool, NoticeError> {
        let n = verify_revocation(signed, &self.anchors)?;
        let new = self
            .list
            .revoke_subject(n.kind, &n.id, &n.reason)
            .map_err(|e| NoticeError::Malformed(e.to_string()))?;
        // Apply even when the entry was already listed: the operator may have
        // revoked locally before the sweep ran.
        self.ex.apply_revocations();
        Ok(new)
    }

    /// Revoke here (the operator's own node) and send the notice to every
    /// peer. `signed` must verify against this node's anchors.
    pub async fn issue(&self, signed: SignedRevocation) -> Result<bool, NoticeError> {
        let new = self.accept(&signed)?;
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
    fn on_peer_control(&self, ctx: &PeerCtx, payload: &serde_json::Value) -> Vec<serde_json::Value> {
        let signed: SignedRevocation = match serde_json::from_value(payload.clone()) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(peer = %ctx.peer_id, error = %e, "malformed revocation message");
                return Vec::new();
            }
        };
        match self.accept(&signed) {
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

#[cfg(test)]
#[path = "mesh_swarm_revoke_tests.rs"]
mod tests;
