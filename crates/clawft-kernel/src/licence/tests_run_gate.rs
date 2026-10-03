//! The placement run gate's verdicts and reasons (ADR-106 phase 3), and the
//! status listings.

use std::sync::Arc;

use super::tests_common::*;
use super::*;
use crate::revocation::{RevocationKind, RevocationList};

fn req<'a>(sha: &'a str, b3: &'a str) -> RunRequest<'a> {
    RunRequest { cog_id: "fall-detect", version: "1.2.0", sha256: sha, blake3: b3 }
}

fn check(fx: &Fx, arch: &str) -> Result<RunVerdict, RunRefusal> {
    let (sha, b3) = (sha_of(arch), b3_of(arch));
    check_run(&fx.store, Some(&fx.approvals), &req(&sha, &b3))
}

fn code(r: Result<RunVerdict, RunRefusal>) -> &'static str {
    r.unwrap_err().code()
}

#[test]
fn a_node_that_never_held_a_binding_is_not_gated() {
    let fx = Fx::new();
    assert_eq!(check(&fx, "aarch64"), Ok(RunVerdict::NotSeedBound));
}

#[test]
fn every_refusal_has_its_own_code_in_the_gate_order() {
    let fx = Fx::new();
    fx.bind();
    assert_eq!(code(check(&fx, "aarch64")), "no_grant");
    fx.store.accept_grant(&grant(1, T0, 3600, &["aarch64"])).unwrap();
    assert_eq!(code(check(&fx, "aarch64")), "no_approval");
    // A binary the valid grant does not list.
    assert_eq!(code(check(&fx, "x86_64")), "not_in_grant");
    fx.approvals.accept(&approval(&[sha_of("aarch64")])).unwrap();
    let Ok(RunVerdict::Permit(p)) = check(&fx, "aarch64") else { panic!("permit") };
    assert_eq!(p.blake3, b3_of("aarch64"));
    // Revoking the artifact hash withdraws it.
    let list = Arc::new(RevocationList::new(fx.dir.path().join("revoked.json")));
    fx.store.attach_revocations(list.clone());
    list.revoke_subject(RevocationKind::ArtifactHash, &b3_of("aarch64"), "withdrawn").unwrap();
    assert_eq!(code(check(&fx, "aarch64")), "hash_revoked");
}

#[test]
fn a_lapsed_grant_and_an_inactive_binding_are_named() {
    let fx = Fx::new();
    fx.bind();
    fx.store.accept_grant(&grant(1, T0, 3600, &["aarch64"])).unwrap();
    fx.approvals.accept(&approval(&[sha_of("aarch64")])).unwrap();
    assert!(matches!(check(&fx, "aarch64"), Ok(RunVerdict::Permit(_))));
    fx.set_now(T0 + 3601);
    assert_eq!(code(check(&fx, "aarch64")), "grant_lapsed");
    fx.store.accept_binding(&binding(2, BindState::Unbound), posture(), &NoExtraChecks).unwrap();
    let e = check(&fx, "aarch64").unwrap_err();
    assert_eq!(e.code(), "binding_inactive");
    assert!(e.remedy("fall-detect", "1.2.0").contains("node status"));
}

#[test]
fn without_an_approval_store_a_cognitum_run_is_refused_for_no_approval() {
    let fx = Fx::new();
    fx.bind();
    fx.store.accept_grant(&grant(1, T0, 3600, &["aarch64"])).unwrap();
    let (sha, b3) = (sha_of("aarch64"), b3_of("aarch64"));
    let gate = StoreRunGate { grants: fx.store.clone(), approvals: None };
    assert_eq!(gate.check(&req(&sha, &b3)).unwrap_err(), RunRefusal::NoApproval);
    assert!(RunRefusal::NoApproval.remedy("fall-detect", "1.2.0").contains("checkout approve fall-detect@1.2.0"));
}

