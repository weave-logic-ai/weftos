//! Seed licence proxy, member side (ADR-106 phase 1a).
//!
//! Types, stores and policy for the mesh checkout of Cognitum cogs, as pure
//! in-process code (no network, no transport):
//!
//! - [`MeshId`] and [`LocalMeshId`]: the mesh a binding, grant or approval is for.
//! - [`BindingRecord`]: the operator-signed Seed to mesh binding (v2), with the
//!   *member* verification profile ([`verify_binding_member`]). The *steward*
//!   profile (age window, live identity match, fingerprint) is phase 1d and
//!   plugs in through [`BindingExtraCheck`].
//! - [`CheckoutGrant`]: signed by the Seed's grant key; highest `seq` wins.
//! - [`Approval`]: the operator's additive, content-addressed hash approval.
//! - [`CheckoutGrantStore`] and [`ApprovalStore`]: persisted, atomic, 0600,
//!   size-capped, fail-closed.
//! - [`MeshCheckoutPolicy`]: the swarm redistribution policy. With no binding
//!   it behaves exactly like [`crate::mesh_swarm_state::ManifestPolicy`].
//! - [`may_run`]: the run gate (valid grant AND an approval covering the sha256).
//!
//! Every signature is Ed25519, checked with `verify_strict`, over
//! `domain tag || "\n" || canonical JSON payload`. The signer keys are the
//! existing [`crate::workload_pkg::TrustAnchors`] operator keys; the grant key
//! is the one the binding names.

mod approval;
mod approval_store;
mod binding;
mod chain_sink;
mod exchange;
mod exchange_sync;
mod exchange_types;
mod floor;
mod gate;
mod grant;
mod persist;
mod policy;
mod store;
mod store_accept;
mod store_load;
mod store_sync;

#[cfg(test)]
mod tests_common;
#[cfg(test)]
mod tests_exchange;
#[cfg(test)]
mod tests_fixes;
#[cfg(test)]
mod tests_sync;
#[cfg(test)]
mod tests_policy;
#[cfg(test)]
mod tests_review;
#[cfg(test)]
mod tests_store;
#[cfg(test)]
mod tests_types;

use std::fmt;
use std::sync::{Arc, Mutex, RwLock};

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::workload_pkg::codec::{hex_decode_exact, hex_encode};

pub use approval::{Approval, SignedApproval, sign_approval, verify_approval};
pub use approval_store::ApprovalStore;
pub use binding::{
    AdmissionPosture, BindState, BindingExtraCheck, BindingRecord, NoExtraChecks, SignedBinding,
    sign_binding, verify_binding_member,
};
pub use chain_sink::{ChainLicenceSink, LICENCE_EVENT_PREFIX};
pub use exchange::{
    CtxAdmission, ExchangeError, LicenceExchange, LicenceExchangeConfig, LicenceExchangeParts,
    PeerAdmission, PostureFn, Receipt, Spend, sign_unbind,
};
pub use exchange_sync::{GrantCursor, SYNC_MAX_BYTES, SYNC_MAX_ENTRIES, SyncMsg};
pub use floor::FloorState;
pub use gate::{RunDenied, RunPermit, RunRequest, may_run};
pub use grant::{
    CheckoutGrant, GrantArtifact, LicenceRef, SignedGrant, sign_grant, verify_grant,
};
pub use policy::MeshCheckoutPolicy;
pub use store::{CheckoutGrantStore, VerifiedCheckoutGrant};

/// Domain tag of the mesh id hash.
pub const MESH_ID_DOMAIN: &str = "weft-licence-v1/mesh-id";
/// Domain tag of binding records.
pub const BINDING_DOMAIN: &str = "weft-licence-v1/binding";
/// Domain tag of checkout grants.
pub const GRANT_DOMAIN: &str = "weft-licence-v1/grant";
/// Domain tag of operator hash approvals.
pub const APPROVAL_DOMAIN: &str = "weft-licence-v1/approval";

