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
    assert!(l.a.rev.on_peer_control(&ctx, &forged).is_empty());
    assert!(l.a.rev.on_peer_control(&ctx, &serde_json::json!({"nope": 1})).is_empty());
    tokio::time::sleep(Duration::from_millis(50)).await;
    for p in [&l.a, &l.b, &l.c] {
        assert!(p.list.list_subjects(None).is_empty());
        assert_eq!(p.node.ex.servable_artifacts().len(), 3);
    }
}
