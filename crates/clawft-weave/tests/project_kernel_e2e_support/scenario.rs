//! The scenario of `project_kernel_e2e.rs`, step by step (see its module
//! docs). Every wait is bounded and waits for an observable condition.

use std::path::{Path, PathBuf};

use clawft_kernel::chain::{ChainEvent, ChainManager};
use clawft_types::project::{ChildState, ServeVia};
use clawft_types::runtime_paths::{RootSource, RuntimePaths, child_run_dir, revoked_marker};
use clawft_weave::mesh_local_registry::{SessionState, registry};
use clawft_weave::project_cert_rpc::{on_identity_change, revoked_marker_path};
use clawft_weave::project_supervisor::child::pid_alive;
use clawft_weave::project_supervisor::state;
use serde_json::{Value, json};

use crate::world::{Dirs, Reaper, World, is_ours, kill9, rpc, wait_for};

/// `app.install` is permitted by the parent policy (unlike `workload.*`, which
/// the parent default-denies), so a refusal can only come from the overlay.
const DENY_RULE: &str = "e2e.no-app-install";
const OVERLAY_DENY: &str = "schema = 1\n\n[[deny]]\nid = \"e2e.no-app-install\"\nactions = [\"app.install\"]\nreason = \"e2e: this project installs no apps\"\n";
/// Seconds a real (debug) kernel may take to boot and answer.
const BOOT_SECS: u64 = 120;

pub fn run(dirs: Dirs) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    let _reaper = Reaper(dirs.run_root.clone());
    rt.block_on(scenario(dirs));
    rt.shutdown_timeout(std::time::Duration::from_secs(5));
}

fn ok(v: &Value, what: &str) -> Value {
    assert_eq!(v["ok"], true, "{what}: {v}");
    v["result"].clone()
}

fn legacy_marker(dirs: &Dirs, id: &str) -> PathBuf {
    // What the writer used to derive: `<manifests>/../run/<id>/revoked`.
    dirs.manifests.parent().unwrap().join("run").join(id).join("revoked")
}

async fn scenario(dirs: Dirs) {
    // A leftover the adoption scan must list and never signal: a pid file
    // naming pid 1, which is alive and not one of our kernels.
    let leftover = clawft_types::project::new_id();
    std::fs::create_dir_all(dirs.run_root.join(&leftover)).unwrap();
    std::fs::write(dirs.run_root.join(&leftover).join("kernel.pid"), "1\n").unwrap();

    let early = marker_paths_agree_without_supervisor(&dirs).await;
    let w = World::start(dirs).await;
    let sup = w.sup.clone();
    assert_eq!(sup.config().run_root, w.dirs.run_root, "supervisor run root is $WEFTOS_RUNTIME_DIR");
    assert!(state::is_marked_revoked(&sup.config().run_root, &early), "supervisor sees the writer's marker");
    let child_view = RuntimePaths::child_at(sup.run_dir(&early), &early, &w.dirs.base).unwrap();
    assert!(child_view.revoked_marker().is_file(), "the child's own path is the writer's");

    // post_boot's adoption scan ran: the leftover is listed, not signalled.
    wait_for("adoption scan lists the leftover", 30, || async {
        let st = w.call("project.status", json!({})).await;
        st["result"]["unverifiable"]
            .as_array()
            .is_some_and(|a| a.iter().any(|u| u.to_string().contains(&leftover)))
            .then_some(())
    })
    .await;
    assert!(pid_alive(1));

    let (id, root) = register(&w).await;
    let child_sock = sup.run_dir(&id).join("kernel.sock");
    let paths = RuntimePaths::child_at(sup.run_dir(&id), &id, &root).unwrap();

    // Start: the real child registers (PoP, certificate) and answers.
    let r = w.call("project.start", json!({"id": id})).await;
    assert_eq!(r["ok"], true, "project.start: {r}\n--- kernel.log ---\n{}", w.log_tail(&id));
    let pid1 = r["result"]["pid"].as_u64().unwrap() as u32;
    assert!(is_ours(pid1));
    let node_id = handshake(&child_sock, &id, pid1).await;
    let cert = ok(&w.call("project.cert.show", json!({"id": id})).await, "cert.show");
    assert_eq!(cert["certified"], true, "{cert}");
    assert_eq!(cert["cert"]["project_key_id"], node_id.as_str(), "node id is the certified key id");
    assert_eq!(cert["cert"]["serial"], 1);
    assert_eq!(w.user_events("project.register").await.len(), 1);
    assert!(
        registry().sessions().iter().any(|(s, st)| s.facts.project_id == id && *st == SessionState::Live),
        "a live mesh-local session"
    );

    overlay_denial(&child_sock, &paths).await;
    idle_stop_and_anchor(&w, &id, pid1, &paths).await;

    // On demand it comes back: same node id, no second certificate.
    let r = ok(&w.call("project.ensure_running", json!({"id": id})).await, "ensure_running");
    assert_eq!(r["started"], true);
    let pid2 = r["pid"].as_u64().unwrap() as u32;
    assert_ne!(pid2, pid1);
    assert_eq!(handshake(&child_sock, &id, pid2).await, node_id);
    let cert = ok(&w.call("project.cert.show", json!({"id": id})).await, "cert.show");
    assert_eq!(cert["cert"]["serial"], 1, "a restart mints no second certificate");
    assert_eq!(w.user_events("project.register").await.len(), 1);

    three_kills(&w, &id, pid2).await;

    // An explicit restart clears the failure.
    let r = w.call("project.restart", json!({"id": id})).await;
    assert_eq!(r["ok"], true, "project.restart: {r}\n--- kernel.log ---\n{}", w.log_tail(&id));
    let pid3 = r["result"]["pid"].as_u64().unwrap() as u32;
    assert_eq!(sup.status(&id).await.state, ChildState::Running);

    revoke(&w, &id, &root, pid3).await;
    assert!(pid_alive(1), "the unverifiable leftover was never signalled");
}

