use std::sync::Arc;
use std::time::Duration;

use clawft_mesh_local::client::{connect_with_retry, Backoff, ClientConfig, ClientError, MeshLocalClient, RegisterParams};
use clawft_mesh_local::proto::{BindState, ErrorKind, Message, ProjectBinding, ServiceRecord};
use clawft_mesh_local::testing::{TestServer, TestServerConfig};
use clawft_mesh_local::{InjectedPeer, WeftAddr};
use ed25519_dalek::SigningKey;
use serde_json::json;

const SERVICE_UID: u32 = 4242;

struct Env {
    _dir: tempfile::TempDir,
    server: TestServer,
    cfg: ClientConfig,
}

fn key(n: u8) -> SigningKey {
    SigningKey::from_bytes(&[n; 32])
}

fn env_with(uid: u32, tweak: impl FnOnce(&mut TestServerConfig)) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let mut sc = TestServerConfig::new(key(100), Arc::new(InjectedPeer::uid(uid)));
    tweak(&mut sc);
    let server = TestServer::start(&dir.path().join("mesh.sock"), sc).unwrap();
    let mut cfg = ClientConfig::new(server.path(), server.service_record(SERVICE_UID));
    cfg.server_peer = Some(Arc::new(InjectedPeer::uid(SERVICE_UID)));
    cfg.deadline = Duration::from_secs(5);
    cfg.machine_pin = Some(dir.path().join("mesh/machine.pub"));
    cfg.build_sha = "client-sha".into();
    Env { _dir: dir, server, cfg }
}

fn env(uid: u32) -> Env {
    env_with(uid, |_| {})
}

