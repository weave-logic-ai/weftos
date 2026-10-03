//! The licence form of binding (ADR-106 phase 1d): the STEWARD profile run
//! over an operator-signed `licence::BindingRecord` v2.
//!
//! Order, so an untrusted record costs the Seed and the replay state nothing:
//! admission posture, pinned link, member checks (operator signature,
//! canonical payload, `mesh_id`), the steward checks that need no Seed, then
//! the live `GET /api/v1/identity` match, then the replay memory (persisted
//! before anything is accepted), then the store. A refusal is chained as
//! `workload.refuse`; a bind as `workload.node.bind`.
//!
//! If the store refuses after the replay memory was persisted (it cannot,
//! after the `seq` pre-check, except on a disk error), that `bound_at` is
//! spent and the operator signs a newer record.

use serde_json::json;
use sha2::{Digest, Sha256};

use super::{
    BIND_CHAIN_SOURCE, BindError, SeedBinder, write_state, MAX_BOUND_DEVICES,
};
use crate::chain::{EVENT_KIND_WORKLOAD_NODE_BIND, EVENT_KIND_WORKLOAD_NODE_UNBIND};
use crate::licence::{
    AdmissionPosture, BindState as LicState, BindingExtraCheck, BindingRecord, CheckoutGrantStore, LicenceError,
    NoExtraChecks, Outcome, SignedBinding, StewardCheck, key_id,
};
use crate::workload_pkg::codec::hex_decode_exact;
use crate::workload_runtime::seed::SeedApiRuntime;
use crate::workload_runtime::types::WorkloadRuntime;

/// Stable chain code of a licence refusal.
pub(super) fn licence_code(e: &LicenceError) -> &'static str {
    match e {
        LicenceError::Malformed(_) | LicenceError::TooLarge => "malformed",
        LicenceError::BadSignature => "bad_signature",
        LicenceError::UntrustedKey => "untrusted_operator",
        LicenceError::WrongMesh => "wrong_mesh",
        LicenceError::NoLocalMesh => "no_local_mesh",
        LicenceError::NoBinding => "no_binding",
        LicenceError::Conflict(_) => "binding_conflict",
        LicenceError::BindingRefused(_) => "binding_refused",
        LicenceError::Poisoned(_) => "store_poisoned",
        LicenceError::Persist(_) => "state_unwritable",
        LicenceError::CheckFailed(c) => match c.as_str() {
            "not_a_bind" => "not_a_bind",
            "not_an_unbind" => "not_an_unbind",
            "steward_mismatch" => "steward_mismatch",
            "fingerprint_mismatch" => "fingerprint_mismatch",
            "expired" => "expired",
            "seq_not_greater" => "seq_not_greater",
            "device_mismatch" => "device_mismatch",
            "identity_mismatch_device_id" => "identity_mismatch",
            "identity_mismatch_device_key" => "identity_mismatch",
            _ => "check_failed",
        },
        _ => "licence_refused",
    }
}

/// Everything one steward bind needs.
pub struct StewardBind<'a> {
    /// The operator-signed v2 record.
    pub signed: &'a SignedBinding,
    /// The Seed's adapter runtime (its link must be pinned).
    pub rt: &'a SeedApiRuntime,
    /// The node's checkout store (holds the live local mesh id).
    pub store: &'a CheckoutGrantStore,
    /// The node's admission state.
    pub posture: AdmissionPosture,
    /// The grant key fingerprint the operator confirmed at `init` time.
    pub confirmed_fingerprint: &'a str,
    /// This node's id (the steward the record must name).
    pub steward_node_id: &'a str,
    /// This node's request-signing key, 64 hex chars.
    pub steward_pubkey: &'a str,
    /// Now, unix seconds.
    pub now: u64,
}

fn lic(e: LicenceError) -> BindError {
    BindError::Licence(e)
}

impl SeedBinder {
    /// The mesh id (hex) a v2 record bound `device_id` to, while bound.
    pub fn bound_mesh(&self, device_id: &str) -> Option<String> {
        self.meshes.lock().ok()?.get(device_id).cloned()
    }

    /// Steward-profile bind of a v2 record. See the module docs.
    pub async fn bind_v2(&self, b: &StewardBind<'_>) -> Result<BindingRecord, BindError> {
        match self.verify_v2(b).await {
            Ok(r) => Ok(r),
            Err(e) => {
                self.refuse_record(&b.signed.payload, &e);
                Err(e)
            }
        }
    }

    fn steward_check(&self, b: &StewardBind<'_>, held: Option<u64>) -> StewardCheck {
        StewardCheck {
            now: b.now,
            max_age_secs: self.max_age_secs,
            skew_secs: super::BIND_CLOCK_SKEW_SECS,
            confirmed_fingerprint: b.confirmed_fingerprint.to_owned(),
            steward_node_id: b.steward_node_id.to_owned(),
            steward_pubkey: b.steward_pubkey.to_owned(),
            held_seq: held,
            seed_identity: None,
        }
    }

