//! The one mesh binding (ADR-106 section 3), applied over USB.
//!
//! The binding is signed by a pinned operator key, so the record itself is the
//! authority and needs no steward signature (the steward key is only known
//! once the binding exists). It is applied by `weft-licence bind <file>`, not
//! over HTTP: the grant key and the binding both come from physical presence.

use std::path::Path;

use weft_licence_wire::{
    BindState, BindingRecord, SignedBinding, hex_decode_exact, verify_binding_signature,
};

use crate::error::SvcError;
use crate::fsio;
use crate::keys;
use crate::state::OperatorKeys;

const BINDING_FILE: &str = "binding.json";

/// An accepted binding.
#[derive(Debug, Clone)]
pub struct BindingState {
    /// The operator-signed envelope as stored.
    pub signed: SignedBinding,
    /// Its verified content.
    pub record: BindingRecord,
}

impl BindingState {
    /// The steward's request-signing key.
    pub fn steward_key(&self) -> [u8; 32] {
        hex_decode_exact::<32>(&self.record.steward_pubkey).unwrap_or([0; 32])
    }

    /// True while the binding is in force.
    pub fn is_bound(&self) -> bool {
        self.record.state == BindState::Bound
    }
}

/// Load and re-verify the stored binding. A file that no longer verifies is
/// corrupt: the service refuses to start rather than guess.
pub fn load(state_dir: &Path, ops: &OperatorKeys) -> Result<Option<BindingState>, SvcError> {
    let path = state_dir.join(BINDING_FILE);
    let Some(raw) = fsio::read_capped(&path, 64 * 1024).map_err(|e| SvcError::Io(e.to_string()))? else {
        return Ok(None);
    };
    let corrupt = || SvcError::Corrupt(path.display().to_string());
    let signed: SignedBinding = serde_json::from_slice(&raw).map_err(|_| corrupt())?;
    let record = verify_binding_signature(&signed, &|pk| ops.contains(pk)).map_err(|_| corrupt())?;
    Ok(Some(BindingState { signed, record }))
}

/// Apply an operator-signed binding record.
///
/// - Signed by a pinned operator key, v2.
/// - `device_id` equals this Seed's. (The device public key is not checked:
///   question C4, no device-key API yet. STUB.)
/// - `grant_pubkey` is this Seed's own grant key: the operator confirmed the
///   fingerprint `init` printed.
/// - `seq` above the stored one; an equal `seq` with different content is a
///   conflict.
/// - A `bound` record for another mesh while bound is `seed_bound_elsewhere`.
/// - An `unbound` record keeps the record (so `seq` persists) and deletes the
///   grant key; `init` runs again before the next bind.
pub fn apply(
    state_dir: &Path,
    device_id: &str,
    ops: &OperatorKeys,
    signed: &SignedBinding,
    current: Option<&BindingState>,
) -> Result<BindingState, SvcError> {
    let refuse = |m: &str| Err(SvcError::Bind(m.to_string()));
    let record = verify_binding_signature(signed, &|pk| ops.contains(pk))
        .map_err(|e| SvcError::Bind(e.to_string()))?;
    if record.device_id != device_id {
        return refuse("device_id is not this Seed's");
    }
    if let Some(cur) = current {
        if record.seq == cur.record.seq {
            return if record.mesh_id == cur.record.mesh_id && cur.signed.payload == signed.payload {
                Ok(cur.clone())
            } else {
                refuse("binding_conflict: a different record at an equal seq")
            };
        }
        if record.seq < cur.record.seq {
            return refuse("stale: seq is not above the stored one");
        }
        if cur.is_bound() && record.state == BindState::Bound && record.mesh_id != cur.record.mesh_id {
            return Err(SvcError::BoundElsewhere);
        }
        if record.state == BindState::Unbound && record.mesh_id != cur.record.mesh_id {
            return refuse("unbind names another mesh");
        }
    } else if record.state == BindState::Unbound {
        return refuse("nothing is bound");
    }
    if record.state == BindState::Bound {
        let sk = keys::load(state_dir)?;
        if hex_decode_exact::<32>(&record.grant_pubkey) != Some(sk.verifying_key().to_bytes()) {
            return refuse("grant_pubkey is not this Seed's grant key");
        }
    }
    let raw = serde_json::to_vec(signed).map_err(|e| SvcError::Persist(e.to_string()))?;
    fsio::write_atomic(&state_dir.join(BINDING_FILE), &raw).map_err(|e| SvcError::Persist(e.to_string()))?;
    if record.state == BindState::Unbound {
        keys::delete(state_dir)?;
    }
    Ok(BindingState { signed: signed.clone(), record })
}
