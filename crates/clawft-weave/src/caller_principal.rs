//! Establishing the caller's [`VerifiedProject`] (ADR-103 A6, Phase 2 package I).
//!
//! Called once per request by the daemon's authorization step, before the
//! capability check. The three cryptographic sources, and nothing else:
//!
//! 1. a validated token whose scope names a project
//!    ([`VerifiedProject::from_token`]);
//! 2. a user-signed forward header verified against the child's certificate
//!    key ([`crate::project_forward`]); only on a kernel bound to a project;
//! 3. the kernel's own bound project ([`VerifiedProject::from_bound`]): the
//!    socket you reached is the project's own.
//!
//! `Request.project` is never a source. It is only checked against them:
//! a claim that disagrees with a verified source is refused
//! (`project_scope_mismatch`), a claim with no verified source stays a
//! [`ClaimedProject`](crate::rpc_ext::ClaimedProject) and keeps its Phase 1
//! meaning (registry check in `scope_gate`, labelled `claimed` in audit).
//!
//! Honest limit: a [`VerifiedProject`] is only as strong as the transport.
//! A same-uid local process can still write any `Request.project` on the
//! claim path, and can use any unscoped token it can read. The verified
//! sources keep one user's projects from acting as each other; they are not
//! a boundary against a hostile local process (Phase 3 peer credentials,
//! Phase 4 sandboxes).

use clawft_rpc::{ForwardHeader, Response};

use crate::project_forward::ForwardError;
use crate::rpc_ext::CallerCtx;
use crate::verified_project::VerifiedProject;

/// `error_kind` when a request names a project other than its verified one.
pub const SCOPE_MISMATCH_KIND: &str = "project_scope_mismatch";

/// The verified sources found for a request, before they are reconciled.
#[derive(Default)]
pub struct Sources {
    pub token: Option<VerifiedProject>,
    pub forward: Option<Result<VerifiedProject, ForwardError>>,
    pub bound: Option<VerifiedProject>,
}

/// Reconcile `sources` with the client's `claim`. Pure.
///
/// Every verified source present must agree with the others and with the
/// claim; the result is the strongest of them (token, forward, bound).
pub fn reconcile(sources: Sources, claim: Option<&str>) -> Result<Option<VerifiedProject>, Response> {
    let mismatch = |what: &str| {
        Response::error_with_kind(
            SCOPE_MISMATCH_KIND,
            format!("the request names a project other than {what}"),
        )
    };
    let forward = match sources.forward {
        Some(Ok(v)) => Some(v),
        Some(Err(e)) => return Err(Response::error_with_kind(e.kind(), e.to_string())),
        None => None,
    };
    let present = [&sources.token, &forward, &sources.bound];
    let mut agreed: Option<&VerifiedProject> = None;
    for v in present.into_iter().flatten() {
        match agreed {
            Some(a) if a.as_str() != v.as_str() => {
                return Err(mismatch("the one its credentials are scoped to"));
            }
            _ => agreed = Some(v),
        }
    }
    if let (Some(a), Some(c)) = (agreed, claim)
        && a.as_str() != c
    {
        return Err(mismatch("the one its credentials are scoped to"));
    }
    Ok(sources.token.or(forward).or(sources.bound))
}

/// Gather the sources for `caller` and reconcile them. `Err` is the refusal.
pub async fn establish(
    caller: &CallerCtx,
    kernel: &crate::rpc_ext::KernelRef,
) -> Result<Option<VerifiedProject>, Response> {
    let bound_state = crate::handshake_rpc::bound();
    let bound = VerifiedProject::from_bound(&bound_state);
    let token = match caller.auth.as_deref().map(str::trim) {
        Some(t) if t.starts_with(clawft_kernel::token_authority::SECRET_PREFIX) => {
            match crate::token_rpc::authority_for(kernel).await {
                Some(a) => a.validate(t).and_then(|info| VerifiedProject::from_token(&info)),
                None => None,
            }
        }
        _ => None,
    };
    let forward = caller
        .forward
        .as_ref()
        .map(|h| verify_forward(h, bound_state.project_id.as_deref()));
    reconcile(Sources { token, forward, bound }, caller.project.as_ref().map(|c| c.as_str()))
}

/// Record this kernel's bound project (if any) as the project every
/// governance request is attributed to (daemon boot, once).
pub fn attest_instance(node_id: &str) {
    if let Some(v) = VerifiedProject::from_bound(&crate::handshake_rpc::bound()) {
        clawft_kernel::governance_project::set_instance_project(v.attest(), node_id);
    }
}

fn verify_forward(h: &ForwardHeader, bound: Option<&str>) -> Result<VerifiedProject, ForwardError> {
    match bound {
        Some(b) => crate::project_forward::verify_installed(h, b),
        // The user daemon is not a child: it has no one to verify a forward for.
        None => Err(ForwardError::Unavailable),
    }
}

#[cfg(test)]
#[path = "caller_principal_tests.rs"]
mod tests;
