//! Binding record v2 (ADR-106 section 3): one Seed, one mesh.

use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};

use super::{
    BINDING_DOMAIN, LicenceError, MAX_UNIX_TIME, MeshId, SignedEnvelope, envelope_key, parse_canonical,
    sign_envelope, valid_hex32, valid_token, verify_envelope,
};
use crate::workload_pkg::codec::hex_decode_exact;
use crate::workload_pkg::{KeyOrigin, TrustAnchors};

/// Whether a binding binds or unbinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindState {
    /// The Seed is bound to the mesh.
    Bound,
    /// The binding is withdrawn; every grant under its key stops.
    Unbound,
}

/// The operator-signed Seed to mesh binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingRecord {
    /// Always 2.
    pub v: u32,
    /// The Seed's device id.
    pub device_id: String,
    /// The Seed's device public key, 64 hex chars.
    pub device_pubkey: String,
    /// The mesh, as [`MeshId::to_hex`].
    pub mesh_id: String,
    /// The grant key `weft-licence` signs grants with, 64 hex chars.
    pub grant_pubkey: String,
    /// The steward node's id.
    pub steward_node_id: String,
    /// The steward's request-signing key, 64 hex chars.
    pub steward_pubkey: String,
    /// Bound or unbound.
    pub state: BindState,
    /// Strictly increasing per mesh; a higher `seq` replaces a lower one.
    pub seq: u64,
    /// When the operator signed it, unix seconds.
    pub bound_at: u64,
}

/// A signed [`BindingRecord`].
pub type SignedBinding = SignedEnvelope;

/// Sign `record` as the operator.
pub fn sign_binding(
    record: &BindingRecord,
    operator: &SigningKey,
) -> Result<SignedBinding, LicenceError> {
    sign_envelope(BINDING_DOMAIN, record, operator)
}

/// A key is acceptable only when pinned as an operator key.
pub(crate) fn require_operator(anchors: &TrustAnchors, pk: &[u8; 32]) -> Result<(), LicenceError> {
    match anchors.signer(pk) {
        Some(k) if k.origin == KeyOrigin::Operator => Ok(()),
        _ => Err(LicenceError::UntrustedKey),
    }
}

/// The MEMBER verification profile: pinned operator signature, strict
/// signature check, canonical payload, `mesh_id` equal to `local`. Members
/// have no Seed link, so there is no age check and no identity match. The
/// stateful rules (`seq` above the stored one, equal-`seq` conflicts) are the
/// store's ([`super::CheckoutGrantStore::accept_binding`]).
pub fn verify_binding_member(
    signed: &SignedBinding,
    anchors: &TrustAnchors,
    local: &MeshId,
) -> Result<BindingRecord, LicenceError> {
    let rec = verify_binding_signature(signed, anchors)?;
    if rec.mesh_id != local.to_hex() {
        return Err(LicenceError::WrongMesh);
    }
    Ok(rec)
}

/// Signature and shape only, with no mesh check (store load re-verification).
pub(crate) fn verify_binding_signature(
    signed: &SignedBinding,
    anchors: &TrustAnchors,
) -> Result<BindingRecord, LicenceError> {
    let pk = envelope_key(signed)?;
    require_operator(anchors, &pk)?;
    verify_envelope(BINDING_DOMAIN, signed, &pk)?;
    let rec: BindingRecord = parse_canonical(&signed.payload)?;
    if rec.v != 2 {
        return Err(LicenceError::Malformed("binding version".into()));
    }
    let hex_ok = [
        &rec.device_pubkey,
        &rec.mesh_id,
        &rec.grant_pubkey,
        &rec.steward_pubkey,
    ]
    .iter()
    .all(|s| valid_hex32(s));
    if !hex_ok || !valid_token(&rec.device_id) || !valid_token(&rec.steward_node_id) {
        return Err(LicenceError::Malformed("binding field".into()));
    }
    // The grant and steward keys must not be a pinned trust-anchor key, or a
    // binding could hand an operator or release key the role of a Seed key.
    for hex in [&rec.grant_pubkey, &rec.steward_pubkey] {
        let pk = hex_decode_exact::<32>(hex).unwrap_or_default();
        let anchored = anchors.signers.iter().chain(&anchors.cognitum).any(|k| k.public_key == pk);
        if anchored {
            return Err(LicenceError::Malformed("binding key is a trust anchor".into()));
        }
    }
    if rec.bound_at > MAX_UNIX_TIME {
        return Err(LicenceError::Malformed("binding bounds".into()));
    }
    Ok(rec)
}

/// Where the STEWARD profile (phase 1d) plugs in: the 600 s age window, the
/// live `GET /api/v1/identity` match, the grant key fingerprint the operator
/// confirmed. Runs after the member checks and before the record is stored.
pub trait BindingExtraCheck {
    /// Refuse the record with [`LicenceError::CheckFailed`].
    fn check(&self, record: &BindingRecord) -> Result<(), LicenceError>;
}

/// The member profile adds nothing.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoExtraChecks;

impl BindingExtraCheck for NoExtraChecks {
    fn check(&self, _: &BindingRecord) -> Result<(), LicenceError> {
        Ok(())
    }
}

/// What the node's mesh admission looks like when a binding arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdmissionPosture {
    /// Admission mode is `enforce` (not `off` or `observe`).
    pub enforce: bool,
    /// A governance verdict source is bound.
    pub verdict_source_bound: bool,
    /// `admission_open_membership` is on.
    pub open_membership: bool,
}

impl AdmissionPosture {
    /// A binding is accepted only under enforce, with a verdict source and
    /// open membership off.
    pub fn check(&self) -> Result<(), LicenceError> {
        if self.open_membership {
            Err(LicenceError::BindingRefused("open_membership"))
        } else if !self.enforce {
            Err(LicenceError::BindingRefused("admission_not_enforce"))
        } else if !self.verdict_source_bound {
            Err(LicenceError::BindingRefused("no_verdict_source"))
        } else {
            Ok(())
        }
    }
}
