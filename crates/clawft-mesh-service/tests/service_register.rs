//! Registration, binding, certificates, admin gating, limits and versions,
//! against a real service on tempdirs as the current user.

mod common;

use std::time::Duration;

use clawft_mesh_local::client::RegisterParams;
use clawft_mesh_local::proto::{BindState, ErrorKind, Message, Role};
use clawft_mesh_service::config::BindPolicy;
use common::*;

fn now() -> u64 {
    clawft_mesh_local::client::now_unix()
}

#[tokio::test]
async fn register_new_then_existing_with_a_verifying_certificate() {
    let h = Harness::start().await;
    let c = h.connect(None, 1, RegisterParams::default()).await.unwrap();
    assert_eq!(c.register_ack().bind, BindState::New);
    assert_eq!(c.register_ack().user_id, user_id(1));
    c.cert().verify(&h.record().machine_pubkey, now()).unwrap();
    let first = c.cert().serial;
    c.close().await;

    let c = h.connect_retry(None, 1, RegisterParams::default()).await.unwrap();
    assert_eq!(c.register_ack().bind, BindState::Existing);
    assert_eq!(c.cert().serial, first, "a reconnect reuses a cert with over half its life left: no new journal record");
}

#[tokio::test]
async fn a_second_key_for_the_same_uid_conflicts_with_a_rebind_remedy() {
    let h = Harness::start().await;
    h.connect(None, 1, RegisterParams::default()).await.unwrap().close().await;
    match h.connect_retry(None, 2, RegisterParams::default()).await {
        Err(clawft_mesh_local::client::ClientError::Server(e)) => {
            assert_eq!(e.kind, ErrorKind::BindConflict);
            assert!(e.remedy.contains("rebind"), "remedy was {:?}", e.remedy);
        }
        other => panic!("expected bind_conflict, got {:?}", other.map(|_| ())),
    }
}

