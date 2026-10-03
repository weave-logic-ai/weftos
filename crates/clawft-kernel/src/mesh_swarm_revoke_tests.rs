//! A signed revocation crosses a three-node line (A - B - C) once, evicts
//! the package everywhere, and only operator-pinned keys can issue one.

use super::*;
use crate::artifact_store::ArtifactStore;
use crate::chain::EVENT_KIND_ARTIFACT_REVOKE;
use crate::mesh_artifact::ExchangeConfig;
use crate::mesh_artifact_pkg::ExchangedPackage;
use crate::mesh_artifact_tests::{Node, anchors_for, events, key, node_with, signed_package};
use crate::workload_pkg::codec::hex_encode;

struct Peer {
    id: String,
    node: Node,
    rt: Arc<MeshRuntime>,
    rev: Arc<RevocationExchange>,
    list: Arc<RevocationList>,
    _tmp: tempfile::TempDir,
}

fn peer(id: &str, pkg_dir: &std::path::Path, anchors: &TrustAnchors) -> (Peer, ExchangedPackage) {
    let cfg = ExchangeConfig {
        piece_size: 1024 * 1024,
        block_size: 256 * 1024,
        ..Default::default()
    };
    let node = node_with(id, ArtifactStore::new_memory(), cfg);
    let pkg = node.ex.seed_package_dir(pkg_dir, anchors).unwrap();
    let rt = Arc::new(MeshRuntime::new(id.to_string()));
    let tmp = tempfile::tempdir().unwrap();
    let list = Arc::new(RevocationList::new(tmp.path().join("revoked.json")));
    let rev = RevocationExchange::start(node.ex.clone(), list.clone(), anchors.clone(), rt.clone());
    (
        Peer {
            id: id.to_string(),
            node,
            rt,
            rev,
            list,
            _tmp: tmp,
        },
        pkg,
    )
}

/// Connect two runtimes in process.
fn link(a: &Peer, b: &Peer) {
    let (tx_ab, mut rx_ab) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
    let (tx_ba, mut rx_ba) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
    let (b_rt, back) = (b.rt.clone(), tx_ba.clone());
    tokio::spawn(async move {
        while let Some(bytes) = rx_ab.recv().await {
            let _ = b_rt.handle_incoming_peer(&bytes, back.clone(), None).await;
        }
    });
    let (a_rt, back) = (a.rt.clone(), tx_ab.clone());
    tokio::spawn(async move {
        while let Some(bytes) = rx_ba.recv().await {
            let _ = a_rt.handle_incoming_peer(&bytes, back.clone(), None).await;
        }
    });
    a.rt.add_peer(b.id.clone(), tx_ab);
    b.rt.add_peer(a.id.clone(), tx_ba);
}

async fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
    for _ in 0..400 {
        if ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("timed out waiting for {what}");
}

use std::time::Duration;

struct Line {
    a: Peer,
    b: Peer,
    c: Peer,
    pkg: ExchangedPackage,
    _tmp: tempfile::TempDir,
}

fn line() -> Line {
    let tmp = tempfile::tempdir().unwrap();
    let k = key(1);
    let dir = signed_package(tmp.path(), 2 * 1024 * 1024, &k);
    let anchors = anchors_for(&k);
    let (a, pkg) = peer("node-a", &dir, &anchors);
    let (b, _) = peer("node-b", &dir, &anchors);
    let (c, _) = peer("node-c", &dir, &anchors);
    link(&a, &b);
    link(&b, &c); // A and C are not neighbours
    Line {
        a,
        b,
        c,
        pkg,
        _tmp: tmp,
    }
}

fn notice(l: &Line, k: &SigningKey) -> SignedRevocation {
    sign_revocation(RevocationKind::Package, &l.pkg.package_id, "compromised", 1, k).unwrap()
}

#[tokio::test]
async fn a_revocation_reaches_every_node_and_evicts_the_package() {
    let l = line();
    for p in [&l.a, &l.b, &l.c] {
        assert_eq!(p.node.ex.servable_artifacts().len(), 3);
    }
    assert!(l.a.rev.issue(notice(&l, &key(1))).await.unwrap());

    wait_for("C, two hops away, to evict", || l.c.node.ex.store().count() == 0).await;
    for p in [&l.a, &l.b, &l.c] {
        assert!(p.list.is_subject_revoked(RevocationKind::Package, &l.pkg.package_id));
        assert!(p.node.ex.servable_artifacts().is_empty(), "{} still seeds", p.id);
        assert_eq!(p.node.ex.store().count(), 0, "{} still holds bytes", p.id);
        assert_eq!(events(&p.node.chain, EVENT_KIND_ARTIFACT_REVOKE).len(), 3, "{}", p.id);
    }
}

