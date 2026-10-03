//! Operator hash approval (ADR-106 section 4, step 7).
//!
//! Additive and content-addressed: keyed by (mesh_id, cog, version, sha256
//! set), no `seq`. A duplicate is idempotent and a new approval never
//! replaces an older one. Withdrawal is the existing `ArtifactHash`
//! revocation of the artifact's BLAKE3.

use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};

use super::binding::require_operator;
use super::{
    APPROVAL_DOMAIN, LicenceError, MeshId, SignedEnvelope, envelope_key, parse_canonical,
    sha256_hex, sign_envelope, valid_hex32, valid_token, verify_envelope,
};
use crate::workload_pkg::TrustAnchors;

/// Most sha256 entries one approval lists.
pub const MAX_APPROVAL_HASHES: usize = 32;

/// An operator's approval to run these binaries of one cog version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Approval {
    /// Always 1.
    pub v: u32,
    /// The mesh, as [`MeshId::to_hex`].
    pub mesh_id: String,
    /// Cog id.
    pub cog_id: String,
    /// Cog version.
    pub version: String,
    /// sha256 of each approved binary: lower-case hex, sorted, no duplicates.
    pub sha256: Vec<String>,
    /// When the operator signed it, unix seconds.
    pub approved_at: u64,
}

impl Approval {
    /// The content key (provenance `approval_id`): a hash of mesh, cog,
    /// version and the sorted sha256 set. `approved_at` is not part of it, so
    /// re-signing the same content later is a duplicate.
    pub fn content_key(&self) -> String {
        let mut s = format!(
            "weft-licence-v1/approval-key\n{}\n{}\n{}",
            self.mesh_id, self.cog_id, self.version
        );
        for h in &self.sha256 {
            s.push('\n');
            s.push_str(h);
        }
        sha256_hex(s.as_bytes())
    }

    /// True when `sha256` is one of the approved binaries.
    pub fn covers(&self, sha256: &str) -> bool {
        self.sha256.iter().any(|h| h == sha256)
    }
}

/// A signed [`Approval`].
pub type SignedApproval = SignedEnvelope;

/// Sign `approval` as the operator. The sha256 list is sorted and
/// de-duplicated first so there is one canonical spelling.
pub fn sign_approval(
    approval: &Approval,
    operator: &SigningKey,
) -> Result<SignedApproval, LicenceError> {
    let mut a = approval.clone();
    a.sha256.sort();
    a.sha256.dedup();
    sign_envelope(APPROVAL_DOMAIN, &a, operator)
}

/// Verify `signed`: pinned operator key, strict signature, canonical payload,
/// well-formed fields, and `mesh_id` equal to `local` (A2).
pub fn verify_approval(
    signed: &SignedApproval,
    anchors: &TrustAnchors,
    local: &MeshId,
) -> Result<Approval, LicenceError> {
    let a = verify_approval_signature(signed, anchors)?;
    if a.mesh_id != local.to_hex() {
        return Err(LicenceError::WrongMesh);
    }
    Ok(a)
}

/// Signature and shape only, with no mesh check (store load).
pub(crate) fn verify_approval_signature(
    signed: &SignedApproval,
    anchors: &TrustAnchors,
) -> Result<Approval, LicenceError> {
    let pk = envelope_key(signed)?;
    require_operator(anchors, &pk)?;
    verify_envelope(APPROVAL_DOMAIN, signed, &pk)?;
    let a: Approval = parse_canonical(&signed.payload)?;
    let sorted = a.sha256.windows(2).all(|w| w[0] < w[1]);
    if a.v != 1
        || !valid_hex32(&a.mesh_id)
        || !valid_token(&a.cog_id)
        || !valid_token(&a.version)
        || a.sha256.is_empty()
        || a.sha256.len() > MAX_APPROVAL_HASHES
        || !sorted
        || !a.sha256.iter().all(|h| valid_hex32(h))
    {
        return Err(LicenceError::Malformed("approval field".into()));
    }
    Ok(a)
}
