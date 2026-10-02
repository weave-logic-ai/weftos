//! `authorize_caller` for supervised-child peers (review S9) and token project
//! scope (review S3): in-process, over a kernel with an isolated chain.

use super::*;
use crate::child_peer::PeerClass;
use crate::rpc_ext::CallerCtx;
use clawft_kernel::token_authority::Issuer;

const A: &str = "01JB8Z3Q0V6X9KQ4M2N7T5R1WD";
const B: &str = "01JB8Z3Q0V6X9KQ4M2N7T5R1WE";

async fn kernel() -> Arc<tokio::sync::RwLock<Kernel<NativePlatform>>> {
    let cfg = KernelConfig {
        chain: Some(clawft_types::config::ChainConfig::isolated_in(&tempfile::tempdir().unwrap().keep())),
        ..KernelConfig::default()
    };
    let k = Kernel::boot(clawft_types::config::Config::default(), cfg, Arc::new(NativePlatform::new()))
        .await
        .expect("kernel boots");
    Arc::new(tokio::sync::RwLock::new(k))
}

fn cli(peer: PeerClass) -> CallerCtx {
    // The client library stamps "admin" on every call without auth.
    CallerCtx::from_auth(Some("admin".into())).with_peer(peer)
}

async fn authz(c: &mut CallerCtx, method: &str, k: &Arc<tokio::sync::RwLock<Kernel<NativePlatform>>>) -> Result<(), Response> {
    authorize_caller(c, method, &serde_json::Value::Null, k).await.map(|_| ())
}

#[tokio::test]
async fn a_literal_admin_from_a_supervised_childs_process_group_grants_nothing() {
    let k = kernel().await;
    // Admin verbs, including minting an owner token: refused.
    for m in ["auth.token.issue", "project.revoke", "governance.parent.push", "kernel.shutdown", "chain.append"] {
        let e = authz(&mut cli(PeerClass::Child), m, &k).await.unwrap_err();
        assert!(!e.ok, "{m} must be refused for a child peer");
    }
    // The child's own read-level parent calls still work (anonymous).
    for m in ["kernel.handshake", "mesh.register", "project.anchor.submit"] {
        authz(&mut cli(PeerClass::Child), m, &k).await.unwrap_or_else(|e| panic!("{m}: {:?}", e.error));
    }
    // The owner's CLI is unchanged.
    for m in ["auth.token.issue", "project.revoke", "kernel.shutdown"] {
        authz(&mut cli(PeerClass::Owner), m, &k).await.unwrap_or_else(|e| panic!("{m}: {:?}", e.error));
    }
}

#[tokio::test]
async fn another_uid_still_gets_the_uid_mismatch_error() {
    let k = kernel().await;
    let e = authz(&mut cli(PeerClass::OtherUid), "kernel.handshake", &k).await.unwrap_err();
    assert_eq!(e.error_kind.as_deref(), Some("peer_uid_mismatch"));
}

#[tokio::test]
async fn a_child_keeps_only_its_project_token() {
    let k = kernel().await;
    let authority = crate::token_rpc::authority_for(&k).await.expect("authority");
    let (secret, _) = authority
        .issue_project(A, chrono::Duration::seconds(600), &Issuer { uid: None })
        .unwrap();
    let mut c = CallerCtx::from_auth(Some(secret.clone())).with_peer(PeerClass::Child);
    // Its token still works for what the parent link calls...
    authz(&mut c, "shared.embed", &k).await.unwrap_or_else(|e| panic!("{:?}", e.error));
    // ...and nothing else (the project-token method allow-list).
    let e = authz(&mut c, "auth.token.issue", &k).await.unwrap_err();
    assert_eq!(e.error_kind.as_deref(), Some(crate::project_token_scope::DENIED_KIND));
}

#[tokio::test]
async fn a_tokens_project_is_the_claim_and_a_different_claim_is_refused() {
    let k = kernel().await;
    let authority = crate::token_rpc::authority_for(&k).await.expect("authority");
    let (secret, info) = authority
        .issue("t", Some(chrono::Duration::seconds(600)), Some(A.to_owned()), &Issuer { uid: None })
        .unwrap();
    assert_eq!(info.project.as_deref(), Some(A));
    let with_claim = |claim: Option<&str>| {
        let mut r = clawft_rpc::Request::new("kernel.status");
        r.auth = Some(secret.clone());
        r.project = claim.map(str::to_owned);
        CallerCtx::from_request(&r)
    };
    // No claim, or the token's own project: accepted.
    authz(&mut with_claim(None), "kernel.status", &k).await.unwrap_or_else(|e| panic!("{:?}", e.error));
    authz(&mut with_claim(Some(A)), "kernel.status", &k).await.unwrap_or_else(|e| panic!("{:?}", e.error));
    // A different claim: refused before any capability check.
    let e = authz(&mut with_claim(Some(B)), "kernel.status", &k).await.unwrap_err();
    assert_eq!(e.error_kind.as_deref(), Some(crate::caller_principal::SCOPE_MISMATCH_KIND));
    // An unscoped token has no project to enforce.
    let (open, _) = authority.issue("u", Some(chrono::Duration::seconds(600)), None, &Issuer { uid: None }).unwrap();
    let mut r = clawft_rpc::Request::new("kernel.status");
    r.auth = Some(open);
    r.project = Some(B.into());
    authz(&mut CallerCtx::from_request(&r), "kernel.status", &k).await.unwrap_or_else(|e| panic!("{:?}", e.error));
}