#[test]
fn status_rows_list_validity_and_orphaned_approvals() {
    let fx = Fx::new();
    fx.bind();
    fx.store.accept_grant(&grant(1, T0, 3600, &["aarch64"])).unwrap();
    fx.approvals.accept(&approval(&[sha_of("aarch64")])).unwrap();
    let rows = fx.store.grant_rows();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].valid && !rows[0].withdrawn);
    assert_eq!(rows[0].artifacts[0].sha256, sha_of("aarch64"));
    assert_eq!(fx.store.effective_now(), Some(T0));
    fx.set_now(T0 + 7200);
    assert!(!fx.store.grant_rows()[0].valid, "expired");
    let a = fx.approvals.rows();
    assert_eq!(a.len(), 1);
    assert!(a[0].active);
    assert!(fx.approvals.orphaned_approvals().is_empty());
    // A mesh id change orphans it.
    fx.local.set(Some(other_mesh()));
    assert!(!fx.approvals.rows()[0].active);
    assert_eq!(fx.approvals.orphaned_approvals()[0].cog_id, "fall-detect");
    assert!(fx.store.grant_rows().iter().all(|r| !r.valid), "no binding in effect for the new id");
}


#[test]
fn a_deleted_store_on_a_node_that_was_bound_fails_closed() {
    let mut fx = Fx::new();
    fx.bind();
    fx.store.accept_grant(&grant(1, T0, 3600, &["aarch64"])).unwrap();
    fx.approvals.accept(&approval(&[sha_of("aarch64")])).unwrap();
    assert!(matches!(check(&fx, "aarch64"), Ok(RunVerdict::Permit(_))));
    assert!(fx.dir.path().join(BOUND_MARKER).exists(), "the first binding wrote the marker");
    std::fs::remove_file(fx.dir.path().join(super::store::GRANTS_FILE)).unwrap();
    std::fs::remove_file(fx.dir.path().join(super::approval_store::APPROVALS_FILE)).unwrap();
    fx.restart();
    assert!(fx.store.held_binding().is_none() && fx.approvals.is_empty());
    let e = check(&fx, "aarch64").unwrap_err();
    assert_eq!(e.code(), "binding_inactive");
    // Without the marker too, held approvals alone keep the gate on.
    std::fs::remove_file(fx.dir.path().join(BOUND_MARKER)).unwrap();
    fx.approvals.accept(&approval(&[sha_of("aarch64")])).unwrap();
    assert_eq!(check(&fx, "aarch64").unwrap_err().code(), "binding_inactive");
}

#[test]
fn a_poisoned_store_fails_closed() {
    let fx = Fx::new();
    std::fs::write(fx.dir.path().join(super::store::GRANTS_FILE), b"{not json").unwrap();
    let poisoned = CheckoutGrantStore::open_or_poisoned(fx.dir.path(), anchors(), fx.local.clone(), clock_of(&fx.clock));
    assert!(poisoned.poisoned().is_some());
    let (sha, b3) = (sha_of("aarch64"), b3_of("aarch64"));
    let e = check_run(&poisoned, Some(&fx.approvals), &req(&sha, &b3)).unwrap_err();
    assert_eq!(e.code(), "binding_inactive");
}

#[test]
fn a_held_grant_claims_its_bytes_and_a_revoked_hash_is_claimed_too() {
    let fx = Fx::new();
    fx.bind();
    fx.store.accept_grant(&grant(1, T0, 3600, &["aarch64"])).unwrap();
    let gate = StoreRunGate { grants: fx.store.clone(), approvals: Some(fx.approvals.clone()) };
    assert!(gate.claims(&sha_of("aarch64"), "x"));
    assert!(gate.claims("x", &b3_of("aarch64")));
    assert!(!gate.claims(&sha_of("x86_64"), &b3_of("x86_64")));
    let list = Arc::new(RevocationList::new(fx.dir.path().join("revoked.json")));
    fx.store.attach_revocations(list.clone());
    list.revoke_subject(RevocationKind::ArtifactHash, &b3_of("x86_64"), "bad").unwrap();
    assert!(gate.claims("x", &b3_of("x86_64")));
}
