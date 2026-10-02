//! Start-up safety, persistence, state-directory contents, facts, health and
//! the "must not own" checks.

mod common;

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use clawft_kernel::node_facts_advert::{verify_node_facts, SignedNodeFacts};
use clawft_mesh_local::client::RegisterParams;
use clawft_mesh_local::proto::{BindState, ErrorKind, Message};
use clawft_mesh_service::{start_with, JournalError, StartError};
use common::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn now() -> u64 {
    clawft_mesh_local::client::now_unix()
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> =
        std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

#[tokio::test]
async fn a_restart_keeps_bindings_and_clients_register_again() {
    let mut h = Harness::start().await;
    let c = h.connect(None, 1, RegisterParams::default()).await.unwrap();
    assert_eq!(c.register_ack().bind, BindState::New);
    let serial = c.cert().serial;
    c.close().await;
    let node = h.svc().node_id.clone();
    h.restart().await;
    assert_eq!(h.svc().node_id, node, "the box key persists");
    let c = h.connect_retry(None, 1, RegisterParams::default()).await.unwrap();
    assert_eq!(c.register_ack().bind, BindState::Existing, "bindings are a fold of the journal");
    assert!(c.cert().serial > serial, "serials continue across restarts");
    // The same uid cannot bind a different key after the restart either.
    assert_eq!(server_kind(h.connect(None, 2, RegisterParams::default()).await), ErrorKind::BindConflict);
}

#[tokio::test]
async fn a_torn_journal_tail_after_a_crash_is_quarantined_and_gates_binds_until_accepted() {
    let mut h = Harness::start().await;
    h.connect(None, 1, RegisterParams::default()).await.unwrap().close().await;
    h.stop().await;
    // kill -9 mid-append: a partial record with no newline at the end.
    let mut f = std::fs::OpenOptions::new().append(true).open(h.state_dir().join("journal.jsonl")).unwrap();
    f.write_all(br#"{"v":1,"seq":999,"ts":1,"prev":"00"#).unwrap();
    drop(f);
    h.begin().await.expect("the service still starts");

    // Existing registrations cannot get a fresh certificate while read-only.
    assert_eq!(server_kind(h.connect(None, 1, RegisterParams::default()).await), ErrorKind::Forbidden);
    let v = h.admin_ok(Message::JournalVerify {}).await;
    assert_eq!(v["ok"], true, "the surviving chain verifies");
    assert_eq!(v["read_only"], true);
    assert!(!v["pending_quarantines"].as_array().unwrap().is_empty());

    h.admin_ok(Message::JournalAcceptTruncate { quarantine_seq: None, floor: None }).await;
    let c = h.connect_retry(None, 1, RegisterParams::default()).await.unwrap();
    assert_eq!(c.register_ack().bind, BindState::Existing, "the earlier binding survived");
    let v = h.admin_ok(Message::JournalVerify {}).await;
    assert_eq!(v["read_only"], false);
}

#[tokio::test]
async fn a_second_service_on_one_state_dir_is_refused_naming_the_holder() {
    let h = Harness::start().await;
    let mut cfg = h.cfg.clone();
    cfg.socket = h.dir.path().join("r2").join("s");
    match start_with(cfg, peer_source(h.next_uid.clone()), Default::default()).await {
        Err(StartError::Journal(JournalError::Locked { holder_pid })) => {
            assert_eq!(holder_pid, Some(std::process::id()));
        }
        Err(other) => panic!("expected a lock error, got {other}"),
        Ok(_) => panic!("a second service must not start on the same state dir"),
    }
}

#[tokio::test]
async fn a_live_socket_is_refused_and_a_stale_one_is_replaced() {
    let h = Harness::start().await;
    let other = tempfile::Builder::new().prefix("m").tempdir().unwrap();
    let mut cfg = config_in(other.path(), h.euid);
    cfg.socket = h.cfg.socket.clone();
    match start_with(cfg, peer_source(h.next_uid.clone()), Default::default()).await {
        Err(StartError::Socket { reason, .. }) => assert!(reason.contains("already listening"), "{reason}"),
        Err(other) => panic!("expected a socket error, got {other}"),
        Ok(_) => panic!("must not take over a live service's socket"),
    }

    // A stale socket file (nobody listening) is ours to replace.
    let dir = tempfile::Builder::new().prefix("m").tempdir().unwrap();
    let cfg = config_in(dir.path(), h.euid);
    std::fs::create_dir_all(cfg.socket.parent().unwrap()).unwrap();
    drop(std::os::unix::net::UnixListener::bind(&cfg.socket).unwrap());
    let svc = start_with(cfg, peer_source(h.next_uid.clone()), Default::default()).await.expect("stale socket replaced");
    svc.shutdown().await;
}

#[tokio::test]
async fn unsafe_directories_stop_the_service() {
    let euid = clawft_mesh_local::peer::own_uid().await.unwrap();
    let next = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(REAL));

    let dir = tempfile::Builder::new().prefix("m").tempdir().unwrap();
    let cfg = config_in(dir.path(), euid);
    std::fs::create_dir_all(&cfg.state_dir).unwrap();
    std::fs::set_permissions(&cfg.state_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        start_with(cfg, peer_source(next.clone()), Default::default()).await,
        Err(StartError::State(_))
    ));

    let dir = tempfile::Builder::new().prefix("m").tempdir().unwrap();
    let cfg = config_in(dir.path(), euid);
    let sock_dir = cfg.socket.parent().unwrap().to_path_buf();
    std::fs::create_dir_all(&sock_dir).unwrap();
    std::fs::set_permissions(&sock_dir, std::fs::Permissions::from_mode(0o777)).unwrap();
    assert!(matches!(
        start_with(cfg, peer_source(next.clone()), Default::default()).await,
        Err(StartError::SocketDir(_))
    ));

    // Group-writable is refused as well.
    let dir = tempfile::Builder::new().prefix("m").tempdir().unwrap();
    let cfg = config_in(dir.path(), euid);
    let sock_dir = cfg.socket.parent().unwrap().to_path_buf();
    std::fs::create_dir_all(&sock_dir).unwrap();
    std::fs::set_permissions(&sock_dir, std::fs::Permissions::from_mode(0o775)).unwrap();
    assert!(matches!(
        start_with(cfg, peer_source(next.clone()), Default::default()).await,
        Err(StartError::SocketDir(_))
    ));

    // A symlinked state dir is refused too.
    let dir = tempfile::Builder::new().prefix("m").tempdir().unwrap();
    let cfg = config_in(dir.path(), euid);
    let real = dir.path().join("real");
    std::fs::create_dir_all(&real).unwrap();
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::os::unix::fs::symlink(&real, &cfg.state_dir).unwrap();
    assert!(matches!(start_with(cfg, peer_source(next), Default::default()).await, Err(StartError::State(_))));
}

