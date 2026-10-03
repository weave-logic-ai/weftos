//! ADR-103 Phase 3 integration, as the current user (P3-X): the real machine
//! mesh service and user daemons' mesh links through the real daemon glue
//! (resolve, register, link task), with a stub inbox and chain standing in for
//! the kernel's router and chain; `mesh_boot_user` covers the daemon boot path.
//! It walks the plan's section-10 exit list as far as it goes without root. What still
//! needs the owner's installed service is listed in
//! `docs/guides/weftos-deployment-sops.md` (machine mesh service migration).
//!
//! Driven by `scripts/dev/mesh-p3-e2e.sh` and `scripts/build.sh test-mesh-service`.
#![cfg(all(unix, feature = "mesh"))]

mod mesh_e2e;

use std::str::FromStr;
use std::sync::atomic::Ordering;
use std::time::Duration;

use clawft_kernel::a2a::RemoteForwarder;
use clawft_kernel::ipc::{KernelMessage, MessageTarget};
use clawft_kernel::mesh_admit::AdmitHello;
use clawft_kernel::mesh_ipc::MeshIpcEnvelope;
use clawft_mesh_local::client::{ClientError, MeshLocalClient, RegisterParams};
use clawft_mesh_local::proto::{BindState, ErrorKind, Message};
use clawft_mesh_local::{WeftAddr, hexser};
use clawft_types::config::MeshServicePolicy;
use clawft_weave::mesh_doctor::{MeshProbe, mode_findings};
use clawft_weave::mesh_local_chain::{KIND_ANCHOR, KIND_BOUND};
use clawft_weave::mesh_local_glue::{Resolved, Timings, build_endpoint, resolve};
use clawft_rpc::doctor::Severity;
use mesh_e2e::*;

fn km(text: &str) -> KernelMessage {
    KernelMessage::text(0, MessageTarget::Topic("ignored".into()), text)
}

fn addr(s: &str) -> WeftAddr {
    WeftAddr::from_str(s).expect("weft address")
}

/// Register a raw client as `uid` with `seed`'s key (no daemon glue).
async fn raw_register(svc: &Svc, uid: u32, k: ed25519_dalek::SigningKey, params: RegisterParams) -> Result<MeshLocalClient, ClientError> {
    let mut ep = other_endpoint(svc, k, Vec::new());
    ep.client.own_uid = Some(uid);
    svc.next_uid.store(uid, Ordering::SeqCst);
    let r = MeshLocalClient::connect_and_register(&ep.client, &ep.user_key, &params).await;
    svc.next_uid.store(REAL, Ordering::SeqCst);
    r
}

fn server_kind(r: Result<MeshLocalClient, ClientError>) -> ErrorKind {
    match r {
        Err(ClientError::Server(e)) => e.kind,
        Err(other) => panic!("expected a server error, got {other}"),
        Ok(_) => panic!("expected a server error, but it registered"),
    }
}

#[tokio::test]
async fn a_daemon_registers_in_service_mode_under_the_service_node_id() {
    let svc = Svc::start().await;
    let home = tempfile::tempdir().unwrap();
    let a = daemon_as_me(&svc, home.path()).await;

    // mode = service with the service's node id; the record came from beside
    // the socket, not from the 0700 state dir.
    assert_eq!(a.node_id, svc.node_id());
    let st = a.state.get().expect("handshake mesh state");
    assert_eq!((st.mode.as_str(), st.state.as_deref()), ("service", Some("connected")));
    assert_eq!(st.service_node_id.as_deref(), Some(svc.node_id().as_str()));
    assert!(st.cert_serial.is_some() && st.proto == Some(clawft_mesh_local::PROTO_MAX));
    assert!(home.path().join(".weftos/user.key").exists(), "user key created in the daemon's home");
    assert!(home.path().join(".weftos/mesh/machine.pub").exists(), "machine key pinned on first contact");

    // The bind is journalled to this uid, how = tofu.
    let binds = svc.journal("user.bind");
    assert_eq!(binds.len(), 1, "{binds:?}");
    assert_eq!(binds[0]["body"]["principal"], serde_json::json!({"kind": "uid", "id": svc.euid}));
    assert_eq!(binds[0]["body"]["how"], "tofu");
    assert_eq!(binds[0]["body"]["user_id"], a.user_id);
    wait_until("the daemon records the binding on its chain", || !a.chain.of(KIND_BOUND).is_empty()).await;

    // The doctor reads the same status and agrees with the daemon's mode.
    let status = svc.admin(Message::Status {}).await;
    let probe = MeshProbe { record_present: true, status: Some(status), ..MeshProbe::default() };
    let f = mode_findings(Some("user"), a.state.get().as_ref(), &probe);
    assert_eq!(f[0].severity, Severity::Ok, "{f:?}");
}