#[tokio::test]
async fn a_known_notice_is_not_applied_or_forwarded_again() {
    let l = line();
    let n = notice(&l, &key(1));
    assert!(l.a.rev.issue(n.clone()).await.unwrap());
    wait_for("C", || l.c.node.ex.store().count() == 0).await;
    // Issued again: already known, so no new chain events anywhere.
    assert!(!l.a.rev.issue(n.clone()).await.unwrap());
    assert!(!l.b.rev.accept(&n).unwrap());
    tokio::time::sleep(Duration::from_millis(100)).await;
    for p in [&l.a, &l.b, &l.c] {
        assert_eq!(events(&p.node.chain, EVENT_KIND_ARTIFACT_REVOKE).len(), 3, "{}", p.id);
    }
}

#[tokio::test]
async fn only_pinned_operator_keys_can_revoke() {
    let l = line();
    // Valid signature, but the key is not pinned.
    let stranger = notice(&l, &key(9));
    assert_eq!(l.a.rev.accept(&stranger), Err(NoticeError::UnauthorizedSigner));
    // Altered after signing.
    let mut tampered = notice(&l, &key(1));
    tampered.payload = tampered.payload.replace("compromised", "harmless");
    assert_eq!(l.a.rev.accept(&tampered), Err(NoticeError::BadSignature));
    // A Cognitum release key is pinned for verifying packages, not for revoking.
    let mut anchors = TrustAnchors::default();
    let pk = key(5).verifying_key().to_bytes();
    anchors
        .push_signer("cog", &hex_encode(&pk), KeyOrigin::CognitumRelease)
        .unwrap();
    assert_eq!(
        verify_revocation(&notice(&l, &key(5)), &anchors),
        Err(NoticeError::UnauthorizedSigner)
    );
    // Over the wire, forged notices change nothing on any node.
    let forged = serde_json::to_value(&stranger).unwrap();
    let ctx = PeerCtx::unauthenticated("node-b");
    assert!(l.a.rev.on_peer_control(&ctx, 0, &forged).is_empty());
    assert!(l.a.rev.on_peer_control(&ctx, 0, &serde_json::json!({"nope": 1})).is_empty());
    tokio::time::sleep(Duration::from_millis(50)).await;
    for p in [&l.a, &l.b, &l.c] {
        assert!(p.list.list_subjects(None).is_empty());
        assert_eq!(p.node.ex.servable_artifacts().len(), 3);
    }
}

#[tokio::test]
async fn a_revoked_signer_key_can_no_longer_revoke() {
    let l = line();
    // Revoke the operator key itself, then try to use it.
    let own = hex_encode(&key(1).verifying_key().to_bytes());
    l.a.list.revoke_subject(RevocationKind::SignerKey, &own, "key leaked").unwrap();
    assert_eq!(l.a.rev.accept(&notice(&l, &key(1))), Err(NoticeError::SignerRevoked));
    // The notice that revoked it earlier is a separate, valid act: a second
    // operator key, still pinned, may still issue notices.
    assert!(l.a.list.is_subject_revoked(RevocationKind::SignerKey, &own));
}

#[tokio::test]
async fn notices_are_rate_limited_per_node() {
    let l = line();
    let mut refused = 0;
    for i in 0..30 {
        let n = sign_revocation(RevocationKind::ArtifactHash, &hex_encode(&[i as u8; 32]), "x", 1, &key(1)).unwrap();
        if l.a.rev.accept(&n) == Err(NoticeError::RateLimited) {
            refused += 1;
        }
    }
    assert!(refused >= 15, "burst of {NOTICE_BURST} then ~{NOTICES_PER_SEC}/s: {refused} refused");
}

#[tokio::test]
async fn a_duplicate_notice_is_not_swept_again() {
    let l = line();
    let n = notice(&l, &key(1));
    assert!(l.a.rev.accept(&n).unwrap());
    let before = events(&l.a.node.chain, EVENT_KIND_ARTIFACT_REVOKE).len();
    // Seed again by hand: a sweep would evict it, so a skipped sweep leaves it.
    assert!(!l.a.rev.accept(&n).unwrap());
    assert_eq!(events(&l.a.node.chain, EVENT_KIND_ARTIFACT_REVOKE).len(), before);
}

#[test]
fn a_malleated_signature_is_refused_by_strict_verification() {
    // Adding the group order L to S yields a second valid-looking encoding
    // that non-strict verifiers can accept; verify_strict must not.
    let k = key(1);
    let anchors = anchors_for(&k);
    let good = sign_revocation(RevocationKind::ArtifactHash, &hex_encode(&[7; 32]), "x", 1, &k).unwrap();
    assert!(verify_revocation(&good, &anchors).is_ok());
    let mut bad = good.clone();
    bad.signature[63] |= 0xf0; // S out of canonical range
    assert!(verify_revocation(&bad, &anchors).is_err());
}

