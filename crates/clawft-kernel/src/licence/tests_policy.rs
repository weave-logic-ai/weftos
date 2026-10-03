//! Policy, the exchange grant source and the run gate.

use std::sync::Arc;

use super::tests_common::*;
use super::*;
use crate::artifact_store::ArtifactStore;
use crate::mesh_artifact::{ArtifactExchange, ExchangeConfig};
use crate::mesh_swarm_state::{
    Audience, GrantInfo, GrantOrigin, ManifestPolicy, RedistributionPolicy, ServePeer,
};
use crate::revocation::{RevocationKind, RevocationList};

const DAY: u64 = 86_400;

fn cognitum(version: &str) -> GrantInfo {
    GrantInfo {
        package_id: format!("checkout:fall-detect@{version}"),
        signers: vec![pk_hex(&grant_key())],
        origin: GrantOrigin::Cognitum { cog_id: "fall-detect".into(), version: version.into() },
    }
}

fn opt_in() -> GrantInfo {
    GrantInfo { package_id: "other".into(), signers: vec![], origin: GrantOrigin::OptIn }
}

fn not_flagged() -> GrantInfo {
    GrantInfo { package_id: "nf".into(), signers: vec![], origin: GrantOrigin::NotFlagged }
}

fn hash() -> [u8; 32] {
    b3_bytes("aarch64")
}

fn policy(fx: &Fx) -> MeshCheckoutPolicy {
    MeshCheckoutPolicy::new(fx.store.clone())
}

fn serve_verified() -> ServePeer {
    ServePeer::verified("peer-1")
}

fn serve_claimed() -> ServePeer {
    ServePeer::unverified("peer-2")
}

fn allowed(p: &impl RedistributionPolicy, g: &[GrantInfo], a: &Audience<'_>) -> bool {
    p.allows(&hash(), g, a)
}

fn bind_and_grant(fx: &Fx) {
    fx.bind();
    fx.store.accept_grant(&grant(1, T0, DAY, &["aarch64"])).unwrap();
}

#[test]
fn without_a_binding_the_policy_equals_manifest_policy() {
    let fx = Fx::new();
    let (p, base) = (policy(&fx), ManifestPolicy);
    let (v, c) = (serve_verified(), serve_claimed());
    let audiences = [Audience::Advertise, Audience::Seed, Audience::Serve(&v), Audience::Serve(&c)];
    let lists: Vec<Vec<GrantInfo>> = vec![
        vec![],
        vec![opt_in()],
        vec![not_flagged()],
        vec![cognitum("1.2.0")],
        vec![opt_in(), cognitum("1.2.0")],
        vec![opt_in(), not_flagged()],
    ];
    for g in &lists {
        for a in &audiences {
            assert_eq!(allowed(&p, g, a), allowed(&base, g, a), "{g:?} / {a:?}");
        }
    }
    // Also with a poisoned store and a refused binding (open membership).
    let open = AdmissionPosture { open_membership: true, ..posture() };
    let _ = fx.store.accept_binding(&binding(1, BindState::Bound), open, &NoExtraChecks);
    for g in &lists {
        for a in &audiences {
            assert_eq!(allowed(&p, g, a), allowed(&base, g, a));
        }
    }
}

#[test]
fn a_binding_that_arrives_at_runtime_takes_effect_without_a_restart() {
    let fx = Fx::new();
    let p = policy(&fx); // built first, as the daemon does
    let (v, g) = (serve_verified(), [cognitum("1.2.0")]);
    assert!(!allowed(&p, &g, &Audience::Serve(&v)));
    bind_and_grant(&fx);
    assert!(allowed(&p, &g, &Audience::Serve(&v)));
    assert!(allowed(&p, &g, &Audience::Seed));
}

#[test]
fn only_verified_serve_and_seed_are_allowed_never_advertise() {
    let fx = Fx::new();
    bind_and_grant(&fx);
    let p = policy(&fx);
    let (v, c, g) = (serve_verified(), serve_claimed(), [cognitum("1.2.0")]);
    assert!(allowed(&p, &g, &Audience::Serve(&v)));
    assert!(allowed(&p, &g, &Audience::Seed));
    assert!(!allowed(&p, &g, &Audience::Advertise));
    assert!(!allowed(&p, &g, &Audience::Serve(&c)));
}

#[test]
fn the_grant_must_cover_the_hash_cog_and_version() {
    let fx = Fx::new();
    bind_and_grant(&fx);
    let (p, v) = (policy(&fx), serve_verified());
    assert!(!allowed(&p, &[cognitum("9.9.9")], &Audience::Serve(&v)), "other version");
    assert!(!p.allows(&b3_bytes("x86_64"), &[cognitum("1.2.0")], &Audience::Serve(&v)), "other hash");
    let mut other_cog = cognitum("1.2.0");
    other_cog.origin =
        GrantOrigin::Cognitum { cog_id: "other-cog".into(), version: "1.2.0".into() };
    assert!(!allowed(&p, &[other_cog], &Audience::Serve(&v)));
}

