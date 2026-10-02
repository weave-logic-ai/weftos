//! Package H: child bootstrap against a real user-daemon dispatch path
//! (ADR-103 A6, Phase 2). A tempdir `HOME`, a tempdir manifest store, a
//! kernel with an isolated chain standing in for the user daemon, and the
//! real `project_boot::bootstrap` as the child. Nothing touches `~/.weftos`.
//!
//! Covers: the register happy path; wrong proof of possession, replayed
//! challenge nonce, expired / copied / loose `spawn.json`, unknown project,
//! wrong root and a second session all refused; the genesis names the
//! user-chain head and the chain verifies; a restart keeps the node id and
//! mints no second certificate; a parent-down boot of a registered child
//! continues degraded while a never-registered child refuses; a squatter
//! on the parent socket cannot get a child to run.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use clawft_kernel::chain::ChainManager;
use clawft_kernel::project_identity as ident;
use clawft_rpc::mesh_local::{MeshRole, NonceReply, PROTOCOL_TAG, RegisterRequest};
use clawft_types::config::overlay::Limits;
use clawft_types::project::SpawnFile;
use clawft_types::project::cert::{PopOp, key_id};
use clawft_weave::mesh_local_registry::{SpawnExpectation, expect_spawn, now_unix, registry};
use clawft_weave::project_boot::{BootError, bootstrap};
use clawft_weave::project_boot_run::ensure_genesis;
use common::{Daemon, SERIAL, rpc};
use ed25519_dalek::SigningKey;
use serde_json::{Value, json};

const T: Duration = Duration::from_secs(5);

struct World {
    d: Daemon,
    home: PathBuf,
    user_key: SigningKey,
    _tmp: tempfile::TempDir,
}

struct Proj {
    id: String,
    root: PathBuf,
    run: PathBuf,
}

async fn world() -> World {
    let d = common::spawn().await;
    clawft_weave::user_daemon::enter();
    let user_key = {
        let k = d.kernel.read().await;
        k.chain_manager().unwrap().signing_key_clone().unwrap()
    };
    let tmp = tempfile::tempdir().unwrap();
    World { d, home: tmp.path().join("home"), user_key, _tmp: tmp }
}

/// Register a project with the user daemon and lay out its run dir with a
/// signed parent policy, as the supervisor would.
async fn project(w: &World) -> Proj {
    let root = tempfile::tempdir().unwrap().keep();
    let r = rpc(&w.d.sock, "project.register", json!({"root": root, "name": "p"}), Some("admin"), None).await;
    assert_eq!(r["ok"], true, "{r}");
    let id = r["result"]["project"]["id"].as_str().unwrap().to_owned();
    let stored = PathBuf::from(r["result"]["project"]["root"].as_str().unwrap());
    let run = w.home.join(".weftos/run").join(&id);
    std::fs::create_dir_all(&run).unwrap();
    clawft_kernel::parent_policy::export_rules_to(
        &run.join("parent-policy.json"),
        vec![],
        0.5,
        false,
        &Limits::default(),
        &w.user_key,
    )
    .unwrap();
    Proj { id, root: stored, run }
}

fn spawn_file(w: &World, p: &Proj, sock: &Path, nonce: &str, now: u64) -> SpawnFile {
    let upk = w.user_key.verifying_key().to_bytes();
    SpawnFile::new(
        nonce.to_owned(),
        sock.to_path_buf(),
        ident::hex(&upk),
        key_id(&upk),
        p.id.clone(),
        p.root.clone(),
        Some("wft_test_token".into()),
        now,
    )
}

/// What the supervisor does: write spawn.json, file the expectation.
fn spawn(w: &World, p: &Proj, nonce: &str) -> SpawnFile {
    let s = spawn_file(w, p, &w.d.sock, nonce, now_unix());
    s.write(&p.run.join("spawn.json")).unwrap();
    expect_spawn(SpawnExpectation {
        project_id: p.id.clone(),
        nonce: nonce.to_owned(),
        pid: 0,
        exe_sha: "ee".repeat(32),
        root: p.root.clone(),
        expires_unix: s.expires_unix,
    });
    s
}

fn nonce(c: char) -> String {
    c.to_string().repeat(32)
}