#[tokio::test]
async fn the_state_dir_holds_only_the_planned_files_after_a_full_session() {
    let h = Harness::start().await;
    let a = h.connect(None, 1, RegisterParams::default()).await.unwrap();
    let b = h.connect(Some(9001), 2, RegisterParams::default()).await.unwrap();
    drop((a, b));
    h.admin_ok(Message::BindingsList {}).await;
    h.admin_ok(Message::PeerRevoke { node_id: "d".repeat(32), reason: "test".into() }).await;
    h.admin_ok(Message::BindRevoke { uid: 9001, reason: "x".into() }).await;
    h.admin_ok(Message::FactsGet {}).await;

    // Plan 1.1: node.key, journal.jsonl (+ segments), revoked.json, facts.json,
    // service.json, plus the single-writer lock. Nothing else: no chain, no
    // governance state, no tokens, no secrets of any user.
    let allowed = [
        "node.key", "journal.jsonl", "revoked.json", "facts.json", "service.json", "mesh.lock", "force-revoked.json",
    ];
    for n in names(&h.state_dir()) {
        let segment = n.starts_with("journal.") && n.ends_with(".jsonl");
        assert!(allowed.contains(&n.as_str()) || segment, "unexpected file in the state dir: {n}");
    }
    for must in ["node.key", "journal.jsonl", "facts.json", "service.json"] {
        assert!(names(&h.state_dir()).contains(&must.to_string()), "{must} missing");
    }
    for private in ["node.key", "journal.jsonl", "facts.json"] {
        let mode = std::fs::metadata(h.state_dir().join(private)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode & 0o077, 0, "{private} must not be readable by others (mode {mode:o})");
    }
    assert_eq!(std::fs::metadata(h.state_dir()).unwrap().permissions().mode() & 0o777, 0o700);
    // The record clients pin sits beside the socket and is world-readable.
    let sock_dir = h.socket().parent().unwrap().to_path_buf();
    assert!(names(&sock_dir).contains(&"service.json".to_string()));
}