    async fn verify_v2(&self, b: &StewardBind<'_>) -> Result<BindingRecord, BindError> {
        let poisoned = || BindError::Malformed("binder poisoned".into());
        b.posture.check().map_err(lic)?;
        // Before anything is read from the Seed: an identity read over an
        // unauthenticated link proves nothing.
        b.rt.link_security()
            .require_pinned(b.rt.node_id())
            .map_err(BindError::UnpinnedTransport)?;
        let rec = b.store.verify_member(b.signed).map_err(lic)?;
        let held = b.store.held_binding().map(|h| h.seq);
        let mut check = self.steward_check(b, held);
        check.check(&rec).map_err(lic)?;
        // The Seed is asked last, so an untrusted record costs it nothing.
        let (device_id, device_key) = b
            .rt
            .identity()
            .await
            .map_err(|e| BindError::Seed(e.to_string()))?;
        check.seed_identity = Some((device_id, device_key));
        check.check(&rec).map_err(lic)?;

        let digest: [u8; 32] = Sha256::digest(
            [b.signed.payload.as_bytes(), b.signed.signature.as_bytes()].concat(),
        )
        .into();
        let mut seen = self.seen.lock().map_err(|_| poisoned())?;
        let mut bound = self.bound.lock().map_err(|_| poisoned())?;
        let mut meshes = self.meshes.lock().map_err(|_| poisoned())?;
        if seen.contains(&digest) {
            return Err(BindError::Replayed);
        }
        if bound.get(&rec.device_id).is_some_and(|(at, _)| rec.bound_at <= *at) {
            return Err(BindError::Replayed);
        }
        if meshes.get(&rec.device_id).is_some_and(|m| *m != rec.mesh_id) {
            return Err(BindError::SeedBoundElsewhere { device_id: rec.device_id });
        }
        if !bound.contains_key(&rec.device_id) && bound.len() >= MAX_BOUND_DEVICES {
            return Err(BindError::State("too many bound devices".into()));
        }
        let node_id = b.rt.node_id().to_owned();
        let mut next = bound.clone();
        next.insert(rec.device_id.clone(), (rec.bound_at, node_id));
        let mut next_meshes = meshes.clone();
        next_meshes.insert(rec.device_id.clone(), rec.mesh_id.clone());
        // Saved before it is accepted: if the replay memory cannot be
        // persisted, nothing is bound.
        if let Some(path) = &self.state_file {
            write_state(path, &next, &next_meshes).map_err(BindError::State)?;
        }
        *bound = next;
        *meshes = next_meshes;
        seen.insert(digest);
        drop((seen, bound, meshes));
        match b.store.accept_binding(b.signed, b.posture, &check).map_err(lic)? {
            Outcome::Applied => {}
            Outcome::Duplicate | Outcome::Ignored => return Err(BindError::Replayed),
        }
        self.chain.append(
            BIND_CHAIN_SOURCE,
            EVENT_KIND_WORKLOAD_NODE_BIND,
            Some(json!({
                "v": 2,
                "device_id": rec.device_id, "device_pubkey": rec.device_pubkey,
                "mesh_id": rec.mesh_id, "seq": rec.seq, "bound_at": rec.bound_at,
                "grant_fingerprint": b.confirmed_fingerprint,
                "steward_node_id": rec.steward_node_id,
                "operator_key": b.signed.public_key,
                "record_hash": crate::workload_pkg::codec::hex_encode(&Sha256::digest(
                    b.signed.payload.as_bytes())),
            })),
        );
        Ok(rec)
    }

    /// Withdraw the binding with an operator-signed `unbound` record. Needs
    /// no Seed (it may be gone). Any operator-Admin node may do it; the steward
    /// state forgets the device's mesh so the Seed can be bound again.
    pub fn unbind_v2(
        &self,
        signed: &SignedBinding,
        store: &CheckoutGrantStore,
    ) -> Result<BindingRecord, BindError> {
        match self.unbind_inner(signed, store) {
            Ok(r) => Ok(r),
            Err(e) => {
                self.refuse_record(&signed.payload, &e);
                Err(e)
            }
        }
    }

    fn unbind_inner(
        &self,
        signed: &SignedBinding,
        store: &CheckoutGrantStore,
    ) -> Result<BindingRecord, BindError> {
        let rec = store.verify_member(signed).map_err(lic)?;
        if rec.state != LicState::Unbound {
            return Err(lic(LicenceError::CheckFailed("not_an_unbind".into())));
        }
        let held = store.held_binding().ok_or(lic(LicenceError::NoBinding))?;
        if held.device_id != rec.device_id {
            return Err(lic(LicenceError::CheckFailed("device_mismatch".into())));
        }
        if rec.seq <= held.seq {
            return Err(lic(LicenceError::CheckFailed("seq_not_greater".into())));
        }
        // Turning the licence off must work under any admission posture.
        let off = AdmissionPosture {
            enforce: true,
            verdict_source_bound: true,
            open_membership: false,
        };
        store.accept_binding(signed, off, &NoExtraChecks).map_err(lic)?;
        let bound = self.bound.lock().map_err(|_| BindError::Malformed("binder poisoned".into()))?;
        let mut meshes =
            self.meshes.lock().map_err(|_| BindError::Malformed("binder poisoned".into()))?;
        let mut next = meshes.clone();
        next.remove(&rec.device_id);
        if let Some(path) = &self.state_file {
            write_state(path, &bound, &next).map_err(BindError::State)?;
        }
        *meshes = next;
        self.chain.append(
            BIND_CHAIN_SOURCE,
            EVENT_KIND_WORKLOAD_NODE_UNBIND,
            Some(json!({
                "v": 2, "device_id": rec.device_id, "mesh_id": rec.mesh_id, "seq": rec.seq,
                "operator_key": signed.public_key,
                "record_hash": crate::workload_pkg::codec::hex_encode(&Sha256::digest(
                    signed.payload.as_bytes())),
            })),
        );
        Ok(rec)
    }
}

/// The fingerprint of a grant key in hex form, for display and the CLI's
/// confirmation (`ed25519:` plus 16 hex).
pub fn grant_fingerprint(grant_pubkey_hex: &str) -> Option<String> {
    hex_decode_exact::<32>(grant_pubkey_hex).map(|k| key_id(&k))
}
