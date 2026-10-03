//! Review round of phase 1b: serve sessions, restrictive propagation,
//! admitted-only floods, forwarding rules, conflicts, sync limits.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use super::exchange_sync::Pending;
use super::tests_common::*;
use super::tests_exchange::*;
use super::*;
use crate::ipc::{KernelMessage, MessagePayload, MessageTarget};
use crate::mesh_admit::PeerClass;
use crate::mesh_delivery::PeerCtx;
use crate::mesh_runtime::COG_SYNC_TOPIC;

fn grant_v(i: u32, seq: u64, issued: u64, ttl: u64) -> SignedGrant {
    let mut r = grant_rec(seq, issued, ttl, &["x86_64"]);
    r.version = format!("v{i}");
    sign_grant(&r, &grant_key()).unwrap()
}

fn ctx(peer: &str) -> PeerCtx {
    PeerCtx {
        peer_id: peer.into(),
        node_verified: true,
        class: PeerClass::Node,
        remote_static: None,
        src_scope: None,
    }
}

fn seed_grants(n: &TNode, count: u32) {
    n.fx.bind();
    for i in 0..count {
        n.fx.store.accept_grant(&grant_v(i, 1, T0, 3600)).unwrap();
    }
}

fn held(n: &TNode) -> usize {
    (0..1000).filter(|i| n.fx.store.held_grant("fall-detect", &format!("v{i}")).is_some()).count()
}

fn req(g: Option<GrantCursor>) -> SyncMsg {
    SyncMsg::Request { grant_after: g, approval_after: None }
}

