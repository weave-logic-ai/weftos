//! Catch-up sync over in-process mesh runtimes (ADR-106 phase 1b).

use std::sync::Arc;

use super::exchange_types::Budget;
use super::tests_common::*;
use super::tests_exchange::*;
use super::*;
use crate::ipc::{KernelMessage, MessagePayload, MessageTarget};
use crate::mesh_runtime::COG_SYNC_TOPIC;

fn grant_v(i: u32, seq: u64) -> SignedGrant {
    let mut r = grant_rec(seq, T0, 3600, &["x86_64"]);
    r.version = format!("v{i}");
    sign_grant(&r, &grant_key()).unwrap()
}

fn approval_i(i: u32) -> SignedApproval {
    approval(&[sha256_hex(format!("bin-{i}").as_bytes())])
}

fn seed(n: &TNode, grants: u32, approvals: u32) {
    n.fx.bind();
    for i in 0..grants {
        n.fx.store.accept_grant(&grant_v(i, 1)).unwrap();
    }
    for i in 0..approvals {
        n.fx.approvals.accept(&approval_i(i)).unwrap();
    }
}

fn held(n: &TNode) -> usize {
    (0..1000).filter(|i| n.fx.store.held_grant("fall-detect", &format!("v{i}")).is_some()).count()
}

async fn send_sync(from: &TNode, to: &str, msg: SyncMsg) {
    let m = KernelMessage::new(
        0,
        MessageTarget::Topic(COG_SYNC_TOPIC.into()),
        MessagePayload::Json(serde_json::to_value(msg).unwrap()),
    );
    from.rt.route_to_remote(to, m).await.unwrap();
}

fn response(
    binding: Option<SignedBinding>,
    grants: Vec<SignedGrant>,
    approvals: Vec<SignedApproval>,
) -> SyncMsg {
    SyncMsg::Response { binding, grants, approvals, more_grants: false, more_approvals: false }
}

/// A node that never answers (no local mesh id) but can inject frames.
fn mute(id: &str) -> TNode {
    let m = tnode(id);
    m.fx.local.set(None);
    m
}

#[tokio::test]
async fn a_late_joiner_reaches_the_binding_and_the_highest_grants() {
    let a = tnode("node-a");
    seed(&a, 2, 2);
    a.fx.store.accept_grant(&grant_v(0, 2)).unwrap(); // v0 is now at seq 2
    let b = tnode("node-b");
    link(&a, &b, true);
    b.ex.sync_peer("node-a").await;
    wait_for("B to converge", || bound(&b) && held(&b) == 2 && b.fx.approvals.len() == 2).await;
    assert_eq!(b.fx.store.held_grant("fall-detect", "v0").unwrap().0, 2);
    assert_eq!(b.fx.store.held_grant("fall-detect", "v1").unwrap().0, 1);
}

#[tokio::test]
async fn a_node_syncs_when_a_peer_connects() {
    let a = tnode("node-a");
    seed(&a, 1, 1);
    let b = tnode_with("node-b", Arc::new(AdmitAll), LicenceExchangeConfig::default());
    link(&a, &b, true);
    wait_for("B to sync on connect", || bound(&b) && held(&b) == 1).await;
}

#[tokio::test]
async fn a_partition_heals_through_sync() {
    let (a, b, c) = (tnode("node-a"), tnode("node-b"), tnode("node-c"));
    link(&a, &b, true);
    // C was bound, then cut off.
    c.fx.bind();
    a.fx.bind();
    let prev = binding_rec(1, BindState::Bound, &grant_key(), &mesh());
    a.ex.issue_binding(sign_unbind(&prev, T0 + 1, &op()).unwrap()).await.unwrap();
    a.ex.issue_approval(approval_i(1)).await.unwrap();
    wait_for("B to hear", || b.fx.store.held_binding().is_some_and(|(s, _)| s == 2)).await;
    assert_eq!(c.fx.store.held_binding().unwrap().0, 1);
    // The partition heals: C reaches B and syncs.
    link(&b, &c, true);
    c.ex.sync_peer("node-b").await;
    wait_for("C to catch up", || {
        c.fx.store.held_binding().is_some_and(|(s, _)| s == 2) && c.fx.approvals.len() == 1
    })
    .await;
    assert_eq!(c.fx.store.binding_status(), Err(LicenceError::Unbound));
}

