//! The run gate (ADR-106 section 4, step 7): bytes are installed or run only
//! with BOTH a valid grant for this mesh and an operator hash approval whose
//! sha256 set covers the binary. A grant alone never makes bytes runnable,
//! and the grant key cannot sign an approval. Not wired into placement yet
//! (phase 3); exported and tested here.

use super::{ApprovalStore, CheckoutGrantStore};

/// What is about to be installed or run.
#[derive(Debug, Clone, Copy)]
pub struct RunRequest<'a> {
    /// Cog id.
    pub cog_id: &'a str,
    /// Cog version.
    pub version: &'a str,
    /// sha256 of the binary, lower-case hex, computed from the bytes.
    pub sha256: &'a str,
    /// BLAKE3 of the binary, lower-case hex, computed from the bytes (never
    /// copied from a grant: the revocation check is made on this value).
    pub blake3: &'a str,
}

/// Why the gate refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RunDenied {
    /// No valid grant for this mesh lists a binary with exactly this
    /// (sha256, blake3) pair.
    #[error("no valid checkout grant covers this binary")]
    NoValidGrant,
    /// No operator approval for this cog version covers this sha256.
    #[error("no operator approval covers this binary")]
    NoApproval,
    /// The binary's BLAKE3 is revoked (this also withdraws its approval).
    #[error("the binary's hash is revoked")]
    HashRevoked,
}

/// Evidence for the run, for provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunPermit {
    /// The covering grant's id.
    pub grant_id: String,
    /// The covering approval's content key.
    pub approval_id: String,
    /// The binary's BLAKE3, lower-case hex.
    pub blake3: String,
}

/// May `req` be installed or run? Needs a valid grant and a covering approval,
/// and the binary's `ArtifactHash` must not be revoked.
pub fn may_run(
    grants: &CheckoutGrantStore,
    approvals: &ApprovalStore,
    req: &RunRequest<'_>,
) -> Result<RunPermit, RunDenied> {
    let (grant, art) = grants
        .valid_grant_for_artifact(req.cog_id, req.version, req.sha256, req.blake3)
        .ok_or(RunDenied::NoValidGrant)?;
    if grants.is_hash_revoked(req.blake3) {
        return Err(RunDenied::HashRevoked);
    }
    let approval_id = approvals
        .covering(req.cog_id, req.version, req.sha256)
        .ok_or(RunDenied::NoApproval)?;
    Ok(RunPermit { grant_id: grant.grant_id, approval_id, blake3: art.blake3 })
}
