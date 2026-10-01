//! Integration tests for the per-user daemon (ADR-103 Phase 1, package D):
//! the handshake inside `kernel.status`, the `project.*` RPCs and their
//! capabilities, and the one-user-daemon-per-uid lock.
//!
//! Nothing here touches the real HOME: the runtime root is a tempdir via
//! `WEFTOS_RUNTIME_DIR`, the chain is isolated, and the manifest store is a
//! tempdir injected into the handlers.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Once, OnceLock};
use std::time::Duration;

use clawft_kernel::boot::Kernel;
use clawft_platform::NativePlatform;
use clawft_types::config::{AgentDefaults, AgentsConfig, Config, KernelConfig};
use clawft_types::runtime_paths::{RootSource, RuntimePaths};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::watch;

static ENV: Once = Once::new();
static SCRATCH: OnceLock<tempfile::TempDir> = OnceLock::new();

fn scratch() -> &'static Path {
    SCRATCH.get_or_init(|| tempfile::tempdir().unwrap()).path()
}

/// Point every `RuntimePaths::resolve()` at a tempdir, once per process.
fn isolate_env() {
    ENV.call_once(|| {
        let root = scratch().join("run");
        std::fs::create_dir_all(&root).unwrap();
        // SAFETY: called once before any kernel boots in this test binary.
        unsafe { std::env::set_var("WEFTOS_RUNTIME_DIR", &root) };
    });
}

fn manifests() -> PathBuf {
    let dir = scratch().join("projects");
    clawft_weave::project_rpc::init_manifests_dir(dir.clone());
    dir
}

fn base_config() -> Config {
    Config {
        agents: AgentsConfig {
            defaults: AgentDefaults {
                workspace: "~/.clawft/workspace".into(),
                model: "test/model".into(),
                max_tokens: 1024,
                temperature: 0.5,
                max_tool_iterations: 5,
                memory_window: 10,
            },
            ..AgentsConfig::default()
        },
        ..Config::default()
    }
}

async fn spawn_test_daemon() -> (PathBuf, watch::Sender<bool>) {
    isolate_env();
    let tmp = tempfile::tempdir().unwrap().keep();
    let socket_path = tmp.join("kernel.sock");
    let kernel_config = KernelConfig {
        chain: Some(clawft_types::config::ChainConfig::isolated_in(&tmp)),
        ..KernelConfig::default()
    };
    let kernel = Kernel::boot(base_config(), kernel_config, Arc::new(NativePlatform::new()))
        .await
        .expect("kernel boot");
    let kernel = Arc::new(tokio::sync::RwLock::new(kernel));
    let listener = UnixListener::bind(&socket_path).unwrap();
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let tx = shutdown_tx.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                accepted = listener.accept() => match accepted {
                    Ok((stream, _)) => {
                        tokio::spawn(clawft_weave::daemon::handle_connection(
                            stream, Arc::clone(&kernel), tx.clone()));
                    }
                    Err(_) => break,
                },
                _ = shutdown_rx.changed() => if *shutdown_rx.borrow() { break; },
            }
        }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    (socket_path, shutdown_tx)
}

async fn call(socket: &Path, method: &str, params: Value, auth: &str) -> Value {
    let stream = UnixStream::connect(socket).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let req = json!({ "id": "t", "method": method, "params": params, "auth": auth });
    let mut line = serde_json::to_string(&req).unwrap();
    line.push('\n');
    writer.write_all(line.as_bytes()).await.unwrap();
    let mut ack = String::new();
    reader.read_line(&mut ack).await.unwrap();
    serde_json::from_str(ack.trim()).unwrap()
}

/// Resets the process-wide user profile when a test ends, pass or fail.
struct ProfileGuard;

impl Drop for ProfileGuard {
    fn drop(&mut self) {
        clawft_weave::user_daemon::leave();
    }
}