async fn boot(p: &Proj) -> Result<clawft_weave::project_boot::ChildBoot, BootError> {
    bootstrap(&p.run, &p.id, now_unix(), T).await
}

async fn user_events(w: &World, kind: &str) -> Vec<Value> {
    let k = w.d.kernel.read().await;
    k.chain_manager()
        .unwrap()
        .tail(0)
        .into_iter()
        .filter(|e| e.kind == kind)
        .filter_map(|e| e.payload)
        .collect()
}

/// A hand-built `mesh.register` for the refusal tests.
async fn raw_register(
    w: &World,
    p: &Proj,
    spawn_nonce: Option<&str>,
    root: &Path,
    pop_key: &SigningKey,
    reuse_nonce: Option<&str>,
) -> (Value, String) {
    let key = SigningKey::from_bytes(&[7u8; 32]);
    let upk = w.user_key.verifying_key().to_bytes();
    let nonce = match reuse_nonce {
        Some(n) => n.to_owned(),
        None => {
            let c = rpc(&w.d.sock, "mesh.challenge", json!({"project_id": p.id}), None, None).await;
            assert_eq!(c["ok"], true, "{c}");
            c["result"]["nonce"].as_str().unwrap().to_owned()
        }
    };
    let sig = ident::pop_sign(pop_key, PopOp::Register, &key_id(&upk), &nonce, &p.id).unwrap();
    let req = RegisterRequest {
        protocol: PROTOCOL_TAG.into(),
        role: MeshRole::Project,
        project_id: p.id.clone(),
        project_pubkey: ident::hex(&key.verifying_key().to_bytes()),
        cert: None,
        addresses: vec![p.id.clone()],
        topic_prefixes: vec![],
        version: "t".into(),
        build_sha: String::new(),
        pid: 4242,
        socket: p.run.join("kernel.sock").to_string_lossy().into_owned(),
        features: vec![],
        client_nonce: nonce_hex(),
        root_sha256: clawft_weave::project_cert_rpc::root_sha256(root),
        spawn_nonce: spawn_nonce.map(str::to_owned),
        nonce_reply: NonceReply { nonce: nonce.clone(), sig: ident::hex(&sig) },
    };
    let r = rpc(&w.d.sock, "mesh.register", serde_json::to_value(req).unwrap(), None, None).await;
    (r, nonce)
}

fn nonce_hex() -> String {
    "dd".repeat(16)
}

fn kind(v: &Value) -> &str {
    v["error_kind"].as_str().unwrap_or("")
}

