//! The lifecycle RPCs on the real dispatch path: capability checks, name
//! resolution, the JSON shapes, and `project.token.refresh`. One test: it
//! installs the process-wide supervisor.

use std::sync::Arc;
use std::time::Duration;

use clawft_kernel::boot::Kernel;
use clawft_platform::NativePlatform;
use clawft_types::config::{ChainConfig, Config, KernelConfig};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{RwLock, watch};

use super::fixture::{self, Fixture};

async fn call(sock: &std::path::Path, method: &str, params: Value, auth: Option<&str>) -> Value {
    let (r, mut w) = UnixStream::connect(sock).await.unwrap().into_split();
    let mut req = json!({ "id": "t", "method": method, "params": params });
    if let Some(a) = auth {
        req["auth"] = json!(a);
    }
    w.write_all(format!("{req}\n").as_bytes()).await.unwrap();
    let mut line = String::new();
    BufReader::new(r).read_line(&mut line).await.unwrap();
    serde_json::from_str(line.trim()).unwrap()
}

fn denied(v: &Value) -> bool {
    v["ok"] == false && v["error"].as_str().unwrap_or("").contains("permission denied")
}

pub fn lifecycle_rpc_end_to_end() {
    let fx = Fixture::new();
    fx.behavior("serve");
    super::rt().block_on(async {
        // A real kernel behind the real dispatch path.
        let kcfg = KernelConfig {
            chain: Some(ChainConfig::isolated_in(&tempfile::tempdir().unwrap().keep())),
            ..KernelConfig::default()
        };
        let kernel = Kernel::boot(Config::default(), kcfg, Arc::new(NativePlatform::new())).await.unwrap();
        let kernel = Arc::new(RwLock::new(kernel));
        let sock = fx.tmp.path().join("daemon.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        let (tx, _rx) = watch::channel(false);
        let (k, t) = (Arc::clone(&kernel), tx.clone());
        tokio::spawn(async move {
            while let Ok((s, _)) = listener.accept().await {
                tokio::spawn(clawft_weave::daemon::handle_connection_peer(s, Arc::clone(&k), t.clone(), false));
            }
        });
        tokio::time::sleep(Duration::from_millis(30)).await;

        // The supervisor shares the daemon's chain and token authority, as
        // `post_boot` wires it.
        let authority = clawft_weave::token_rpc::authority_for(&kernel).await.unwrap();
        let mut deps = fx.deps();
        let chain = Arc::clone(kernel.read().await.chain_manager().unwrap());
        // The daemon signs certificates with its chain key: the supervisor
        // must trust exactly that one.
        let user_key = chain.signing_key_clone().expect("the isolated chain has a signing key");
        deps.cert_env.chain = Arc::clone(&chain);
        deps.cert_env.user_key = user_key.clone();
        deps.tokens = Some(Arc::clone(&authority));
        let sup = clawft_weave::project_supervisor::Supervisor::new(fx.cfg(), deps);
        assert!(clawft_weave::project_supervisor::install_global(sup));

        // Admin (the local owner's literal scope) only.
        for m in ["project.start", "project.ensure_running", "project.stop", "project.restart", "project.status", "project.stop_all"] {
            assert!(denied(&call(&sock, m, json!({"id": fx.id}), None).await), "{m} anonymous");
            assert!(denied(&call(&sock, m, json!({"id": fx.id}), Some("write")).await), "{m} write scope");
        }
        // Status with no id lists children (none yet).
        let r = call(&sock, "project.status", json!({}), Some("admin")).await;
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(r["result"]["children"], json!([]));
        // Bad and unknown ids.
        let r = call(&sock, "project.start", json!({"id": "nope"}), Some("admin")).await;
        assert_eq!(r["error_kind"], "project_not_found", "{r}");
        let r = call(&sock, "project.start", json!({}), Some("admin")).await;
        assert_eq!(r["error_kind"], "invalid_params");

        // Start by registered NAME; idempotent.
        let r = call(&sock, "project.ensure_running", json!({"id": "demo"}), Some("admin")).await;
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(r["result"]["project_id"], fx.id.as_str());
        assert_eq!(r["result"]["started"], true);
        let pid = r["result"]["pid"].as_u64().unwrap();
        assert_eq!(r["result"]["socket"], fx.run_dir().join("kernel.sock").display().to_string());
        let again = call(&sock, "project.start", json!({"id": fx.id}), Some("admin")).await;
        assert_eq!((again["result"]["started"].clone(), again["result"]["pid"].as_u64()), (json!(false), Some(pid)));
        let st = call(&sock, "project.status", json!({"id": fx.id}), Some("admin")).await;
        assert_eq!((st["result"]["state"].as_str(), st["result"]["pid"].as_u64()), (Some("running"), Some(pid)));
        let all = call(&sock, "project.status", json!({}), Some("admin")).await;
        assert_eq!(all["result"]["children"][0]["project_id"], fx.id.as_str());

        // The child's project token: Write only. It cannot run Admin
        // methods, but it can renew itself, and only itself.
        let token = fixture::seen_spawn(&fx.run_dir()).project_token.unwrap();
        for m in ["project.start", "project.stop_all", "project.restart", "kernel.shutdown"] {
            let r = call(&sock, m, json!({"id": fx.id}), Some(&token)).await;
            assert!(denied(&r) || r["error_kind"] == "project_token_method_denied", "{m} with a project token: {r}");
        }
        // Write methods outside the allow-list are refused before they run.
        for m in ["agent.list", "cron.add", "agent.spawn", "workload.place", "chain.tail", "mesh.register"] {
            let r = call(&sock, m, json!({}), Some(&token)).await;
            assert_eq!(r["error_kind"], "project_token_method_denied", "{m}: {r}");
        }
        let r = call(&sock, "kernel.handshake", json!({}), Some(&token)).await;
        assert_eq!(r["ok"], true, "the link's probe is allowed: {r}");
        let r = call(&sock, "project.token.refresh", json!({"id": fx.id}), Some(&token)).await;
        assert_eq!(r["ok"], true, "{r}");
        let fresh = r["result"]["token"].as_str().unwrap().to_owned();
        assert!(fresh.starts_with("wft_") && fresh != token);
        assert!(r["result"]["expires_at"].as_str().is_some());
        // Anonymous, literal scope and another project's id are refused.
        let r = call(&sock, "project.token.refresh", json!({"id": fx.id}), None).await;
        assert_eq!(r["ok"], false, "{r}");
        let r = call(&sock, "project.token.refresh", json!({"id": fx.id}), Some("write")).await;
        assert_eq!(r["error_kind"], "token_required", "{r}");
        let other = clawft_types::project::new_id();
        let r = call(&sock, "project.token.refresh", json!({"id": other}), Some(&fresh)).await;
        assert_eq!(r["error_kind"], "token_refresh_refused", "{r}");

        // Stop, restart, stop_all.
        let r = call(&sock, "project.stop", json!({"id": fx.id}), Some("admin")).await;
        assert_eq!(r["result"]["stopped"], true, "{r}");
        let r = call(&sock, "project.stop", json!({"id": fx.id}), Some("admin")).await;
        assert_eq!(r["result"]["stopped"], false, "stopping twice is harmless");
        let r = call(&sock, "project.restart", json!({"id": fx.id}), Some("admin")).await;
        assert_eq!(r["result"]["started"], true, "{r}");
        let r = call(&sock, "project.stop_all", json!({}), Some("admin")).await;
        assert_eq!(r["result"]["stopped"], json!([fx.id]), "{r}");

        identity_changes(&fx, &sock, &chain, &user_key).await;
    });
}

/// The REAL `project.rekey` / `project.revoke` RPCs against a certified
/// project with a running child: rekey stops the child and writes no marker;
/// revoke writes the terminal marker at exactly the path the child checks and
/// the project is never respawned.
async fn identity_changes(
    fx: &Fixture,
    sock: &std::path::Path,
    chain: &Arc<clawft_kernel::chain::ChainManager>,
    user_key: &ed25519_dalek::SigningKey,
) {
    use clawft_kernel::project_identity as ident;
    use clawft_types::project::cert::{PopOp, key_id};
    use clawft_weave::project_cert_rpc::{CertEnv, RegisterRequest, SpawnInfo, claim_nonce, issue_challenge, register, root_sha256};
    use clawft_weave::project_supervisor::child::pid_alive;

    // The cert RPCs only run on the user daemon. Enter it with this fixture's
    // run root, so even without the installed supervisor nothing resolves
    // the real `~/.weftos/run`.
    clawft_weave::user_daemon::enter_at(&fx.run_root);
    clawft_weave::project_rpc::init_manifests_dir(fx.mdir.clone());
    let env = CertEnv { chain: Arc::clone(chain), user_key: user_key.clone(), manifests_dir: fx.mdir.clone() };
    let ukid = key_id(&user_key.verifying_key().to_bytes());
    let k1 = ed25519_dalek::SigningKey::from_bytes(&[11u8; 32]);
    let n = issue_challenge(&fx.id).unwrap();
    register(
        &env,
        RegisterRequest {
            project_id: fx.id.clone(),
            project_pubkey: k1.verifying_key().to_bytes(),
            root_sha256: root_sha256(&fx.root),
            spawn: SpawnInfo { pid: 1, exe_sha: "ab".repeat(32) },
            pop_sig: ident::pop_sign(&k1, PopOp::Register, &ukid, &n, &fx.id).unwrap(),
            nonce: claim_nonce(&n, &fx.id).unwrap(),
        },
        chrono::Utc::now(),
    )
    .unwrap();

    let r = call(sock, "project.ensure_running", json!({"id": fx.id}), Some("admin")).await;
    assert_eq!(r["ok"], true, "{r}");
    let pid1 = r["result"]["pid"].as_u64().unwrap() as u32;

    // Rekey: the old child is stopped, NO marker, and it starts again.
    let k2 = ed25519_dalek::SigningKey::from_bytes(&[12u8; 32]);
    let n = issue_challenge(&fx.id).unwrap();
    let r = call(
        sock,
        "project.rekey",
        json!({"id": fx.id, "new_pubkey": ident::hex(&k2.verifying_key().to_bytes()), "nonce": n,
               "pop_sig": ident::hex(&ident::pop_sign(&k2, PopOp::Rekey, &ukid, &n, &fx.id).unwrap()),
               "reason": "rotate"}),
        Some("admin"),
    )
    .await;
    assert_eq!(r["ok"], true, "{r}");
    assert!(!pid_alive(pid1), "the old child was stopped");
    assert!(!fx.run_dir().join("revoked").exists(), "rekey must not write the marker");
    let r = call(sock, "project.ensure_running", json!({"id": fx.id}), Some("admin")).await;
    assert_eq!(r["ok"], true, "a rekeyed project starts normally: {r}");
    let pid2 = r["result"]["pid"].as_u64().unwrap() as u32;

    // Repair of a corrupt identity journal may change what a project's key
    // is: its child is stopped (and starts again on demand).
    std::fs::write(fx.mdir.join(clawft_kernel::project_identity::JOURNAL_FILE), "garbage\nmore garbage\n").unwrap();
    let r = call(sock, "project.identity.repair", json!({}), Some("admin")).await;
    assert_eq!(r["ok"], true, "{r}");
    assert!(r["result"]["key_changes"].as_array().is_some_and(|c| c.iter().any(|x| x["id"] == fx.id.as_str())), "{r}");
    wait_gone(pid2).await;
    assert!(!fx.run_dir().join("revoked").exists());
    let r = call(sock, "project.ensure_running", json!({"id": fx.id}), Some("admin")).await;
    assert_eq!(r["ok"], true, "{r}");
    let pid2 = r["result"]["pid"].as_u64().unwrap() as u32;

    // Revoke: terminal marker at the exact path the child checks.
    let r = call(sock, "project.revoke", json!({"id": fx.id, "reason": "test"}), Some("admin")).await;
    assert_eq!(r["ok"], true, "{r}");
    let child_paths = clawft_types::runtime_paths::RuntimePaths::child_with(&fx.home, &fx.id, &fx.root).unwrap();
    let marker = child_paths.root().join("revoked");
    assert_eq!(marker, fx.run_dir().join("revoked"), "the supervisor's run dir is the child's");
    assert!(marker.exists(), "revoke writes the marker");
    use std::os::unix::fs::PermissionsExt as _;
    assert_eq!(std::fs::metadata(&marker).unwrap().permissions().mode() & 0o077, 0);
    wait_gone(pid2).await;
    let r = call(sock, "project.ensure_running", json!({"id": fx.id}), Some("admin")).await;
    assert_eq!(r["error_kind"], "project_revoked", "revoke is terminal: {r}");
    clawft_types::runtime_paths::set_user_profile(false);
}

async fn wait_gone(pid: u32) {
    use clawft_weave::project_supervisor::child::pid_alive;
    for _ in 0..500 {
        if !pid_alive(pid) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("pid {pid} still alive");
}
