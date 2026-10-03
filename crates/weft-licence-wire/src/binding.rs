//! Binding record v2 (ADR-106 section 3): one Seed, one mesh.

use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};

use crate::{
    BINDING_DOMAIN, LicenceError, SignedEnvelope, envelope_key, parse_canonical, sign_envelope,
    valid_hex32, valid_token, verify_envelope,
};

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
    /// The mesh, as `MeshId::to_hex`.
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

/// Signature and shape only, with no mesh check. `operator_ok` says whether
/// a signer key is a pinned operator key; the caller owns that policy.
pub fn verify_binding_signature(
    signed: &SignedBinding,
    operator_ok: &dyn Fn(&[u8; 32]) -> bool,
) -> Result<BindingRecord, LicenceError> {
    let pk = envelope_key(signed)?;
    if !operator_ok(&pk) {
        return Err(LicenceError::UntrustedKey);
    }
    verify_envelope(BINDING_DOMAIN, signed, &pk)?;
    let rec: BindingRecord = parse_canonical(&signed.payload)?;
    if rec.v != 2 {
        return Err(LicenceError::Malformed("binding version".into()));
    }
    let hex_ok = [&rec.device_pubkey, &rec.mesh_id, &rec.grant_pubkey, &rec.steward_pubkey]
        .iter()
        .all(|s| valid_hex32(s));
    if !hex_ok || !valid_token(&rec.device_id) || !valid_token(&rec.steward_node_id) {
        return Err(LicenceError::Malformed("binding field".into()));
    }
    Ok(rec)
}
