//! The STEWARD verification profile for a binding (ADR-106 section 3,
//! phase 1d), plugged into the store through [`BindingExtraCheck`].
//!
//! It runs after the member checks (operator signature, canonical payload,
//! `mesh_id` equal to the local id) and adds what only the steward can know:
//! the age window, the live Seed identity (read by the caller, because the
//! hook is synchronous), the grant key fingerprint the operator confirmed
//! over the USB link, that this node is the steward the record names, and a
//! `seq` strictly above the one held.

use super::{BindState, BindingExtraCheck, BindingRecord, LicenceError, key_id};
use crate::workload_pkg::codec::hex_decode_exact;

/// Inputs of the steward checks.
#[derive(Debug, Clone)]
pub struct StewardCheck {
    /// Now, unix seconds.
    pub now: u64,
    /// Oldest `bound_at` accepted, in seconds before `now` (600).
    pub max_age_secs: u64,
    /// Newest `bound_at` accepted, in seconds after `now`.
    pub skew_secs: u64,
    /// The grant key fingerprint (`ed25519:` plus 16 hex) the operator
    /// compared at `weft-licence init` time.
    pub confirmed_fingerprint: String,
    /// This node's id: the steward the record must name.
    pub steward_node_id: String,
    /// This node's request-signing key, 64 hex chars.
    pub steward_pubkey: String,
    /// The `seq` of the binding held now, if any.
    pub held_seq: Option<u64>,
    /// The Seed's live `(device_id, public_key)`. `None` skips the identity
    /// match (the pre-check that runs before the Seed is contacted).
    pub seed_identity: Option<(String, String)>,
}

fn fail(code: &str) -> LicenceError {
    LicenceError::CheckFailed(code.into())
}

impl BindingExtraCheck for StewardCheck {
    fn check(&self, r: &BindingRecord) -> Result<(), LicenceError> {
        if r.state != BindState::Bound {
            return Err(fail("not_a_bind"));
        }
        if r.steward_node_id != self.steward_node_id || r.steward_pubkey != self.steward_pubkey {
            return Err(fail("steward_mismatch"));
        }
        let gk = hex_decode_exact::<32>(&r.grant_pubkey).ok_or_else(|| fail("grant_key"))?;
        if key_id(&gk) != self.confirmed_fingerprint {
            return Err(fail("fingerprint_mismatch"));
        }
        if r.bound_at > self.now.saturating_add(self.skew_secs)
            || self.now.saturating_sub(r.bound_at) > self.max_age_secs
        {
            return Err(fail("expired"));
        }
        if self.held_seq.is_some_and(|h| r.seq <= h) {
            return Err(fail("seq_not_greater"));
        }
        if let Some((id, key)) = &self.seed_identity {
            if *id != r.device_id {
                return Err(fail("identity_mismatch_device_id"));
            }
            if *key != r.device_pubkey {
                return Err(fail("identity_mismatch_device_key"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests_common::*;
    use super::*;

    fn rec() -> BindingRecord {
        binding_rec(2, BindState::Bound, &grant_key(), &mesh())
    }

    fn check() -> StewardCheck {
        StewardCheck {
            now: T0,
            max_age_secs: 600,
            skew_secs: 60,
            confirmed_fingerprint: key_id(&grant_key().verifying_key().to_bytes()),
            steward_node_id: "node-steward".into(),
            steward_pubkey: pk_hex(&sk(21)),
            held_seq: Some(1),
            seed_identity: Some(("seed-test".into(), pk_hex(&sk(20)))),
        }
    }

    fn refused(c: &StewardCheck, r: &BindingRecord) -> String {
        match c.check(r) {
            Err(LicenceError::CheckFailed(code)) => code,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_matching_record_passes() {
        assert_eq!(check().check(&rec()), Ok(()));
    }

    #[test]
    fn each_check_refuses_with_its_own_code() {
        let mut c = check();
        c.confirmed_fingerprint = key_id(&sk(99).verifying_key().to_bytes());
        assert_eq!(refused(&c, &rec()), "fingerprint_mismatch");

        let mut c = check();
        c.steward_node_id = "other".into();
        assert_eq!(refused(&c, &rec()), "steward_mismatch");

        let mut c = check();
        c.now = T0 + 601;
        assert_eq!(refused(&c, &rec()), "expired");
        c.now = T0 - 61;
        assert_eq!(refused(&c, &rec()), "expired");
        c.now = T0 + 600;
        assert_eq!(c.check(&rec()), Ok(()), "600 s is inside the window");

        let mut c = check();
        c.held_seq = Some(2);
        assert_eq!(refused(&c, &rec()), "seq_not_greater");

        let mut c = check();
        c.seed_identity = Some(("someone-else".into(), pk_hex(&sk(20))));
        assert_eq!(refused(&c, &rec()), "identity_mismatch_device_id");
        c.seed_identity = Some(("seed-test".into(), pk_hex(&sk(77))));
        assert_eq!(refused(&c, &rec()), "identity_mismatch_device_key");

        let unbind = binding_rec(2, BindState::Unbound, &grant_key(), &mesh());
        assert_eq!(refused(&check(), &unbind), "not_a_bind");
    }

    #[test]
    fn the_precheck_without_a_seed_skips_only_the_identity_match() {
        let mut c = check();
        c.seed_identity = None;
        assert_eq!(c.check(&rec()), Ok(()));
        c.confirmed_fingerprint = "ed25519:0000000000000000".into();
        assert_eq!(refused(&c, &rec()), "fingerprint_mismatch");
    }
}