#[tokio::test]
async fn facts_are_signed_by_the_box_key_and_bound_to_the_revocation_statement() {
    let h = Harness::start().await;
    let c = h.connect(None, 1, RegisterParams::default()).await.unwrap();
    drop(c);
    h.admin_ok(Message::BindRevoke { uid: h.euid, reason: "t".into() }).await;
    let doc = h.admin_ok(Message::FactsGet {}).await;
    let signed: SignedNodeFacts = serde_json::from_value(doc["facts"].clone()).unwrap();
    let facts = verify_node_facts(&signed, now()).expect("signed facts verify");
    assert_eq!(facts.node_id, h.svc().node_id);
    let note = facts.notes.iter().find(|n| n.probe == "mesh.revocations").expect("revocation note");
    assert!(note.note.starts_with("sha256:"));
    // The revocation statement carries its own signature by the same key.
    let rev = &doc["revocations"];
    assert_eq!(rev["public_key"], clawft_mesh_local::hexser::encode(&h.record().machine_pubkey));
    assert_eq!(rev["signature"].as_str().unwrap().len(), 128);
}

#[tokio::test]
async fn policy_set_validates_and_survives_a_restart_through_the_journal() {
    let mut h = Harness::start().await;
    // enforce needs a pinned genesis and noise: refused, nothing changes.
    assert_eq!(
        h.admin_err(Message::PolicySet { admission: Some("enforce".into()), cluster_owner_uid: None }).await,
        ErrorKind::BadRequest
    );
    assert_eq!(
        h.admin_err(Message::PolicySet { admission: Some("bogus".into()), cluster_owner_uid: None }).await,
        ErrorKind::BadRequest
    );
    h.admin_ok(Message::PolicySet { admission: Some("off".into()), cluster_owner_uid: Some(4321) }).await;
    let s = h.admin_ok(Message::Status {}).await;
    assert_eq!((s["admission"].as_str(), s["cluster_owner_uid"].as_u64()), (Some("off"), Some(4321)));
    h.restart().await;
    let s = h.admin_ok(Message::Status {}).await;
    assert_eq!((s["admission"].as_str(), s["cluster_owner_uid"].as_u64()), (Some("off"), Some(4321)));
}

#[tokio::test]
async fn health_answers_on_loopback_only_with_no_secrets() {
    let h = Harness::start().await;
    let addr = h.svc().health_addr.expect("health listener");
    assert!(addr.ip().is_loopback());
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(b"GET /health HTTP/1.0\r\n\r\n").await.unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    assert!(out.starts_with("HTTP/1.0 200"), "{out}");
    let body = out.split("\r\n\r\n").nth(1).unwrap();
    let v: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(v["node_id"], h.svc().node_id.as_str());
    assert!(!out.contains("pubkey") && !out.contains("uid"), "health must not expose keys or uids");
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(b"GET /admin HTTP/1.0\r\n\r\n").await.unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    assert!(out.starts_with("HTTP/1.0 404"));
}

#[tokio::test]
async fn a_peer_revoke_is_journalled_and_listed_in_the_revocation_file() {
    let h = Harness::start().await;
    h.admin_ok(Message::PeerRevoke { node_id: "d".repeat(32), reason: "stolen".into() }).await;
    let revoked = std::fs::read_to_string(h.state_dir().join("revoked.json")).unwrap();
    assert!(revoked.contains(&"d".repeat(32)));
    assert_eq!(h.admin_err(Message::PeerRevoke { node_id: "bad id!".into(), reason: String::new() }).await, ErrorKind::BadRequest);
    h.admin_ok(Message::PeerUnrevoke { node_id: "d".repeat(32) }).await;
    let revoked = std::fs::read_to_string(h.state_dir().join("revoked.json")).unwrap();
    assert!(!revoked.contains(&"d".repeat(32)));
}

/// "The service exposes no RPC method that evaluates governance": the
/// mesh-local handlers must not touch the gate, the chain or the kernel.
#[test]
fn no_local_server_handler_evaluates_governance() {
    let files = [
        ("local_server.rs", include_str!("../src/local_server.rs")),
        ("handlers.rs", include_str!("../src/handlers.rs")),
        ("admin.rs", include_str!("../src/admin.rs")),
        ("register.rs", include_str!("../src/register.rs")),
    ];
    let forbidden = [
        "GateBackend", "GateDecision", "clawft_kernel::gate", "ChainManager", "Kernel::", "TileZero",
        "\"cluster.join\"", "evaluate",
    ];
    for (name, src) in files {
        let code: String = src
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        // Ignore the test module at the end of local_server.rs.
        let code = code.split("#[cfg(test)]").next().unwrap();
        for word in forbidden {
            assert!(!code.contains(word), "{name} mentions `{word}`: handlers must not evaluate governance");
        }
    }
}

