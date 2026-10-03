//! Wire types of the ADR-106 Seed licence proxy: the checkout grant, the signed
//! envelope, the mesh id and the shared error type.
//!
//! This crate is small and portable on purpose. Both the member side
//! (`clawft-kernel::licence`, which re-exports everything here) and the Seed
//! side (`weft-licence`, built for armv7 and aarch64) use it, so the grant
//! wire format is defined exactly once.
//!
//! Every signature is Ed25519, checked with `verify_strict`, over
//! `domain tag || "\n" || canonical JSON payload`.

use std::fmt;

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod grant;
pub use grant::{
    CheckoutGrant, GrantArtifact, LicenceRef, MAX_GRANT_ARTIFACTS, SignedGrant, sign_grant,
    verify_grant, verify_grant_signature,
};

/// Lower-case hex encoding.
pub fn hex_encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(DIGITS[(b >> 4) as usize] as char);
        s.push(DIGITS[(b & 0x0f) as usize] as char);
    }
    s
}

/// Decode lower-case hex of exactly `N` bytes. Upper-case is refused so each
/// value has one spelling.
pub fn hex_decode_exact<const N: usize>(s: &str) -> Option<[u8; N]> {
    if s.len() != N * 2 {
        return None;
    }
    let mut out = [0u8; N];
    let bytes = s.as_bytes();
    for (i, slot) in out.iter_mut().enumerate() {
        let hi = hex_val(bytes[2 * i])?;
        let lo = hex_val(bytes[2 * i + 1])?;
        *slot = (hi << 4) | lo;
    }
    Some(out)
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    }
}

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
/// A floor this far ahead of the newest accepted `issued_at` is poisoning.
pub const FAR_FUTURE_CLAMP_SECS: u64 = 30 * 24 * 3600;
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

pub fn signed_bytes(domain: &str, payload: &str) -> Vec<u8> {
    let mut v = Vec::with_capacity(domain.len() + 1 + payload.len());
    v.extend_from_slice(domain.as_bytes());
    v.push(b'\n');
    v.extend_from_slice(payload.as_bytes());
    v
}

/// Serialize `record` (struct field order is the canonical order) and sign it.
pub fn sign_envelope<T: Serialize>(
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
pub fn envelope_key(env: &SignedEnvelope) -> Result<[u8; 32], LicenceError> {
    if env.payload.len() > MAX_PAYLOAD_BYTES {
        return Err(LicenceError::TooLarge);
    }
    hex_decode_exact::<32>(&env.public_key)
        .ok_or_else(|| LicenceError::Malformed("public key".into()))
}

/// Strictly verify `env` under `domain` against `pk`.
pub fn verify_envelope(
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
pub fn parse_canonical<T: Serialize + DeserializeOwned>(
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
pub fn valid_token(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+' | '@' | '/'))
}

/// 64 lower-case hex chars.
pub fn valid_hex32(s: &str) -> bool {
    hex_decode_exact::<32>(s).is_some()
}
