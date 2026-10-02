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
use clawft_rpc::mesh_local::{
    MeshRole, NonceReply, PROTOCOL_TAG, RegisterRequest, bind_signed_bytes, session_signed_bytes,
};
use clawft_types::config::overlay::Limits;
use clawft_types::project::cert::{PopOp, key_id};
use clawft_types::project::{SpawnError, SpawnFile};
use clawft_weave::mesh_local_registry::{SpawnExpectation, expect_spawn, now_unix, registry};
use clawft_weave::project_boot::{BootError, bootstrap};
use clawft_weave::project_boot_run::ensure_genesis;
use common::{Daemon, SERIAL, rpc};
use ed25519_dalek::{Signer, SigningKey};
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

/// Where every user-profile path of this process resolves (cluster peers and
/// the rest of the run root), never the real `~/.weftos/run`.
fn scratch_run() -> &'static Path {
    static DIR: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    DIR.get_or_init(|| tempfile::tempdir().unwrap()).path()
}

async fn world() -> World {
    // Before the kernel boots, so nothing it resolves lands in `~/.weftos`.
    clawft_weave::user_daemon::enter_at(scratch_run());
    let d = common::spawn().await;
    let user_key = {
        let k = d.kernel.read().await;
        k.chain_manager().unwrap().signing_key_clone().unwrap()
    };
    let tmp = tempfile::tempdir().unwrap();
    World {
        d,
        home: tmp.path().join("home"),
        user_key,
        _tmp: tmp,
    }
}

/// Register a project with the user daemon and lay out its run dir with a
/// signed parent policy, as the supervisor would.
async fn project(w: &World) -> Proj {
    let root = tempfile::tempdir().unwrap().keep();
    let r = rpc(
        &w.d.sock,
        "project.register",
        json!({"root": root, "name": "p"}),
        Some("admin"),
        None,
    )
    .await;
    assert_eq!(r["ok"], true, "{r}");
    let id = r["result"]["project"]["id"].as_str().unwrap().to_owned();
    let stored = PathBuf::from(r["result"]["project"]["root"].as_str().unwrap());
    let run = w.d.manifests.parent().unwrap().join("run").join(&id);
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
    Proj {
        id,
        root: stored,
        run,
    }
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
    })
    .unwrap();
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
    raw_register_with(w, p, spawn_nonce, root, pop_key, reuse_nonce, |_| {}).await
}

async fn raw_register_with(
    w: &World,
    p: &Proj,
    spawn_nonce: Option<&str>,
    root: &Path,
    pop_key: &SigningKey,
    reuse_nonce: Option<&str>,
    tamper: impl FnOnce(&mut RegisterRequest),
) -> (Value, String) {
    let key = SigningKey::from_bytes(&[7u8; 32]);
    let upk = w.user_key.verifying_key().to_bytes();
    let nonce = match reuse_nonce {
        Some(n) => n.to_owned(),
        None => {
            let c = rpc(
                &w.d.sock,
                "mesh.challenge",
                json!({"project_id": p.id}),
                None,
                None,
            )
            .await;
            assert_eq!(c["ok"], true, "{c}");
            c["result"]["nonce"].as_str().unwrap().to_owned()
        }
    };
    let sig = ident::pop_sign(pop_key, PopOp::Register, &key_id(&upk), &nonce, &p.id).unwrap();
    let socket = p.run.join("kernel.sock").to_string_lossy().into_owned();
    let bind = key.sign(&bind_signed_bytes(
        &p.id,
        &nonce,
        &nonce_hex(),
        &socket,
        4242,
    ));
    let mut req = RegisterRequest {
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
        socket,
        features: vec![],
        bind_sig: ident::hex(&bind.to_bytes()),
        client_nonce: nonce_hex(),
        root_sha256: clawft_weave::project_cert_rpc::root_sha256(root),
        spawn_nonce: spawn_nonce.map(str::to_owned),
        nonce_reply: NonceReply {
            nonce: nonce.clone(),
            sig: ident::hex(&sig),
        },
    };
    tamper(&mut req);
    let r = rpc(
        &w.d.sock,
        "mesh.register",
        serde_json::to_value(req).unwrap(),
        None,
        None,
    )
    .await;
    (r, nonce)
}

