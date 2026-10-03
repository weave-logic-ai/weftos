//! Mesh forwarding: a request through node A's proxy reaches a workload on
//! node B over (in-memory, authenticated-by-construction) mesh streams.
//! Every upstream is a fake on a random loopback port.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use async_trait::async_trait;

use super::listener::{InferProxy, OccupiedPolicy, Started};
use super::mesh_forward::{InferPeer, Served, forward_remote, serve_infer};
use super::support::*;
use super::table::PlacementTable;
use super::types::*;
use super::upstream::Upstream;
use super::wire::{self, Resp};
use crate::mesh::MeshStream;
use super::mesh_forward::{read_frame, write_frame};
use crate::mesh_framing::{FrameType, MeshFrame};
use crate::mesh_test_support::connected_pair;

struct Cluster {
    a: Arc<PlacementTable>,
    b: Arc<PlacementTable>,
    mesh: Arc<FakeMesh>,
    up_b: Fake,
    audit: Arc<Audit>,
}

async fn cluster(reply: Reply) -> Cluster {
    let audit = Arc::new(Audit::default());
    let mesh = {
        let mut m = FakeMesh::new("node-a");
        Arc::get_mut(&mut m).unwrap().audit = Some(audit.clone());
        m
    };
    let a = Arc::new(PlacementTable::new(
        "node-a",
        Some(mesh.clone() as Arc<dyn MeshDialer>),
        Some(audit.clone()),
    ));
    let b = Arc::new(PlacementTable::new("node-b", None, None));
    let up_b = fake(reply).await;
    b.register_local("hermes", &up_b.base(), Some("m".into()), "openai-v1", "LlamaCpp")
        .unwrap();
    mesh.add_node("node-b", b.clone(), true);
    Cluster { a, b, mesh, up_b, audit }
}

fn advertise(c: &Cluster) -> bool {
    c.b.expose_to_mesh("hermes", true);
    let ad = c.b.advertisement("hermes", 100).unwrap();
    c.a.ingest_advertisement(&ad)
}

async fn proxy_a(c: &Cluster) -> InferProxy {
    match InferProxy::start(
        "hermes",
        "127.0.0.1:0".parse().unwrap(),
        OccupiedPolicy::Refuse,
        c.a.clone(),
        small_limits(),
        Some(c.audit.clone()),
    )
    .await
    .unwrap()
    {
        Started::Running(p) => p,
        Started::Adopted(_) => panic!(),
    }
}