/// The only place a verdict is decided is the cluster owner's daemon; the
/// service side only forwards and caches.
#[test]
fn verdicts_module_only_forwards_and_caches() {
    let src = include_str!("../src/verdicts.rs");
    for word in ["GateBackend", "GateDecision", "ChainManager"] {
        assert!(!src.lines().filter(|l| !l.trim_start().starts_with("//")).any(|l| l.contains(word)), "{word}");
    }
}

#[tokio::test]
async fn enforce_needs_an_explicit_cluster_owner() {
    let h = Harness::with(|c, _| {
        c.genesis_hash = Some([1; 32]);
        c.noise = true;
    })
    .await;
    // Genesis and noise are in place, but nobody is the cluster owner.
    let enforce = || Message::PolicySet { admission: Some("enforce".into()), cluster_owner_uid: None };
    assert_eq!(h.admin_err(enforce()).await, ErrorKind::BadRequest);
    let s = h.admin_ok(Message::Status {}).await;
    assert_eq!(s["admission"], "observe", "refused means unchanged");
    // Setting the owner in the same request is explicit and allowed.
    h.admin_ok(Message::PolicySet { admission: Some("enforce".into()), cluster_owner_uid: Some(h.euid) }).await;
    assert_eq!(h.admin_ok(Message::Status {}).await["admission"], "enforce");
}