#[tokio::test(flavor = "multi_thread")]
async fn register_happy_path() {
    let _g = SERIAL.lock().await;
    let w = world().await;
    let p = project(&w).await;
    spawn(&w, &p, &nonce('a'));
    let c = boot(&p).await.expect("child boots");
    assert_eq!(c.cert.serial, 1);
    assert_eq!(c.cert.project_id, p.id);
    assert!(c.degraded.is_none());
    let session = c.session.clone().expect("session");
    assert!(!p.run.join("spawn.json").exists(), "spawn.json is consumed");
    let cert_file = p.root.join(".weftos/project.cert.json");
    assert!(cert_file.is_file(), "certificate persisted in the project");
    assert_eq!(
        clawft_kernel::node_id_from_pubkey(&c.key.verifying_key().to_bytes()),
        c.cert.project_key_id,
        "node id is the certified project key id"
    );
    let route = registry().route_for(&p.id).expect("registered");
    assert_eq!(route.session, session);
    assert_eq!(route.addresses, vec![p.id.clone()]);
    assert_eq!(user_events(&w, "project.register").await.iter().filter(|e| e["cert"]["project_id"] == p.id.as_str()).count(), 1);
    // The real heartbeat path.
    let hb = rpc(&w.d.sock, "mesh.heartbeat", json!({"session": session, "activity": {"busy": {"agents": 2}}}), None, None).await;
    assert_eq!(hb["ok"], true, "{hb}");
    let bad = rpc(&w.d.sock, "mesh.heartbeat", json!({"session": "nope"}), None, None).await;
    assert_eq!(kind(&bad), "unknown_session");
    let un = rpc(&w.d.sock, "mesh.unregister", json!({"session": session, "reason": "test"}), None, None).await;
    assert_eq!(un["ok"], true, "{un}");
    assert!(registry().route_for(&p.id).is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_pop_is_refused_and_does_not_burn_the_spawn_nonce() {
    let _g = SERIAL.lock().await;
    let w = world().await;
    let p = project(&w).await;
    expect_spawn(SpawnExpectation {
        project_id: p.id.clone(),
        nonce: nonce('b'),
        pid: 0,
        exe_sha: String::new(),
        root: p.root.clone(),
        expires_unix: now_unix() + 60,
    });
    let other = SigningKey::from_bytes(&[9u8; 32]);
    let (r, _) = raw_register(&w, &p, Some(&nonce('b')), &p.root, &other, None).await;
    assert_eq!(kind(&r), "pop_failed", "{r}");
    // The right key still gets in with the same spawn nonce.
    let key = SigningKey::from_bytes(&[7u8; 32]);
    let (ok, _) = raw_register(&w, &p, Some(&nonce('b')), &p.root, &key, None).await;
    assert_eq!(ok["ok"], true, "{ok}");
}

#[tokio::test(flavor = "multi_thread")]
async fn replayed_challenge_nonce_is_refused() {
    let _g = SERIAL.lock().await;
    let w = world().await;
    let p = project(&w).await;
    let key = SigningKey::from_bytes(&[7u8; 32]);
    let mk = || {
        expect_spawn(SpawnExpectation {
            project_id: p.id.clone(),
            nonce: nonce('c'),
            pid: 0,
            exe_sha: String::new(),
            root: p.root.clone(),
            expires_unix: now_unix() + 60,
        })
    };
    mk();
    let (first, n) = raw_register(&w, &p, Some(&nonce('c')), &p.root, &key, None).await;
    assert_eq!(first["ok"], true, "{first}");
    registry().evict(&p.id);
    mk();
    let (replay, _) = raw_register(&w, &p, Some(&nonce('c')), &p.root, &key, Some(&n)).await;
    assert_eq!(kind(&replay), "pop_failed", "{replay}");
}

#[tokio::test(flavor = "multi_thread")]
async fn expired_loose_and_copied_spawn_files_are_refused() {
    let _g = SERIAL.lock().await;
    let w = world().await;
    let p = project(&w).await;
    // Expired.
    spawn_file(&w, &p, &w.d.sock, &nonce('d'), now_unix() - 120).write(&p.run.join("spawn.json")).unwrap();
    let e = boot(&p).await.unwrap_err();
    assert!(matches!(e, BootError::Spawn(_)), "{e}");
    assert!(e.to_string().contains("started by the user daemon"), "{e}");
    // Group/world readable.
    spawn(&w, &p, &nonce('e'));
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p.run.join("spawn.json"), std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    let e = boot(&p).await.unwrap_err();
    assert!(e.to_string().contains("0644"), "{e}");
    // A copy taken before the real child registered, used afterwards.
    let s = spawn(&w, &p, &nonce('f'));
    let copy = p.run.join("copy.json");
    std::fs::copy(p.run.join("spawn.json"), &copy).unwrap();
    boot(&p).await.expect("the real child registers");
    s.write(&p.run.join("spawn.json")).unwrap(); // the attacker's copy, back in place
    let e = boot(&p).await.unwrap_err();
    assert!(
        matches!(&e, BootError::Refused { kind, .. } if kind == "spawn_not_expected" || kind == "second_session"),
        "{e}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_project_wrong_root_and_second_session_are_refused() {
    let _g = SERIAL.lock().await;
    let w = world().await;
    let p = project(&w).await;
    // No spawn expected: no challenge, no registration.
    let c = rpc(&w.d.sock, "mesh.challenge", json!({"project_id": p.id}), None, None).await;
    assert_eq!(kind(&c), "spawn_not_expected", "{c}");
    // A project the manifest store does not know, even with an expectation.
    let ghost = Proj { id: clawft_types::project::new_id(), root: p.root.clone(), run: p.run.clone() };
    expect_spawn(SpawnExpectation {
        project_id: ghost.id.clone(),
        nonce: nonce('1'),
        pid: 0,
        exe_sha: String::new(),
        root: p.root.clone(),
        expires_unix: now_unix() + 60,
    });
    let c = rpc(&w.d.sock, "mesh.challenge", json!({"project_id": ghost.id}), None, None).await;
    assert_eq!(kind(&c), "project_not_found", "{c}");
    // Wrong root: spawned in p.root, child claims another directory.
    let key = SigningKey::from_bytes(&[7u8; 32]);
    let mk = |n: char| {
        expect_spawn(SpawnExpectation {
            project_id: p.id.clone(),
            nonce: nonce(n),
            pid: 0,
            exe_sha: String::new(),
            root: p.root.clone(),
            expires_unix: now_unix() + 60,
        })
    };
    mk('2');
    let elsewhere = tempfile::tempdir().unwrap();
    let (r, _) = raw_register(&w, &p, Some(&nonce('2')), elsewhere.path(), &key, None).await;
    assert_eq!(kind(&r), "root_mismatch", "{r}");
    // First session wins, the second is refused while the first beats.
    let (first, _) = raw_register(&w, &p, Some(&nonce('2')), &p.root, &key, None).await;
    assert_eq!(first["ok"], true, "{first}");
    mk('3');
    let (second, _) = raw_register(&w, &p, Some(&nonce('3')), &p.root, &key, None).await;
    assert_eq!(kind(&second), "second_session", "{second}");
    // Without a spawn nonce a live session is refused too.
    let (nonce_less, _) = raw_register(&w, &p, None, &p.root, &key, None).await;
    assert_eq!(kind(&nonce_less), "second_session", "{nonce_less}");
    // A project the supervisor never spawned.
    let q = project(&w).await;
    let n = clawft_weave::project_cert_rpc::issue_challenge(&q.id).unwrap();
    let (stranger, _) = raw_register(&w, &q, None, &q.root, &key, Some(&n)).await;
    assert_eq!(kind(&stranger), "spawn_not_expected", "{stranger}");
}

#[tokio::test(flavor = "multi_thread")]
async fn genesis_names_the_user_chain_head_and_the_chain_verifies() {
    let _g = SERIAL.lock().await;
    let w = world().await;
    let p = project(&w).await;
    spawn(&w, &p, &nonce('4'));
    let c = boot(&p).await.unwrap();
    let head = c.parent_head.clone().expect("the parent reports its head");
    {
        let k = w.d.kernel.read().await;
        let user_chain = k.chain_manager().unwrap();
        assert!(head.user_seq <= user_chain.head_sequence());
        let at_seq = user_chain.tail(0).into_iter().find(|e| e.sequence == head.user_seq).expect("head event exists");
        assert_eq!(ident::hex(&at_seq.hash), head.user_event_hash, "the genesis names a real user-chain event");
    }
    let chain = ChainManager::new(0, 1000);
    chain.append("kernel", "boot.init", None);
    assert!(ensure_genesis(&chain, &c.cert, &head));
    assert!(!ensure_genesis(&chain, &c.cert, &head), "written once");
    let g = chain.tail(0).into_iter().find(|e| e.kind == "project.genesis").unwrap();
    assert_eq!(g.payload.as_ref().unwrap()["parent_head"]["user_event_hash"], head.user_event_hash);
    assert_eq!(g.payload.as_ref().unwrap()["cert"]["project_id"], p.id.as_str());
    assert!(chain.verify_integrity().valid);
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_keeps_node_id_and_mints_no_second_certificate() {
    let _g = SERIAL.lock().await;
    let w = world().await;
    let p = project(&w).await;
    spawn(&w, &p, &nonce('5'));
    let first = boot(&p).await.unwrap();
    // The supervisor restarts the child: old session evicted, new spawn.
    registry().evict(&p.id);
    spawn(&w, &p, &nonce('6'));
    let second = boot(&p).await.unwrap();
    assert_eq!(first.cert.project_key_id, second.cert.project_key_id);
    assert_eq!(second.cert.serial, 1);
    assert_eq!(first.cert, second.cert);
    assert_ne!(first.session, second.session);
    let registers = user_events(&w, "project.register").await;
    assert_eq!(registers.iter().filter(|e| e["cert"]["project_id"] == p.id.as_str()).count(), 1, "no second certificate");
}

#[tokio::test(flavor = "multi_thread")]
async fn parent_down_registered_child_degrades_and_never_registered_child_refuses() {
    let _g = SERIAL.lock().await;
    let w = world().await;
    let p = project(&w).await;
    spawn(&w, &p, &nonce('7'));
    let first = boot(&p).await.unwrap();
    // The parent goes away (a socket path with nothing behind it).
    let dead = w.home.join("gone.sock");
    spawn_file(&w, &p, &dead, &nonce('8'), now_unix()).write(&p.run.join("spawn.json")).unwrap();
    let again = boot(&p).await.expect("a registered child continues degraded");
    assert!(again.degraded.is_some() && again.session.is_none());
    assert_eq!(again.cert, first.cert, "the cached certificate is the one in force");
    assert_eq!(again.cert.project_key_id, first.cert.project_key_id);
    // A project that never registered has no certificate to fall back on.
    let q = project(&w).await;
    spawn_file(&w, &q, &dead, &nonce('9'), now_unix()).write(&q.run.join("spawn.json")).unwrap();
    let e = boot(&q).await.unwrap_err();
    assert!(matches!(e, BootError::NeverRegistered(_)), "{e}");
    assert!(!q.root.join(".weftos/project.cert.json").exists());
}

/// Answer every request with a canned success: a squatter on the parent
/// socket that cannot sign with the user key.
fn squatter(path: PathBuf, cert: Value) -> std::thread::JoinHandle<()> {
    use std::io::{BufRead, BufReader, Write};
    let l = std::os::unix::net::UnixListener::bind(&path).unwrap();
    std::thread::spawn(move || {
        for s in l.incoming().take(2) {
            let s = s.unwrap();
            let mut line = String::new();
            BufReader::new(&s).read_line(&mut line).unwrap();
            let req: Value = serde_json::from_str(&line).unwrap();
            let result = if req["method"] == "mesh.challenge" {
                json!({"nonce": "aa".repeat(16), "user_key_id": ""})
            } else {
                json!({"ok": true, "session": "S", "cert": cert, "accepted": [], "heartbeat_secs": 15,
                       "proto": {"current": 1, "min": 1}, "parent_sig": "00".repeat(64)})
            };
            (&s).write_all(format!("{}\n", json!({"ok": true, "result": result})).as_bytes()).unwrap();
        }
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_squatter_on_the_parent_socket_cannot_get_a_child_to_run() {
    let _g = SERIAL.lock().await;
    let w = world().await;
    let p = project(&w).await;
    spawn(&w, &p, &nonce('a'));
    let real = boot(&p).await.unwrap();
    let cert = serde_json::to_value(&real.cert).unwrap();
    // Same project, new spawn, but the parent socket is the squatter's,
    // which replays the (public) certificate.
    let fake = w.home.join("fake.sock");
    std::fs::create_dir_all(&w.home).unwrap();
    let h = squatter(fake.clone(), cert);
    registry().evict(&p.id);
    spawn_file(&w, &p, &fake, &nonce('b'), now_unix()).write(&p.run.join("spawn.json")).unwrap();
    let e = boot(&p).await.unwrap_err();
    assert!(matches!(&e, BootError::Untrusted(m) if m.contains("user key")), "{e}");
    drop(h);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_marker_stops_the_boot_and_a_mismatched_pin_is_refused() {
    let _g = SERIAL.lock().await;
    let w = world().await;
    let p = project(&w).await;
    spawn(&w, &p, &nonce('c'));
    std::fs::write(p.run.join("user.pub"), format!("{}\n", "11".repeat(32))).unwrap();
    let e = boot(&p).await.unwrap_err();
    assert!(matches!(e, BootError::Untrusted(_)), "pin differs from spawn.json: {e}");
    spawn(&w, &p, &nonce('d'));
    std::fs::remove_file(p.run.join("user.pub")).unwrap();
    std::fs::write(p.run.join("revoked"), "").unwrap();
    let e = boot(&p).await.unwrap_err();
    assert!(matches!(e, BootError::Revoked(_)), "{e}");
}
