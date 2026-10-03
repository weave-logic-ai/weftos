//! Binding record v2 (ADR-106 section 3): one Seed, one mesh.

pub use weft_licence_wire::{BindState, BindingRecord, SignedBinding, sign_binding};

use super::{LicenceError, MeshId};
use crate::workload_pkg::{KeyOrigin, TrustAnchors};

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
    weft_licence_wire::verify_binding_signature(signed, &|pk| {
        require_operator(anchors, pk).is_ok()
    })
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