#[tokio::test]
async fn admin_verbs_journal_first_and_change_nothing_when_the_journal_refuses() {
    let mut h = Harness::start().await;
    h.connect(None, 1, RegisterParams::default()).await.unwrap().close().await;
    h.stop().await;
    let mut f = std::fs::OpenOptions::new().append(true).open(h.state_dir().join("journal.jsonl")).unwrap();
    f.write_all(br#"{"v":1,"seq":999,"ts":1,"prev":"00"#).unwrap();
    drop(f);
    h.begin().await.expect("starts read-only");

    // Trust-increasing verbs are refused on a read-only journal and apply nothing.
    let set = Message::PolicySet { admission: Some("off".into()), cluster_owner_uid: Some(77) };
    assert_eq!(h.admin_err(set).await, ErrorKind::Forbidden);
    let s = h.admin_ok(Message::Status {}).await;
    assert_eq!((s["admission"].as_str(), s["cluster_owner_uid"].is_null()), (Some("observe"), true));
    assert_eq!(h.admin_err(Message::PeerUnrevoke { node_id: "d".repeat(32) }).await, ErrorKind::Forbidden);
    // Revoking only reduces trust: allowed, and journalled before it is applied.
    h.admin_ok(Message::PeerRevoke { node_id: "d".repeat(32), reason: "x".into() }).await;
    assert!(std::fs::read_to_string(h.state_dir().join("revoked.json")).unwrap().contains(&"d".repeat(32)));
}

#[tokio::test]
async fn a_broken_journal_does_not_stop_a_peer_revocation_but_does_stop_loosening() {
    let h = Harness::start().await;
    h.svc().state.core.lock().unwrap().journal.inject_write_failure();
    // Tightening applies first and journals best-effort, with a warning.
    let data = h.admin_ok(Message::PeerRevoke { node_id: "e".repeat(32), reason: "x".into() }).await;
    assert!(data["warning"].as_str().unwrap().contains("journal record failed"), "{data}");
    assert!(h.svc().state.revocations.is_revoked(&"e".repeat(32)));
    assert!(std::fs::read_to_string(h.state_dir().join("revoked.json")).unwrap().contains(&"e".repeat(32)));
    // Loosening stays journal-first: the journal is poisoned, so nothing changes.
    assert_eq!(
        h.admin_err(Message::PeerUnrevoke { node_id: "e".repeat(32) }).await,
        ErrorKind::Forbidden
    );
    assert!(h.svc().state.revocations.is_revoked(&"e".repeat(32)));
}

#[tokio::test]
async fn a_binding_revocation_is_enforced_even_when_the_journal_cannot_record_it() {
    let h = Harness::start().await;
    let c = h.connect(None, 1, RegisterParams::default()).await.unwrap();
    h.svc().state.core.lock().unwrap().journal.inject_write_failure();
    let data = h.admin_ok(Message::BindRevoke { uid: h.euid, reason: "compromised".into() }).await;
    assert!(data["warning"].as_str().unwrap().contains("enforced in memory"), "{data}");
    // The live registration is cut off and the uid cannot come back.
    let mut gone = false;
    for _ in 0..40 {
        if c.request(Message::Ping {}).await.is_err() {
            gone = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(gone);
    assert_eq!(server_kind(h.connect(None, 1, RegisterParams::default()).await), ErrorKind::Forbidden);
}

#[tokio::test]
async fn policy_set_with_a_failing_journal_applies_nothing() {
    let h = Harness::start().await;
    h.svc().state.core.lock().unwrap().journal.inject_write_failure();
    let set = Message::PolicySet { admission: Some("off".into()), cluster_owner_uid: Some(77) };
    assert_eq!(h.admin_err(set).await, ErrorKind::Forbidden);
    let s = h.admin_ok(Message::Status {}).await;
    assert_eq!((s["admission"].as_str(), s["cluster_owner_uid"].is_null()), (Some("observe"), true));
}

#[tokio::test]
async fn a_journalled_enforce_with_an_incomplete_mesh_toml_stops_the_service() {
    let mut h = Harness::with(|c, _| {
        c.genesis_hash = Some([1; 32]);
        c.noise = true;
        c.cluster_owner_uid = Some(c.admin_uids[0]);
    })
    .await;
    h.admin_ok(Message::PolicySet { admission: Some("enforce".into()), cluster_owner_uid: None }).await;
    h.stop().await;
    // mesh.toml loses its genesis hash: the effective mode (enforce) cannot be built.
    let genesis = h.cfg.genesis_hash.take();
    assert!(matches!(h.begin().await, Err(StartError::Config(_))), "must refuse, not run as allow-all");
    // Same with noise removed, and with the owner removed.
    h.cfg.genesis_hash = genesis;
    h.cfg.noise = false;
    assert!(matches!(h.begin().await, Err(StartError::Config(_))));
    h.cfg.noise = true;
    h.cfg.cluster_owner_uid = None;
    assert!(matches!(h.begin().await, Err(StartError::Config(_))));
    h.cfg.cluster_owner_uid = Some(h.euid);
    h.begin().await.expect("a complete configuration starts");
}

#[tokio::test]
async fn a_force_revocation_survives_a_restart_and_is_listed_in_facts_and_status() {
    let mut h = Harness::start().await;
    let c = h.connect(None, 1, RegisterParams::default()).await.unwrap();
    let serial = c.cert().serial;
    drop(c);
    h.svc().state.core.lock().unwrap().journal.inject_write_failure();
    let data = h.admin_ok(Message::BindRevoke { uid: h.euid, reason: "compromised".into() }).await;
    assert!(data["warning"].as_str().unwrap().contains("force-revoked.json"), "{data}");
    let file = h.state_dir().join("force-revoked.json");
    assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o077, 0, "0600");

    // Status and bindings show it; the signed facts revoke its serials.
    let s = h.admin_ok(Message::Status {}).await;
    assert_eq!(s["force_revoked"].as_array().unwrap().len(), 1);
    let b = h.admin_ok(Message::BindingsList {}).await;
    assert_eq!(b["force_revoked"].as_array().unwrap().len(), 1);
    let facts = h.admin_ok(Message::FactsGet {}).await;
    let payload: serde_json::Value = serde_json::from_str(facts["revocations"]["payload"].as_str().unwrap()).unwrap();
    assert!(payload["serials"].as_array().unwrap().iter().any(|x| x == serial), "{payload}");
    assert!(payload["ranges"].as_array().unwrap().iter().any(|r| r["user_id"] == user_id(1)));

    // The journal never recorded it, but a restart still keeps the uid out.
    h.restart().await;
    assert_eq!(server_kind(h.connect_retry(None, 1, RegisterParams::default()).await), ErrorKind::Forbidden);
    assert_eq!(h.admin_ok(Message::Status {}).await["force_revoked"].as_array().unwrap().len(), 1);

    // A (journalled) rebind clears it, in memory and on disk.
    h.admin_ok(Message::BindRebind { uid: h.euid, user_pubkey: Some(pubkey_hex(2)) }).await;
    assert!(h.connect_retry(None, 2, RegisterParams::default()).await.is_ok());
    assert!(!std::fs::read_to_string(&file).unwrap().contains("uid"));
    h.restart().await;
    assert!(h.admin_ok(Message::Status {}).await["force_revoked"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn a_corrupt_force_revoked_file_stops_the_service() {
    let mut h = Harness::start().await;
    h.stop().await;
    std::fs::write(h.state_dir().join("force-revoked.json"), b"not json").unwrap();
    assert!(matches!(h.begin().await, Err(StartError::Config(_))), "fail closed");
}