/// Before any supervisor exists: the writer, a supervisor's check and the
/// child's own path name one file under `$WEFTOS_RUNTIME_DIR`, and none is
/// derived from the (overridden) manifest store.
async fn marker_paths_agree_without_supervisor(dirs: &Dirs) -> String {
    clawft_weave::user_daemon::enter();
    clawft_weave::project_rpc::init_manifests_dir(dirs.manifests.clone());
    assert!(clawft_weave::project_supervisor::global().is_none());
    let id = clawft_types::project::new_id();
    let expected = revoked_marker(&dirs.run_root, &id).unwrap();
    assert_eq!(revoked_marker_path(&id), Some(expected.clone()));
    assert_ne!(expected, legacy_marker(dirs, &id));
    on_identity_change(&id, "project.revoke").await;
    assert!(expected.is_file(), "writer wrote {}", expected.display());
    assert!(!legacy_marker(dirs, &id).exists(), "nothing under the manifest store");
    use std::os::unix::fs::PermissionsExt as _;
    assert_eq!(std::fs::metadata(&expected).unwrap().permissions().mode() & 0o077, 0);
    let child = RuntimePaths::child_at(child_run_dir(&dirs.run_root, &id).unwrap(), &id, &dirs.base).unwrap();
    assert_eq!(child.revoked_marker(), expected);
    assert!(state::is_marked_revoked(&dirs.run_root, &id));
    id
}

async fn register(w: &World) -> (String, PathBuf) {
    let root = w.dirs.base.join("p");
    std::fs::create_dir_all(&root).unwrap();
    let r = ok(&w.call("project.register", json!({"root": root, "name": "e2e"})).await, "register");
    let id = r["project"]["id"].as_str().unwrap().to_owned();
    let root = PathBuf::from(r["project"]["root"].as_str().unwrap());
    clawft_types::project::update_manifest(&w.dirs.manifests, &id, |m| {
        let s = m.serve.get_or_insert_with(Default::default);
        s.via = ServeVia::ChildKernel;
        s.restart_max = Some(2);
        s.restart_window_secs = Some(600);
        // Long, so only the explicit idle pass below can stop it (the 30 s
        // idle loop never does within this test).
        s.idle_stop_secs = Some(3600);
    })
    .unwrap()
    .unwrap();
    std::fs::create_dir_all(root.join(".weftos")).unwrap();
    std::fs::write(root.join(".weftos/overlay.toml"), OVERLAY_DENY).unwrap();
    (id, root)
}

