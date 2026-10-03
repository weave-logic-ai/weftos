//! Floods of bindings, grants and approvals over in-process mesh runtimes
//! (ADR-106 phase 1b). Sync tests are in `tests_sync`.

use std::sync::Arc;
use std::time::Duration;

use super::tests_common::*;
use super::*;
use crate::mesh_admit::PeerClass;
use crate::mesh_delivery::PeerCtx;
use crate::ipc::{KernelMessage, MessagePayload, MessageTarget};
use crate::mesh_runtime::{COG_BINDING_TOPIC, MeshRuntime};

pub(super) struct AdmitAll;

impl PeerAdmission for AdmitAll {
    fn admitted(&self, _: &PeerCtx) -> bool {
        true
    }

    fn peer_admitted(&self, _: &MeshRuntime, _: &str) -> bool {
        true
    }
}

pub(super) struct TNode {
    pub id: String,
    pub fx: Fx,
    pub rt: Arc<MeshRuntime>,
    pub ex: Arc<LicenceExchange>,
}

pub(super) fn quiet_cfg() -> LicenceExchangeConfig {
    LicenceExchangeConfig { sync_on_connect: false, ..Default::default() }
}

pub(super) fn tnode_with(
    id: &str,
    admission: Arc<dyn PeerAdmission>,
    config: LicenceExchangeConfig,
) -> TNode {
    let fx = Fx::new();
    let rt = Arc::new(MeshRuntime::new(id.to_string()));
    let ex = LicenceExchange::start(LicenceExchangeParts {
        store: fx.store.clone(),
        approvals: fx.approvals.clone(),
        anchors: anchors(),
        runtime: rt.clone(),
        posture: Arc::new(posture),
        admission,
        sink: fx.sink.clone(),
        config,
    });
    TNode { id: id.to_string(), fx, rt, ex }
}

pub(super) fn tnode(id: &str) -> TNode {
    tnode_with(id, Arc::new(AdmitAll), quiet_cfg())
}

/// Connect two runtimes in process. `verified` is what each side's
/// connection reports for the other (a verified full node, or not).
pub(super) fn link(a: &TNode, b: &TNode, verified: bool) {
    let class = if verified { PeerClass::Node } else { PeerClass::Legacy };
    link_as(a, b, verified, class);
}

/// [`link`] where each side's admission classed the other as `class`. The
/// context matches `mesh_serve`'s: an admitted peer is `node_verified`
/// whatever its class (a leaf is `{node_verified: true, class: Leaf}`).
pub(super) fn link_as(a: &TNode, b: &TNode, verified: bool, class: PeerClass) {
    let (tx_ab, rx_ab) = tokio::sync::mpsc::channel::<Vec<u8>>(1024);
    let (tx_ba, rx_ba) = tokio::sync::mpsc::channel::<Vec<u8>>(1024);
    for (to, from_id, mut rx, back) in [
        (b.rt.clone(), a.id.clone(), rx_ab, tx_ba.clone()),
        (a.rt.clone(), b.id.clone(), rx_ba, tx_ab.clone()),
    ] {
        tokio::spawn(async move {
            let ctx = PeerCtx {
                peer_id: from_id,
                node_verified: verified,
                class,
                remote_static: None,
                src_scope: None,
            };
            while let Some(bytes) = rx.recv().await {
                let _ = to.handle_incoming_peer(&bytes, back.clone(), Some(&ctx)).await;
            }
        });
    }
    if verified {
        let tally = crate::mesh_runtime::RouteTally::default();
        assert!(a.rt.register_authenticated_as(b.id.clone(), tx_ab, true, class, &tally));
        assert!(b.rt.register_authenticated_as(a.id.clone(), tx_ba, true, class, &tally));
    } else {
        a.rt.add_peer(b.id.clone(), tx_ab);
        b.rt.add_peer(a.id.clone(), tx_ba);
    }
}

pub(super) async fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
    for _ in 0..600 {
        if ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("timed out waiting for {what}");
}

