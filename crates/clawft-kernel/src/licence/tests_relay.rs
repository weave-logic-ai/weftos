//! The steward relay and the swarm transport, against the stub licence.

use std::sync::atomic::Ordering;
use std::time::Duration;

use super::tests_common::*;
use super::tests_stub::*;
use super::*;
use crate::ipc::{KernelMessage, MessagePayload, MessageTarget};
use crate::mesh_artifact_types::ArtifactKey;
use crate::mesh_cog::TOPIC_CHECKOUT_REPLY;
use crate::mesh_delivery::PeerCtx;
use crate::mesh_swarm_fetch::SwarmFetchOptions;
use crate::mesh_swarm_picker::PeerCandidate;
use std::sync::Arc;

struct Rig {
    net: Arc<Net>,
    stub: Arc<StubLicence>,
    steward: Arc<Member>,
    gate: Arc<TestGate>,
    flood: Arc<NetFlood>,
}

fn rig(permit: bool, delay: Duration) -> Rig {
    let net = Arc::new(Net::default());
    let clock = Arc::new(std::sync::atomic::AtomicU64::new(T0));
    let stub = StubLicence::new(clock.clone(), delay);
    let gate = TestGate::new(permit);
    let flood = Arc::new(NetFlood { net: net.clone(), from: "a".into(), floods: Default::default() });
    let steward = add_member(&net, "a", |fx, ex| {
        Some(Arc::new(CheckoutRelay::new(
            fx.store.clone(),
            ex.clone(),
            steward_client(&stub, &clock),
            gate.clone(),
            flood.clone(),
            None,
        )))
    });
    Rig { net, stub, steward, gate, flood }
}

fn content(arch: &str) -> ArtifactKey {
    ArtifactKey::Content(b3_bytes(arch))
}

fn admitted(id: &str) -> PeerCtx {
    PeerCtx {
        peer_id: id.into(),
        node_verified: true,
        class: crate::mesh_admit::PeerClass::Node,
        remote_static: None,
        src_scope: None,
    }
}

async fn fetch_from(m: &Arc<Member>, from: &[&str]) {
    let cands: Vec<PeerCandidate> = from.iter().map(|p| PeerCandidate::new(*p)).collect();
    m.ex.swarm_fetch(m.mesh.tunnel().dialer(), cands, content("aarch64"), &SwarmFetchOptions::default())
        .await
        .expect("fetch");
}

#[tokio::test]
async fn three_nodes_one_checkout_one_transfer_then_peers_share() {
    let r = rig(true, Duration::ZERO);
    let (b, c) = (add_member(&r.net, "b", |_, _| None), add_member(&r.net, "c", |_, _| None));

    let g = r.steward.mesh.checkout_local(&wire("aarch64")).await.expect("steward checkout");
    assert_eq!(r.stub.checkouts.load(Ordering::SeqCst), 1);
    assert_eq!(r.stub.byte_transfers.load(Ordering::SeqCst), 1);
    assert_eq!(r.flood.floods.load(Ordering::SeqCst), 1, "the grant was flooded once");
    assert!(g.payload.contains("fall-detect"));
    assert!(r.gate.asked.lock().unwrap().iter().any(|(_, a)| a == "cog.checkout"));

    // B fetches from the steward; C only from B.
    fetch_from(&b, &["a"]).await;
    fetch_from(&c, &["b"]).await;
    for m in [&b, &c] {
        let d = m.ex.resolve(&content("aarch64")).expect("holds the bytes");
        assert_eq!(m.ex.read_all(&d.id()).unwrap(), bytes_of("aarch64"));
    }
    let served = |m: &Arc<Member>| m.mesh.tunnel().counters.served_sessions.load(Ordering::SeqCst);
    assert_eq!((served(&r.steward), served(&b), served(&c)), (1, 1, 0));
    assert!(
        !r.net.log.lock().unwrap().iter().any(|(f, t, _)| (f == "c" && t == "a") || (f == "a" && t == "c")),
        "the steward and C never talked: C got its bytes from a peer"
    );

    // The licence saw exactly one checkout and one byte transfer in all.
    assert_eq!(r.stub.checkouts.load(Ordering::SeqCst), 1);
    assert_eq!(r.stub.byte_transfers.load(Ordering::SeqCst), 1);

    // A grant alone does not make the bytes runnable.
    let (sha, b3) = (sha_of("aarch64"), b3_of("aarch64"));
    let denied = may_run(
        &b.fx.store,
        &b.fx.approvals,
        &RunRequest { cog_id: "fall-detect", version: "1.2.0", sha256: &sha, blake3: &b3 },
    );
    assert_eq!(denied.unwrap_err(), RunDenied::NoApproval);
}