#[tokio::test]
async fn a_scoped_weft_send_between_two_daemons_is_delivered_and_stamped() {
    let svc = Svc::start().await;
    let home = tempfile::tempdir().unwrap();
    let a = daemon_as_me(&svc, home.path()).await;
    // B opts in to receiving from A; A does not opt in to B.
    let b = daemon_as_other(&svc, key(2), vec![a.user_id.clone()]).await;
    assert_ne!(a.user_id, b.user_id);
    assert_eq!(b.node_id, a.node_id, "both daemons are on the one machine node");

    let fwd_a = a.handle.as_ref().unwrap().forwarder.clone();
    let dest = addr(&format!("weft://local/{}/_/chat", b.user_id));
    fwd_a.send_to(dest, &km("hello b")).await.expect("A -> B delivered");
    wait_until("B receives", || !b.received().is_empty()).await;
    let got = b.received().remove(0);
    assert_eq!(got.peer_id, svc.node_id(), "source is the machine node");
    assert_eq!(got.scope.as_ref().unwrap().user_id, b.user_id, "delivered to B's scope");
    assert_eq!(
        got.src_scope.as_ref().map(|s| s.user_id.as_str()),
        Some(a.user_id.as_str()),
        "stamped with A's user id from A's machine-issued certificate"
    );
    assert!(matches!(&got.msg.target, MessageTarget::Topic(t) if t == "chat"));

    // B -> A is refused: A accepts no other tenant.
    let fwd_b = b.handle.as_ref().unwrap().forwarder.clone();
    let back = addr(&format!("weft://local/{}/_/chat", a.user_id));
    assert!(fwd_b.send_to(back, &km("hello a")).await.is_err());
    // A remote node nobody knows is an error, not a silent drop.
    assert!(fwd_a.forward(&"d".repeat(32), km("far")).await.is_err());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(a.received().is_empty(), "A got nothing it did not accept");
}