/// The child's handshake names the project and the pid; returns `node_id`.
async fn handshake(sock: &Path, id: &str, pid: u32) -> String {
    let h = ok(&rpc(sock, "kernel.handshake", json!({}), None).await, "child handshake");
    assert_eq!(h["project_id"], id, "{h}");
    assert_eq!(h["pid"], pid, "{h}");
    h["node_id"].as_str().unwrap().to_owned()
}

/// An app manifest outside the project tree.
fn app_dir(paths: &RuntimePaths) -> PathBuf {
    let RootSource::Child { project_root, .. } = paths.source() else { unreachable!() };
    let dir = project_root.parent().unwrap().join("app");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("weftapp.toml"), "name = \"e2e-app\"\nversion = \"0.1.0\"\n").unwrap();
    dir
}

/// The overlay's deny refuses `app.install`, which the parent permits;
/// clearing the overlay and `governance.reload` (the only way a disk edit
/// takes effect) permits it again.
async fn overlay_denial(sock: &Path, paths: &RuntimePaths) {
    let install = json!({"path": app_dir(paths)});
    let denied = rpc(sock, "app.install", install.clone(), Some("admin")).await;
    assert_eq!(denied["ok"], false, "{denied}");
    let msg = denied["error"].as_str().unwrap_or_default();
    assert!(msg.contains("governance denied 'app.install'") && msg.contains(DENY_RULE), "{denied}");
    std::fs::write(paths.overlay().unwrap(), "schema = 1\n").unwrap();
    ok(&rpc(sock, "governance.reload", json!({}), Some("admin")).await, "governance.reload");
    ok(&rpc(sock, "app.install", install, Some("admin")).await, "app.install after reload");
    ok(&rpc(sock, "app.remove", json!({"name": "e2e-app"}), Some("admin")).await, "app.remove");
}

fn kinds<'a>(events: &'a [ChainEvent], kind: &str) -> Vec<&'a ChainEvent> {
    events.iter().filter(|e| e.kind == kind).collect()
}

/// Idle stop through the real idle pass: graceful shutdown, the final head
/// anchored on the user chain, and a project chain that verifies and carries
/// the overlay's rule hash.
async fn idle_stop_and_anchor(w: &World, id: &str, pid: u32, paths: &RuntimePaths) {
    let future = state::now_unix() + 7200;
    assert_eq!(w.sup.idle_pass(future).await, vec![id.to_owned()], "idle pass stops the quiet project");
    wait_for("idle-stopped child gone", 30, || async { (!pid_alive(pid)).then_some(()) }).await;
    assert_eq!(w.sup.status(id).await.state, ChildState::Stopped);
    assert_eq!(w.user_events("project.kernel.idle_stop").await.len(), 1);
    let anchors = w.user_events("project.anchor").await;
    assert!(
        anchors.iter().any(|e| e.payload.as_ref().is_some_and(|p| p.to_string().contains(id))),
        "the final head was anchored on the user chain: {anchors:?}\n--- kernel.log ---\n{}",
        w.log_tail(id)
    );

    let chain = ChainManager::load_from_rvf(&paths.chain_rvf(), 1000).expect("project chain saved on clean shutdown");
    assert!(chain.verify_integrity().valid);
    let events = chain.tail_from(0);
    assert_eq!(kinds(&events, "project.genesis").len(), 1, "one genesis");
    let applied = kinds(&events, "governance.overlay.applied");
    assert!(applied.len() >= 2, "boot and reload were applied: {applied:?}");
    let hash_of = |e: &ChainEvent| e.payload.as_ref().unwrap()["effective_hash"].as_str().unwrap().to_owned();
    let (boot_hash, reload_hash) = (hash_of(applied[0]), hash_of(applied[applied.len() - 1]));
    assert_ne!(boot_hash, reload_hash, "clearing the deny changed the effective rules");
    let decided = |kind: &str| -> Vec<&ChainEvent> {
        kinds(&events, kind)
            .into_iter()
            .filter(|e| e.payload.as_ref().is_some_and(|p| p["action"] == "app.install"))
            .collect()
    };
    let denied = decided("governance.deny");
    assert_eq!(denied.len(), 1, "the denied install was chained");
    assert!(denied[0].payload.as_ref().unwrap().to_string().contains(DENY_RULE));
    let stamped = denied[0].rule_hash.map(hex::encode);
    assert_eq!(stamped.as_deref(), Some(boot_hash.as_str()), "the denial carries the overlay's rule hash");
    let permitted = decided("governance.permit");
    assert!(
        permitted.iter().any(|e| e.rule_hash.map(hex::encode).as_deref() == Some(reload_hash.as_str())),
        "the permit after reload carries the new rule hash"
    );
    let first = events.iter().position(|e| e.kind == "governance.overlay.applied").unwrap();
    assert!(events[first + 1..].iter().all(|e| e.rule_hash.is_some()), "every later event is stamped");
    assert!(!kinds(&events, "project.anchored").is_empty(), "two-way link on the project chain");
}