#[tokio::test]
async fn approve_policy_parks_the_bind_until_an_admin_approves() {
    let h = Harness::with(|c, _| c.bind_policy = BindPolicy::Approve).await;
    assert_eq!(server_kind(h.connect(None, 1, RegisterParams::default()).await), ErrorKind::BindPending);
    let list = h.admin_ok(Message::BindingsList {}).await;
    assert_eq!(list["pending"].as_array().unwrap().len(), 1);
    assert!(list["bound"].as_array().unwrap().is_empty());

    h.admin_ok(Message::BindApprove { uid: h.euid, user_id: Some(user_id(1)) }).await;
    let c = h.connect_retry(None, 1, RegisterParams::default()).await.unwrap();
    assert_eq!(c.register_ack().bind, BindState::Existing);
    let list = h.admin_ok(Message::BindingsList {}).await;
    assert_eq!(list["bound"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn a_second_daemon_for_the_same_user_gets_address_in_use_with_the_holder_pid() {
    let h = Harness::start().await;
    let _first = h.connect(None, 1, RegisterParams::default()).await.unwrap();
    match h.connect(None, 1, RegisterParams::default()).await {
        Err(clawft_mesh_local::client::ClientError::Server(e)) => {
            assert_eq!(e.kind, ErrorKind::AddressInUse);
            assert_eq!(e.data.unwrap()["holder_pid"], std::process::id());
        }
        other => panic!("expected address_in_use, got {:?}", other.map(|_| ())),
    }
}

#[tokio::test]
async fn another_uid_cannot_take_a_key_bound_to_the_first_uid() {
    let h = Harness::start().await;
    h.connect(None, 1, RegisterParams::default()).await.unwrap().close().await;
    // The second uid holds the same seed (a copied key file) but is not the bound principal.
    assert_eq!(
        server_kind(h.connect(Some(9001), 1, RegisterParams::default()).await),
        ErrorKind::BindConflict
    );
    // With its own key it gets its own, different address.
    let c = h.connect(Some(9001), 2, RegisterParams::default()).await.unwrap();
    assert_eq!(c.register_ack().user_id, user_id(2));
    assert_ne!(c.register_ack().user_id, user_id(1));
}

#[tokio::test]
async fn admin_verbs_are_refused_to_non_admins_and_to_registered_daemons() {
    let h = Harness::with(|c, _| c.admin_uids.clear()).await;
    // Role admin from a uid that is not root or listed: refused at hello.
    match h.admin().await {
        Err(clawft_mesh_service::admin_client::AdminError::Server(e)) => assert_eq!(e.kind, ErrorKind::Forbidden),
        other => panic!("expected forbidden, got {:?}", other.map(|_| ())),
    }
    // A registered user connection may not use admin verbs either, and survives the refusal.
    let c = h.connect(None, 1, RegisterParams::default()).await.unwrap();
    match c.request(Message::BindingsList {}).await {
        Err(clawft_mesh_local::client::ClientError::Server(e)) => assert_eq!(e.kind, ErrorKind::Forbidden),
        other => panic!("expected forbidden, got {other:?}"),
    }
    assert!(matches!(c.request(Message::Ping {}).await, Ok(Message::Pong {})));
}

#[tokio::test]
async fn renew_is_throttled_then_issues_a_new_serial_and_revocation_refuses_it() {
    let h = Harness::with(|c, _| c.cert_ttl_s = 10).await;
    let mut c = h.connect(None, 1, RegisterParams::default()).await.unwrap();
    let s0 = c.cert().serial;
    assert_eq!(c.renew().await.unwrap().serial, s0, "inside the quarter-lifetime window the cert is returned as is");
    tokio::time::sleep(Duration::from_millis(3000)).await;
    let s1 = c.renew().await.unwrap().serial;
    assert!(s1 > s0);

    h.admin_ok(Message::BindRevoke { uid: h.euid, reason: "test".into() }).await;
    // The live registration is closed with a reason.
    let mut closed = false;
    for _ in 0..40 {
        if c.renew().await.is_err() {
            closed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(closed, "a revoked binding can no longer renew");
}

#[tokio::test]
async fn revoked_uid_needs_approval_for_a_new_key_and_facts_carry_the_serials() {
    let h = Harness::start().await;
    let c = h.connect(None, 1, RegisterParams::default()).await.unwrap();
    let serial = c.cert().serial;
    h.admin_ok(Message::BindRevoke { uid: h.euid, reason: "compromised".into() }).await;
    drop(c);

    // A revoked key is never accepted again.
    assert_eq!(
        server_kind(h.connect_retry(None, 1, RegisterParams::default()).await),
        ErrorKind::BindConflict
    );
    // A fresh key for a revoked uid needs an approver: it is parked, then approved.
    assert_eq!(server_kind(h.connect(None, 3, RegisterParams::default()).await), ErrorKind::BindPending);
    h.admin_ok(Message::BindApprove { uid: h.euid, user_id: Some(user_id(3)) }).await;
    assert!(h.connect_retry(None, 3, RegisterParams::default()).await.is_ok());

    let facts = h.admin_ok(Message::FactsGet {}).await;
    let payload: serde_json::Value =
        serde_json::from_str(facts["revocations"]["payload"].as_str().unwrap()).unwrap();
    assert!(payload["serials"].as_array().unwrap().iter().any(|s| s == serial));
    let ranges = payload["ranges"].as_array().unwrap();
    assert!(ranges.iter().any(|r| r["user_id"] == user_id(1) && r["through"].as_u64().unwrap() >= serial));
}

#[tokio::test]
async fn rebind_replaces_the_key_and_revokes_the_old_one() {
    let h = Harness::start().await;
    let old = h.connect(None, 1, RegisterParams::default()).await.unwrap();
    h.admin_ok(Message::BindRebind { uid: h.euid, user_pubkey: Some(pubkey_hex(2)) }).await;
    // The old registration is closed.
    let mut gone = false;
    for _ in 0..40 {
        if old.request(Message::Ping {}).await.is_err() {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(gone);
    let c = h.connect_retry(None, 2, RegisterParams::default()).await.unwrap();
    assert_eq!(c.register_ack().bind, BindState::Existing);
    assert_eq!(server_kind(h.connect(None, 1, RegisterParams::default()).await), ErrorKind::BindConflict);
}

#[tokio::test]
async fn rebind_without_a_key_uses_the_last_conflicting_offer() {
    let h = Harness::start().await;
    h.connect(None, 1, RegisterParams::default()).await.unwrap().close().await;
    assert_eq!(server_kind(h.connect_retry(None, 2, RegisterParams::default()).await), ErrorKind::BindConflict);
    h.admin_ok(Message::BindRebind { uid: h.euid, user_pubkey: None }).await;
    assert!(h.connect_retry(None, 2, RegisterParams::default()).await.is_ok());
    // And with nothing offered it says so instead of guessing.
    assert_eq!(h.admin_err(Message::BindRebind { uid: 4242, user_pubkey: None }).await, ErrorKind::BadRequest);
}

#[tokio::test]
async fn the_sixth_register_in_a_minute_is_rate_limited() {
    let h = Harness::start().await;
    let _a = h.connect(None, 1, RegisterParams::default()).await.unwrap();
    for _ in 0..4 {
        assert_eq!(server_kind(h.connect(None, 2, RegisterParams::default()).await), ErrorKind::BindConflict);
    }
    assert_eq!(server_kind(h.connect(None, 2, RegisterParams::default()).await), ErrorKind::RateLimited);
}

#[tokio::test]
async fn connections_per_principal_are_capped() {
    let h = Harness::with(|_, l| l.per_principal = 2).await;
    let (mut a, mut b, mut c) = (Raw::connect(&h.socket()).await, Raw::connect(&h.socket()).await, Raw::connect(&h.socket()).await);
    // Slots are taken at accept time; hold the first two idle, the third is refused.
    let refused = c.recv().await.expect("an error frame");
    assert!(matches!(refused.msg, Message::Error(ref e) if e.kind == ErrorKind::RateLimited), "{refused:?}");
    a.send(hello(Role::User, 1, 1)).await;
    assert!(matches!(a.recv().await.unwrap().msg, Message::HelloAck(_)));
    b.send(Message::Bye {}).await;
}

#[tokio::test]
async fn version_mismatch_names_the_remedy_in_both_directions() {
    let h = Harness::start().await;
    let mut newer = Raw::connect(&h.socket()).await;
    newer.send(hello(Role::User, 2, 2)).await;
    match newer.recv().await.unwrap().msg {
        Message::Error(e) => {
            assert_eq!(e.kind, ErrorKind::ProtoMismatch);
            assert!(e.remedy.contains("restart the service"), "{}", e.remedy);
            assert_eq!(e.data.as_ref().unwrap()["service"]["sha"], "test-sha");
        }
        other => panic!("{other:?}"),
    }
    let mut older = Raw::connect(&h.socket()).await;
    older.send(hello(Role::User, 0, 0)).await;
    match older.recv().await.unwrap().msg {
        Message::Error(e) => assert!(e.remedy.contains("update the user daemon"), "{}", e.remedy),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn read_only_verbs_work_for_any_connection_and_unknown_verbs_do_not_close_it() {
    let h = Harness::with(|c, _| c.admin_uids.clear()).await;
    let mut r = Raw::connect(&h.socket()).await;
    r.send(hello(Role::User, 1, 1)).await;
    assert!(matches!(r.recv().await.unwrap().msg, Message::HelloAck(_)));
    r.send(Message::Status {}).await;
    match r.recv().await.unwrap().msg {
        Message::Reply { data } => {
            assert_eq!(data["node_id"], h.svc().node_id.as_str());
            assert!(data["registrations"].as_array().unwrap().is_empty(), "details are admin-only");
        }
        other => panic!("{other:?}"),
    }
    assert!(data_peers_empty(&mut r).await);
    r.send(Message::Subscribe { prefix: "x".into() }).await;
    assert!(matches!(r.recv().await.unwrap().msg, Message::Error(e) if e.kind == ErrorKind::Unsupported));
    r.send(Message::Ping {}).await;
    assert!(matches!(r.recv().await.unwrap().msg, Message::Pong {}));
}

async fn data_peers_empty(r: &mut Raw) -> bool {
    r.send(Message::Status {}).await;
    let status_peers_empty = match r.recv().await.unwrap().msg {
        Message::Reply { data } => data["peers"].as_array().unwrap().is_empty(),
        other => panic!("{other:?}"),
    };
    r.send(Message::PeersList {}).await;
    let refused = matches!(r.recv().await.unwrap().msg, Message::Error(e) if e.kind == ErrorKind::Forbidden);
    status_peers_empty && refused
}

#[tokio::test]
async fn approve_refuses_when_the_pending_key_is_not_the_one_the_admin_looked_at() {
    let h = Harness::with(|c, _| c.bind_policy = BindPolicy::Approve).await;
    assert_eq!(server_kind(h.connect(None, 1, RegisterParams::default()).await), ErrorKind::BindPending);
    let wrong = Message::BindApprove { uid: h.euid, user_id: Some(user_id(9)) };
    assert_eq!(h.admin_err(wrong).await, ErrorKind::BadRequest);
    let list = h.admin_ok(Message::BindingsList {}).await;
    assert!(list["bound"].as_array().unwrap().is_empty(), "nothing was approved");
    h.admin_ok(Message::BindApprove { uid: h.euid, user_id: Some(user_id(1)) }).await;
    assert!(h.connect_retry(None, 1, RegisterParams::default()).await.is_ok());
}

#[tokio::test]
async fn client_supplied_exe_is_truncated_and_stripped_before_it_is_shown() {
    let h = Harness::start().await;
    let _c = h.connect(None, 1, RegisterParams::default()).await.unwrap();
    let status = h.admin_ok(Message::Status {}).await;
    let exe = status["registrations"][0]["exe"].as_str().unwrap().to_string();
    assert!(exe.len() <= 256, "{}", exe.len());
    assert!(!exe.chars().any(char::is_control), "{exe:?}");
}

#[tokio::test]
async fn a_tofu_bind_never_makes_anyone_the_cluster_owner() {
    let h = Harness::start().await;
    let _c = h.connect(None, 1, RegisterParams::default()).await.unwrap();
    let status = h.admin_ok(Message::Status {}).await;
    assert!(status["cluster_owner_uid"].is_null(), "owner must be explicit, never inferred: {status}");
    // So there is nobody to ask: admissions fail closed (observe-only).
    let d = h
        .svc()
        .state
        .verdicts
        .ask(clawft_mesh_local::proto::VerdictRequest {
            subject: clawft_mesh_local::proto::VerdictSubject::PeerAdmit,
            peer: clawft_mesh_local::proto::PeerInfo {
                node_id: "n".into(),
                pubkey: clawft_mesh_local::hexser::encode(&[1; 32]),
                platform: String::new(),
                capabilities: vec![],
                genesis_hash: String::new(),
                chain_seq: 0,
            },
            topic: None,
        })
        .await;
    assert!(matches!(d, clawft_mesh_service::verdicts::Decision::Unavailable(_)));
}