/// Largest signed payload accepted, in bytes.
pub const MAX_PAYLOAD_BYTES: usize = 16 * 1024;
/// Largest persisted store file read back, in bytes.
pub const MAX_STORE_BYTES: u64 = 8 * 1024 * 1024;
/// A grant issued more than this far ahead of the local clock is deferred.
pub const GRANT_SKEW_SECS: u64 = 300;
/// Longest grant lifetime (`expires_at - issued_at`): 7 days.
pub const MAX_GRANT_TTL_SECS: u64 = 7 * 24 * 3600;
/// The clock high-water mark grows at most this far past the newest accepted `issued_at`.
pub const FAR_FUTURE_CLAMP_SECS: u64 = 30 * 24 * 3600;
/// Latest time accepted in any record or floor (2100-01-01), unix seconds.
pub const MAX_UNIX_TIME: u64 = 4_102_444_800;
/// Largest artifact size a grant may list: 1 GiB.
pub const MAX_ARTIFACT_BYTES: u64 = 1 << 30;
/// Most floor entries a store file may hold (one per grant key, in practice one).
pub const MAX_FLOORS: usize = 8;
/// Most (cog, version) grant slots held.
pub const MAX_GRANT_SLOTS: usize = 512;
/// Most approvals held.
pub const MAX_APPROVALS: usize = 4096;

/// Why a licence record or store operation was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LicenceError {
    /// Malformed, non-canonical or invalid content.
    #[error("malformed licence record: {0}")]
    Malformed(String),
    /// Payload or file over the size cap.
    #[error("licence record too large")]
    TooLarge,
    /// The signature does not verify.
    #[error("licence signature does not verify")]
    BadSignature,
    /// Signed by a key that is not the pinned operator key (binding,
    /// approval) or not the bound grant key (grant).
    #[error("licence signer is not trusted for this record")]
    UntrustedKey,
    /// The grant key has been revoked (`SignerKey`).
    #[error("grant key is revoked")]
    KeyRevoked,
    /// The record names another mesh.
    #[error("record is for another mesh")]
    WrongMesh,
    /// This node has no local mesh id (no mesh nonce configured).
    #[error("no local mesh id")]
    NoLocalMesh,
    /// No binding is accepted.
    #[error("no binding")]
    NoBinding,
    /// The binding is `unbound`.
    #[error("binding is unbound")]
    Unbound,
    /// The stored binding is for a mesh id this node no longer computes.
    #[error("binding is orphaned")]
    Orphaned,
    /// A different record at an equal `seq` (both are refused).
    #[error("conflicting records at seq {0}")]
    Conflict(u64),
    /// A newer grant leaves out an arch the current one carries.
    #[error("newer grant drops arch {0}")]
    DropsArch(String),
    /// A newer grant changes the hashes of an arch it already carried.
    #[error("newer grant changes artifact {0}")]
    ChangesArtifact(String),
    /// Issued ahead of the local clock: retry at the next sync.
    #[error("grant is not yet valid")]
    NotYetValid,
    /// `expires_at - issued_at` is over the maximum.
    #[error("grant lifetime exceeds the maximum")]
    TtlTooLong,
    /// The checkout part of the policy refuses bindings.
    #[error("binding refused: {0}")]
    BindingRefused(&'static str),
    /// A steward-profile check failed.
    #[error("binding check failed: {0}")]
    CheckFailed(String),
    /// The store file was unreadable at load; writes are refused.
    #[error("licence store is poisoned: {0}")]
    Poisoned(String),
    /// Persisting to disk failed.
    #[error("licence store persist failed: {0}")]
    Persist(String),
    /// A store capacity limit was reached.
    #[error("licence store is full")]
    Full,
}

/// What an accepted record did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// New state was recorded.
    Applied,
    /// A record that only restricts (an unbind, a withdrawal) is in force
    /// but could not be saved; `tick` retries. Propagate it like `Applied`.
    AppliedUnsaved,
    /// Already held; nothing changed.
    Duplicate,
    /// A lower `seq` than the one held; ignored.
    Ignored,
}

