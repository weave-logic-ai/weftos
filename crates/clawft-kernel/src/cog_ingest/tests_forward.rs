//! Two-node forwarding and the owner's checks.

use std::sync::Arc;

use ed25519_dalek::SigningKey;

use super::forward::{
    FORWARD_RESPONSE_DOMAIN, ForwardOutcome, ForwardRefusal, ForwardRequest,
    ForwardResponse,
};
use super::tests::*;
use super::*;
use crate::workload_ctl::msg::open;

// ── Two nodes ───────────────────────────────────────────────────────────

struct TwoNodes {
    bridge: Arc<IngestBridge>,
    reg: Arc<TokenRegistry>,
    owner: Arc<StoreOwnerService>,
    key_a: SigningKey,
    node_b: String,
    proj_store: Arc<MemoryIngestStore>,
    ctl_store: Arc<MemoryIngestStore>,
}

/// Node A runs the cog and the bridge; node B owns the stores.
fn two_nodes(budget: RateBudget) -> TwoNodes {
    let (key_a, key_b) = (key(21), key(22));
    let (proj_store, ctl_store) = (mem(), mem());
    let dir: Arc<dyn StoreDirectory> = Arc::new(
        StaticDirectory::new()
            .with_project(PROJECT, proj_store.clone())
            .with_fallback(ctl_store.clone()),
    );
    let owner = Arc::new(StoreOwnerService::new(
        key_b.clone(),
        Arc::new(KeyPolicy::new().allow_any(pubkey(&key_a))),
        dir,
    ));
    let conn = Arc::new(OwnerConnector::new(false));
    let addr = conn.register_local("node-b", owner.clone());
    let node_b = node_id(&key_b);
    let fwd: Arc<dyn Forwarder> = Arc::new(MeshForwarder::new(
        key_a.clone(),
        &node_b,
        pubkey(&key_b),
        &addr,
        conn,
    ));
    let router = StaticRouter::new()
        .with_project(PROJECT, fwd.clone())
        .with_project("01J9ZXW0OTHERPROJECTAAAAAA", fwd.clone())
        .with_controller("ctl", fwd);
    let (bridge, reg) = bridge_over(router, budget);
    TwoNodes {
        bridge,
        reg,
        owner,
        key_a,
        node_b,
        proj_store,
        ctl_store,
    }
}

#[tokio::test]
async fn vectors_from_a_cog_on_node_a_are_queryable_on_node_b() {
    let t = two_nodes(RateBudget::default());
    let token = register(&t.reg, "cog-on-a", Some(PROJECT), "ctl");
    let h = t.bridge.bind(lo(), BridgeScope::Any).await.unwrap();
    let vs: Vec<_> = (0..5u64).map(|i| (i, vec8(i as f32))).collect();
    let (st, body) = post(h.addr(), Some(&token), &batch_json(&vs, true)).await;
    assert_eq!(st, 200, "{body}");
    // Queried on B's store: the project's, written by A's bridge.
    assert_eq!(t.proj_store.len(), 5);
    assert_eq!(t.proj_store.query(&vec8(3.0), 1)[0].0, 3);
    let from = t.proj_store.provenance(3).unwrap();
    assert_eq!(from.instance_id, "cog-on-a");
    assert_eq!(from.source_node, node_id(&t.key_a), "the owner records the bridge node");
    assert_eq!(t.ctl_store.len(), 0);

    // Dedup is honoured at the owner.
    let (st, body) = post(h.addr(), Some(&token), &batch_json(&vs, true)).await;
    assert_eq!(st, 200);
    let r: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!((r["accepted"].as_u64(), r["deduped"].as_u64()), (Some(0), Some(5)));
    assert_eq!(t.proj_store.len(), 5);
}

