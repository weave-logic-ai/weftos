//! The lifecycle RPCs on the real dispatch path: capability checks, name
//! resolution, the JSON shapes, and `project.token.refresh`. One test: it
//! installs the process-wide supervisor.

use std::sync::Arc;
use std::time::Duration;

use clawft_kernel::boot::Kernel;
use clawft_platform::NativePlatform;
use clawft_types::config::{ChainConfig, Config, KernelConfig};
use clawft_types::project::spawn::SpawnFile;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{RwLock, watch};

use super::fixture::Fixture;

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
        deps.cert_env.chain = Arc::clone(kernel.read().await.chain_manager().unwrap());
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
        let token = SpawnFile::read(&fx.run_dir().join("spawn.seen.json")).unwrap().project_token;
        for m in ["project.start", "project.stop_all", "project.restart", "kernel.shutdown"] {
            assert!(denied(&call(&sock, m, json!({"id": fx.id}), Some(&token)).await), "{m} with a project token");
        }
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
    });
}