#[tokio::test]
async fn a_request_through_the_proxy_reaches_a_workload_on_another_node() {
    let c = cluster(Reply::ok(r#"{"from":"b"}"#)).await;
    assert!(advertise(&c));
    let p = proxy_a(&c).await;
    assert_eq!(c.a.resolve("hermes"), Some(Target::Remote { node_id: "node-b".into() }));
    let req = String::from_utf8(post(p.addr(), "/v1/chat/completions", r#"{"q":1}"#))
        .unwrap()
        .replace("Content-Type:", "Authorization: Bearer client-secret\r\nContent-Type:");
    let resp = raw(p.addr(), req.as_bytes()).await;
    assert_eq!(status(&resp), 200, "{resp}");
    assert_eq!(body(&resp), r#"{"from":"b"}"#);
    let seen = c.up_b.last();
    assert_eq!(seen.body, br#"{"q":1}"#);
    assert_eq!(seen.header("authorization"), None, "client credentials must not cross the mesh");
    assert_eq!(p.stats().remote.load(Ordering::Relaxed), 1);
    assert_eq!(c.mesh.dials.lock().unwrap().as_slice(), ["node-b"]);
    // In-process consumers go through the proxy for a remote role.
    assert_eq!(
        c.a.base_url_for_role("hermes"),
        Some(format!("http://127.0.0.1:{}/v1", p.addr().port()))
    );
}

#[tokio::test]
async fn large_and_streamed_responses_cross_the_mesh_intact() {
    let mut r = Reply::ok("");
    let big: Vec<u8> = (0..200_000u32).map(|i| b'a' + (i % 26) as u8).collect();
    r.chunks = vec![b"head-".to_vec(), big.clone(), b"-tail".to_vec()];
    let c = cluster(r).await;
    advertise(&c);
    let p = proxy_a(&c).await;
    let resp = raw(p.addr(), &get(p.addr(), "/v1/models")).await;
    let b = body(&resp).as_bytes().to_vec();
    let mut want = b"head-".to_vec();
    want.extend(&big);
    want.extend(b"-tail");
    assert_eq!(b.len(), want.len());
    assert_eq!(String::from_utf8_lossy(&b), String::from_utf8_lossy(&want));
}

#[tokio::test]
async fn local_instance_is_preferred_over_a_remote_one() {
    let c = cluster(Reply::ok("b")).await;
    advertise(&c);
    let local = fake(Reply::ok("a")).await;
    c.a.register_local("hermes", &local.base(), None, "openai-v1", "LlamaCpp").unwrap();
    assert!(matches!(c.a.resolve("hermes"), Some(Target::Local { .. })));
    c.a.deregister_local("hermes");
    assert!(matches!(c.a.resolve("hermes"), Some(Target::Remote { .. })));
}

#[tokio::test]
async fn adverts_from_unadmitted_peers_are_ignored_and_audited() {
    let c = cluster(Reply::ok("b")).await;
    c.b.expose_to_mesh("hermes", true);
    let mut ad = c.b.advertisement("hermes", 100).unwrap();
    ad.node_id = "node-evil".into();
    assert!(!c.a.ingest_advertisement(&ad));
    assert_eq!(c.a.resolve("hermes"), None);
    assert!(c.audit.kinds().contains(&"infer.advert.refused".to_string()));
    // Not an infer service, malformed role, or our own node: all ignored.
    let mut other = c.b.advertisement("hermes", 100).unwrap();
    other.name = "llm".into();
    assert!(!c.a.ingest_advertisement(&other));
    other.name = "infer.a/b".into();
    assert!(!c.a.ingest_advertisement(&other));
    let mut own = c.b.advertisement("hermes", 100).unwrap();
    own.node_id = "node-a".into();
    assert!(!c.a.ingest_advertisement(&own));
}

#[tokio::test]
async fn a_revoked_peer_stops_resolving_at_once() {
    let c = cluster(Reply::ok("b")).await;
    advertise(&c);
    assert!(c.a.resolve("hermes").is_some());
    c.mesh.admitted.lock().unwrap().clear();
    assert_eq!(c.a.resolve("hermes"), None, "admission is rechecked at resolve time");
    let p = proxy_a(&c).await;
    assert_eq!(status(&raw(p.addr(), &get(p.addr(), "/v1/models")).await), 503);
    assert_eq!(c.up_b.count(), 0);
    assert!(c.mesh.dials.lock().unwrap().is_empty(), "no dial to a revoked peer");
}

#[tokio::test]
async fn forwarding_refuses_a_peer_that_is_not_admitted() {
    let c = cluster(Reply::ok("b")).await;
    c.mesh.admitted.lock().unwrap().clear();
    struct Null;
    #[async_trait]
    impl ResponseSink for Null {
        async fn head(&mut self, _: u16, _: Option<&str>) -> Result<(), ProxyError> { Ok(()) }
        async fn chunk(&mut self, _: &[u8]) -> Result<(), ProxyError> { Ok(()) }
    }
    let req = ProxyRequest {
        role: "hermes".into(), method: Method::Get, path: "/v1/models".into(),
        content_type: None, accept: None, authorization: None, body: vec![],
    };
    let e = forward_remote(c.mesh.as_ref(), "node-b", &req, &mut Null, &small_limits()).await.unwrap_err();
    assert!(matches!(e, ProxyError::Refused(_)), "{e:?}");
    assert_eq!(c.up_b.count(), 0);
}

#[tokio::test]
async fn the_serving_side_refuses_an_unverified_peer() {
    let c = cluster(Reply::ok("b")).await;
    advertise(&c);
    *c.mesh.verified.lock().unwrap() = false;
    let p = proxy_a(&c).await;
    let resp = raw(p.addr(), &get(p.addr(), "/v1/models")).await;
    assert_eq!(status(&resp), 502, "{resp}");
    assert!(body(&resp).contains("not verified"), "{resp}");
    assert_eq!(c.up_b.count(), 0, "an unverified peer reached the model server");
    assert!(c.audit.kinds().contains(&"infer.mesh.refused".to_string()));
}

#[tokio::test]
async fn a_role_not_exposed_to_the_mesh_is_not_served() {
    let c = cluster(Reply::ok("b")).await;
    // Admitted and verified peer, but node-b never exposed the role.
    let ad = {
        c.b.expose_to_mesh("hermes", true);
        let ad = c.b.advertisement("hermes", 100).unwrap();
        c.b.expose_to_mesh("hermes", false);
        ad
    };
    assert!(c.b.advertisement("hermes", 101).is_none(), "unexposed roles are not advertised");
    c.a.ingest_advertisement(&ad);
    let p = proxy_a(&c).await;
    let resp = raw(p.addr(), &get(p.addr(), "/v1/models")).await;
    assert_eq!(status(&resp), 502, "{resp}");
    assert_eq!(c.up_b.count(), 0);
}

#[tokio::test]
async fn a_peer_cannot_make_the_server_forward_onward() {
    // Node B knows a remote instance on C but has no local one: a request
    // from A must not be bounced to C.
    let audit = Arc::new(Audit::default());
    let mesh_b = FakeMesh::new("node-b");
    let b = Arc::new(PlacementTable::new("node-b", Some(mesh_b.clone() as Arc<dyn MeshDialer>), None));
    let c_table = Arc::new(PlacementTable::new("node-c", None, None));
    let up_c = fake(Reply::ok("c")).await;
    c_table.register_local("hermes", &up_c.base(), None, "openai-v1", "LlamaCpp").unwrap();
    c_table.expose_to_mesh("hermes", true);
    mesh_b.add_node("node-c", c_table.clone(), true);
    b.ingest_advertisement(&c_table.advertisement("hermes", 5).unwrap());
    b.expose_to_mesh("hermes", true);
    assert!(matches!(b.resolve("hermes"), Some(Target::Remote { .. })));

    let (mut client, mut server) = connected_pair().await.unwrap();
    let up = Upstream::new(small_limits()).unwrap();
    let bt = b.clone();
    let f = move |r: &str| bt.local_for_mesh(r);
    let peer = InferPeer { node_id: "node-a".into(), verified: true };
    let h = tokio::spawn(async move { serve_infer(&mut server, &peer, &f, &up, Some(audit.as_ref())).await });
    let req = ProxyRequest {
        role: "hermes".into(), method: Method::Get, path: "/v1/models".into(),
        content_type: None, accept: None, authorization: None, body: vec![],
    };
    write_frame(&mut client, &MeshFrame { frame_type: FrameType::InferRequest, payload: wire::encode_request(&req).unwrap() }).await.unwrap();
    let f = read_frame(&mut client).await.unwrap();
    assert!(matches!(wire::decode_resp(&f.payload).unwrap(), Resp::Error(_)));
    assert!(matches!(h.await.unwrap().unwrap(), Served::Failed(_)));
    assert_eq!(up_c.count(), 0);
    assert!(mesh_b.dials.lock().unwrap().is_empty());
}

#[tokio::test]
async fn the_server_rejects_the_wrong_frame_and_malformed_requests() {
    let up = Upstream::new(small_limits()).unwrap();
    let f = |_: &str| None::<String>;
    let peer = InferPeer { node_id: "n".into(), verified: true };
    for frame in [
        MeshFrame { frame_type: FrameType::Heartbeat, payload: vec![1] },
        MeshFrame { frame_type: FrameType::InferRequest, payload: b"garbage".to_vec() },
        MeshFrame { frame_type: FrameType::InferResponse, payload: vec![2] },
    ] {
        let (mut client, mut server) = connected_pair().await.unwrap();
        write_frame(&mut client, &frame).await.unwrap();
        let r = serve_infer(&mut server, &peer, &f, &up, None).await.unwrap();
        assert!(matches!(r, Served::Failed(_)), "{:?}", frame.frame_type);
    }
}

/// A peer that answers with a scripted list of frames.
struct Script(Vec<Vec<u8>>, bool);

#[async_trait]
impl MeshDialer for Script {
    fn is_admitted(&self, _: &str) -> bool { true }
    async fn dial(&self, _: &str) -> Result<Box<dyn MeshStream>, ProxyError> {
        let (client, mut server) = connected_pair().await.unwrap();
        let (frames, stall) = (self.0.clone(), self.1);
        tokio::spawn(async move {
            let _ = read_frame(&mut server).await;
            for f in frames {
                let _ = write_frame(&mut server, &MeshFrame { frame_type: FrameType::InferResponse, payload: f }).await;
            }
            if stall {
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        });
        Ok(Box::new(client))
    }
}

struct Collect(Vec<u8>, Option<u16>);

#[async_trait]
impl ResponseSink for Collect {
    async fn head(&mut self, s: u16, _: Option<&str>) -> Result<(), ProxyError> { self.1 = Some(s); Ok(()) }
    async fn chunk(&mut self, d: &[u8]) -> Result<(), ProxyError> { self.0.extend_from_slice(d); Ok(()) }
}

async fn drive(frames: Vec<Vec<u8>>, stall: bool, limits: ProxyLimits) -> (Result<(), ProxyError>, Collect) {
    let req = ProxyRequest {
        role: "hermes".into(), method: Method::Get, path: "/v1/models".into(),
        content_type: None, accept: None, authorization: None, body: vec![],
    };
    let mut sink = Collect(vec![], None);
    let r = forward_remote(&Script(frames, stall), "peer", &req, &mut sink, &limits).await;
    (r, sink)
}

fn head() -> Vec<u8> { wire::encode_resp(&Resp::Head { status: 200, content_type: None }) }
fn chunk(n: usize) -> Vec<u8> { wire::encode_resp(&Resp::Chunk(vec![b'x'; n])) }
fn end() -> Vec<u8> { wire::encode_resp(&Resp::End) }

#[tokio::test]
async fn a_hostile_peer_cannot_break_the_consumer() {
    let l = small_limits();
    // Well-formed.
    let (r, s) = drive(vec![head(), chunk(3), end()], false, l.clone()).await;
    assert!(r.is_ok() && s.0 == b"xxx" && s.1 == Some(200));
    // Body before head.
    let (r, _) = drive(vec![chunk(3), end()], false, l.clone()).await;
    assert!(matches!(r, Err(ProxyError::Mesh(_))), "{r:?}");
    // End before head.
    let (r, _) = drive(vec![end()], false, l.clone()).await;
    assert!(matches!(r, Err(ProxyError::Mesh(_))));
    // Two heads.
    let (r, _) = drive(vec![head(), head(), end()], false, l.clone()).await;
    assert!(matches!(r, Err(ProxyError::Mesh(_))));
    // Garbage frame.
    let (r, _) = drive(vec![vec![0xEE, 1, 2]], false, l.clone()).await;
    assert!(matches!(r, Err(ProxyError::Mesh(_))));
    // Remote error is reported, not swallowed.
    let (r, _) = drive(vec![wire::encode_resp(&Resp::Error("boom".into()))], false, l.clone()).await;
    assert!(matches!(r, Err(ProxyError::Upstream(m)) if m.contains("boom")));
    // Endless body is capped.
    let small = ProxyLimits { max_response_body: 1000, ..l.clone() };
    let mut frames = vec![head()];
    frames.extend((0..50).map(|_| chunk(100)));
    let (r, s) = drive(frames, false, small).await;
    assert!(matches!(r, Err(ProxyError::TooLarge(_))), "{r:?}");
    assert!(s.0.len() <= 1000);
    // A peer that goes silent mid-response times out.
    let (r, _) = drive(vec![head(), chunk(1)], true, l).await;
    assert!(matches!(r, Err(ProxyError::Timeout(_))), "{r:?}");
}

#[tokio::test]
async fn advertisement_shape_and_generation() {
    let c = cluster(Reply::ok("b")).await;
    assert!(c.b.advertisement("hermes", 1).is_none(), "not exposed, not advertised");
    c.b.expose_to_mesh("hermes", true);
    let ad = c.b.advertisement("hermes", 42).unwrap();
    assert_eq!(ad.name, "infer.hermes");
    assert_eq!(ad.node_id, "node-b");
    assert_eq!(ad.metadata["role"], "hermes");
    assert_eq!(ad.metadata["api"], "openai-v1");
    assert_eq!(ad.metadata["model"], "m");
    assert_eq!(ad.metadata["port"], c.up_b.addr.port().to_string());
    assert!(ad.methods.iter().any(|m| m == "chat.completions"));
    assert_eq!(c.b.advertisements(42).len(), 1);

    let rx = c.a.subscribe();
    let g = c.a.generation();
    assert!(c.a.ingest_advertisement(&ad));
    assert!(c.a.generation() > g);
    assert!(rx.has_changed().unwrap());
    // A stale advert changes nothing.
    let g = c.a.generation();
    assert!(!c.a.ingest_advertisement(&ad));
    assert_eq!(c.a.generation(), g);
    c.a.remove_node("node-b");
    assert!(c.a.generation() > g);
    assert_eq!(c.a.resolve("hermes"), None);
}