#[test]
fn any_non_cognitum_non_optin_grant_vetoes() {
    let fx = Fx::new();
    bind_and_grant(&fx);
    let (p, v) = (policy(&fx), serve_verified());
    assert!(!allowed(&p, &[cognitum("1.2.0"), not_flagged()], &Audience::Serve(&v)));
    assert!(!allowed(&p, &[], &Audience::Serve(&v)));
    // An OptIn grant alongside is fine.
    assert!(allowed(&p, &[cognitum("1.2.0"), opt_in()], &Audience::Serve(&v)));
}

#[test]
fn expiry_unbind_and_withdrawal_each_stop_serving() {
    let (v, g) = (serve_verified(), [cognitum("1.2.0")]);
    let fx = Fx::new();
    bind_and_grant(&fx);
    let p = policy(&fx);
    assert!(allowed(&p, &g, &Audience::Seed));
    fx.set_now(T0 + DAY);
    assert!(!allowed(&p, &g, &Audience::Serve(&v)), "expired");

    let fx = Fx::new();
    bind_and_grant(&fx);
    let p = policy(&fx);
    fx.store
        .accept_binding(&binding(2, BindState::Unbound), posture(), &NoExtraChecks)
        .unwrap();
    assert!(!allowed(&p, &g, &Audience::Serve(&v)), "unbound");

    let fx = Fx::new();
    bind_and_grant(&fx);
    let p = policy(&fx);
    fx.store.accept_grant(&grant(2, T0 + 1, 0, &["aarch64"])).unwrap();
    assert!(!allowed(&p, &g, &Audience::Serve(&v)), "withdrawn");
}

#[test]
fn an_orphaned_binding_turns_the_checkout_policy_off() {
    let fx = Fx::new();
    bind_and_grant(&fx);
    let (p, v, g) = (policy(&fx), serve_verified(), [cognitum("1.2.0")]);
    assert!(allowed(&p, &g, &Audience::Serve(&v)));
    fx.local.set(Some(other_mesh()));
    assert!(!allowed(&p, &g, &Audience::Serve(&v)));
}

// ── through a real exchange ──────────────────────────────────────

fn exchange(fx: &Fx) -> (ArtifactExchange, Arc<RevocationList>) {
    let cfg = ExchangeConfig { redistribution: Arc::new(policy(fx)), ..ExchangeConfig::default() };
    let ex = ArtifactExchange::new("node-a", Arc::new(ArtifactStore::new_memory()), cfg).unwrap();
    let list = Arc::new(RevocationList::new(fx.dir.path().join("revoked.json")));
    ex.set_revocations(list.clone());
    fx.store.attach_revocations(list.clone());
    (ex, list)
}

#[test]
fn grant_checkout_makes_bytes_shareable_for_serve_and_seed_only() {
    let fx = Fx::new();
    bind_and_grant(&fx);
    let (ex, _) = exchange(&fx);
    // Nothing is shareable before the grant source runs.
    assert!(ex.servable_grant(&hash(), &Audience::Seed).is_none());
    for v in fx.store.verified_grants() {
        ex.grant_checkout(&v);
    }
    let peer = serve_verified();
    assert!(ex.servable_grant(&hash(), &Audience::Seed).is_some());
    assert!(ex.servable_grant(&hash(), &Audience::Serve(&peer)).is_some());
    assert!(ex.servable_grant(&hash(), &Audience::Advertise).is_none());
    assert!(ex.servable_grant(&hash(), &Audience::Serve(&serve_claimed())).is_none());
    // Expiry stops serving; nothing was evicted by it.
    fx.set_now(T0 + DAY);
    assert!(ex.servable_grant(&hash(), &Audience::Seed).is_none());
}

#[test]
fn signer_key_revocation_ends_every_checkout_grant() {
    let fx = Fx::new();
    bind_and_grant(&fx);
    fx.store.accept_grant(&grant(2, T0, DAY, &["aarch64"])).unwrap();
    let (ex, list) = exchange(&fx);
    for v in fx.store.verified_grants() {
        ex.grant_checkout(&v);
    }
    assert!(ex.servable_grant(&hash(), &Audience::Seed).is_some());
    list.revoke_subject(RevocationKind::SignerKey, &pk_hex(&grant_key()), "stolen").unwrap();
    assert!(ex.servable_grant(&hash(), &Audience::Seed).is_none());
    assert_eq!(ex.apply_revocations().len(), 1);
    // And a revoked key's grants are not granted again.
    ex.grant_checkout(&VerifiedCheckoutGrant::for_test(grant_rec(3, T0, DAY, &["aarch64"])));
    assert!(ex.servable_grant(&hash(), &Audience::Seed).is_none());
}

