use chrono::Utc;
use clawft_kernel::token_authority::{TokenInfo, TokenScope};

use super::*;
use crate::handshake_rpc::BoundProject;

const P1: &str = "01JB8Z3Q0V6X9KQ4M2N7T5R1WD";
const P2: &str = "01JB8Z3Q0V6X9KQ4M2N7T5R1WE";

fn token(p: Option<&str>) -> Option<VerifiedProject> {
    VerifiedProject::from_token(&TokenInfo {
        id: "t".into(),
        label: "l".into(),
        issued_at: Utc::now(),
        expires_at: Utc::now(),
        scope: TokenScope::Owner,
        project: p.map(Into::into),
    })
}

fn bound(p: &str) -> Option<VerifiedProject> {
    VerifiedProject::from_bound(&BoundProject {
        project_id: Some(p.into()),
        ..Default::default()
    })
}

fn fwd(p: &str) -> Option<Result<VerifiedProject, ForwardError>> {
    Some(Ok(VerifiedProject::from_verified_forward(p.into())))
}

fn kind(r: Response) -> String {
    r.error_kind.unwrap_or_default()
}

#[test]
fn a_claim_with_no_verified_source_is_never_verified() {
    let r = reconcile(Sources::default(), Some(P1)).unwrap();
    assert!(r.is_none());
    // Even a registered-looking claim next to an unscoped token.
    let r = reconcile(Sources { token: token(None), ..Default::default() }, Some(P1)).unwrap();
    assert!(r.is_none());
}

#[test]
fn a_token_scoped_to_p1_cannot_act_as_p2() {
    let s = Sources { token: token(Some(P1)), ..Default::default() };
    assert_eq!(kind(reconcile(s, Some(P2)).unwrap_err()), SCOPE_MISMATCH_KIND);
    // Nor on a kernel bound to P2.
    let s = Sources { token: token(Some(P1)), bound: bound(P2), ..Default::default() };
    assert_eq!(kind(reconcile(s, None).unwrap_err()), SCOPE_MISMATCH_KIND);
    // Its own project, claimed or not, is verified.
    let s = Sources { token: token(Some(P1)), ..Default::default() };
    assert_eq!(reconcile(s, Some(P1)).unwrap().unwrap().as_str(), P1);
}

#[test]
fn a_childs_own_socket_is_verified_and_refuses_another_claim() {
    let s = Sources { bound: bound(P1), ..Default::default() };
    assert_eq!(reconcile(s, None).unwrap().unwrap().as_str(), P1);
    let s = Sources { bound: bound(P1), ..Default::default() };
    assert_eq!(kind(reconcile(s, Some(P2)).unwrap_err()), SCOPE_MISMATCH_KIND);
}

#[test]
fn forward_sources_and_failures() {
    let s = Sources { forward: fwd(P1), bound: bound(P1), ..Default::default() };
    assert_eq!(reconcile(s, Some(P1)).unwrap().unwrap().as_str(), P1);
    let s = Sources { forward: fwd(P1), ..Default::default() };
    assert_eq!(kind(reconcile(s, Some(P2)).unwrap_err()), SCOPE_MISMATCH_KIND);
    for (e, k) in [
        (ForwardError::BadSignature, "forward_bad_signature"),
        (ForwardError::Replayed, "forward_replayed"),
        (ForwardError::OutsideWindow, "forward_expired"),
        (ForwardError::Unavailable, "forward_unavailable"),
        (ForwardError::WrongProject, "project_scope_mismatch"),
    ] {
        let s = Sources { forward: Some(Err(e)), bound: bound(P1), ..Default::default() };
        assert_eq!(kind(reconcile(s, None).unwrap_err()), k);
    }
}

#[test]
fn the_strongest_source_wins_when_they_agree() {
    let s = Sources { token: token(Some(P1)), forward: fwd(P1), bound: bound(P1) };
    let v = reconcile(s, None).unwrap().unwrap();
    assert_eq!(v.attest().source(), clawft_kernel::governance::AttestSource::TokenScope);
}