/// The page a serve answered with: its grants and the cursor after the last.
fn page(out: Vec<serde_json::Value>) -> Option<(usize, GrantCursor)> {
    let SyncMsg::Response { grants, .. } = serde_json::from_value(out.into_iter().next()?).unwrap()
    else {
        panic!("a response")
    };
    let last: CheckoutGrant = serde_json::from_str(&grants.last()?.payload).unwrap();
    Some((grants.len(), GrantCursor { seq: last.seq, cog_id: last.cog_id, version: last.version }))
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

fn old() -> Instant {
    Instant::now().checked_sub(Duration::from_secs(121)).unwrap()
}

fn mute(id: &str) -> TNode {
    let m = tnode(id);
    m.fx.local.set(None);
    m
}

fn block_saves(n: &TNode) {
    let p = n.fx.dir.path().join("checkout_grants.json");
    std::fs::remove_file(&p).unwrap();
    std::fs::create_dir(&p).unwrap(); // a rename onto a directory fails
}

// ── 1: serve sessions ────────────────────────────────────────────

#[tokio::test]
async fn a_cursor_request_with_no_open_session_gets_no_answer() {
    let a = tnode("node-a");
    seed_grants(&a, 3);
    let cur = GrantCursor { seq: 1, cog_id: "fall-detect".into(), version: "v0".into() };
    assert!(a.ex.serve(&ctx("p"), 1, req(Some(cur.clone()))).is_empty(), "no session");
    // And inside the minute after a fresh sync that was refused for the gap.
    assert!(!a.ex.serve(&ctx("p"), 1, req(None)).is_empty());
    assert!(a.ex.serve(&ctx("q"), 1, req(Some(cur))).is_empty(), "another peer has no session");
}

#[tokio::test]
async fn a_continuation_is_answered_only_against_its_session() {
    let a = tnode("node-a");
    seed_grants(&a, 200);
    let p = ctx("p");
    let (n1, c1) = page(a.ex.serve(&p, 1, req(None))).unwrap();
    assert_eq!(n1, 128);
    let (n2, c2) = page(a.ex.serve(&p, 1, req(Some(c1.clone())))).unwrap();
    assert_eq!(n2, 72);
    // A replay of the first cursor is behind the last page served.
    assert!(a.ex.serve(&p, 1, req(Some(c1))).is_empty(), "rewind refused");
    // A fresh request inside the minute is refused too, cursor or not.
    assert!(a.ex.serve(&p, 1, req(None)).is_empty());
    assert!(!a.ex.serve(&p, 1, req(Some(c2))).is_empty(), "the legitimate cursor still works");
}

#[tokio::test]
async fn a_session_ends_at_max_pages_and_at_its_ttl() {
    let cfg = LicenceExchangeConfig { max_pages: 2, ..quiet_cfg() };
    let a = tnode_with("node-a", Arc::new(AdmitAll), cfg);
    seed_grants(&a, 400);
    let p = ctx("p");
    let (_, c1) = page(a.ex.serve(&p, 1, req(None))).unwrap();
    let (_, c2) = page(a.ex.serve(&p, 1, req(Some(c1))).into_iter().collect()).unwrap();
    assert!(a.ex.serve(&p, 1, req(Some(c2))).is_empty(), "third page is over max_pages");

    let b = tnode("node-b");
    seed_grants(&b, 200);
    let (_, c1) = page(b.ex.serve(&p, 1, req(None))).unwrap();
    b.ex.sessions.get_mut("p").unwrap().opened = old();
    assert!(b.ex.serve(&p, 1, req(Some(c1))).is_empty(), "the session expired");
}

// ── 2: restrictive records propagate even when unsaved ───────────

#[tokio::test]
async fn an_unsaved_unbind_and_withdrawal_are_still_flooded() {
    let (a, b) = (tnode("node-a"), tnode("node-b"));
    link(&a, &b, true);
    seed_grants(&a, 1);
    seed_grants(&b, 1);
    block_saves(&a);

    let w = grant_v(0, 2, T0 + 10, 0);
    assert_eq!(a.ex.issue_grant(w).await, Ok(Receipt::New), "applied in memory, unsaved");
    wait_for("B to hold the withdrawal", || {
        b.fx.store.held_grant("fall-detect", "v0").is_some_and(|(seq, _)| seq == 2)
    })
    .await;

    let prev = binding_rec(1, BindState::Bound, &grant_key(), &mesh());
    assert_eq!(a.ex.issue_binding(sign_unbind(&prev, &op()).unwrap()).await, Ok(Receipt::New));
    wait_for("B unbound", || b.fx.store.binding_status() == Err(LicenceError::Unbound)).await;
    assert_eq!(a.fx.store.binding_status(), Err(LicenceError::Unbound));
}

// ── 3: floods go only to admitted peers ──────────────────────────

#[tokio::test]
async fn floods_never_go_to_unverified_peers() {
    let a = tnode_with("node-a", Arc::new(CtxAdmission), quiet_cfg());
    let (b, c) = (tnode("node-b"), tnode("node-c"));
    link(&a, &b, false); // an unverified route
    link(&a, &c, true); // a verified full node
    a.ex.issue_binding(binding(1, BindState::Bound)).await.unwrap();
    wait_for("C to hold the binding", || bound(&c)).await;
    a.ex.issue_grant(grant(1, T0, 3600, &["x86_64"])).await.unwrap();
    a.ex.issue_approval(approval(&[sha_of("x86_64")])).await.unwrap();
    wait_for("C to hold the rest", || has_grant(&c, "1.2.0") && c.fx.approvals.len() == 1).await;
    settle().await;
    assert!(!bound(&b) && b.fx.approvals.is_empty(), "nothing reached the unverified peer");
    assert_eq!(a.ex.flooded.load(Ordering::Relaxed), 3, "three records, one admitted peer");
}

// ── 4: forwarding of sync-applied records ────────────────────────

#[tokio::test]
async fn a_sync_forwards_bindings_and_withdrawals_but_not_grants_or_approvals() {
    let (b, c) = (tnode("node-b"), tnode("node-c"));
    b.fx.bind();
    c.fx.bind();
    let m = mute("node-m");
    link(&m, &b, true);
    link(&b, &c, true);
    b.ex.sync_peer("node-m").await;
    wait_for("B's request", || b.ex.pending.contains_key("node-m")).await;
    send_sync(
        &m,
        "node-b",
        response(
            Some(binding(2, BindState::Bound)),
            vec![grant_v(1, 1, T0, 3600), grant_v(2, 1, T0 + 5, 0)],
            vec![approval(&[sha_of("x86_64")])],
        ),
    )
    .await;
    wait_for("B to apply the page", || b.fx.approvals.len() == 1 && held(&b) == 2).await;
    wait_for("C to hear the binding and the withdrawal", || {
        c.fx.store.held_signed_binding().is_some_and(|(s, _)| s == 2)
            && c.fx.store.held_grant("fall-detect", "v2").is_some()
    })
    .await;
    settle().await;
    assert!(c.fx.store.held_grant("fall-detect", "v1").is_none(), "a plain grant is not forwarded");
    assert!(c.fx.approvals.is_empty(), "an approval is not forwarded");
}

// ── 5: unbind bytes, conflicts remembered ────────────────────────

#[test]
fn two_unbinds_of_the_same_record_are_byte_identical() {
    let prev = binding_rec(1, BindState::Bound, &grant_key(), &mesh());
    assert_eq!(sign_unbind(&prev, &op()).unwrap(), sign_unbind(&prev, &op()).unwrap());
}

#[tokio::test]
async fn a_conflicting_record_is_chained_once_not_on_every_repeat() {
    let n = tnode("node-n");
    n.fx.bind();
    let mut rec = binding_rec(1, BindState::Bound, &grant_key(), &mesh());
    rec.steward_node_id = "node-other".into();
    let rival = sign_binding(&rec, &op()).unwrap();
    let conflict = Err(ExchangeError::Licence(LicenceError::Conflict(1)));
    assert_eq!(n.ex.accept_binding(&rival, Spend::Exempt), conflict);
    assert_eq!(n.ex.accept_binding(&rival, Spend::Exempt), Ok(Receipt::Known), "remembered");
    let events = n.fx.names().iter().filter(|e| **e == "binding_conflict").count();
    assert_eq!(events, 1);

    n.fx.store.accept_grant(&grant_v(0, 1, T0, 3600)).unwrap();
    let mut r = grant_rec(1, T0, 3600, &["x86_64", "aarch64"]);
    r.version = "v0".into();
    let rival = sign_grant(&r, &grant_key()).unwrap();
    assert!(n.ex.accept_grant(&rival, Spend::Exempt).is_err());
    assert_eq!(n.ex.accept_grant(&rival, Spend::Exempt), Ok(Receipt::Known));
    assert_eq!(n.fx.names().iter().filter(|e| **e == "grant_conflict").count(), 1);
}

// ── sync limits ──────────────────────────────────────────────────

#[tokio::test]
async fn the_seen_set_is_capped_and_forgets_the_oldest_first() {
    let n = tnode("node-s");
    for i in 0..(super::exchange_types::MAX_SEEN as u32 + 10) {
        let mut k = [0u8; 32];
        k[..4].copy_from_slice(&i.to_le_bytes());
        n.ex.remember(k);
    }
    assert_eq!(n.ex.seen_len(), super::exchange_types::MAX_SEEN);
    assert!(!n.ex.already_seen(&[0u8; 32]), "the oldest was dropped");
}

#[tokio::test]
async fn paging_stops_at_max_pages() {
    let a = tnode("node-a");
    seed_grants(&a, 300);
    let b = tnode_with("node-b", Arc::new(AdmitAll), LicenceExchangeConfig { max_pages: 2, ..quiet_cfg() });
    link(&a, &b, true);
    b.ex.sync_peer("node-a").await;
    wait_for("two pages", || held(&b) == 256).await;
    settle().await;
    assert_eq!(held(&b), 256, "a third page is not asked for");
    assert!(b.ex.pending.is_empty());
}

#[tokio::test]
async fn a_page_with_no_progress_does_not_loop() {
    let b = tnode("node-b");
    b.fx.bind();
    let m = mute("node-m");
    link(&m, &b, true);
    b.ex.sync_peer("node-m").await;
    wait_for("request", || b.ex.pending.contains_key("node-m")).await;
    let lie = SyncMsg::Response {
        binding: None,
        grants: vec![],
        approvals: vec![],
        more_grants: true,
        more_approvals: false,
    };
    send_sync(&m, "node-b", lie).await;
    wait_for("the page to be consumed", || b.ex.pending.is_empty()).await;
    settle().await;
    assert!(b.ex.pending.is_empty(), "no follow-up request");

    // With progress there is a follow-up on the new cursor.
    b.ex.sync_peer("node-m").await;
    wait_for("request", || b.ex.pending.contains_key("node-m")).await;
    let more = SyncMsg::Response {
        binding: None,
        grants: vec![grant_v(1, 1, T0, 3600)],
        approvals: vec![],
        more_grants: true,
        more_approvals: false,
    };
    send_sync(&m, "node-b", more).await;
    wait_for("the follow-up", || {
        b.ex.pending.get("node-m").is_some_and(|p| p.pages == 1 && p.grant_after.is_some())
    })
    .await;
}

#[tokio::test]
async fn a_response_after_the_request_expired_is_dropped() {
    let b = tnode("node-b");
    b.fx.bind();
    let m = mute("node-m");
    link(&m, &b, true);
    b.ex.pending.insert(
        "node-m".into(),
        Pending { sent: old(), pages: 0, grant_after: None, approval_after: None },
    );
    send_sync(&m, "node-b", response(None, vec![grant_v(1, 1, T0, 3600)], vec![])).await;
    settle().await;
    assert_eq!(held(&b), 0);
}

#[tokio::test]
async fn a_deferred_entry_does_not_abort_the_page() {
    let b = tnode("node-b");
    b.fx.bind();
    let m = mute("node-m");
    link(&m, &b, true);
    b.ex.sync_peer("node-m").await;
    wait_for("request", || b.ex.pending.contains_key("node-m")).await;
    let ahead = grant_v(1, 1, T0 + 1000, 3600); // beyond the skew: deferred
    send_sync(&m, "node-b", response(None, vec![ahead, grant_v(2, 1, T0, 3600)], vec![])).await;
    wait_for("the entry after the deferred one", || {
        b.fx.store.held_grant("fall-detect", "v2").is_some()
    })
    .await;
    assert!(b.fx.store.held_grant("fall-detect", "v1").is_none());
    assert!(b.ex.banned.is_empty());
}