#[test]
fn another_unrevoked_package_grant_keeps_only_its_own_policy_result() {
    let fx = Fx::new();
    bind_and_grant(&fx);
    let (ex, list) = exchange(&fx);
    for v in fx.store.verified_grants() {
        ex.grant_checkout(&v);
    }
    // A second package, opted in, lists the same hash.
    ex.grant_with(hash(), "pkg-optin", vec![pk_hex(&sk(60))], GrantOrigin::OptIn);
    assert!(ex.servable_grant(&hash(), &Audience::Seed).is_some());
    list.revoke_subject(RevocationKind::SignerKey, &pk_hex(&grant_key()), "stolen").unwrap();
    // Until the sweep the revoked Cognitum grant still vetoes: ManifestPolicy
    // needs every grant OptIn, and the checkout grant no longer validates.
    assert!(ex.servable_grant(&hash(), &Audience::Seed).is_none());
    ex.apply_revocations();
    // After the sweep only the opted-in package is left, and its own result
    // (ManifestPolicy: allowed) applies. The checkout grant adds nothing.
    let g = ex.servable_grant(&hash(), &Audience::Seed).unwrap();
    assert_eq!(g.package_id, "pkg-optin");
}

#[test]
fn a_revoked_hash_is_not_granted_by_grant_checkout() {
    let fx = Fx::new();
    bind_and_grant(&fx);
    let (ex, list) = exchange(&fx);
    list.revoke_subject(RevocationKind::ArtifactHash, &b3_of("aarch64"), "bad build").unwrap();
    for v in fx.store.verified_grants() {
        ex.grant_checkout(&v);
    }
    assert!(ex.servable_grant(&hash(), &Audience::Seed).is_none());
}

// ── run gate ─────────────────────────────────────────────────────

fn req<'a>(sha: &'a str) -> RunRequest<'a> {
    RunRequest { cog_id: "fall-detect", version: "1.2.0", sha256: sha }
}

fn gate(fx: &Fx, sha: &str) -> Result<RunPermit, RunDenied> {
    may_run(&fx.store, &fx.approvals, &req(sha))
}

#[test]
fn a_grant_alone_never_makes_bytes_runnable() {
    let fx = Fx::new();
    bind_and_grant(&fx);
    assert_eq!(gate(&fx, &sha_of("aarch64")), Err(RunDenied::NoApproval));
}

#[test]
fn an_approval_that_does_not_cover_the_sha256_is_refused() {
    let fx = Fx::new();
    bind_and_grant(&fx);
    fx.approvals.accept(&approval(&[sha_of("x86_64")])).unwrap();
    assert_eq!(gate(&fx, &sha_of("aarch64")), Err(RunDenied::NoApproval));
    // Same sha256, different version: not covered either.
    let mut other = approval_rec(&[sha_of("aarch64")], &mesh());
    other.version = "9.9.9".into();
    fx.approvals.accept(&sign_approval(&other, &op()).unwrap()).unwrap();
    assert_eq!(gate(&fx, &sha_of("aarch64")), Err(RunDenied::NoApproval));
}

#[test]
fn an_approval_without_a_valid_grant_is_refused() {
    let fx = Fx::new();
    fx.approvals.accept(&approval(&[sha_of("aarch64")])).unwrap();
    assert_eq!(gate(&fx, &sha_of("aarch64")), Err(RunDenied::NoValidGrant));
    bind_and_grant(&fx);
    assert!(gate(&fx, &sha_of("aarch64")).is_ok());
    fx.set_now(T0 + DAY); // the grant lapses: a start is refused
    assert_eq!(gate(&fx, &sha_of("aarch64")), Err(RunDenied::NoValidGrant));
}

#[test]
fn grant_plus_covering_approval_may_run_and_names_its_evidence() {
    let fx = Fx::new();
    bind_and_grant(&fx);
    let a = approval(&[sha_of("aarch64")]);
    fx.approvals.accept(&a).unwrap();
    let permit = gate(&fx, &sha_of("aarch64")).unwrap();
    assert_eq!(permit.blake3, b3_of("aarch64"));
    assert_eq!(permit.grant_id.len(), 64);
    let key = verify_approval(&a, &anchors(), &mesh()).unwrap().content_key();
    assert_eq!(permit.approval_id, key);
}

#[test]
fn an_approval_for_another_mesh_cannot_open_the_gate() {
    let fx = Fx::new();
    bind_and_grant(&fx);
    let wrong = sign_approval(&approval_rec(&[sha_of("aarch64")], &other_mesh()), &op()).unwrap();
    assert_eq!(fx.approvals.accept(&wrong), Err(LicenceError::WrongMesh));
    assert_eq!(gate(&fx, &sha_of("aarch64")), Err(RunDenied::NoApproval));
}

#[test]
fn an_artifact_hash_revocation_withdraws_the_approval() {
    let fx = Fx::new();
    bind_and_grant(&fx);
    let list = Arc::new(RevocationList::new(fx.dir.path().join("revoked.json")));
    fx.store.attach_revocations(list.clone());
    fx.approvals.accept(&approval(&[sha_of("aarch64")])).unwrap();
    assert!(gate(&fx, &sha_of("aarch64")).is_ok());
    list.revoke_subject(RevocationKind::ArtifactHash, &b3_of("aarch64"), "withdrawn").unwrap();
    assert_eq!(gate(&fx, &sha_of("aarch64")), Err(RunDenied::HashRevoked));
}