#[tokio::test]
async fn a_member_requests_a_checkout_through_the_steward() {
    let r = rig(true, Duration::ZERO);
    let b = add_member(&r.net, "b", |_, _| None);
    let g = b.mesh.request_checkout("a", wire("aarch64"), Duration::from_secs(5)).await.expect("granted");
    assert!(g.payload.contains("fall-detect"));
    assert_eq!(r.stub.checkouts.load(Ordering::SeqCst), 1);
    assert!(b.fx.store.valid_grant_covering(&b3_of("aarch64"), "fall-detect", "1.2.0").is_some());
    fetch_from(&b, &["a"]).await;
    assert_eq!(r.stub.byte_transfers.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_refusal_reaches_the_requester_with_its_code() {
    let r = rig(true, Duration::ZERO);
    let b = add_member(&r.net, "b", |_, _| None);
    let mut w = wire("aarch64");
    w.cog_id = "other-cog".into();
    let e = b.mesh.request_checkout("a", w, Duration::from_secs(5)).await.unwrap_err();
    assert_eq!(e, CheckoutRefusal::Licence("cog_unlicensed".into()));
    assert_eq!(r.stub.byte_transfers.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn unadmitted_legacy_and_leaf_peers_are_refused_and_get_no_answer() {
    let r = rig(true, Duration::ZERO);
    let (u, l, f) = (
        add_member(&r.net, "u", |_, _| None),
        add_member(&r.net, "l", |_, _| None),
        add_member(&r.net, "f", |_, _| None),
    );
    {
        let mut s = r.net.stamps.lock().unwrap();
        s.insert("u".into(), Stamp::Unadmitted);
        s.insert("l".into(), Stamp::Leaf);
        s.insert("f".into(), Stamp::Unadmitted);
    }
    for m in [&u, &l, &f] {
        let e = m.mesh.request_checkout("a", wire("aarch64"), Duration::from_millis(150)).await;
        assert!(e.is_err(), "no grant for an unadmitted sender");
    }
    assert_eq!(r.stub.checkouts.load(Ordering::SeqCst), 0, "nothing reached the licence");
    assert_eq!(r.steward.mesh.counters.refused_unverified.load(Ordering::SeqCst), 3);
    assert!(
        !r.net.log.lock().unwrap().iter().any(|(f, _, t)| f == "a" && t == TOPIC_CHECKOUT_REPLY),
        "the steward does not even answer them"
    );
    // Calling the relay directly with such a context is refused too.
    let relay_ctx = [PeerCtx::unauthenticated("u"), {
        let mut c = admitted("l");
        c.class = crate::mesh_admit::PeerClass::Leaf;
        c
    }];
    let relay = {
        // Reach the relay through a fresh one wired like the steward's.
        let clock = Arc::new(std::sync::atomic::AtomicU64::new(T0));
        CheckoutRelay::new(
            r.steward.fx.store.clone(),
            r.steward.ex.clone(),
            steward_client(&r.stub, &clock),
            TestGate::new(true),
            Arc::new(NoFlood),
            None,
        )
    };
    for c in &relay_ctx {
        assert_eq!(relay.handle(CheckoutCaller::Peer(c), &wire("aarch64")).await.unwrap_err(), CheckoutRefusal::NotAdmitted);
    }
}

#[tokio::test]
async fn an_unadmitted_peer_cannot_open_an_artifact_session() {
    let r = rig(true, Duration::ZERO);
    r.steward.mesh.checkout_local(&wire("aarch64")).await.unwrap();
    let u = add_member(&r.net, "u", |_, _| None);
    r.net.stamps.lock().unwrap().insert("u".into(), Stamp::Unadmitted);
    // Install the grant on U so only admission can stop it.
    let g = r.steward.fx.store.verified_grants().remove(0);
    assert_eq!(g.grant().cog_id, "fall-detect");
    let cands = vec![PeerCandidate::new("a")];
    let out = u
        .ex
        .swarm_fetch(u.mesh.tunnel().dialer(), cands, content("aarch64"), &SwarmFetchOptions::default())
        .await;
    assert!(out.is_err(), "no session is served to an unadmitted sender");
    assert_eq!(r.steward.mesh.tunnel().counters.served_sessions.load(Ordering::SeqCst), 0);
    assert!(r.steward.mesh.counters.refused_unverified.load(Ordering::SeqCst) >= 1);
}

#[tokio::test]
async fn the_gate_decides_and_a_denial_never_reaches_the_licence() {
    let r = rig(false, Duration::ZERO);
    let e = r.steward.mesh.checkout_local(&wire("aarch64")).await.unwrap_err();
    assert!(matches!(e, CheckoutRefusal::GateDenied(_)), "{e:?}");
    assert_eq!(e.code(), "gate_denied");
    assert_eq!(r.stub.checkouts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn duplicate_requests_are_merged_into_one_licence_call() {
    let r = rig(true, Duration::from_millis(60));
    let mut set = tokio::task::JoinSet::new();
    for i in 0..6 {
        let m = r.steward.mesh.clone();
        set.spawn(async move {
            let mut w = wire("aarch64");
            w.request_id = format!("dup-{i}");
            m.checkout_local(&w).await
        });
    }
    while let Some(res) = set.join_next().await {
        assert!(res.unwrap().is_ok());
    }
    assert_eq!(r.stub.checkouts.load(Ordering::SeqCst), 1, "six identical requests, one checkout");
    assert_eq!(r.stub.byte_transfers.load(Ordering::SeqCst), 1);
    assert_eq!(r.flood.floods.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn licence_refusals_keep_their_codes() {
    let r = rig(true, Duration::ZERO);
    let mut w = wire("aarch64");
    w.cog_id = "nope".into();
    assert_eq!(r.steward.mesh.checkout_local(&w).await.unwrap_err().code(), "cog_unlicensed");
    let mut w = wire("aarch64");
    w.arch = "bad arch!".into();
    assert_eq!(r.steward.mesh.checkout_local(&w).await.unwrap_err().code(), "bad_request");
}

#[tokio::test]
async fn a_grant_from_the_wrong_signer_or_with_wrong_bytes_is_refused_and_registers_nothing() {
    let r = rig(true, Duration::ZERO);
    r.stub.set_mode(Mode::WrongSigner);
    let e = r.steward.mesh.checkout_local(&wire("aarch64")).await.unwrap_err();
    assert_eq!(e.code(), "bad_grant");
    assert_eq!(r.stub.byte_transfers.load(Ordering::SeqCst), 0, "no bytes are fetched for a bad grant");

    r.stub.set_mode(Mode::WrongBytes);
    let e = r.steward.mesh.checkout_local(&wire("aarch64")).await.unwrap_err();
    assert_eq!(e.code(), "artifact_mismatch");
    assert!(r.steward.fx.store.verified_grants().is_empty(), "no grant is registered for bytes that do not match");
    assert!(r.steward.ex.resolve(&content("aarch64")).is_none());
    assert_eq!(r.flood.floods.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_seed_clock_ahead_of_ours_is_reported_as_skew() {
    let r = rig(true, Duration::ZERO);
    r.stub.set_mode(Mode::SeedAhead);
    let e = r.steward.mesh.checkout_local(&wire("aarch64")).await.unwrap_err();
    assert_eq!(e, CheckoutRefusal::SeedClockSkew);
    assert_eq!(e.code(), "seed_clock_skew");
    assert_eq!(r.flood.floods.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn the_stub_refuses_unsigned_and_forged_requests_and_the_stewards_budget_is_isolated() {
    let r = rig(true, Duration::ZERO);
    let unsigned = LicenceRequest {
        method: "GET".into(),
        path: format!("{GRANTS_PATH}?since=0"),
        auth: None,
        body: vec![],
    };
    assert_eq!(r.stub.handle(unsigned.clone()).unwrap().status, 401, "an unsigned GET /grants is refused");
    // Identity needs no signature.
    let ident = LicenceRequest { method: "GET".into(), path: "/licence/v1/identity".into(), auth: None, body: vec![] };
    assert_eq!(r.stub.handle(ident).unwrap().status, 200);

    let budget = r.stub.steward_budget_left();
    let forged = sign_request(&sk(99), STEWARD_NODE, "POST", CHECKOUT_PATH, b"{}".to_vec(), T0, [3; 16]);
    // The unsigned GET above already used one slot of the pool.
    for _ in 0..UNSIGNED_POOL - 1 {
        assert_eq!(r.stub.handle(forged.clone()).unwrap().status, 401);
    }
    assert_eq!(r.stub.handle(forged.clone()).unwrap().status, 429, "the unsigned pool is bounded");
    assert_eq!(r.stub.steward_budget_left(), budget, "forged traffic did not charge the steward");
    assert_eq!(r.stub.unsigned_left(), 0);

    // The steward still gets through.
    r.steward.mesh.checkout_local(&wire("aarch64")).await.expect("steward unaffected");
    assert_eq!(r.stub.steward_budget_left(), budget - 2, "checkout + artifact were charged to the steward");
}

#[tokio::test]
async fn an_artifact_frame_is_chunked_and_reassembled() {
    // A piece larger than one tunnel chunk crosses the tunnel intact.
    let net = Arc::new(Net::default());
    let a = add_member(&net, "a", |_, _| None);
    let b = add_member(&net, "b", |_, _| None);
    let big: Vec<u8> = (0..(crate::mesh_artifact_tunnel::CHUNK * 3 + 17)).map(|i| (i % 251) as u8).collect();
    let d = a.ex.seed_bytes(&big).unwrap();
    // Grant it as a Cognitum checkout on both nodes so the policy serves it.
    let mut rec = grant_rec(1, T0, 3600, &["aarch64"]);
    rec.artifacts[0].size = big.len() as u64;
    rec.artifacts[0].blake3 = crate::workload_pkg::codec::hex_encode(&d.content_hash);
    rec.artifacts[0].sha256 = sha256_hex(&big);
    let signed = sign_grant(&rec, &grant_key()).unwrap();
    for m in [&a, &b] {
        install_grant(&m.fx.store, &m.ex, &signed).unwrap();
    }
    let key = ArtifactKey::Content(d.content_hash);
    b.ex.swarm_fetch(b.mesh.tunnel().dialer(), vec![PeerCandidate::new("a")], key, &SwarmFetchOptions::default())
        .await
        .unwrap();
    assert_eq!(b.ex.read_all(&d.id()).unwrap(), big);
}

#[tokio::test]
async fn forged_tunnel_and_reply_frames_from_a_stranger_are_ignored() {
    let r = rig(true, Duration::ZERO);
    let n_before = r.steward.mesh.tunnel().serving_sessions();
    let junk = KernelMessage::new(
        0,
        MessageTarget::Topic(TOPIC_CHECKOUT_REPLY.into()),
        MessagePayload::Json(serde_json::json!({"request_id": "nope", "code": "x"})),
    );
    assert!(r.steward.mesh.on_delivery(&admitted("zz"), junk).await);
    assert_eq!(r.steward.mesh.counters.stray_replies.load(Ordering::SeqCst), 1);
    for payload in [MessagePayload::Text("x".into()), MessagePayload::Json(serde_json::json!({"v": 9}))] {
        let m = KernelMessage::new(0, MessageTarget::Topic("mesh.artifact.tunnel".into()), payload);
        r.steward.mesh.on_delivery(&admitted("zz"), m).await;
    }
    assert_eq!(r.steward.mesh.tunnel().serving_sessions(), n_before);
}

/// What the other end of a `workload.ctl` stream saw when it asked for the
/// bytes while the controller was waiting on a call.
async fn ctl_meta_reply(steward: &Arc<Member>, verified: bool) -> crate::mesh_artifact_wire::ArtifactMsg {
    use crate::mesh::MeshStream;
    use crate::mesh_artifact_wire::ArtifactMsg;
    use crate::mesh_test_support::connected_pair;
    use crate::workload_ctl::msg::{CtlRequest, method};
    use crate::workload_ctl::session::CtlConnection;

    let (client, mut server) = connected_pair().await.unwrap();
    let probe = tokio::spawn(async move {
        let _request = server.recv().await.unwrap();
        let ask = ArtifactMsg::MetaRequest { key: content("aarch64") };
        server.send(&ask.to_wire().unwrap()).await.unwrap();
        let reply = server.recv().await.unwrap();
        ArtifactMsg::from_wire(&reply).unwrap()
    });
    let key = ed25519_dalek::SigningKey::from_bytes(&[90; 32]);
    let req = CtlRequest::new(&key, method::DESCRIBE, "peer-x", 1_000, 60_000, None, serde_json::json!({}));
    let signed = req.sign(&key).unwrap();
    let mut conn = CtlConnection::new(Box::new(client), "ctl").with_peer_verified(verified);
    // The call itself ends when the probe goes away; only what was served matters.
    let _ = conn
        .call("peer-x", method::DESCRIBE, &signed, Some(&steward.ex), Duration::from_millis(500))
        .await;
    probe.await.unwrap()
}

#[tokio::test]
async fn a_ctl_connection_serves_checkout_bytes_only_to_a_peer_it_knows_is_verified() {
    use crate::mesh_artifact_wire::ArtifactMsg;
    let r = rig(true, Duration::ZERO);
    r.steward.mesh.checkout_local(&wire("aarch64")).await.unwrap();
    // Unverified is the default: the checkout policy refuses to serve it.
    assert!(matches!(ctl_meta_reply(&r.steward, false).await, ArtifactMsg::Reject { .. }));
    assert!(matches!(ctl_meta_reply(&r.steward, true).await, ArtifactMsg::Meta { .. }));
}