/// The identity of one mesh:
/// `sha256("weft-licence-v1/mesh-id\n" || genesis_pin || mesh_nonce)`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct MeshId([u8; 32]);

impl MeshId {
    /// Derive the id from the genesis pin and the operator's `mesh_nonce`.
    pub fn derive(genesis_pin: &[u8; 32], mesh_nonce: &[u8; 32]) -> Self {
        let mut h = Sha256::new();
        h.update(MESH_ID_DOMAIN.as_bytes());
        h.update(b"\n");
        h.update(genesis_pin);
        h.update(mesh_nonce);
        Self(h.finalize().into())
    }

    /// Parse the lower-case hex form.
    pub fn from_hex(s: &str) -> Option<Self> {
        hex_decode_exact::<32>(s).map(Self)
    }

    /// Lower-case hex form.
    pub fn to_hex(&self) -> String {
        hex_encode(&self.0)
    }
}

impl fmt::Debug for MeshId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MeshId({})", &self.to_hex()[..12])
    }
}

/// Shared, live handle to this node's mesh id. A change (new nonce or pin)
/// reaches every store holding a clone, which then report orphaned state.
#[derive(Clone, Default)]
pub struct LocalMeshId(Arc<RwLock<Option<MeshId>>>);

impl LocalMeshId {
    /// A handle holding `id`.
    pub fn new(id: MeshId) -> Self {
        Self(Arc::new(RwLock::new(Some(id))))
    }

    /// A handle with no id yet (no mesh nonce configured).
    pub fn unset() -> Self {
        Self::default()
    }

    /// The current id.
    pub fn get(&self) -> Option<MeshId> {
        *self.0.read().unwrap_or_else(|p| p.into_inner())
    }

    /// Replace the id.
    pub fn set(&self, id: Option<MeshId>) {
        *self.0.write().unwrap_or_else(|p| p.into_inner()) = id;
    }
}

/// Source of unix seconds. Injected so tests control the clock.
pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// The wall clock.
pub fn system_clock() -> Clock {
    Arc::new(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    })
}

/// Something worth chaining. The daemon maps these to chain events; the
/// stores only report them through a [`LicenceEventSink`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LicenceEvent {
    /// A binding was refused (`open_membership`, `observe`, ...).
    BindingRefused(String),
    /// Two different bindings at one `seq`.
    BindingConflict(u64),
    /// The stored binding's mesh id no longer matches the local one.
    BindingOrphaned {
        /// Mesh id in the stored binding.
        stored: String,
        /// Mesh id this node computes now.
        local: String,
    },
    /// Two different grants at one `seq` for one (cog, version).
    GrantConflict {
        /// Cog id.
        cog_id: String,
        /// Version.
        version: String,
        /// The contested `seq`.
        seq: u64,
    },
    /// An operator reset the floor.
    FloorReset(u64),
    /// A sync response carried a bad signature; the peer is banned from sync.
    SyncBadSignature {
        /// The peer that sent it.
        peer: String,
    },
}

impl LicenceEvent {
    /// The chain event name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::BindingRefused(_) => "binding_refused",
            Self::BindingConflict(_) => "binding_conflict",
            Self::BindingOrphaned { .. } => "binding_orphaned",
            Self::GrantConflict { .. } => "grant_conflict",
            Self::FloorReset(_) => "floor_reset",
            Self::SyncBadSignature { .. } => "sync_bad_signature",
        }
    }
}

/// Receives [`LicenceEvent`]s. Called after the store's lock is released, but
/// a sink must still not call back into the store.
pub trait LicenceEventSink: Send + Sync {
    /// Handle one event.
    fn emit(&self, event: LicenceEvent);
}

/// Drops every event.
#[derive(Debug, Default)]
pub struct NoopSink;

impl LicenceEventSink for NoopSink {
    fn emit(&self, _: LicenceEvent) {}
}

/// Keeps every event (tests, and callers that drain into the chain).
#[derive(Debug, Default)]
pub struct RecordingSink(Mutex<Vec<LicenceEvent>>);