fn params() -> RegisterParams {
    RegisterParams {
        projects: vec![ProjectBinding { project_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".into(), project_pubkey: [3; 32], cert_sig: [4; 64] }],
        topic_prefixes: vec!["kernel.".into()],
        capabilities: vec!["verdict".into()],
        version: "0.8.1".into(),
    }
}

#[tokio::test]
async fn connect_negotiate_register_and_get_a_verified_cert() {
    let e = env(501);
    let c = MeshLocalClient::connect_and_register(&e.cfg, &key(1), &params()).await.unwrap();
    assert_eq!(c.proto(), 1);
    assert_eq!(c.hello_ack().uid, 501, "uid comes from the injected peer credential");
    assert_eq!(c.register_ack().bind, BindState::New);
    assert_eq!(c.register_ack().accepted.addresses, vec!["01ARZ3NDEKTSV4RRFFQ69G5FAV"]);
    assert_eq!(c.register_ack().accepted.topic_prefixes, vec!["kernel."]);
    c.cert().verify(&e.server.machine_pubkey(), clawft_mesh_local::client::now_unix()).unwrap();
    assert_eq!(c.cert().user_pubkey, key(1).verifying_key().to_bytes());
    let regs = e.server.registrations();
    assert_eq!(regs.len(), 1);
    assert_eq!(regs[0].1.build_sha, "client-sha");
    c.close().await;
}

#[tokio::test]
async fn reregister_same_key_is_existing_and_other_key_conflicts() {
    let e = env(501);
    let c1 = MeshLocalClient::connect_and_register(&e.cfg, &key(1), &params()).await.unwrap();
    drop(c1);
    let c2 = MeshLocalClient::connect_and_register(&e.cfg, &key(1), &params()).await.unwrap();
    assert_eq!(c2.register_ack().bind, BindState::Existing);
    let err = MeshLocalClient::connect_and_register(&e.cfg, &key(2), &params()).await.err().unwrap();
    match err {
        ClientError::Server(b) => {
            assert_eq!(b.kind, ErrorKind::BindConflict);
            assert!(!b.remedy.is_empty());
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn different_uids_are_distinct_principals() {
    let a = env(501);
    let b = env(502);
    let ca = MeshLocalClient::connect_and_register(&a.cfg, &key(1), &params()).await.unwrap();
    let cb = MeshLocalClient::connect_and_register(&b.cfg, &key(1), &params()).await.unwrap();
    assert_eq!(ca.hello_ack().uid, 501);
    assert_eq!(cb.hello_ack().uid, 502);
    assert_ne!(ca.hello_ack().challenge, cb.hello_ack().challenge);
}

#[tokio::test]
async fn request_reply_correlation_under_concurrency() {
    let e = env(501);
    let c = Arc::new(MeshLocalClient::connect_and_register(&e.cfg, &key(1), &params()).await.unwrap());
    let mut tasks = Vec::new();
    for i in 0..32 {
        let c = c.clone();
        tasks.push(tokio::spawn(async move {
            let dest: WeftAddr = "weft://local/_/_/t".parse().unwrap();
            let r = c.send(&dest, json!({"i": i})).await.unwrap();
            assert_eq!(r, Message::Ack {});
            let s = c.request(Message::Status {}).await.unwrap();
            assert!(matches!(s, Message::Reply { .. }));
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    assert_eq!(e.server.sent().len(), 32);
    // An unsupported verb yields a typed server error.
    match c.request(Message::PeersList {}).await {
        Err(ClientError::Server(b)) => assert_eq!(b.kind, ErrorKind::Unsupported),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn renew_returns_a_newer_serial() {
    let e = env(501);
    let mut c = MeshLocalClient::connect_and_register(&e.cfg, &key(1), &params()).await.unwrap();
    let first = c.cert().serial;
    let renewed = c.renew().await.unwrap().clone();
    assert!(renewed.serial > first);
    assert_eq!(c.cert(), &renewed);
}

#[tokio::test]
async fn version_negotiation_against_a_live_server() {
    // Client newer than the service.
    let e = env_with(501, |s| s.proto = (1, 1));
    let mut cfg = e.cfg.clone();
    cfg.proto = (2, 3);
    match MeshLocalClient::connect_and_register(&cfg, &key(1), &params()).await.err().unwrap() {
        ClientError::Server(b) => {
            assert_eq!(b.kind, ErrorKind::ProtoMismatch);
            assert_eq!(b.remedy, "restart the service after `weaver update`");
        }
        other => panic!("{other:?}"),
    }
    // Client older than the service.
    let e = env_with(501, |s| s.proto = (3, 4));
    let mut cfg = e.cfg.clone();
    cfg.proto = (1, 2);
    match MeshLocalClient::connect_and_register(&cfg, &key(1), &params()).await.err().unwrap() {
        ClientError::Server(b) => assert_eq!(b.remedy, "update the user daemon"),
        other => panic!("{other:?}"),
    }
    // Overlap: the highest common version wins and features intersect.
    let e = env_with(501, |s| {
        s.proto = (1, 3);
        s.features = vec!["verdict".into(), "future".into()];
    });
    let mut cfg = e.cfg.clone();
    cfg.proto = (1, 2);
    cfg.features = vec!["verdict".into(), "other".into()];
    let c = MeshLocalClient::connect_and_register(&cfg, &key(1), &params()).await.unwrap();
    assert_eq!(c.proto(), 2);
    assert_eq!(c.features(), ["verdict".to_string()]);
}

#[tokio::test]
async fn client_refuses_a_server_with_the_wrong_uid() {
    let e = env(501);
    let mut cfg = e.cfg.clone();
    cfg.server_peer = Some(Arc::new(InjectedPeer::uid(1000)));
    match MeshLocalClient::connect_and_register(&cfg, &key(1), &params()).await.err().unwrap() {
        ClientError::ServerUid { expected, .. } => assert_eq!(expected, SERVICE_UID),
        other => panic!("{other:?}"),
    }
    assert!(e.server.registrations().is_empty(), "nothing may be sent to an impostor");
    // Root is acceptable.
    cfg.server_peer = Some(Arc::new(InjectedPeer::uid(0)));
    MeshLocalClient::connect_and_register(&cfg, &key(1), &params()).await.unwrap();
}

#[tokio::test]
async fn machine_key_is_pinned_on_first_contact() {
    let e = env(501);
    let c = MeshLocalClient::connect_and_register(&e.cfg, &key(1), &params()).await.unwrap();
    drop(c);
    let pin = e.cfg.machine_pin.clone().unwrap();
    assert_eq!(std::fs::read_to_string(&pin).unwrap(), clawft_mesh_local::hexser::encode(&e.server.machine_pubkey()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&pin).unwrap().permissions().mode() & 0o777, 0o600);
    }
    // Another machine key behind the same socket path is a hard error.
    std::fs::write(&pin, "ab".repeat(32)).unwrap();
    match MeshLocalClient::connect_and_register(&e.cfg, &key(1), &params()).await.err().unwrap() {
        ClientError::MachineKeyChanged { pinned, .. } => assert_eq!(pinned, "ab".repeat(32)),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn service_record_mismatch_is_a_machine_key_change() {
    let e = env(501);
    let mut cfg = e.cfg.clone();
    let other = key(55).verifying_key().to_bytes();
    cfg.service = ServiceRecord { machine_pubkey: other, node_id: clawft_mesh_local::node_id_from_pubkey(&other), ..cfg.service };
    let err = MeshLocalClient::connect_and_register(&cfg, &key(1), &params()).await.err().unwrap();
    assert!(matches!(err, ClientError::MachineKeyChanged { .. }), "{err:?}");
    assert!(!err.is_retryable());
}

#[tokio::test]
async fn unreachable_socket_is_retryable_and_backoff_is_bounded() {
    let e = env(501);
    let mut cfg = e.cfg.clone();
    cfg.socket_path = e._dir.path().join("absent.sock");
    let err = MeshLocalClient::connect_and_register(&cfg, &key(1), &params()).await.err().unwrap();
    assert!(err.is_retryable());
    let t = std::time::Instant::now();
    let b = Backoff::new(Duration::from_millis(5), Duration::from_millis(20));
    let err = connect_with_retry(&cfg, &key(1), &params(), b, 3).await.err().unwrap();
    assert!(matches!(err, ClientError::Io(_)));
    assert!(t.elapsed() < Duration::from_secs(2));

    let mut b = Backoff::new(Duration::from_millis(100), Duration::from_secs(1));
    for _ in 0..20 {
        let d = b.next_delay();
        assert!(d <= Duration::from_secs(1) && d >= Duration::from_millis(50));
    }
    b.reset();
    assert!(b.next_delay() <= Duration::from_millis(100));
}

#[tokio::test]
async fn retry_succeeds_once_the_server_appears() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("late.sock");
    let sc = TestServerConfig::new(key(100), Arc::new(InjectedPeer::uid(501)));
    let rec = {
        let tmp = TestServer::start(&dir.path().join("probe.sock"), sc.clone()).unwrap();
        tmp.service_record(SERVICE_UID)
    };
    let mut cfg = ClientConfig::new(&sock, rec);
    cfg.server_peer = Some(Arc::new(InjectedPeer::uid(SERVICE_UID)));
    let sock2 = sock.clone();
    let starter = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(120)).await;
        TestServer::start(&sock2, sc).unwrap()
    });
    let b = Backoff::new(Duration::from_millis(20), Duration::from_millis(60));
    let c = connect_with_retry(&cfg, &key(1), &params(), b, 40).await.unwrap();
    assert_eq!(c.hello_ack().uid, 501);
    let _server = starter.await.unwrap();
}

#[tokio::test]
async fn closed_connection_ends_events_and_fails_requests() {
    let e = env(501);
    let mut c = MeshLocalClient::connect_and_register(&e.cfg, &key(1), &params()).await.unwrap();
    drop(e.server);
    let ev = tokio::time::timeout(Duration::from_secs(5), c.next_event()).await;
    assert!(matches!(ev, Ok(None)), "event stream must end: {ev:?}");
    let r = c.request(Message::Ping {}).await;
    assert!(matches!(r, Err(ClientError::Closed | ClientError::Frame(_))), "{r:?}");
}