/// Three kills: the first two are restarted (backoff 1 s, 2 s), the third
/// spends the budget (`restart_max = 2`): `failed`, no automatic start.
async fn three_kills(w: &World, id: &str, mut pid: u32) {
    let spawns = w.sup.launcher().spawn_count();
    for round in 1..=3 {
        assert!(is_ours(pid), "round {round}: pid {pid} is one of our kernels");
        kill9(pid);
        if round < 3 {
            let old = pid;
            pid = wait_for(&format!("restart after kill {round}"), BOOT_SECS, || async {
                let st = w.sup.status(id).await;
                (st.state == ChildState::Running).then_some(st.pid).flatten().filter(|p| *p != old)
            })
            .await;
        }
    }
    wait_for("failed after the third kill", 30, || async {
        (w.sup.status(id).await.state == ChildState::Failed).then_some(())
    })
    .await;
    assert_eq!(w.sup.launcher().spawn_count(), spawns + 2, "two restarts, then none");
    assert_eq!(w.user_events("project.kernel.failed").await.len(), 1);
    assert!(w.user_events("project.kernel.restarted").await.len() >= 2);
    let delays: Vec<u64> = w
        .user_events("project.kernel.exited")
        .await
        .iter()
        .filter_map(|e| e.payload.as_ref()?["restart_in_ms"].as_u64())
        .collect();
    assert_eq!(delays, [1000, 2000], "production backoff 1 s doubling");
    let r = w.call("project.ensure_running", json!({"id": id})).await;
    assert_eq!(r["error_kind"], "project_failed", "no automatic start: {r}");
    assert_eq!(w.sup.launcher().spawn_count(), spawns + 2);
}

/// Revoke: the child is stopped, the marker is where writer, supervisor and
/// child all look, and the project never starts again.
async fn revoke(w: &World, id: &str, root: &Path, pid: u32) {
    ok(&w.call("project.revoke", json!({"id": id, "reason": "e2e"})).await, "project.revoke");
    wait_for("revoked child gone", 30, || async { (!pid_alive(pid)).then_some(()) }).await;
    let marker = revoked_marker(&w.dirs.run_root, id).unwrap();
    assert!(marker.is_file(), "marker at {}", marker.display());
    let child = RuntimePaths::child_at(w.sup.run_dir(id), id, root).unwrap();
    assert_eq!(child.revoked_marker(), marker, "the child checks the same file");
    assert!(state::is_marked_revoked(&w.sup.config().run_root, id));
    assert!(!legacy_marker(&w.dirs, id).exists());
    let r = w.call("project.ensure_running", json!({"id": id})).await;
    assert_eq!(r["error_kind"], "project_revoked", "revoke is terminal: {r}");
    assert_eq!(w.sup.status(id).await.state, ChildState::Failed);
}