fn nonce_hex() -> String {
    "dd".repeat(16)
}

fn proof(key: &SigningKey, op: &str, session: &str, pid: u32, at: u64, extra: &str) -> String {
    ident::hex(
        &key.sign(&session_signed_bytes(op, session, pid, at, extra))
            .to_bytes(),
    )
}

/// A heartbeat/unregister request body signed like the real child does it.
fn beat_body(key: &SigningKey, op: &str, session: &str, pid: u32, at: u64, agents: u32) -> Value {
    let act = clawft_rpc::mesh_local::Activity {
        last_activity_unix: 0,
        busy: clawft_rpc::mesh_local::Busy {
            agents,
            workloads: 0,
            streams: 0,
        },
    };
    let extra = if op == "heartbeat" {
        clawft_rpc::mesh_local::activity_digest(&act)
    } else {
        String::new()
    };
    json!({"session": session, "pid": pid, "at_unix": at,
           "sig": proof(key, op, session, pid, at, &extra), "activity": {"busy": {"agents": agents}}})
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
    assert_eq!(
        user_events(&w, "project.register")
            .await
            .iter()
            .filter(|e| e["cert"]["project_id"] == p.id.as_str())
            .count(),
        1
    );
    // The real heartbeat path, signed by the certified key.
    let pid = std::process::id();
    let beat = |session: &str, pid: u32, at: u64, key: &SigningKey, op: &str| {
        beat_body(key, op, session, pid, at, 2)
    };
    let hb = rpc(
        &w.d.sock,
        "mesh.heartbeat",
        beat(&session, pid, now_unix(), &c.key, "heartbeat"),
        None,
        None,
    )
    .await;
    assert_eq!(hb["ok"], true, "{hb}");
    // A session id alone is not a credential.
    let bare = rpc(
        &w.d.sock,
        "mesh.heartbeat",
        json!({"session": session}),
        None,
        None,
    )
    .await;
    assert_eq!(kind(&bare), "bad_session_proof", "{bare}");
    let other = SigningKey::from_bytes(&[3u8; 32]);
    for (what, params) in [
        (
            "wrong key",
            beat(&session, pid, now_unix(), &other, "heartbeat"),
        ),
        (
            "wrong pid",
            beat(&session, pid + 1, now_unix(), &c.key, "heartbeat"),
        ),
        (
            "stale",
            beat(&session, pid, now_unix() - 120, &c.key, "heartbeat"),
        ),
        (
            "wrong op",
            beat(&session, pid, now_unix(), &c.key, "unregister"),
        ),
    ] {
        let r = rpc(&w.d.sock, "mesh.heartbeat", params, None, None).await;
        assert_eq!(kind(&r), "bad_session_proof", "{what}: {r}");
    }
    let bad = rpc(
        &w.d.sock,
        "mesh.heartbeat",
        beat("nope", pid, now_unix(), &c.key, "heartbeat"),
        None,
        None,
    )
    .await;
    assert_eq!(kind(&bad), "unknown_session");
    // A caller who learned the session id cannot end it.
    let guessed = rpc(
        &w.d.sock,
        "mesh.unregister",
        json!({"session": session, "reason": "evil"}),
        None,
        None,
    )
    .await;
    assert_eq!(kind(&guessed), "bad_session_proof", "{guessed}");
    assert!(registry().route_for(&p.id).is_some(), "still registered");
    let un = rpc(
        &w.d.sock,
        "mesh.unregister",
        {
            let mut v = beat(&session, pid, now_unix(), &c.key, "unregister");
            v["reason"] = json!("test");
            v
        },
        None,
        None,
    )
    .await;
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
    })
    .unwrap();
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
        .unwrap()
    };
    mk();
    let (first, n) = raw_register(&w, &p, Some(&nonce('c')), &p.root, &key, None).await;
    assert_eq!(first["ok"], true, "{first}");
    registry().evict(&p.id);
    mk();
    let (replay, _) = raw_register(&w, &p, Some(&nonce('c')), &p.root, &key, Some(&n)).await;
    assert_eq!(kind(&replay), "challenge_unknown", "{replay}");
}