#[tokio::test]
async fn two_node_project_less_goes_to_the_owner_fallback_and_unknown_projects_fail() {
    let t = two_nodes(RateBudget::default());
    let h = t.bridge.bind(lo(), BridgeScope::Any).await.unwrap();
    let none = register(&t.reg, "no-project", None, "ctl");
    assert_eq!(post(h.addr(), Some(&none), &batch_json(&[(1, vec8(1.0))], false)).await.0, 200);
    assert_eq!((t.ctl_store.len(), t.proj_store.len()), (1, 0));

    // Routed to B, but B has no store for that project.
    let other = register(&t.reg, "other", Some("01J9ZXW0OTHERPROJECTAAAAAA"), "ctl");
    let (st, body) = post(h.addr(), Some(&other), &batch_json(&[(2, vec8(2.0))], false)).await;
    assert_eq!(st, 502, "{body}");
    assert!(!body.contains("refused"), "owner detail stays off the wire: {body}");
    assert_eq!((t.ctl_store.len(), t.proj_store.len()), (1, 0));

    // No route at all.
    let lost = register(&t.reg, "lost", Some("01J9ZXW0NOROUTEAAAAAAAAAAA"), "ctl");
    assert_eq!(post(h.addr(), Some(&lost), &batch_json(&[(3, vec8(3.0))], false)).await.0, 502);
}

#[tokio::test]
async fn the_kept_connection_serves_many_batches() {
    let t = two_nodes(RateBudget::default());
    let token = register(&t.reg, "cog", Some(PROJECT), "ctl");
    let h = t.bridge.bind(lo(), BridgeScope::Any).await.unwrap();
    for i in 0..4u64 {
        let b = batch_json(&[(i, vec8(i as f32))], false);
        assert_eq!(post(h.addr(), Some(&token), &b).await.0, 200);
    }
    assert_eq!(t.proj_store.len(), 4);
}

fn signed_for(
    k: &SigningKey,
    target: &str,
    project: Option<&str>,
    ttl_ms: u64,
) -> (ForwardRequest, crate::workload_ctl::msg::SignedCtl) {
    let b = InstanceBinding::new("i", project.map(String::from), "ctl");
    let batch = parse_batch(batch_json(&[(1, vec8(1.0))], false).as_bytes()).unwrap();
    ForwardRequest::signed(k, target, &b, &batch, ttl_ms).unwrap()
}

fn outcome_of(resp: &crate::workload_ctl::msg::SignedCtl, owner: &SigningKey) -> ForwardOutcome {
    let pk = open(FORWARD_RESPONSE_DOMAIN, resp).unwrap();
    assert_eq!(pk, pubkey(owner));
    serde_json::from_str::<ForwardResponse>(&resp.payload).unwrap().outcome
}