#[tokio::test]
async fn a_second_uid_cannot_take_the_first_uids_address() {
    let svc = Svc::start().await;
    let home = tempfile::tempdir().unwrap();
    let a = daemon_as_me(&svc, home.path()).await;
    let (a_key, _) = clawft_weave::user_key::resolve_user_key(home.path(), false).unwrap();

    // The same key (a copied user.key) from another uid: refused while A holds
    // the address, and refused by the binding once A is gone.
    assert_eq!(
        server_kind(raw_register(&svc, 9002, a_key.clone(), RegisterParams::default()).await),
        ErrorKind::AddressInUse
    );
    // Its own key, claiming A's topic prefix: registered, but the claim is refused.
    let params = RegisterParams { topic_prefixes: vec![format!("user/{}/", a.user_id)], ..RegisterParams::default() };
    let c = raw_register(&svc, 9002, key(3), params).await.expect("own key registers");
    assert!(c.register_ack().accepted.topic_prefixes.is_empty(), "{:?}", c.register_ack());
    assert_eq!(c.register_ack().rejected.len(), 1);
    // A is untouched.
    assert_eq!(a.link_state().as_deref(), Some("connected"));
    let b = svc.admin(Message::BindingsList {}).await;
    let mine = b["bound"].as_array().unwrap().iter().find(|x| x["principal"]["uid"] == svc.euid).cloned().unwrap();
    assert_eq!(mine["user_id"], a.user_id);
    let mut a = a;
    a.shutdown().await;
    drop(c);
    let mut registered = 1;
    for _ in 0..80 {
        registered = svc.admin(Message::Status {}).await["registered"].as_u64().unwrap();
        if registered == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(registered, 0, "A's registration is released");
    assert_eq!(server_kind(raw_register(&svc, 9002, a_key, RegisterParams::default()).await), ErrorKind::BindConflict);
}

#[tokio::test]
async fn rebind_revokes_the_old_key_and_its_daemon_stays_out() {
    // The register budget is raised so the assertions below see the binding,
    // not the rate limit; the attempt count is asserted separately.
    let limits = clawft_mesh_service::limits::LimitConfig { registers: 1000, ..Default::default() };
    let svc = Svc::with_limits(limits).await;
    // Production-shaped backoff ceiling (scaled): 20 ms base, 1 s max.
    let slow = Timings { backoff: (Duration::from_millis(20), Duration::from_secs(1)), ..fast() };
    let mut b = daemon_as_other_with(&svc, key(2), Vec::new(), slow).await;
    let old_serial = b.state.get().unwrap().cert_serial.unwrap();
    let new_key = key(4);
    let pubkey = hexser::encode(&new_key.verifying_key().to_bytes());
    svc.admin(Message::BindRebind { uid: OTHER_UID, user_pubkey: Some(pubkey) }).await;

    // The old registration is dropped and every reconnect with the old key fails.
    svc.next_uid.store(OTHER_UID, Ordering::SeqCst);
    let before = svc.injected_conns.load(Ordering::SeqCst);
    wait_until("B's link drops", || b.link_state().as_deref() == Some("reconnecting")).await;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(b.link_state().as_deref(), Some("reconnecting"), "the old key cannot come back");
    // Refused registrations retry at the ceiling, not the exponential ramp
    // (which would be ~6 attempts in this window and exhaust the default
    // budget of 5 per minute for the uid).
    let attempts = svc.injected_conns.load(Ordering::SeqCst) - before;
    assert!((1..=3).contains(&attempts), "{attempts} register attempts after the refusal");
    b.shutdown().await;
    assert_eq!(server_kind(raw_register(&svc, OTHER_UID, key(2), RegisterParams::default()).await), ErrorKind::BindConflict);
    let c = raw_register(&svc, OTHER_UID, new_key, RegisterParams::default()).await.expect("new key registers");
    assert_eq!(c.register_ack().bind, BindState::Existing);

    // The old key's serials are revoked and published in the signed facts.
    let facts = svc.admin(Message::FactsGet {}).await;
    let payload: serde_json::Value =
        serde_json::from_str(facts["revocations"]["payload"].as_str().unwrap()).unwrap();
    let ranges = payload["ranges"].as_array().unwrap();
    assert!(
        ranges.iter().any(|r| r["user_id"] == b.user_id && r["through"].as_u64().unwrap() >= old_serial),
        "{payload}"
    );
    assert_eq!(svc.journal("user.bind").last().unwrap()["body"]["how"], "rebind");
}

#[tokio::test]
async fn observe_admission_journals_a_bad_peer_and_still_admits_it() {
    let svc = Svc::start().await;
    let home = tempfile::tempdir().unwrap();
    let a = daemon_as_me(&svc, home.path()).await;
    let t = clawft_kernel::mesh_serve::transport_for("tcp", None);
    let mut peer = t.connect(&svc.running().mesh_addr.unwrap().to_string()).await.expect("mesh listener");

    // A signed hello on a plaintext channel: no handshake hash to bind it to.
    let bad_key = key(9);
    let hello = AdmitHello::sign(&bad_key, &[0; 32], &[], &[0x11; 32], clawft_mesh_local::client::now_unix(), "linux", vec![]);
    peer.send(&hello.to_bytes()).await.unwrap();
    let env = MeshIpcEnvelope::new(hello.node_id.clone(), svc.node_id(), km("from a bad peer"));
    peer.send(&env.to_bytes().unwrap()).await.unwrap();

    wait_until("observe still admits and delivers to the only tenant", || !a.received().is_empty()).await;
    let refused = svc.journal("peer.refuse");
    assert!(
        refused.iter().any(|r| r["body"]["mode"] == "observe" && !r["body"]["reason"].as_str().unwrap_or("").is_empty()),
        "{refused:?}"
    );
    assert!(svc.journal("peer.admit").is_empty(), "nothing was admitted as verified");
}

#[tokio::test]
async fn a_service_restart_keeps_bindings_and_chains_and_the_daemon_reconnects() {
    let mut svc = Svc::start().await;
    let home = tempfile::tempdir().unwrap();
    let a = daemon_as_me(&svc, home.path()).await;
    let mut b = daemon_as_other(&svc, key(2), Vec::new()).await;
    wait_until("A anchors the journal head", || !a.chain.of(KIND_ANCHOR).is_empty()).await;
    let first = a.chain.of(KIND_ANCHOR)[0].clone();
    let node = svc.node_id();
    // B's daemon stops with the service (its reconnects would race A's for the
    // injected uid); it boots again afterwards like a restarted daemon.
    b.shutdown().await;

    svc.stop().await;
    wait_until("A notices", || a.link_state().as_deref() == Some("reconnecting")).await;
    svc.begin().await;
    assert_eq!(svc.node_id(), node, "the box key persists");
    wait_until("A reconnects on its own", || a.link_state().as_deref() == Some("connected")).await;
    wait_until("A records the new binding session", || a.chain.of(KIND_BOUND).len() >= 2).await;
    let anchored = || a.chain.of(KIND_ANCHOR).iter().any(|x| x["seq"].as_u64() > first["seq"].as_u64());
    wait_until("A anchors the journal again, further along", anchored).await;
    let b2 = daemon_as_other(&svc, key(2), Vec::new()).await;
    assert_eq!(b2.user_id, b.user_id);

    let v = svc.admin(Message::JournalVerify {}).await;
    assert_eq!(v["ok"], true, "{v}");
    let bound = svc.admin(Message::BindingsList {}).await["bound"].as_array().unwrap().len();
    assert_eq!(bound, 2, "both bindings survive the restart (journal fold)");
    assert_eq!(svc.journal("user.bind").len(), 2, "re-registration is not a new bind");
}

#[tokio::test]
async fn a_certificate_that_expires_while_the_service_is_down_is_replaced_on_reconnect() {
    // The shortest certificate the service allows. Stop the service before the
    // half-life renewal, keep it down past expiry, then bring it back.
    let mut svc = Svc::with_config(Default::default(), |c| c.cert_ttl_s = 10).await;
    let home = tempfile::tempdir().unwrap();
    let a = daemon_as_me(&svc, home.path()).await;
    wait_until("A is connected", || a.link_state().as_deref() == Some("connected")).await;
    let issued = svc.journal("user.cert.issue");
    let (serial, not_after) = {
        let last = issued.last().expect("a certificate was issued");
        (last["body"]["serial"].as_u64().unwrap(), last["body"]["not_after"].as_u64().unwrap())
    };
    let bound = a.chain.of(KIND_BOUND).len();
    svc.stop().await;
    wait_until("A notices", || a.link_state().as_deref() == Some("reconnecting")).await;
    let now = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    while now() <= not_after {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    svc.begin().await;
    wait_until("A reconnects with the service back", || a.link_state().as_deref() == Some("connected")).await;
    wait_until("A records a new binding session", || a.chain.of(KIND_BOUND).len() > bound).await;
    let newest = svc.journal("user.cert.issue");
    let newest = newest.last().unwrap();
    assert!(newest["body"]["serial"].as_u64().unwrap() > serial, "the expired certificate was replaced, not reused");
    assert!(newest["body"]["not_after"].as_u64().unwrap() > now(), "and the new one is live");
    assert_eq!(svc.journal("user.bind").len(), 1, "the binding itself is unchanged");
}

#[tokio::test]
async fn service_off_is_collapsed_and_required_without_a_service_fails() {
    let mut svc = Svc::start().await;
    let home = tempfile::tempdir().unwrap();

    // off: never probes, even with a live service; nothing registers.
    let off = svc.mesh_cfg(MeshServicePolicy::Off);
    let ep = build_endpoint(&off, home.path(), "sha").unwrap();
    assert!(matches!(resolve(&off, Ok(ep)).await.unwrap(), Resolved::Collapsed));
    let s = svc.admin(Message::Status {}).await;
    assert_eq!(s["registered"], 0);
    assert!(svc.journal("user.bind").is_empty());
    assert!(!home.path().join(".weftos/mesh/machine.pub").exists(), "off never contacted the service");

    // auto + a service: service mode; then the doctor flags a daemon left collapsed.
    let auto = svc.mesh_cfg(MeshServicePolicy::Auto);
    let ep = build_endpoint(&auto, home.path(), "sha").unwrap();
    assert!(matches!(resolve(&auto, Ok(ep)).await.unwrap(), Resolved::Service(_)));
    let probe = MeshProbe { record_present: true, status: Some(s), ..MeshProbe::default() };
    let collapsed = clawft_weave::mesh_state::plain("collapsed");
    assert_eq!(mode_findings(Some("user"), Some(&collapsed), &probe)[0].severity, Severity::Warn);
    // The same collapsed mode from a project daemon is not a mismatch.
    assert_eq!(mode_findings(None, Some(&collapsed), &probe)[0].severity, Severity::Ok);

    // No service: auto collapses, required fails boot with the reason.
    svc.stop().await;
    let _ = std::fs::remove_file(svc.socket());
    let ep = build_endpoint(&auto, home.path(), "sha").unwrap();
    assert!(matches!(resolve(&auto, Ok(ep)).await.unwrap(), Resolved::Collapsed));
    let req = svc.mesh_cfg(MeshServicePolicy::Required);
    let ep = build_endpoint(&req, home.path(), "sha").unwrap();
    let e = resolve(&req, Ok(ep)).await.err().expect("required without a service");
    assert!(e.contains("required"), "{e}");
}

#[tokio::test]
async fn a_record_that_appears_shortly_after_the_socket_is_waited_for() {
    let svc = Svc::start().await;
    let home = tempfile::tempdir().unwrap();
    let path = svc.socket().parent().unwrap().join("service.json");
    let bytes = std::fs::read(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    let writer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        // Atomic publish, as the service does: never a visible empty file.
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, bytes).unwrap();
        std::fs::rename(&tmp, &path).unwrap();
    });
    let cfg = svc.mesh_cfg(MeshServicePolicy::Auto);
    let ep = build_endpoint(&cfg, home.path(), "sha").expect("the record is waited for").expect("socket present");
    writer.join().unwrap();
    assert_eq!(ep.client.service.node_id, svc.node_id());
}