#[tokio::test(flavor = "multi_thread")]
async fn expired_loose_and_copied_spawn_files_are_refused() {
    let _g = SERIAL.lock().await;
    let w = world().await;
    let p = project(&w).await;
    // Expired.
    spawn_file(&w, &p, &w.d.sock, &nonce('d'), now_unix() - 120)
        .write(&p.run.join("spawn.json"))
        .unwrap();
    let e = boot(&p).await.unwrap_err();
    assert!(
        matches!(e, BootError::Spawn(SpawnError::Expired { .. })),
        "{e}"
    );
    assert!(e.to_string().contains("started by the user daemon"), "{e}");
    // Group/world readable.
    spawn(&w, &p, &nonce('e'));
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            p.run.join("spawn.json"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
    }
    let e = boot(&p).await.unwrap_err();
    assert!(
        matches!(e, BootError::Spawn(SpawnError::Insecure { .. })),
        "{e}"
    );
    assert!(e.to_string().contains("0644"), "{e}");
    // A copy taken before the real child registered, used afterwards.
    let s = spawn(&w, &p, &nonce('f'));
    let copy = p.run.join("copy.json");
    std::fs::copy(p.run.join("spawn.json"), &copy).unwrap();
    boot(&p).await.expect("the real child registers");
    s.write(&p.run.join("spawn.json")).unwrap(); // the attacker's copy, back in place
    // While the real child lives: refused as a second session, up front.
    let e = boot(&p).await.unwrap_err();
    assert!(
        matches!(&e, BootError::Refused { kind, .. } if kind == "second_session"),
        "{e}"
    );
    // After it is gone the nonce is simply used up.
    registry().evict(&p.id);
    s.write(&p.run.join("spawn.json")).unwrap();
    let e = boot(&p).await.unwrap_err();
    assert!(
        matches!(&e, BootError::Refused { kind, .. } if kind == "spawn_not_expected"),
        "{e}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_project_wrong_root_and_second_session_are_refused() {
    let _g = SERIAL.lock().await;
    let w = world().await;
    let p = project(&w).await;
    // No spawn expected: no challenge, no registration.
    let c = rpc(
        &w.d.sock,
        "mesh.challenge",
        json!({"project_id": p.id}),
        None,
        None,
    )
    .await;
    assert_eq!(kind(&c), "spawn_not_expected", "{c}");
    // A project the manifest store does not know, even with an expectation.
    let ghost = Proj {
        id: clawft_types::project::new_id(),
        root: p.root.clone(),
        run: p.run.clone(),
    };
    expect_spawn(SpawnExpectation {
        project_id: ghost.id.clone(),
        nonce: nonce('1'),
        pid: 0,
        exe_sha: String::new(),
        root: p.root.clone(),
        expires_unix: now_unix() + 60,
    })
    .unwrap();
    let c = rpc(
        &w.d.sock,
        "mesh.challenge",
        json!({"project_id": ghost.id}),
        None,
        None,
    )
    .await;
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
        .unwrap()
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
        let at_seq = user_chain
            .tail(0)
            .into_iter()
            .find(|e| e.sequence == head.user_seq)
            .expect("head event exists");
        assert_eq!(
            ident::hex(&at_seq.hash),
            head.user_event_hash,
            "the genesis names a real user-chain event"
        );
    }
    let chain = ChainManager::new(0, 1000);
    chain.append("kernel", "boot.init", None);
    assert!(ensure_genesis(&chain, &c.cert, &head));
    assert!(!ensure_genesis(&chain, &c.cert, &head), "written once");
    let g = chain
        .tail(0)
        .into_iter()
        .find(|e| e.kind == "project.genesis")
        .unwrap();
    assert_eq!(
        g.payload.as_ref().unwrap()["parent_head"]["user_event_hash"],
        head.user_event_hash
    );
    assert_eq!(
        g.payload.as_ref().unwrap()["cert"]["project_id"],
        p.id.as_str()
    );
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
    assert_eq!(
        registers
            .iter()
            .filter(|e| e["cert"]["project_id"] == p.id.as_str())
            .count(),
        1,
        "no second certificate"
    );
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
    spawn_file(&w, &p, &dead, &nonce('8'), now_unix())
        .write(&p.run.join("spawn.json"))
        .unwrap();
    let again = boot(&p)
        .await
        .expect("a registered child continues degraded");
    assert!(again.degraded.is_some() && again.session.is_none());
    assert_eq!(
        again.cert, first.cert,
        "the cached certificate is the one in force"
    );
    assert_eq!(again.cert.project_key_id, first.cert.project_key_id);
    // A project that never registered has no certificate to fall back on.
    let q = project(&w).await;
    spawn_file(&w, &q, &dead, &nonce('9'), now_unix())
        .write(&q.run.join("spawn.json"))
        .unwrap();
    let e = boot(&q).await.unwrap_err();
    assert!(matches!(e, BootError::NeverRegistered(_)), "{e}");
    assert!(!q.root.join(".weftos/project.cert.json").exists());
}

