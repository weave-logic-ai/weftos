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
            .with_project_store(PROJECT, proj_store.clone())
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
        .with_project_route(PROJECT, fwd.clone())
        .with_project_route("01J9ZXW0PRJCTBBBBBBBBBBBBB", fwd.clone())
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
    assert_eq!(t.proj_store.query(&vec8(3.0), 1)[0].id, 3);
    let from = t.proj_store.provenance("cog-on-a", 3).unwrap();
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
    let other = register(&t.reg, "other", Some("01J9ZXW0PRJCTBBBBBBBBBBBBB"), "ctl");
    let (st, body) = post(h.addr(), Some(&other), &batch_json(&[(2, vec8(2.0))], false)).await;
    assert_eq!(st, 502, "{body}");
    assert!(!body.contains("refused"), "owner detail stays off the wire: {body}");
    assert_eq!((t.ctl_store.len(), t.proj_store.len()), (1, 0));

    // No route at all.
    let lost = register(&t.reg, "lost", Some("01J9ZXW0PRJCTCCCCCCCCCCCCC"), "ctl");
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
    s.payload = s.payload.replace(PROJECT, "01J9ZXW0PRJCTBBBBBBBBBBBBB");
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
            .with_project_store(PROJECT, store.clone())
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
    for p in [Some("01J9ZXW0PRJCTBBBBBBBBBBBBB"), None] {
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


// ── Hooks: container listeners are scoped to one token ──────────────────

#[tokio::test]
async fn container_routes_get_a_token_scoped_listener_as_their_upstream() {
    let store = mem();
    let (bridge, reg) = bridge_over(
        StaticRouter::new().with_controller(
            "ctl",
            Arc::new(LocalForwarder::new(
                "n",
                Arc::new(StaticDirectory::new().with_fallback(store.clone())),
            )),
        ),
        RateBudget::default(),
    );
    let hooks = IngestHooks::new("own", reg.clone(), bridge, "127.0.0.1:80".parse().unwrap(), Some("127.0.0.1".parse().unwrap()));
    let mk = || crate::workload_runtime::HostContract::default_feed();

    // Native: the shared URL, no listener of its own.
    let (c, l) = hooks.prepare("native", mk()).await.unwrap();
    assert!(l.is_none() && c.ingest_upstream.is_none());
    assert_eq!(c.ingest_url.as_deref(), Some("http://127.0.0.1:80/api/v1/store/ingest"));

    // Container: its own listener, accepting only its own token.
    let (c1, l1) = hooks.prepare("container", mk()).await.unwrap();
    let (c2, l2) = hooks.prepare("container", mk()).await.unwrap();
    let up1 = c1.ingest_upstream.unwrap();
    assert_eq!(Some(up1), l1.as_ref().map(|h| h.addr()));
    let (t1, t2) = (c1.token.expose().to_string(), c2.token.expose().to_string());
    let lease1 = hooks.lease(InstanceBinding::new("one", None, "ctl"), c1, l1).unwrap();
    let _lease2 = hooks.lease(InstanceBinding::new("two", None, "ctl"), c2, l2).unwrap();
    let b = batch_json(&[(1, vec8(1.0))], false);
    assert_eq!(post(up1, Some(&t1), &b).await.0, 200);
    assert_eq!(post(up1, Some(&t2), &b).await.0, 403, "another instance's valid token");

    // Deactivate (stop) closes the door; activate (start) reopens it.
    hooks.deactivate(&lease1);
    assert_eq!(post(up1, Some(&t1), &b).await.0, 401);
    hooks.activate(&lease1).unwrap();
    assert_eq!(post(up1, Some(&t1), &b).await.0, 200);
}

#[tokio::test]
async fn a_scoped_listener_closes_when_its_lease_drops() {
    let (bridge, reg) = bridge_over(StaticRouter::new(), RateBudget::default());
    let hooks = IngestHooks::new(
        "own",
        reg,
        bridge,
        "127.0.0.1:80".parse().unwrap(),
        Some("127.0.0.1".parse().unwrap()),
    );
    let (c, l) = hooks
        .prepare("container", crate::workload_runtime::HostContract::default_feed())
        .await
        .unwrap();
    let up = c.ingest_upstream.unwrap();
    let lease = hooks.lease(InstanceBinding::new("i", None, "ctl"), c, l).unwrap();
    assert!(tokio::net::TcpStream::connect(up).await.is_ok(), "listening while leased");
    drop(lease);
    let mut closed = false;
    for _ in 0..40 {
        if tokio::net::TcpStream::connect(up).await.is_err() {
            closed = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(closed, "the scoped listener must close with its lease");
}

/// Revokes the instance the moment the bridge asks where to send its batch,
/// as a stop or unload completing while a request is in flight would.
struct RevokingRouter {
    inner: StaticRouter,
    reg: Arc<TokenRegistry>,
    instance: String,
}

impl StoreRouter for RevokingRouter {
    fn route(&self, b: &InstanceBinding) -> Option<Arc<dyn Forwarder>> {
        self.reg.revoke(&self.instance);
        self.inner.route(b)
    }
}

#[tokio::test]
async fn a_token_revoked_while_a_request_is_in_flight_writes_nothing() {
    let store = mem();
    let reg = Arc::new(TokenRegistry::new());
    let dir: Arc<dyn StoreDirectory> = Arc::new(StaticDirectory::new().with_fallback(store.clone()));
    let router = RevokingRouter {
        inner: StaticRouter::new().with_controller("ctl", Arc::new(LocalForwarder::new("n", dir))),
        reg: reg.clone(),
        instance: "inst".into(),
    };
    let bridge = IngestBridge::new(reg.clone(), Arc::new(router), RateBudget::default(), BridgeConfig::default());
    let token = register(&reg, "inst", None, "ctl");
    let h = bridge.bind(lo(), BridgeScope::Any).await.unwrap();
    let (st, _) = post(h.addr(), Some(&token), &batch_json(&[(1, vec8(1.0))], false)).await;
    assert_eq!(st, 401, "revoked between authentication and the write");
    assert_eq!(store.len(), 0);
}

#[tokio::test]
async fn a_legitimate_post_gets_through_while_anonymous_connections_hold_every_slot() {
    use std::time::Duration;
    let store = mem();
    let reg = Arc::new(TokenRegistry::new());
    let dir: Arc<dyn StoreDirectory> = Arc::new(StaticDirectory::new().with_fallback(store.clone()));
    let router = StaticRouter::new().with_controller("ctl", Arc::new(LocalForwarder::new("n", dir)));
    let cfg = BridgeConfig {
        preauth_timeout: Duration::from_millis(400),
        max_unauthenticated: 2,
        ..Default::default()
    };
    let bridge = IngestBridge::new(reg.clone(), Arc::new(router), RateBudget::default(), cfg);
    let token = register(&reg, "inst", None, "ctl");
    let h = bridge.bind(lo(), BridgeScope::Any).await.unwrap();
    let b = batch_json(&[(1, vec8(1.0))], false);

    // Two silent connections hold both anonymous slots. They are closed at
    // the pre-auth deadline; the legitimate post waits for a slot (first
    // come, first served) and is served once they go.
    let _a = tokio::net::TcpStream::connect(h.addr()).await.unwrap();
    let _b = tokio::net::TcpStream::connect(h.addr()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(post(h.addr(), Some(&token), &b).await.0, 200, "waited for a slot");
    assert_eq!(store.len(), 1);
}

#[test]
fn vector_ids_are_namespaced_per_instance_in_a_shared_store() {
    fn check(store: &dyn IngestStore) {
        let (a, b) = (
            Provenance { instance_id: "a".into(), source_node: "n".into() },
            Provenance { instance_id: "b".into(), source_node: "n".into() },
        );
        let v = |id, x: f32| IngestVector { id, values: vec8(x) };
        assert_eq!(store.ingest(&a, &[v(1, 1.0)], true).unwrap().accepted, 1);
        // Same id from another instance: not an overwrite, not a dedup.
        let o = store.ingest(&b, &[v(1, 2.0)], true).unwrap();
        assert_eq!((o.accepted, o.deduped, o.total), (1, 0, 2));
        // Identical values from another instance are not deduped against a's.
        let o = store.ingest(&b, &[v(7, 1.0)], true).unwrap();
        assert_eq!((o.accepted, o.deduped), (1, 0));
        // a's own repeat is deduped.
        assert_eq!(store.ingest(&a, &[v(1, 1.0)], true).unwrap().deduped, 1);
        let hits = store.query(&vec8(1.0), 3);
        assert!(hits.iter().any(|h| h.instance_id == "a" && h.id == 1));
        assert!(hits.iter().any(|h| h.instance_id == "b" && h.id == 1));
        // a's value was not replaced by b's write to "id 1".
        let a1 = hits.iter().find(|h| h.instance_id == "a" && h.id == 1).unwrap();
        assert!(a1.distance < 1e-6, "{a1:?}");
    }
    check(&MemoryIngestStore::new(100));
    #[cfg(feature = "ecc")]
    check(&VectorBackendStore::new(Arc::new(crate::vector_hnsw::HnswBackend::new(
        crate::hnsw_service::HnswServiceConfig::default(),
    ))));
}
