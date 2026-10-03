//! Read-only listings for `weaver cog checkout status` and `weaver doctor`
//! (ADR-106 phase 3). Nothing here changes state or gates anything.

use serde::Serialize;

use super::approval::Approval;
use super::store::grant_valid;
use super::{ApprovalStore, CheckoutGrantStore, GrantArtifact};

/// One (cog, version) slot of the grant store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GrantRow {
    /// Cog id.
    pub cog_id: String,
    /// Version.
    pub version: String,
    /// The held grant's `seq`.
    pub seq: u64,
    /// Its id.
    pub grant_id: String,
    /// Unix seconds.
    pub issued_at: u64,
    /// Unix seconds (`<= issued_at` for a withdrawal).
    pub expires_at: u64,
    /// The licence expiry the grant carries.
    pub licence_expires: u64,
    /// A withdrawal (a renewal with `expires_at <= issued_at`).
    pub withdrawn: bool,
    /// Valid right now (binding in effect, key not revoked, unexpired by
    /// `max(now, floor)`).
    pub valid: bool,
    /// The artifacts it lists.
    pub artifacts: Vec<GrantArtifact>,
}

/// One held approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApprovalRow {
    /// The content key (provenance `approval_id`).
    pub approval_id: String,
    /// The approval.
    #[serde(flatten)]
    pub approval: Approval,
    /// False when it names another mesh id (orphaned by a `mesh_nonce` change).
    pub active: bool,
}

impl CheckoutGrantStore {
    /// Every held grant, valid or not, by (cog, version). Validity is judged
    /// as the policy judges it; without a binding in effect none is valid.
    pub fn grant_rows(&self) -> Vec<GrantRow> {
        self.run(|inner, ev| {
            let bound = self.binding_in_effect(inner, ev).ok();
            let eff = bound.as_ref().map(|b| {
                let revoked = self.key_revoked(&b.grant_pubkey);
                (self.eff_now(inner, &b.grant_pubkey), b.mesh_id.clone(), revoked)
            });
            inner
                .slots
                .values()
                .filter_map(|s| s.current.as_ref())
                .map(|h| {
                    let g = &h.body;
                    let valid = eff.as_ref().is_some_and(|(now, mesh, revoked)| {
                        !revoked && &g.mesh_id == mesh && grant_valid(g, *now)
                    });
                    GrantRow {
                        cog_id: g.cog_id.clone(),
                        version: g.version.clone(),
                        seq: g.seq,
                        grant_id: g.grant_id.clone(),
                        issued_at: g.issued_at,
                        expires_at: g.expires_at,
                        licence_expires: g.licence.expires,
                        withdrawn: g.is_withdrawal(),
                        valid,
                        artifacts: g.artifacts.clone(),
                    }
                })
                .collect()
        })
    }

    /// `max(now, floor)` for the bound key, the time grants are judged by.
    pub fn effective_now(&self) -> Option<u64> {
        self.run(|inner, ev| {
            let b = self.binding_in_effect(inner, ev).ok()?;
            Some(self.eff_now(inner, &b.grant_pubkey))
        })
    }
}

impl ApprovalStore {
    /// Every held approval, active or orphaned.
    pub fn rows(&self) -> Vec<ApprovalRow> {
        let local = self.local.get().map(|m| m.to_hex());
        let g = self.lock();
        g.0.iter()
            .map(|(k, (_, a))| ApprovalRow {
                approval_id: k.clone(),
                approval: a.clone(),
                active: g.1.is_none() && Some(&a.mesh_id) == local.as_ref(),
            })
            .collect()
    }

    /// The orphaned approvals themselves (`--reapprove-orphaned` re-signs
    /// them for the new mesh id).
    pub fn orphaned_approvals(&self) -> Vec<Approval> {
        self.rows().into_iter().filter(|r| !r.active).map(|r| r.approval).collect()
    }
}