/// Answer every request with a canned success: a squatter on the parent
/// socket that cannot sign with the user key.
fn squatter(path: PathBuf, cert: Value) -> std::thread::JoinHandle<Vec<Value>> {
    use std::io::{BufRead, BufReader, Write};
    let l = std::os::unix::net::UnixListener::bind(&path).unwrap();
    std::thread::spawn(move || {
        let mut seen = Vec::new();
        for s in l.incoming().take(2) {
            let s = s.unwrap();
            let mut line = String::new();
            BufReader::new(&s).read_line(&mut line).unwrap();
            let req: Value = serde_json::from_str(&line).unwrap();
            seen.push(req.clone());
            let result = if req["method"] == "mesh.challenge" {
                json!({"nonce": "aa".repeat(16), "user_key_id": ""})
            } else {
                json!({"ok": true, "session": "S", "cert": cert, "accepted": [], "heartbeat_secs": 15,
                       "proto": {"current": 1, "min": 1}, "parent_sig": "00".repeat(64)})
            };
            (&s).write_all(format!("{}\n", json!({"ok": true, "result": result})).as_bytes())
                .unwrap();
        }
        seen
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
    spawn_file(&w, &p, &fake, &nonce('b'), now_unix())
        .write(&p.run.join("spawn.json"))
        .unwrap();
    let e = boot(&p).await.unwrap_err();
    assert!(
        matches!(&e, BootError::Untrusted(m) if m.contains("user key")),
        "{e}"
    );
    // What the squatter captured is useless against the real daemon: its
    // proof is bound to a challenge only the squatter issued.
    let seen = h.join().unwrap();
    let captured = seen
        .iter()
        .find(|r| r["method"] == "mesh.register")
        .expect("it saw the register")
        .clone();
    let nb = nonce('b');
    expect_spawn(SpawnExpectation {
        project_id: p.id.clone(),
        nonce: nb.clone(),
        pid: 0,
        exe_sha: String::new(),
        root: p.root.clone(),
        expires_unix: now_unix() + 60,
    })
    .unwrap();
    let replay = rpc(
        &w.d.sock,
        "mesh.register",
        captured["params"].clone(),
        None,
        None,
    )
    .await;
    assert_eq!(kind(&replay), "challenge_unknown", "{replay}");
    assert!(
        clawft_weave::mesh_local_registry::spawn_expected(&p.id, now_unix()),
        "the nonce was not burnt"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_marker_stops_the_boot_and_a_mismatched_pin_is_refused() {
    let _g = SERIAL.lock().await;
    let w = world().await;
    let p = project(&w).await;
    spawn(&w, &p, &nonce('c'));
    std::fs::write(p.run.join("user.pub"), format!("{}\n", "11".repeat(32))).unwrap();
    let e = boot(&p).await.unwrap_err();
    assert!(
        matches!(e, BootError::Untrusted(_)),
        "pin differs from spawn.json: {e}"
    );
    spawn(&w, &p, &nonce('d'));
    std::fs::remove_file(p.run.join("user.pub")).unwrap();
    std::fs::write(p.run.join("revoked"), "").unwrap();
    let e = boot(&p).await.unwrap_err();
    assert!(matches!(e, BootError::Revoked(_)), "{e}");
}

fn expect(p: &Proj, n: char) {
    expect_spawn(SpawnExpectation {
        project_id: p.id.clone(),
        nonce: nonce(n),
        pid: 0,
        exe_sha: String::new(),
        root: p.root.clone(),
        expires_unix: now_unix() + 60,
    })
    .unwrap();
}

fn link(w: &World, p: &Proj) -> clawft_weave::project_boot::LinkParams {
    let upk = w.user_key.verifying_key().to_bytes();
    clawft_weave::project_boot::LinkParams {
        socket: w.d.sock.clone(),
        project_id: p.id.clone(),
        user_pubkey: upk,
        user_key_id: key_id(&upk),
        own_socket: p.run.join("kernel.sock"),
        root: p.root.clone(),
        timeout: T,
    }
}

fn dead_session(
    p: &Proj,
    key: &SigningKey,
    pid: u32,
) -> clawft_weave::mesh_local_registry::NewSession {
    clawft_weave::mesh_local_registry::NewSession {
        project_id: p.id.clone(),
        socket: p.run.join("kernel.sock"),
        pid,
        addresses: vec![p.id.clone()],
        topic_prefixes: vec![],
        version: "t".into(),
        project_key_id: key_id(&key.verifying_key().to_bytes()),
        project_pubkey: key.verifying_key().to_bytes(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_second_child_neither_gets_a_certificate_nor_burns_its_nonce() {
    let _g = SERIAL.lock().await;
    let w = world().await;
    let p = project(&w).await;
    spawn(&w, &p, &nonce('a'));
    boot(&p).await.unwrap();
    expect(&p, 'b');
    let intruder = SigningKey::from_bytes(&[11u8; 32]);
    let (r, _) = raw_register(&w, &p, Some(&nonce('b')), &p.root, &intruder, None).await;
    assert_eq!(kind(&r), "second_session", "{r}");
    assert!(
        clawft_weave::mesh_local_registry::spawn_expected(&p.id, now_unix()),
        "nonce not burnt"
    );
    let n = user_events(&w, "project.register").await;
    assert_eq!(
        n.iter()
            .filter(|e| e["cert"]["project_id"] == p.id.as_str())
            .count(),
        1,
        "no certificate for it"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn bind_signature_and_project_scoped_addresses_are_enforced() {
    let _g = SERIAL.lock().await;
    let w = world().await;
    let p = project(&w).await;
    expect(&p, 'a');
    let key = SigningKey::from_bytes(&[7u8; 32]);
    let tampers: Vec<(&str, Box<dyn FnOnce(&mut RegisterRequest)>, &str)> = vec![
        (
            "no bind_sig",
            Box::new(|r| r.bind_sig.clear()),
            "pop_failed",
        ),
        (
            "socket changed after signing",
            Box::new(|r| r.socket = "/evil.sock".into()),
            "pop_failed",
        ),
        (
            "pid changed after signing",
            Box::new(|r| r.pid = 1),
            "pop_failed",
        ),
        (
            "client_nonce changed",
            Box::new(|r| r.client_nonce = "ee".repeat(16)),
            "pop_failed",
        ),
        (
            "foreign topic",
            Box::new(|r| r.topic_prefixes = vec!["chain/other/".into()]),
            "invalid_params",
        ),
        (
            "foreign address",
            Box::new(|r| r.addresses = vec!["other".into()]),
            "invalid_params",
        ),
    ];
    for (what, f, want) in tampers {
        let (r, _) = raw_register_with(&w, &p, Some(&nonce('a')), &p.root, &key, None, f).await;
        assert_eq!(kind(&r), want, "{what}: {r}");
    }
    assert!(
        clawft_weave::mesh_local_registry::spawn_expected(&p.id, now_unix()),
        "no refusal burnt the nonce"
    );
}

/// A parent that answers every connection with a transient refusal.
fn flaky_parent(path: PathBuf, n: usize, kind: &'static str) -> std::thread::JoinHandle<usize> {
    use std::io::{BufRead, BufReader, Write};
    let l = std::os::unix::net::UnixListener::bind(&path).unwrap();
    std::thread::spawn(move || {
        let mut seen = 0;
        for s in l.incoming().take(n) {
            let s = s.unwrap();
            let mut line = String::new();
            BufReader::new(&s).read_line(&mut line).unwrap();
            seen += 1;
            (&s).write_all(
                format!("{{\"ok\":false,\"error\":\"busy\",\"error_kind\":\"{kind}\"}}\n")
                    .as_bytes(),
            )
            .unwrap();
        }
        seen
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn transient_refusals_are_retried_with_backoff_then_reported() {
    let _g = SERIAL.lock().await;
    let w = world().await;
    let p = project(&w).await;
    std::fs::create_dir_all(&w.home).unwrap();
    let sock = w.home.join("flaky.sock");
    let h = flaky_parent(sock.clone(), 3, "cert_unavailable");
    spawn_file(&w, &p, &sock, &nonce('a'), now_unix())
        .write(&p.run.join("spawn.json"))
        .unwrap();
    let retry = clawft_weave::project_boot::Retry {
        attempts: 3,
        initial: Duration::from_millis(5),
        max: Duration::from_millis(20),
        budget: Duration::from_secs(5),
    };
    let e = clawft_weave::project_boot::bootstrap_with(&p.run, &p.id, now_unix(), T, retry)
        .await
        .unwrap_err();
    assert!(
        matches!(&e, BootError::Refused { kind, .. } if kind == "cert_unavailable"),
        "{e}"
    );
    assert_eq!(h.join().unwrap(), 3, "three attempts, not one");
}

#[tokio::test(flavor = "multi_thread")]
async fn re_register_needs_the_known_pid_and_no_spawn_nonce() {
    let _g = SERIAL.lock().await;
    let w = world().await;
    let p = project(&w).await;
    spawn(&w, &p, &nonce('a'));
    let c = boot(&p).await.unwrap();
    let params = link(&w, &p);
    let now = now_unix();
    // Live session: refused.
    let live =
        clawft_weave::project_boot::register_once(&params, &c.key, Some(&c.cert), None, now).await;
    assert!(
        matches!(&live, Err(clawft_weave::project_boot::RegError::Refused { kind, .. }) if kind == "second_session"),
        "{live:?}"
    );
    // The parent forgot the child (restart): an expired record of our pid.
    registry().evict(&p.id);
    registry().adopt_expired(dead_session(&p, &c.key, std::process::id()));
    let again =
        clawft_weave::project_boot::register_once(&params, &c.key, Some(&c.cert), None, now)
            .await
            .expect("re-registers");
    assert_eq!(again.cert, c.cert, "same certificate, no second one");
    assert_ne!(Some(again.session), c.session);
    // An expired record of another pid does not let this process in.
    registry().evict(&p.id);
    registry().adopt_expired(dead_session(&p, &c.key, 1));
    let other =
        clawft_weave::project_boot::register_once(&params, &c.key, Some(&c.cert), None, now).await;
    assert!(
        matches!(&other, Err(clawft_weave::project_boot::RegError::Refused { kind, .. }) if kind == "pid_mismatch"),
        "{other:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn revoke_and_rekey_drop_the_session_and_the_marker_stops_a_degraded_boot() {
    let _g = SERIAL.lock().await;
    let w = world().await;
    let p = project(&w).await;
    spawn(&w, &p, &nonce('a'));
    let c = boot(&p).await.unwrap();
    let marker = clawft_types::runtime_paths::RuntimePaths::child_at(&p.run, &p.id, &p.root)
        .unwrap()
        .root()
        .join("revoked");
    assert_eq!(marker, p.run.join("revoked"));
    assert!(!marker.exists());
    let r = rpc(
        &w.d.sock,
        "project.revoke",
        json!({"id": p.id, "reason": "test"}),
        Some("admin"),
        None,
    )
    .await;
    assert_eq!(r["ok"], true, "{r}");
    // The real RPC wrote it at exactly the path the child's RuntimePaths resolves.
    assert!(marker.is_file(), "marker at {}", marker.display());
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&marker).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    // The session is gone: the running child's next beat fails, and its
    // re-register is refused as revoked (fatal in the child).
    let pid = std::process::id();
    let at = now_unix();
    let hb = rpc(
        &w.d.sock,
        "mesh.heartbeat",
        json!({"session": c.session, "pid": pid, "at_unix": at,
        "sig": proof(&c.key, "heartbeat", c.session.as_ref().unwrap(), pid, at, "0:0:0:0")}),
        None,
        None,
    )
    .await;
    assert_eq!(kind(&hb), "unknown_session", "{hb}");
    registry().adopt_expired(dead_session(&p, &c.key, pid));
    let again = clawft_weave::project_boot::register_once(
        &link(&w, &p),
        &c.key,
        Some(&c.cert),
        None,
        now_unix(),
    )
    .await;
    assert!(
        matches!(&again, Err(clawft_weave::project_boot::RegError::Refused { kind, .. }) if kind == "key_revoked"),
        "{again:?}"
    );
    // A degraded boot (parent unreachable, cached cert) still refuses.
    let dead = w.home.join("gone.sock");
    spawn_file(&w, &p, &dead, &nonce('b'), now_unix())
        .write(&p.run.join("spawn.json"))
        .unwrap();
    let e = boot(&p).await.unwrap_err();
    assert!(matches!(e, BootError::Revoked(_)), "{e}");
}

#[tokio::test(flavor = "multi_thread")]
async fn revoke_marks_a_stopped_child_whose_run_dir_is_gone() {
    let _g = SERIAL.lock().await;
    let w = world().await;
    let p = project(&w).await;
    spawn(&w, &p, &nonce('a'));
    boot(&p).await.unwrap();
    std::fs::remove_dir_all(&p.run).unwrap();
    let r = rpc(
        &w.d.sock,
        "project.revoke",
        json!({"id": p.id}),
        Some("admin"),
        None,
    )
    .await;
    assert_eq!(r["ok"], true, "{r}");
    assert!(
        p.run.join("revoked").is_file(),
        "marker created with its dir"
    );
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&p.run).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn rekey_drops_the_session_writes_no_marker_and_the_rekeyed_child_boots() {
    let _g = SERIAL.lock().await;
    let w = world().await;
    let q = project(&w).await;
    spawn(&w, &q, &nonce('c'));
    let old = boot(&q).await.unwrap();
    let ch = rpc(
        &w.d.sock,
        "project.cert.challenge",
        json!({"id": q.id}),
        Some("admin"),
        None,
    )
    .await;
    assert_eq!(ch["ok"], true, "{ch}");
    let new_key = SigningKey::from_bytes(&[5u8; 32]);
    let sig = ident::pop_sign(
        &new_key,
        PopOp::Rekey,
        ch["result"]["user_key_id"].as_str().unwrap(),
        ch["result"]["nonce"].as_str().unwrap(),
        &q.id,
    )
    .unwrap();
    let rk = rpc(
        &w.d.sock,
        "project.rekey",
        json!({"id": q.id, "new_pubkey": ident::hex(&new_key.verifying_key().to_bytes()),
        "nonce": ch["result"]["nonce"], "pop_sig": ident::hex(&sig)}),
        Some("admin"),
        None,
    )
    .await;
    assert_eq!(rk["ok"], true, "{rk}");
    assert!(
        registry().route_for(&q.id).is_none(),
        "rekey drops the session"
    );
    assert!(!q.run.join("revoked").exists(), "rekey writes no marker");
    // The old key can never register again.
    let again = clawft_weave::project_boot::register_once(
        &link(&w, &q),
        &old.key,
        Some(&old.cert),
        None,
        now_unix(),
    )
    .await;
    assert!(again.is_err());
    // The owner installs the new key; the rekeyed child boots under serial 2.
    let key_file = q.root.join(".weftos/project.key");
    std::fs::write(&key_file, new_key.to_bytes()).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    spawn(&w, &q, &nonce('d'));
    let new = boot(&q).await.expect("the rekeyed child boots");
    assert_eq!(new.cert.serial, 2);
    assert_eq!(
        new.cert.project_pubkey,
        ident::hex(&new_key.verifying_key().to_bytes())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_evicted_challenge_is_challenge_unknown_and_the_child_retries_it() {
    let _g = SERIAL.lock().await;
    let w = world().await;
    let p = project(&w).await;
    expect(&p, 'a');
    // A flooder pushes this challenge out between challenge and register.
    let c = rpc(
        &w.d.sock,
        "mesh.challenge",
        json!({"project_id": p.id}),
        None,
        None,
    )
    .await;
    let n = c["result"]["nonce"].as_str().unwrap().to_owned();
    for _ in 0..5 {
        let f = rpc(
            &w.d.sock,
            "mesh.challenge",
            json!({"project_id": p.id}),
            None,
            None,
        )
        .await;
        assert_eq!(f["ok"], true, "{f}");
    }
    let key = SigningKey::from_bytes(&[7u8; 32]);
    let (r, _) = raw_register(&w, &p, Some(&nonce('a')), &p.root, &key, Some(&n)).await;
    assert_eq!(kind(&r), "challenge_unknown", "{r}");
    assert!(
        clawft_weave::mesh_local_registry::spawn_expected(&p.id, now_unix()),
        "nonce not burnt"
    );
    // The child treats it as "ask again" inside its attempt budget.
    let sock = w.home.join("evicting.sock");
    std::fs::create_dir_all(&w.home).unwrap();
    let h = flaky_parent(sock.clone(), 3, "challenge_unknown");
    spawn_file(&w, &p, &sock, &nonce('b'), now_unix())
        .write(&p.run.join("spawn.json"))
        .unwrap();
    let retry = clawft_weave::project_boot::Retry {
        attempts: 3,
        initial: Duration::from_millis(5),
        max: Duration::from_millis(20),
        budget: Duration::from_secs(5),
    };
    let e = clawft_weave::project_boot::bootstrap_with(&p.run, &p.id, now_unix(), T, retry)
        .await
        .unwrap_err();
    assert!(
        matches!(&e, BootError::Refused { kind, .. } if kind == "challenge_unknown"),
        "{e}"
    );
    assert_eq!(h.join().unwrap(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_retry_budget_bounds_total_time() {
    let _g = SERIAL.lock().await;
    let w = world().await;
    let p = project(&w).await;
    std::fs::create_dir_all(&w.home).unwrap();
    let sock = w.home.join("busy.sock");
    let h = flaky_parent(sock.clone(), 1000, "cert_unavailable");
    spawn_file(&w, &p, &sock, &nonce('a'), now_unix())
        .write(&p.run.join("spawn.json"))
        .unwrap();
    let retry = clawft_weave::project_boot::Retry {
        attempts: 1000,
        initial: Duration::from_millis(40),
        max: Duration::from_millis(40),
        budget: Duration::from_millis(300),
    };
    let start = std::time::Instant::now();
    let e = clawft_weave::project_boot::bootstrap_with(&p.run, &p.id, now_unix(), T, retry)
        .await
        .unwrap_err();
    assert!(matches!(&e, BootError::Refused { .. }), "{e}");
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "stopped by the budget, not the attempt count"
    );
    drop(h);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_captured_heartbeat_is_replayable_only_inside_the_proof_window() {
    let _g = SERIAL.lock().await;
    let w = world().await;
    let p = project(&w).await;
    spawn(&w, &p, &nonce('a'));
    let c = boot(&p).await.unwrap();
    let s = c.session.clone().unwrap();
    let pid = std::process::id();
    let at = now_unix();
    let body = beat_body(&c.key, "heartbeat", &s, pid, at, 1);
    // Replays inside the window succeed: a captured beat proves liveness for
    // at most SESSION_PROOF_WINDOW_SECS (30 s), nothing more.
    for _ in 0..2 {
        let r = rpc(&w.d.sock, "mesh.heartbeat", body.clone(), None, None).await;
        assert_eq!(r["ok"], true, "{r}");
    }
    let near = beat_body(&c.key, "heartbeat", &s, pid, at - 25, 1);
    assert_eq!(
        rpc(&w.d.sock, "mesh.heartbeat", near, None, None).await["ok"],
        true
    );
    let old = beat_body(&c.key, "heartbeat", &s, pid, at - 35, 1);
    assert_eq!(
        kind(&rpc(&w.d.sock, "mesh.heartbeat", old, None, None).await),
        "bad_session_proof"
    );
    // The reported activity is covered by the signature.
    let mut forged = body.clone();
    forged["activity"] = json!({"busy": {"agents": 0}});
    assert_eq!(
        kind(&rpc(&w.d.sock, "mesh.heartbeat", forged, None, None).await),
        "bad_session_proof"
    );
}
