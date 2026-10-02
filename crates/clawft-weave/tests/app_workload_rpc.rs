//! Live-daemon test for the `app.*` and `workload.*` RPC families
//! (card mesh-placement-06). Boots a real kernel, serves it on a temp
//! Unix socket through `daemon::handle_connection`, and calls every RPC
//! over the wire. Before this card each `app.*` call returned
//! `unknown method: app.*`.

#![cfg(unix)]

use std::sync::Arc;
use std::time::Duration;

use clawft_kernel::boot::Kernel;
use clawft_platform::NativePlatform;
use clawft_types::config::{ChainConfig, Config, KernelConfig};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{RwLock, watch};

type KernelRef = Arc<RwLock<Kernel<NativePlatform>>>;

const HASH: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

async fn spawn_daemon(tmp: &std::path::Path) -> (std::path::PathBuf, watch::Sender<bool>, KernelRef) {
    let socket = tmp.join("k.sock");
    let kcfg = KernelConfig {
        enabled: true,
        max_processes: 64,
        chain: Some(ChainConfig {
            checkpoint_path: Some(tmp.join("chain.json").to_string_lossy().into_owned()),
            ..ChainConfig::default()
        }),
        ..KernelConfig::default()
    };
    let kernel = Kernel::boot(Config::default(), kcfg, Arc::new(NativePlatform::new()))
        .await
        .expect("kernel boot");
    let kernel = Arc::new(RwLock::new(kernel));
    let listener = UnixListener::bind(&socket).unwrap();
    let (tx, mut rx) = watch::channel(false);
    let (k2, tx2) = (Arc::clone(&kernel), tx.clone());
    tokio::spawn(async move {
        loop {
            tokio::select! {
                r = listener.accept() => match r {
                    Ok((s, _)) => { tokio::spawn(clawft_weave::daemon::handle_connection(s, Arc::clone(&k2), tx2.clone())); }
                    Err(_) => break,
                },
                _ = rx.changed() => if *rx.borrow() { break; },
            }
        }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    (socket, tx, kernel)
}

async fn call_as(socket: &std::path::Path, auth: Option<&str>, method: &str, params: Value) -> Value {
    let stream = UnixStream::connect(socket).await.unwrap();
    let (r, mut w) = stream.into_split();
    let mut req = json!({ "id": "t", "proto": 1, "method": method, "params": params });
    if let Some(a) = auth {
        req["auth"] = json!(a);
    }
    let mut line = serde_json::to_string(&req).unwrap();
    line.push('\n');
    w.write_all(line.as_bytes()).await.unwrap();
    let mut out = String::new();
    BufReader::new(r).read_line(&mut out).await.unwrap();
    serde_json::from_str(out.trim()).unwrap()
}

async fn call(socket: &std::path::Path, method: &str, params: Value) -> Value {
    call_as(socket, Some("admin"), method, params).await
}

fn not_unknown(resp: &Value, method: &str) {
    let err = resp["error"].as_str().unwrap_or("");
    assert!(!err.contains("unknown method"), "{method} is not dispatched: {resp}");
}

async fn chain_kinds(kernel: &KernelRef) -> Vec<String> {
    let k = kernel.read().await;
    let cm = k.chain_manager().expect("chain enabled");
    cm.tail(cm.len()).into_iter().map(|e| e.kind).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn app_rpcs_round_trip_against_live_daemon() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, shutdown, kernel) = spawn_daemon(tmp.path()).await;

    // Unique name: the daemon AppManager persists to a cwd-relative file.
    let name = format!("w06-app-{}-{}", std::process::id(), chrono::Utc::now().timestamp_micros());
    let app_dir = tmp.path().join("app");
    std::fs::create_dir(&app_dir).unwrap();
    std::fs::write(
        app_dir.join("weftapp.toml"),
        format!("name = \"{name}\"\nversion = \"0.1.0\"\n\n[[agents]]\nid = \"worker\"\n"),
    )
    .unwrap();

    // Anonymous callers cannot mutate the catalog.
    let anon = call_as(&sock, None, "app.install", json!({"path": app_dir})).await;
    assert_eq!(anon["ok"], false);
    assert!(anon["error"].as_str().unwrap().contains("permission denied"), "{anon}");

    let r = call(&sock, "app.install", json!({"path": app_dir})).await;
    not_unknown(&r, "app.install");
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(r["result"], json!(name));

    let list = call(&sock, "app.list", json!({})).await;
    not_unknown(&list, "app.list");
    let row = list["result"].as_array().unwrap().iter().find(|a| a["name"] == json!(name)).cloned();
    assert_eq!(row.unwrap()["state"], "installed");

    let ins = call(&sock, "app.inspect", json!({"name": name})).await;
    not_unknown(&ins, "app.inspect");
    assert_eq!(ins["result"]["manifest"]["agents"][0]["id"], "worker");

    let st = call(&sock, "app.start", json!({"name": name})).await;
    not_unknown(&st, "app.start");
    assert_eq!(st["ok"], true, "{st}");
    let pid = st["result"]["agent_pids"][0].as_u64().expect("agent spawned");
    {
        let k = kernel.read().await;
        let entry = k.process_table().get(pid).expect("agent in process table");
        assert_eq!(entry.agent_id, format!("{name}/worker"));
    }
    let ins = call(&sock, "app.inspect", json!({"name": name})).await;
    assert_eq!(ins["result"]["state"], "Running");
    assert_eq!(ins["result"]["agent_pids"], json!([pid]));

    let sp = call(&sock, "app.stop", json!({"name": name})).await;
    not_unknown(&sp, "app.stop");
    assert_eq!(sp["ok"], true, "{sp}");
    tokio::time::sleep(Duration::from_millis(100)).await;
    {
        let k = kernel.read().await;
        let state = k.process_table().get(pid).map(|e| format!("{:?}", e.state));
        assert!(
            state.as_deref().is_none_or(|s| !s.starts_with("Running")),
            "agent still running after app.stop: {state:?}"
        );
    }
    assert!(!call(&sock, "app.stop", json!({"name": name})).await["ok"].as_bool().unwrap());

    let rm = call(&sock, "app.remove", json!({"name": name})).await;
    not_unknown(&rm, "app.remove");
    assert_eq!(rm["ok"], true, "{rm}");
    let list = call(&sock, "app.list", json!({})).await;
    assert!(!list["result"].as_array().unwrap().iter().any(|a| a["name"] == json!(name)));

    let kinds = chain_kinds(&kernel).await;
    for k in ["app.install", "app.start", "app.stop", "app.remove"] {
        assert!(kinds.iter().any(|x| x == k), "chain missing {k}: {kinds:?}");
    }
    let _ = shutdown.send(true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workload_rpcs_against_live_daemon() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, shutdown, kernel) = spawn_daemon(tmp.path()).await;

    let list = call(&sock, "workload.list", json!({})).await;
    not_unknown(&list, "workload.list");
    assert_eq!(list["ok"], true, "{list}");
    assert!(list["result"].is_array());
    // Read verbs are anonymous-callable.
    assert_eq!(call_as(&sock, None, "workload.list", json!({})).await["ok"], true);

    // Mutations need Write; anonymous is refused before the handler.
    let anon = call_as(&sock, None, "workload.install", json!({})).await;
    assert!(anon["error"].as_str().unwrap().contains("permission denied"), "{anon}");

    // Boundary validation.
    let bad = call(&sock, "workload.install", json!({"name": "../x", "kind": "cog", "manifest_hash": HASH})).await;
    assert_eq!(bad["ok"], false);
    assert!(bad["error"].as_str().unwrap().contains("invalid workload name"), "{bad}");

    // The kernel gate decides; either way list, inspect and chain agree.
    let name = format!("w06-{}", std::process::id());
    let inst = call(
        &sock,
        "workload.install",
        json!({"name": name, "kind": "cog", "manifest_hash": HASH, "version": "0.1.0"}),
    )
    .await;
    not_unknown(&inst, "workload.install");
    let listed = call(&sock, "workload.list", json!({})).await["result"].clone();
    let present = listed.as_array().unwrap().iter().any(|w| w["name"] == json!(name));
    let ins = call(&sock, "workload.inspect", json!({"name": name})).await;
    not_unknown(&ins, "workload.inspect");
    let kinds = chain_kinds(&kernel).await;
    if inst["ok"] == json!(true) {
        assert!(present, "permitted install must be listed");
        assert_eq!(ins["result"]["manifest_hash"], HASH);
        assert_eq!(ins["result"]["state"], "installed");
        assert!(kinds.iter().any(|k| k == "workload.install"), "{kinds:?}");

        let un = call(&sock, "workload.unload", json!({"name": name})).await;
        assert_eq!(un["ok"], true, "{un}");
        assert!(chain_kinds(&kernel).await.iter().any(|k| k == "workload.unload"));
        assert_eq!(call(&sock, "workload.inspect", json!({"name": name})).await["ok"], false);
    } else {
        assert!(inst["error"].as_str().unwrap().contains("governance"), "{inst}");
        assert!(!present, "refused install must not be listed");
        assert_eq!(ins["ok"], false);
        assert!(kinds.iter().any(|k| k == "workload.refuse"), "{kinds:?}");
    }

    // Placement (card 12) is routed to the control plane, which fails
    // closed here: this harness gives it no daemon key, and `{}` is not a
    // valid order anyway. Never "unknown method", never a placement.
    let place = call(&sock, "workload.place", json!({})).await;
    assert_eq!(place["ok"], false, "{place}");
    assert!(!place["error"].as_str().unwrap().starts_with("unknown method"), "{place}");
    // Verbs owned by later cards answer explicitly.
    let mig = call(&sock, "workload.migrate", json!({})).await;
    assert!(mig["error"].as_str().unwrap().contains("not available"), "{mig}");
    // An unknown workload.* verb is refused and chained (default deny).
    let bogus = call(&sock, "workload.frobnicate", json!({})).await;
    assert_eq!(bogus["ok"], false, "{bogus}");
    assert!(chain_kinds(&kernel).await.iter().any(|k| k == "workload.refuse"));
    let _ = shutdown.send(true);
}