impl RecordingSink {
    /// The events so far.
    pub fn events(&self) -> Vec<LicenceEvent> {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

impl LicenceEventSink for RecordingSink {
    fn emit(&self, event: LicenceEvent) {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).push(event);
    }
}

/// A signed payload as it travels: the exact signed JSON, key and signature
/// (hex). One shape for every record kind; the domain tag keeps kinds apart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedEnvelope {
    /// Canonical JSON of the record, exactly as signed.
    pub payload: String,
    /// Signer's Ed25519 public key, 64 hex chars.
    pub public_key: String,
    /// Ed25519 signature, 128 hex chars.
    pub signature: String,
}

fn signed_bytes(domain: &str, payload: &str) -> Vec<u8> {
    let mut v = Vec::with_capacity(domain.len() + 1 + payload.len());
    v.extend_from_slice(domain.as_bytes());
    v.push(b'\n');
    v.extend_from_slice(payload.as_bytes());
    v
}

/// Serialize `record` (struct field order is the canonical order) and sign it.
pub(crate) fn sign_envelope<T: Serialize>(
    domain: &str,
    record: &T,
    key: &SigningKey,
) -> Result<SignedEnvelope, LicenceError> {
    let payload =
        serde_json::to_string(record).map_err(|e| LicenceError::Malformed(e.to_string()))?;
    let sig = key.sign(&signed_bytes(domain, &payload));
    Ok(SignedEnvelope {
        payload,
        public_key: hex_encode(&key.verifying_key().to_bytes()),
        signature: hex_encode(&sig.to_bytes()),
    })
}

/// The signer key of `env`, after the size check. Cheap: no signature work.
pub(crate) fn envelope_key(env: &SignedEnvelope) -> Result<[u8; 32], LicenceError> {
    if env.payload.len() > MAX_PAYLOAD_BYTES {
        return Err(LicenceError::TooLarge);
    }
    hex_decode_exact::<32>(&env.public_key)
        .ok_or_else(|| LicenceError::Malformed("public key".into()))
}

/// Strictly verify `env` under `domain` against `pk`.
pub(crate) fn verify_envelope(
    domain: &str,
    env: &SignedEnvelope,
    pk: &[u8; 32],
) -> Result<(), LicenceError> {
    let sig = hex_decode_exact::<64>(&env.signature)
        .ok_or_else(|| LicenceError::Malformed("signature".into()))?;
    let vk = VerifyingKey::from_bytes(pk).map_err(|_| LicenceError::BadSignature)?;
    vk.verify_strict(
        &signed_bytes(domain, &env.payload),
        &Signature::from_bytes(&sig),
    )
    .map_err(|_| LicenceError::BadSignature)
}

/// Parse `payload` and require that re-serializing gives the same bytes, so
/// each record has exactly one signed spelling.
pub(crate) fn parse_canonical<T: Serialize + DeserializeOwned>(
    payload: &str,
) -> Result<T, LicenceError> {
    let v: T =
        serde_json::from_str(payload).map_err(|e| LicenceError::Malformed(e.to_string()))?;
    let again = serde_json::to_string(&v).map_err(|e| LicenceError::Malformed(e.to_string()))?;
    if again != payload {
        return Err(LicenceError::Malformed("payload is not canonical".into()));
    }
    Ok(v)
}

/// `sha256` of `bytes`, lower-case hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex_encode(&Sha256::digest(bytes))
}

/// `"ed25519:"` plus 16 hex chars of `sha256(public key)`: the grant key id
/// the operator compares at `init` time.
pub fn key_id(pk: &[u8; 32]) -> String {
    format!("ed25519:{}", &sha256_hex(pk)[..16])
}

/// A short token: 1 to 128 chars of `[A-Za-z0-9._-+@/]`.
pub(crate) fn valid_token(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+' | '@' | '/'))
}

/// 64 lower-case hex chars.
pub(crate) fn valid_hex32(s: &str) -> bool {
    hex_decode_exact::<32>(s).is_some()
}
