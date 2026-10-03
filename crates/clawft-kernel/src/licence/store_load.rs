//! Persisted shape of [`super::CheckoutGrantStore`] and its load-time
//! re-verification: nothing in the file is trusted.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::binding::verify_binding_signature;
use super::floor::FloorState;
use super::grant::verify_grant_signature;
use super::persist::read_capped;
use super::store::{Held, Inner, Slot};
use super::{
    CheckoutGrant, LicenceError, MAX_FLOORS, MAX_GRANT_SLOTS, MAX_UNIX_TIME, SignedBinding,
    SignedGrant, valid_token,
};
use crate::workload_pkg::TrustAnchors;
use crate::workload_pkg::codec::hex_decode_exact;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StoreFile {
    pub(super) v: u32,
    pub(super) binding: Option<SignedBinding>,
    pub(super) grants: Vec<SlotFile>,
    pub(super) floors: BTreeMap<String, FloorState>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SlotFile {
    pub(super) cog_id: String,
    pub(super) version: String,
    pub(super) current: Option<SignedGrant>,
    pub(super) previous: Option<SignedGrant>,
    pub(super) conflicted: Vec<u64>,
}

/// Read and re-verify a persisted store. Nothing in the file is trusted.
pub(super) fn load(path: &Path, anchors: &TrustAnchors) -> Result<Inner, LicenceError> {
    let Some(bytes) = read_capped(path)? else {
        return Ok(Inner::default());
    };
    let file: StoreFile =
        serde_json::from_slice(&bytes).map_err(|e| LicenceError::Malformed(e.to_string()))?;
    if file.v != 1 || file.grants.len() > MAX_GRANT_SLOTS {
        return Err(LicenceError::Malformed("store version or size".into()));
    }
    let binding = match file.binding {
        Some(s) => Some(Held { body: verify_binding_signature(&s, anchors)?, signed: s }),
        None => None,
    };
    // At most one floor per grant key, only for the bound key, and sane times.
    let bound_key = binding.as_ref().map(|b| b.body.grant_pubkey.as_str());
    let floors_ok = file.floors.len() <= MAX_FLOORS
        && file.floors.iter().all(|(k, f)| {
            Some(k.as_str()) == bound_key && f.max_issued <= MAX_UNIX_TIME && f.hw <= MAX_UNIX_TIME
        });
    if !floors_ok {
        return Err(LicenceError::Malformed("floors".into()));
    }
    let mut inner = Inner { binding, floors: file.floors, ..Inner::default() };
    inner.persisted_hw = inner.floors.values().map(|f| f.hw).max().unwrap_or(0);
    if file.grants.is_empty() {
        return Ok(inner);
    }
    let pk = inner
        .binding
        .as_ref()
        .and_then(|b| hex_decode_exact::<32>(&b.body.grant_pubkey))
        .ok_or_else(|| LicenceError::Malformed("grants without a binding".into()))?;
    let held = |s: Option<SignedGrant>| -> Result<Option<Held<CheckoutGrant>>, LicenceError> {
        s.map(|s| Ok(Held { body: verify_grant_signature(&s, &pk)?, signed: s })).transpose()
    };
    for s in file.grants {
        let slot = Slot {
            current: held(s.current)?,
            previous: held(s.previous)?,
            conflicted: s.conflicted,
        };
        let matches = |g: &Option<Held<CheckoutGrant>>| {
            g.as_ref().is_none_or(|h| h.body.cog_id == s.cog_id && h.body.version == s.version)
        };
        if !valid_token(&s.cog_id)
            || !valid_token(&s.version)
            || !matches(&slot.current)
            || !matches(&slot.previous)
        {
            return Err(LicenceError::Malformed("slot key".into()));
        }
        inner.slots.insert((s.cog_id, s.version), slot);
    }
    Ok(inner)
}