#[tokio::test]
async fn a_response_is_cut_at_the_caps_and_paged_per_record_kind() {
    let a = tnode("node-a");
    seed(&a, 200, 200);
    let SyncMsg::Response { binding, grants, approvals, more_grants, more_approvals } =
        a.ex.build_response(&None, &None)
    else {
        panic!("a response")
    };
    let size = |e: &SignedEnvelope| e.payload.len() + e.public_key.len() + e.signature.len();
    let total: usize = grants.iter().chain(&approvals).chain(&binding).map(size).sum();
    assert!(binding.is_some() && more_grants && more_approvals);
    assert!(grants.len() + approvals.len() < SYNC_MAX_ENTRIES && total <= SYNC_MAX_BYTES);

    // The whole set still arrives, by continuation pages with separate cursors.
    let b = tnode("node-b");
    link(&a, &b, true);
    b.ex.sync_peer("node-a").await;
    wait_for("B to hold every page", || held(&b) == 200 && b.fx.approvals.len() == 200).await;
}

#[tokio::test]
async fn a_large_set_of_one_kind_cannot_hide_the_other() {
    let a = tnode("node-a");
    seed(&a, 3, 300);
    let SyncMsg::Response { grants, approvals, more_grants, more_approvals, .. } =
        a.ex.build_response(&None, &None)
    else {
        panic!("a response")
    };
    assert_eq!((grants.len(), more_grants), (3, false), "all grants on page one");
    assert!(!approvals.is_empty() && more_approvals);
    // A follow-up on the approval cursor alone: no binding, no grants repeated.
    let last: Approval = serde_json::from_str(&approvals.last().unwrap().payload).unwrap();
    let cursor = Some(last.content_key());
    let gcur = grants.last().map(|g| {
        let g: CheckoutGrant = serde_json::from_str(&g.payload).unwrap();
        GrantCursor { seq: g.seq, cog_id: g.cog_id, version: g.version }
    });
    let SyncMsg::Response { binding, grants, approvals: next, .. } =
        a.ex.build_response(&gcur, &cursor)
    else {
        panic!("a response")
    };
    assert!(binding.is_none() && grants.is_empty() && !next.is_empty());
}

#[tokio::test]
async fn the_first_bad_signature_aborts_the_response_and_bans_the_peer() {
    let b = tnode("node-b");
    b.fx.bind();
    let m = mute("node-m");
    link(&m, &b, true);
    b.ex.sync_peer("node-m").await;
    wait_for("B's request", || b.ex.pending.contains_key("node-m")).await;

    let mut bad = grant_v(2, 1);
    bad.signature = "00".repeat(64);
    send_sync(
        &m,
        "node-b",
        response(None, vec![grant_v(1, 1), bad, grant_v(3, 1)], vec![approval_i(1)]),
    )
    .await;
    wait_for("the abort", || b.fx.names().contains(&"sync_bad_signature")).await;
    settle().await;
    assert!(b.fx.store.held_grant("fall-detect", "v1").is_some(), "verified before the bad one");
    assert!(b.fx.store.held_grant("fall-detect", "v3").is_none(), "the rest is discarded");
    assert!(b.fx.approvals.is_empty());
    assert!(b.ex.banned.contains_key("node-m"));
    // Banned: no new request goes out.
    b.ex.sync_peer("node-m").await;
    assert!(b.ex.pending.is_empty());
}

