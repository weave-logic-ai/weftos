//! The run gate as placement uses it (ADR-106 section 4 step 7, phase 3).
//!
//! [`check_run`] wraps [`super::may_run`] with the two things placement needs
//! on top: whether the gate applies at all, and a stable reason for every
//! refusal (`weaver workload place --explain` shows it, and the host chains
//! it as `workload.refuse`).
//!
//! - **Applies** when this node holds a Seed binding (bound, unbound or
//!   orphaned), its store is unreadable, the sticky bound marker exists (a
//!   deleted store file does not reset it), or it holds approvals. A node
//!   with none of these is not in a Seed-bound mesh, and its Cognitum cogs
//!   keep the ADR-105 path (operator-signed package) unchanged:
//!   [`RunVerdict::NotSeedBound`].
//! - **Refusals**, in this order: no binding in effect (`binding_inactive`),
//!   no grant held for the cog version (`no_grant`), a grant held but no
//!   longer valid (`grant_lapsed`: expired, withdrawn, key revoked), a valid
//!   grant that does not list this binary (`not_in_grant`), the binary's
//!   BLAKE3 revoked (`hash_revoked`), no operator approval covering its
//!   sha256 (`no_approval`).

use std::sync::Arc;

use super::gate::{RunPermit, RunRequest};
use super::{ApprovalStore, CheckoutGrantStore};

/// Why the gate refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RunRefusal {
    /// A binding is held but none is in effect (unbound, orphaned by a mesh
    /// id change, no local mesh id, or the store is unreadable).
    #[error("no Seed binding is in effect on this node ({0})")]
    BindingInactive(String),
    /// No grant for this cog version was ever received here.
    #[error("no checkout grant is held for this cog version")]
    NoGrant,
    /// A grant is held but is not valid now.
    #[error("the checkout grant for this cog version has lapsed (expired, withdrawn or its key revoked)")]
    GrantLapsed,
    /// A valid grant exists but lists no binary with this (sha256, blake3).
    #[error("the valid checkout grant does not list this binary")]
    NotInGrant,
    /// The binary's BLAKE3 is revoked (this also withdraws its approval).
    #[error("the binary's hash is revoked")]
    HashRevoked,
    /// No operator approval for this cog version covers the binary's sha256.
    #[error("no operator approval covers this binary")]
    NoApproval,
    /// This daemon is another tenant's on a machine of a Seed-licensed mesh:
    /// Cognitum cogs there run under the machine's licence holder (the
    /// cluster owner's daemon), never on the ADR-105 path here.
    #[error("Cognitum cogs on this machine run under its licence holder, not this daemon ({0})")]
    NotHolder(String),
}

impl RunRefusal {
    /// The stable code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::BindingInactive(_) => "binding_inactive",
            Self::NoGrant => "no_grant",
            Self::GrantLapsed => "grant_lapsed",
            Self::NotInGrant => "not_in_grant",
            Self::HashRevoked => "hash_revoked",
            Self::NoApproval => "no_approval",
            Self::NotHolder(_) => "not_holder",
        }
    }

    /// What the operator does about it.
    pub fn remedy(&self, cog_id: &str, version: &str) -> String {
        match self {
            Self::BindingInactive(_) => "check `weaver workload node status`".into(),
            Self::NoGrant | Self::NotInGrant => {
                format!("weaver cog checkout {cog_id}@{version} --arch <arch>")
            }
            Self::GrantLapsed => "renew through the steward (a lapsed licence stops new starts)".into(),
            Self::HashRevoked => "the artifact was revoked; it cannot run".into(),
            Self::NoApproval => format!("weaver cog checkout approve {cog_id}@{version}"),
            Self::NotHolder(_) => {
                format!("place {cog_id}@{version} from the cluster owner's daemon (the machine's licence holder)")
            }
        }
    }
}

/// What the gate decided when it did not refuse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunVerdict {
    /// This node holds no Seed binding: the gate does not apply.
    NotSeedBound,
    /// Grant and approval cover the binary.
    Permit(RunPermit),
}

/// Decide `req`. `approvals` is `None` when this node has no approval store
/// (no mesh): every Cognitum run is then refused for `no_approval` once the
/// grant check passes.
pub fn check_run(
    grants: &CheckoutGrantStore,
    approvals: Option<&ApprovalStore>,
    req: &RunRequest<'_>,
) -> Result<RunVerdict, RunRefusal> {
    let approvals_held = approvals.is_some_and(|a| !a.is_empty() || a.poisoned().is_some());
    let bound_signal = grants.held_binding().is_some()
        || grants.poisoned().is_some()
        || grants.was_ever_bound()
        || approvals_held;
    if !bound_signal {
        return Ok(RunVerdict::NotSeedBound);
    }
    if grants.held_binding().is_none() && grants.poisoned().is_none() {
        // Bound before, but the store holds no binding now: the store file
        // was deleted or replaced. Fail closed until sync brings it back.
        return Err(RunRefusal::BindingInactive(
            "this node was Seed-bound but its licence store holds no binding (store missing?)".into(),
        ));
    }
    grants.binding_status().map_err(|e| RunRefusal::BindingInactive(e.to_string()))?;
    let Some((grant, art)) = grants.valid_grant_for_artifact(req.cog_id, req.version, req.sha256, req.blake3)
    else {
        if grants.held_grant(req.cog_id, req.version).is_none() {
            return Err(RunRefusal::NoGrant);
        }
        let valid = grants
            .verified_grants()
            .iter()
            .any(|v| v.grant().cog_id == req.cog_id && v.grant().version == req.version);
        return Err(if valid { RunRefusal::NotInGrant } else { RunRefusal::GrantLapsed });
    };
    if grants.is_hash_revoked(req.blake3) {
        return Err(RunRefusal::HashRevoked);
    }
    let approval_id = approvals
        .and_then(|a| a.covering(req.cog_id, req.version, req.sha256))
        .ok_or(RunRefusal::NoApproval)?;
    Ok(RunVerdict::Permit(RunPermit { grant_id: grant.grant_id, approval_id, blake3: art.blake3 }))
}

/// The gate a `workload-host` asks before it installs or starts a
/// Cognitum-origin cog.
pub trait CognitumRunGate: Send + Sync {
    /// Decide `req` (hashes computed from the bytes that will run).
    fn check(&self, req: &RunRequest<'_>) -> Result<RunVerdict, RunRefusal>;
    /// True when these bytes are known Cognitum bytes (listed by a held
    /// grant, or a revoked artifact hash) whatever the package says.
    fn claims(&self, _sha256: &str, _blake3: &str) -> bool {
        false
    }
    /// True when this BLAKE3 is revoked as an `ArtifactHash`.
    fn revoked(&self, _blake3: &str) -> bool {
        false
    }
}

/// [`CognitumRunGate`] over this node's stores.
pub struct StoreRunGate {
    /// The node's grant store.
    pub grants: Arc<CheckoutGrantStore>,
    /// The node's approval store (`None` without a mesh).
    pub approvals: Option<Arc<ApprovalStore>>,
}

impl CognitumRunGate for StoreRunGate {
    fn check(&self, req: &RunRequest<'_>) -> Result<RunVerdict, RunRefusal> {
        check_run(&self.grants, self.approvals.as_deref(), req)
    }

    fn claims(&self, sha256: &str, blake3: &str) -> bool {
        self.grants.claims_artifact(sha256, blake3) || self.grants.is_hash_revoked(blake3)
    }

    fn revoked(&self, blake3: &str) -> bool {
        self.grants.is_hash_revoked(blake3)
    }
}