fn refusal(o: ForwardOutcome) -> ForwardRefusal {
    match o {
        ForwardOutcome::Refused { code, .. } => code,
        ForwardOutcome::Ok { .. } => panic!("expected a refusal"),
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[test]
fn owner_refuses_forged_replayed_expired_and_unauthorised_forwards() {
    let t = two_nodes(RateBudget::default());
    let key_b = key(22);

    // Accepted once; the same signed bytes again are a replay.
    let (_, s) = signed_for(&t.key_a, &t.node_b, Some(PROJECT), 30_000);
    let (r, authed) = t.owner.handle_at(&s, now());
    assert!(authed && matches!(outcome_of(&r, &key_b), ForwardOutcome::Ok { .. }));
    let (r, _) = t.owner.handle_at(&s, now());
    assert_eq!(refusal(outcome_of(&r, &key_b)), ForwardRefusal::Replay);

    // A key not in the policy.
    let (_, s) = signed_for(&key(99), &t.node_b, Some(PROJECT), 30_000);
    let (r, authed) = t.owner.handle_at(&s, now());
    assert!(!authed);
    assert_eq!(refusal(outcome_of(&r, &key_b)), ForwardRefusal::Unauthorized);

    // Addressed to another node.
    let (_, s) = signed_for(&t.key_a, "someone-else", Some(PROJECT), 30_000);
    assert_eq!(
        refusal(outcome_of(&t.owner.handle_at(&s, now()).0, &key_b)),
        ForwardRefusal::NotForMe
    );

    // Expired, and issued in the future.
    let (_, s) = signed_for(&t.key_a, &t.node_b, Some(PROJECT), 1_000);
    assert_eq!(
        refusal(outcome_of(&t.owner.handle_at(&s, now() + 60_000).0, &key_b)),
        ForwardRefusal::Expired
    );
    let (_, s) = signed_for(&t.key_a, &t.node_b, Some(PROJECT), 30_000);
    assert_eq!(
        refusal(outcome_of(&t.owner.handle_at(&s, now().saturating_sub(120_000)).0, &key_b)),
        ForwardRefusal::Expired
    );

    // Tampered after signing: another project, more vectors.
    let (_, mut s) = signed_for(&t.key_a, &t.node_b, Some(PROJECT), 30_000);
    s.payload = s.payload.replace(PROJECT, "01J9ZXW0OTHERPROJECTAAAAAA");
    let (r, authed) = t.owner.handle_at(&s, now());
    assert!(!authed);
    assert_eq!(refusal(outcome_of(&r, &key_b)), ForwardRefusal::Signature);

    // A request signed by one key but claiming another node id.
    let (mut req, _) = signed_for(&t.key_a, &t.node_b, Some(PROJECT), 30_000);
    req.requester = node_id(&key(99));
    let payload = serde_json::to_string(&req).unwrap();
    let forged = crate::workload_ctl::msg::sign(forward::FORWARD_DOMAIN, payload, &t.key_a);
    assert_eq!(
        refusal(outcome_of(&t.owner.handle_at(&forged, now()).0, &key_b)),
        ForwardRefusal::Signature
    );

    // Nothing but the one accepted batch was written.
    assert_eq!(t.proj_store.len(), 1);
}

#[test]
fn a_project_restricted_key_cannot_forward_for_other_projects_or_project_less() {
    let (key_a, key_b) = (key(31), key(32));
    let store = mem();
    let dir: Arc<dyn StoreDirectory> = Arc::new(
        StaticDirectory::new()
            .with_project(PROJECT, store.clone())
            .with_fallback(store.clone()),
    );
    let owner = StoreOwnerService::new(
        key_b.clone(),
        Arc::new(KeyPolicy::new().allow_projects(pubkey(&key_a), &[PROJECT])),
        dir,
    );
    let nb = node_id(&key_b);
    let (_, ok) = signed_for(&key_a, &nb, Some(PROJECT), 30_000);
    assert!(matches!(
        outcome_of(&owner.handle_at(&ok, now()).0, &key_b),
        ForwardOutcome::Ok { .. }
    ));
    for p in [Some("01J9ZXW0OTHERPROJECTAAAAAA"), None] {
        let (_, s) = signed_for(&key_a, &nb, p, 30_000);
        assert_eq!(
            refusal(outcome_of(&owner.handle_at(&s, now()).0, &key_b)),
            ForwardRefusal::Unauthorized
        );
    }
    assert_eq!(store.len(), 1);
}

#[tokio::test]
async fn a_forwarder_rejects_a_response_signed_by_the_wrong_owner() {
    let (key_a, key_b, imposter) = (key(41), key(42), key(43));
    let dir: Arc<dyn StoreDirectory> = Arc::new(StaticDirectory::new().with_fallback(mem()));
    // The service at node-b's address is actually the imposter's.
    let svc = Arc::new(StoreOwnerService::new(
        imposter.clone(),
        Arc::new(KeyPolicy::new().allow_any(pubkey(&key_a))),
        dir,
    ));
    let conn = Arc::new(OwnerConnector::new(false));
    let addr = conn.register_local("node-b", svc);
    let fwd = MeshForwarder::new(key_a, &node_id(&key_b), pubkey(&key_b), &addr, conn);
    let b = InstanceBinding::new("i", None, "ctl");
    let batch = parse_batch(batch_json(&[(1, vec8(1.0))], false).as_bytes()).unwrap();
    // The imposter refuses (the request is addressed to node-b, not to it),
    // and in any case its signature is not the pinned owner key.
    let err = fwd.forward(&b, &batch).await.unwrap_err();
    assert!(matches!(err, IngestError::Unavailable(_)), "{err}");
}