#[tokio::test]
async fn kernel_status_carries_the_handshake_default_then_user_profile() {
    let (socket, _shutdown) = spawn_test_daemon().await;

    // Default daemon: a handshake, but no profile, roles or user fields.
    let r = call(&socket, "kernel.status", Value::Null, "admin").await;
    assert_eq!(r["ok"], true, "{r}");
    let h = &r["result"]["handshake"];
    assert_eq!(h["node_id"].as_str().map(str::len), Some(32), "{h}");
    assert_eq!(h["pid"], std::process::id());
    assert_eq!(h["profile"], Value::Null);
    assert_eq!(h["roles"], json!([]));
    assert_eq!(h["user_key_id"], Value::Null);
    // kernel.handshake stays as it was and agrees.
    let hs = call(&socket, "kernel.handshake", Value::Null, "read").await;
    assert_eq!(hs["result"]["node_id"], h["node_id"]);

    // User profile: profile, roles, the local uid and the chain-key id.
    clawft_weave::user_daemon::enter();
    let _guard = ProfileGuard;
    let r = call(&socket, "kernel.status", Value::Null, "admin").await;
    let h = &r["result"]["handshake"];
    assert_eq!(h["profile"], "user", "{h}");
    assert_eq!(h["roles"], json!(["machine", "user"]));
    assert!(h["user_id"].as_str().is_some_and(|u| u.parse::<u32>().is_ok()), "{h}");
    let key = h["user_key_id"].as_str().expect("user_key_id from the chain key");
    assert_eq!(key.len(), 32);
    assert_ne!(Some(key), h["node_id"].as_str(), "user key is not the node key");
    let hs = call(&socket, "kernel.handshake", Value::Null, "read").await;
    assert_eq!(hs["result"]["user_key_id"], h["user_key_id"]);
}

#[tokio::test]
async fn project_rpcs_register_list_show_with_capabilities() {
    let mdir = manifests();
    let (socket, _shutdown) = spawn_test_daemon().await;
    let proj = tempfile::tempdir().unwrap();

    let empty = call(&socket, "project.list", Value::Null, "read").await;
    assert_eq!(empty["ok"], true, "{empty}");
    assert_eq!(empty["result"]["projects"], json!([]));

    // register is Admin: a read-only caller is refused and nothing is written.
    let denied = call(&socket, "project.register", json!({"root": proj.path()}), "read").await;
    assert_eq!(denied["ok"], false, "{denied}");
    assert!(!proj.path().join(".weftos/project.toml").exists());

    let reg = call(&socket, "project.register", json!({"root": proj.path(), "name": "demo"}), "admin").await;
    assert_eq!(reg["ok"], true, "{reg}");
    let id = reg["result"]["project"]["id"].as_str().unwrap().to_owned();
    assert!(mdir.join(format!("{id}.toml")).is_file());

    let list = call(&socket, "project.list", Value::Null, "read").await;
    assert_eq!(list["result"]["projects"][0]["id"], id.as_str());
    let show = call(&socket, "project.show", json!({"id": id}), "read").await;
    assert_eq!(show["result"]["project"]["name"], "demo");
    let missing = call(&socket, "project.show", json!({"id": "01J0000000000000000000000A"}), "read").await;
    assert_eq!(missing["error_kind"], "project_not_found");
}

#[test]
fn a_second_user_daemon_is_refused_naming_the_holder() {
    let t = tempfile::tempdir().unwrap();
    let paths = RuntimePaths::user_with(t.path().join("run").to_str(), None);
    assert_eq!(paths.source(), &RootSource::User);
    let first = clawft_weave::instance_lock::InstanceLock::acquire(&paths).expect("first");
    let err = clawft_weave::instance_lock::InstanceLock::acquire(&paths).expect_err("second");
    assert!(err.to_string().contains(&format!("pid {}", std::process::id())), "{err}");
    drop(first);
    clawft_weave::instance_lock::InstanceLock::acquire(&paths).expect("free after drop");
}

#[test]
fn user_root_is_under_home_weftos_run_and_never_a_project() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    let p = RuntimePaths::user_with(None, Some(&home));
    assert_eq!(p.root(), home.join(".weftos/run"));
    assert_eq!(p.socket(), home.join(".weftos/run/kernel.sock"));
}