#[tokio::test]
async fn entries_under_another_key_or_with_a_stale_seq_are_dropped_before_verification() {
    let b = tnode("node-b");
    seed(&b, 0, 0);
    b.fx.store.accept_grant(&grant_v(1, 2)).unwrap();
    let m = mute("node-m");
    link(&m, &b, true);
    b.ex.sync_peer("node-m").await;
    wait_for("B's request", || b.ex.pending.contains_key("node-m")).await;

    // Garbage signatures everywhere: any of these reaching the verify would ban.
    let mut other_key = grant_v(5, 1);
    other_key.public_key = pk_hex(&sk(77));
    other_key.signature = "00".repeat(64);
    let mut stale = grant_v(1, 1);
    stale.signature = "00".repeat(64);
    let mut stale_binding = binding(1, BindState::Bound);
    stale_binding.signature = "00".repeat(64);
    let mut not_operator = binding(9, BindState::Bound);
    not_operator.public_key = pk_hex(&sk(77));
    not_operator.signature = "00".repeat(64);
    send_sync(
        &m,
        "node-b",
        response(Some(not_operator), vec![other_key, stale, grant_v(9, 1)], vec![]),
    )
    .await;
    wait_for("the good entry", || b.fx.store.held_grant("fall-detect", "v9").is_some()).await;
    assert!(b.ex.banned.is_empty() && !b.fx.names().contains(&"sync_bad_signature"));
    assert_eq!(b.fx.store.held_grant("fall-detect", "v1").unwrap().0, 2);
    // The stale binding record is dropped the same way.
    send_sync(&m, "node-b", response(Some(stale_binding), vec![], vec![])).await;
    settle().await;
    assert!(b.ex.banned.is_empty());
}

#[tokio::test]
async fn a_response_nobody_asked_for_is_dropped_unread() {
    let b = tnode("node-b");
    b.fx.bind();
    let m = mute("node-m");
    link(&m, &b, true);
    send_sync(&m, "node-b", response(None, vec![grant_v(1, 1)], vec![])).await;
    settle().await;
    assert!(b.fx.store.held_grant("fall-detect", "v1").is_none());
}

#[tokio::test]
async fn an_oversized_response_is_cut_at_the_caps_by_the_receiver() {
    let b = tnode("node-b");
    b.fx.bind();
    let m = mute("node-m");
    link(&m, &b, true);
    b.ex.sync_peer("node-m").await;
    wait_for("B's request", || b.ex.pending.contains_key("node-m")).await;
    send_sync(&m, "node-b", response(None, (0..300).map(|i| grant_v(i, 1)).collect(), vec![])).await;
    wait_for("some entries", || held(&b) > 0).await;
    settle().await;
    let n = held(&b);
    assert!((200..=SYNC_MAX_ENTRIES).contains(&n), "{n} entries applied");
}

#[tokio::test]
async fn a_second_sync_within_a_minute_is_not_answered() {
    let a = tnode("node-a");
    seed(&a, 1, 0);
    let b = tnode("node-b");
    link(&a, &b, true);
    b.ex.sync_peer("node-a").await;
    wait_for("the first sync", || held(&b) == 1).await;
    a.fx.store.accept_grant(&grant_v(7, 1)).unwrap();
    b.ex.pending.clear();
    b.ex.sync_peer("node-a").await;
    settle().await;
    assert_eq!(held(&b), 1, "the second sync, a few ms later, gets no answer");
}

#[tokio::test]
async fn a_peer_that_is_not_admitted_gets_no_answer() {
    let a = tnode_with("node-a", Arc::new(CtxAdmission), quiet_cfg());
    seed(&a, 1, 0);
    let (b, c) = (tnode("node-b"), tnode("node-c"));
    link(&a, &b, false); // as seen by A: not verified
    link(&a, &c, true); // a verified full node
    b.ex.sync_peer("node-a").await;
    c.ex.sync_peer("node-a").await;
    wait_for("the admitted peer", || bound(&c) && held(&c) == 1).await;
    settle().await;
    assert!(!bound(&b), "an unverified connection is not answered");
}

#[tokio::test]
async fn sync_traffic_does_not_consume_the_flood_budget() {
    let n = tnode("node-n");
    while n.ex.sync_buckets.take(Budget::Sync, 7, 1.0) {}
    assert_eq!(
        n.ex.accept_binding(&binding(1, BindState::Bound), Spend::Flood(7)),
        Ok(Receipt::New)
    );
    // And the other way: junk on the flood bucket leaves sync tokens.
    for seq in 2..=40 {
        let _ = n.ex.accept_binding(&junk_binding(seq), Spend::Flood(8));
    }
    assert!(n.ex.sync_buckets.take(Budget::Sync, 8, 1.0));
}
