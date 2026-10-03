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

/// The sticky "this node was Seed-bound" marker, beside the store file. It
/// outlives a deleted or replaced store file, so the run gate fails closed
/// instead of treating such a node as never bound.
pub const BOUND_MARKER: &str = "licence-bound.marker";

impl CheckoutGrantStore {
    fn marker(&self) -> std::path::PathBuf {
        self.path.with_file_name(BOUND_MARKER)
    }

    /// Write the marker (best effort; logged on failure).
    pub(super) fn note_bound(&self) {
        let m = self.marker();
        if m.exists() {
            return;
        }
        if let Err(e) = super::persist::write_atomic(&m, b"bound\n") {
            tracing::warn!(error = %e, "could not write the licence bound marker");
        }
    }

    /// True once this node has accepted a binding (the marker exists, or a
    /// binding is held now, which also writes the marker).
    pub fn was_ever_bound(&self) -> bool {
        if self.marker().exists() {
            return true;
        }
        if self.held_binding().is_some() {
            self.note_bound();
            return true;
        }
        false
    }

    /// True when a held grant (current or previous) lists a binary with
    /// this sha256 or this BLAKE3: the bytes are a checked-out Cognitum cog,
    /// whatever package they arrive in.
    pub fn claims_artifact(&self, sha256: &str, blake3: &str) -> bool {
        let g = self.lock();
        g.slots
            .values()
            .flat_map(|s| s.current.iter().chain(s.previous.iter()))
            .flat_map(|h| h.body.artifacts.iter())
            .any(|a| a.sha256 == sha256 || a.blake3 == blake3)
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

    /// The signed envelopes of the orphaned approvals: the CLI verifies each
    /// against the operator's own key before it re-signs it.
    pub fn orphaned_signed(&self) -> Vec<super::SignedApproval> {
        let local = self.local.get().map(|m| m.to_hex());
        let g = self.lock();
        g.0.values().filter(|(_, a)| Some(&a.mesh_id) != local.as_ref()).map(|(s, _)| s.clone()).collect()
    }

    /// The orphaned approvals themselves (`--reapprove-orphaned` re-signs
    /// them for the new mesh id).
    pub fn orphaned_approvals(&self) -> Vec<Approval> {
        self.rows().into_iter().filter(|r| !r.active).map(|r| r.approval).collect()
    }
}