/// [`wait_for`] for work that takes many saves.
pub(super) async fn wait_for_long(what: &str, mut ok: impl FnMut() -> bool) {
    for _ in 0..8000 {
        if ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("timed out waiting for {what}");
}

pub(super) async fn settle() {
    tokio::time::sleep(Duration::from_millis(150)).await;
}

pub(super) fn bound(n: &TNode) -> bool {
    n.fx.store.active_binding().is_some()
}

pub(super) fn has_grant(n: &TNode, version: &str) -> bool {
    n.fx.store.held_grant("fall-detect", version).is_some()
}

struct Line {
    a: TNode,
    b: TNode,
    c: TNode,
}

/// A - B - C: A and C are not neighbours.
fn line() -> Line {
    let (a, b, c) = (tnode("node-a"), tnode("node-b"), tnode("node-c"));
    link(&a, &b, true);
    link(&b, &c, true);
    Line { a, b, c }
}

#[tokio::test]
async fn binding_grant_and_approval_reach_every_node_once() {
    let l = line();
    assert_eq!(l.a.ex.issue_binding(binding(1, BindState::Bound)).await, Ok(Receipt::New));
    wait_for("C to hold the binding", || bound(&l.c)).await;

    let shas = vec![sha_of("x86_64")];
    l.a.ex.issue_grant(grant(1, T0, 3600, &["x86_64"])).await.unwrap();
    l.a.ex.issue_approval(approval(&shas)).await.unwrap();
    wait_for("C to hold grant and approval", || {
        has_grant(&l.c, "1.2.0") && l.c.fx.approvals.len() == 1
    })
    .await;
    for n in [&l.a, &l.b, &l.c] {
        assert!(bound(n) && has_grant(n, "1.2.0") && n.fx.approvals.len() == 1, "{}", n.id);
        assert!(n.fx.store.valid_grant_for_artifact("fall-detect", "1.2.0", &shas[0], &b3_of("x86_64")).is_some());
    }
    // Each record crossed each of the two links exactly once.
    settle().await;
    let sent = |n: &TNode| n.ex.flooded.load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!((sent(&l.a), sent(&l.b), sent(&l.c)), (3, 3, 0), "one forward per record per link");
    // A repeat is known everywhere and goes nowhere new.
    assert_eq!(l.a.ex.issue_binding(binding(1, BindState::Bound)).await, Ok(Receipt::Known));
    assert_eq!(l.b.ex.accept_grant(&grant(1, T0, 3600, &["x86_64"]), Spend::Exempt), Ok(Receipt::Known));
    settle().await;
    assert!(l.a.fx.sink.events().is_empty() && l.c.fx.sink.events().is_empty());
    // The issuer re-sends to its peers by design; they know it and send nothing.
    assert_eq!((sent(&l.a), sent(&l.b), sent(&l.c)), (4, 3, 0), "a repeat is not forwarded");
}

#[tokio::test]
async fn an_unbind_issued_by_a_non_steward_node_reaches_every_node() {
    let l = line();
    l.a.ex.issue_binding(binding(1, BindState::Bound)).await.unwrap();
    wait_for("C bound", || bound(&l.c)).await;
    let prev = binding_rec(1, BindState::Bound, &grant_key(), &mesh());
    // The far end of the line, which is not the steward, unbinds.
    let unbind = sign_unbind(&prev, &op()).unwrap();
    assert_eq!(l.c.ex.issue_binding(unbind).await, Ok(Receipt::New));
    wait_for("A unbound", || {
        l.a.fx.store.binding_status() == Err(LicenceError::Unbound)
    })
    .await;
    for n in [&l.a, &l.b, &l.c] {
        assert_eq!(n.fx.store.binding_status(), Err(LicenceError::Unbound), "{}", n.id);
    }
}

#[tokio::test]
async fn an_approval_for_another_mesh_is_refused_before_any_verify() {
    let l = line();
    let other = sign_approval(&approval_rec(&[sha_of("x86_64")], &other_mesh()), &op()).unwrap();
    assert_eq!(
        l.a.ex.accept_approval(&other, Spend::Exempt),
        Err(ExchangeError::Licence(LicenceError::WrongMesh))
    );
    assert_eq!(l.a.ex.issue_approval(other).await, Err(LicenceError::WrongMesh.into()));
    settle().await;
    assert!(l.a.fx.approvals.is_empty() && l.b.fx.approvals.is_empty());
}

#[tokio::test]
async fn a_grant_ahead_of_the_clock_is_deferred_then_accepted_later() {
    let l = line();
    l.a.ex.issue_binding(binding(1, BindState::Bound)).await.unwrap();
    wait_for("C bound", || bound(&l.c)).await;
    let late = grant(1, T0 + 1000, 3600, &["x86_64"]);
    // Not applied, not forwarded: the issuer holds it only once its clock is there.
    assert_eq!(l.a.ex.issue_grant(late.clone()).await, Ok(Receipt::Deferred));
    settle().await;
    assert!(!has_grant(&l.a, "1.2.0") && !has_grant(&l.b, "1.2.0") && !has_grant(&l.c, "1.2.0"));
    for n in [&l.a, &l.b, &l.c] {
        n.fx.set_now(T0 + 1000);
    }
    assert_eq!(l.a.ex.issue_grant(late).await, Ok(Receipt::New));
    wait_for("C to hold the grant", || has_grant(&l.c, "1.2.0")).await;
}

pub(super) fn junk_binding(seq: u64) -> SignedBinding {
    let mut b = binding(seq, BindState::Bound);
    b.signature = "00".repeat(64);
    b
}

#[tokio::test]
async fn junk_on_one_connection_starves_neither_another_connection_nor_revocation() {
    let n = tnode("node-n");
    // Pinned key, fresh seq each time, bad signature: passes the cheap
    // filters, so each one spends a token on connection 7 and fails verify.
    let mut limited = 0;
    for seq in 1..=40 {
        match n.ex.accept_binding(&junk_binding(seq), Spend::Flood(7)) {
            Err(ExchangeError::RateLimited) => limited += 1,
            Err(ExchangeError::Licence(LicenceError::BadSignature)) => {}
            other => panic!("unexpected {other:?}"),
        }
    }
    assert!(limited > 20, "the connection's bucket must run out ({limited})");
    // Another connection is unaffected, and so is another kind on the same one.
    assert_eq!(n.ex.accept_binding(&binding(1, BindState::Bound), Spend::Flood(8)), Ok(Receipt::New));
    let ap = approval(&[sha_of("x86_64")]);
    assert_eq!(n.ex.accept_approval(&ap, Spend::Flood(7)), Ok(Receipt::New));
    // Sync traffic has its own bucket.
    assert!(n.ex.sync_buckets.take(super::exchange_types::Budget::Sync, 7, 1.0));
}

#[tokio::test]
async fn licence_junk_does_not_consume_the_revocation_budget() {
    use crate::mesh_artifact_tests::{anchors_for, key, node};
    use crate::mesh_swarm_revoke::{RevocationExchange, sign_revocation};
    use crate::revocation::{RevocationKind, RevocationList};

    let n = tnode("node-n");
    let art = node("node-n");
    let tmp = tempfile::tempdir().unwrap();
    let list = Arc::new(RevocationList::new(tmp.path().join("revoked.json")));
    RevocationExchange::start(art.ex.clone(), list.clone(), anchors_for(&key(1)), n.rt.clone());
    let peer = tnode("node-p");
    link(&peer, &n, false);

    // 60 junk binding frames on the one connection, then a revocation on it.
    for seq in 1..=60 {
        let msg = KernelMessage::new(
            0,
            MessageTarget::Topic(COG_BINDING_TOPIC.into()),
            MessagePayload::Json(serde_json::to_value(junk_binding(seq)).unwrap()),
        );
        peer.rt.route_to_remote("node-n", msg).await.unwrap();
    }
    let hash = crate::workload_pkg::codec::hex_encode(&[9u8; 32]);
    let notice = sign_revocation(RevocationKind::ArtifactHash, &hash, "t", 1, &key(1)).unwrap();
    let msg = KernelMessage::new(
        0,
        MessageTarget::Topic(crate::mesh_runtime::REVOKE_TOPIC.into()),
        MessagePayload::Json(serde_json::to_value(&notice).unwrap()),
    );
    peer.rt.route_to_remote("node-n", msg).await.unwrap();
    wait_for("the revocation to apply", || {
        list.is_subject_revoked(RevocationKind::ArtifactHash, &hash)
    })
    .await;
    assert!(!bound(&n));
}

#[tokio::test]
async fn the_exchange_is_inert_while_the_local_mesh_id_is_unset() {
    let a = tnode("node-a");
    let b = tnode("node-b");
    link(&a, &b, true);
    b.fx.local.set(None);
    assert_eq!(
        a.ex.accept_binding(&binding(1, BindState::Bound), Spend::Exempt),
        Ok(Receipt::New)
    );
    assert_eq!(
        b.ex.accept_binding(&binding(1, BindState::Bound), Spend::Exempt),
        Err(LicenceError::NoLocalMesh.into())
    );
    a.ex.issue_binding(binding(2, BindState::Bound)).await.unwrap();
    b.ex.sync_peer("node-a").await;
    settle().await;
    assert!(b.fx.store.held_signed_binding().is_none());
    assert!(b.ex.pending.is_empty(), "no sync request while unset");
}

#[test]
fn every_licence_event_reaches_the_chain() {
    let chain = Arc::new(crate::chain::ChainManager::new(0, 1000));
    let sink = ChainLicenceSink::new(chain.clone());
    let events = [
        LicenceEvent::BindingRefused("open_membership".into()),
        LicenceEvent::BindingConflict(3),
        LicenceEvent::BindingOrphaned { stored: "a".into(), local: "b".into() },
        LicenceEvent::GrantConflict { cog_id: "c".into(), version: "1".into(), seq: 2 },
        LicenceEvent::FloorReset(5),
        LicenceEvent::SyncBadSignature { peer: "node-x".into() },
    ];
    for e in events.iter().cloned() {
        sink.emit(e);
    }
    let kinds: Vec<String> = chain.tail(chain.len()).into_iter().map(|e| e.kind).collect();
    for e in &events {
        assert!(kinds.contains(&format!("licence.{}", e.name())), "{}", e.name());
    }
}

// ── licensed peers only: an admitted leaf gets no floods ─────────

#[tokio::test]
async fn an_admitted_leaf_receives_no_binding_grant_or_approval_flood() {
    let a = tnode_with("node-a", Arc::new(CtxAdmission), quiet_cfg());
    let (leaf, c) = (tnode("leaf-l"), tnode("node-c"));
    link_as(&a, &leaf, true, PeerClass::Leaf); // verified by admission, but a leaf
    link(&a, &c, true); // a verified full node
    assert!(a.rt.peer_verified("leaf-l") && !a.rt.peer_licensed("leaf-l"));
    a.ex.issue_binding(binding(1, BindState::Bound)).await.unwrap();
    wait_for("C to hold the binding", || bound(&c)).await;
    a.ex.issue_grant(grant(1, T0, 3600, &["x86_64"])).await.unwrap();
    a.ex.issue_approval(approval(&[sha_of("x86_64")])).await.unwrap();
    wait_for("C to hold the rest", || has_grant(&c, "1.2.0") && c.fx.approvals.len() == 1).await;
    settle().await;
    assert!(
        !bound(&leaf) && !has_grant(&leaf, "1.2.0") && leaf.fx.approvals.is_empty(),
        "nothing reached the admitted leaf"
    );
    assert_eq!(a.ex.flooded.load(std::sync::atomic::Ordering::Relaxed), 3, "three records, one licensed peer");
    // Inbound, the leaf's (admitted) context is not a licensed peer either (no sync).
    let leaf_ctx = PeerCtx {
        peer_id: "leaf-l".into(),
        node_verified: true,
        class: PeerClass::Leaf,
        remote_static: None,
        src_scope: None,
    };
    assert!(!CtxAdmission.admitted(&leaf_ctx));
}

#[tokio::test]
async fn the_exchange_is_the_relays_grant_flood() {
    let l = line();
    l.a.ex.issue_binding(binding(1, BindState::Bound)).await.unwrap();
    wait_for("C bound", || bound(&l.c)).await;
    // The relay installs the grant locally (`install_grant`), then floods it.
    let g = grant(1, T0, 3600, &["x86_64"]);
    assert_eq!(l.a.fx.store.accept_grant(&g), Ok(Outcome::Applied));
    let flood: Arc<dyn GrantFlood> = l.a.ex.clone();
    flood.flood(&g).await;
    wait_for("C to hold the relayed grant", || has_grant(&l.c, "1.2.0")).await;
    assert!(has_grant(&l.b, "1.2.0"));
}

#[tokio::test]
async fn a_dropped_leaf_route_comes_back_as_a_leaf_not_a_node() {
    let a = tnode_with("node-a", Arc::new(CtxAdmission), quiet_cfg());
    let leaf = tnode("leaf-l");
    link_as(&a, &leaf, true, PeerClass::Leaf);
    // The route goes (as `remove_dead_peers` drops it); the leaf's next frame
    // re-registers it from the connection's admitted context.
    a.rt.disconnect_peer("leaf-l");
    assert!(!a.rt.peer_verified("leaf-l"));
    let msg = KernelMessage::new(
        0,
        MessageTarget::Topic("mesh.subscribe".into()),
        MessagePayload::Json(serde_json::json!({ "topic": "anything" })),
    );
    leaf.rt.route_to_remote("node-a", msg).await.unwrap();
    wait_for("the leaf route to come back", || a.rt.peer_verified("leaf-l")).await;
    assert!(!a.rt.peer_licensed("leaf-l"), "a re-registered leaf is still a leaf");
    a.ex.issue_binding(binding(1, BindState::Bound)).await.unwrap();
    settle().await;
    assert!(!bound(&leaf), "and still gets no flood");
}