#[tokio::test]
async fn junk_on_one_connection_blocks_neither_another_connection_nor_issue() {
    let l = line();
    let ctx = PeerCtx::unauthenticated("node-b");
    // Connection 1 floods: right key, tampered payload (passes the cheap
    // checks, fails the signature), so each one spends its own tokens.
    for i in 0..60u8 {
        let mut junk = sign_revocation(RevocationKind::ArtifactHash, &hex_encode(&[i; 32]), "x", 1, &key(1)).unwrap();
        junk.payload = junk.payload.replace("\"x\"", "\"y\"");
        let v = serde_json::to_value(&junk).unwrap();
        assert!(l.a.rev.on_peer_control(&ctx, 1, &v).is_empty());
    }
    // Connection 1 is now out of tokens even for a genuine notice ...
    let genuine = notice(&l, &key(1));
    assert_eq!(l.a.rev.accept_from(&genuine, Some(1)), Err(NoticeError::RateLimited));
    // ... but a genuine notice on connection 2 goes through,
    assert_eq!(l.a.rev.accept_from(&genuine, Some(2)), Ok(true));
    // and the operator's own issue() is not blocked either.
    let own = sign_revocation(RevocationKind::ArtifactHash, &hex_encode(&[77; 32]), "own", 2, &key(1)).unwrap();
    assert_eq!(l.a.rev.issue(own).await, Ok(true));
}

#[tokio::test]
async fn unpinned_signers_cost_no_budget() {
    let l = line();
    for i in 0..100u8 {
        let n = sign_revocation(RevocationKind::ArtifactHash, &hex_encode(&[i; 32]), "x", 1, &key(9)).unwrap();
        assert_eq!(l.a.rev.accept_from(&n, Some(5)), Err(NoticeError::UnauthorizedSigner));
    }
    // The connection still has its whole budget.
    assert_eq!(l.a.rev.accept_from(&notice(&l, &key(1)), Some(5)), Ok(true));
}

#[tokio::test]
async fn an_applied_notice_is_chained_and_runs_the_hook_once() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let l = line();
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    let pkg_id = l.pkg.package_id.clone();
    assert!(l.b.rev.set_on_applied(Arc::new(move |n| {
        assert_eq!(n.kind, RevocationKind::Package);
        assert_eq!(n.id, pkg_id);
        h.fetch_add(1, Ordering::SeqCst);
    })));
    assert!(!l.b.rev.set_on_applied(Arc::new(|_| {})), "first hook wins");

    let n = notice(&l, &key(1));
    assert!(l.a.rev.issue(n.clone()).await.unwrap());
    wait_for("B's hook", || hits.load(Ordering::SeqCst) == 1).await;
    // A notice B already knows neither re-runs the hook nor re-chains.
    assert!(!l.b.rev.accept(&n).unwrap());
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    // Every node chained the revocation itself, by the signer, once.
    for p in [&l.a, &l.b, &l.c] {
        let ev = events(&p.node.chain, crate::chain::EVENT_KIND_WORKLOAD_REVOKE);
        assert_eq!(ev.len(), 1, "{}", p.id);
        assert_eq!(ev[0]["subject_id"], l.pkg.package_id.as_str());
        assert!(ev[0]["revoked_by"].as_str().unwrap().starts_with("mesh:"), "{}", ev[0]);
        assert_eq!(ev[0]["persisted"], true);
    }
}

#[tokio::test]
async fn a_notice_whose_write_fails_is_still_applied_chained_and_forwarded() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let l = line();
    // B's list cannot write: a file where its directory should be.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("blocker"), "x").unwrap();
    let broken = Arc::new(RevocationList::new(tmp.path().join("blocker").join("r.json")));
    let anchors = anchors_for(&key(1));
    let rt = Arc::new(MeshRuntime::new("node-d".into()));
    let node = node_with("node-d", ArtifactStore::new_memory(), ExchangeConfig::default());
    let rev = RevocationExchange::start(node.ex.clone(), broken.clone(), anchors, rt);
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    rev.set_on_applied(Arc::new(move |_| {
        h.fetch_add(1, Ordering::SeqCst);
    }));
    assert!(rev.accept(&notice(&l, &key(1))).unwrap(), "in force, so new");
    assert!(broken.is_subject_revoked(RevocationKind::Package, &l.pkg.package_id));
    assert_eq!(hits.load(Ordering::SeqCst), 1, "the hook still runs");
    let ev = events(&node.chain, crate::chain::EVENT_KIND_WORKLOAD_REVOKE);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0]["persisted"], false);
}

#[test]
fn the_replay_log_is_bounded_and_evicts_the_oldest() {
    let tmp = tempfile::tempdir().unwrap();
    let k = key(1);
    let (p, _) = peer("node-a", &signed_package(tmp.path(), 1024, &k), &anchors_for(&k));
    let notice = |i: usize| {
        let id = hex_encode(&blake3::hash(&i.to_le_bytes()).as_bytes()[..]);
        sign_revocation(RevocationKind::ArtifactHash, &id, "t", 1, &k).unwrap()
    };
    let first = notice(0);
    p.rev.log_notice(&first);
    for i in 1..MAX_LOGGED + 5 {
        p.rev.log_notice(&notice(i));
    }
    let log = p.rev.log.lock().unwrap();
    assert_eq!(log.entries.len(), MAX_LOGGED);
    assert!(!log.entries.contains(&first), "the oldest went first");
    assert!(log.entries.contains(&notice(MAX_LOGGED + 4)), "the newest is kept");
}
