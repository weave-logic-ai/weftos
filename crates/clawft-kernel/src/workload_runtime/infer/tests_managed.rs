//! Managed llama.cpp / mlx-lm: the adapter launches a configured launcher
//! (here a fake script in a temp dir) with argv built from the spec and the
//! registry-resolved model, probes it, restarts it, and stops only the
//! process it spawned. The "server" is a fake HTTP server on a random
//! loopback port brought up after the script starts.

use std::sync::Arc;
use std::time::Duration;

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::config::RestartPolicy;
use super::fakes::*;
use super::lifecycle::Reconcile;
use super::spec::InferFlavor;
use crate::chain::ChainManager;
use crate::model_manifest::ModelFormat;
use crate::workload_governance::{NetworkPolicy, NodeTrustTier, WorkloadGate};
use crate::workload_runtime::host::{RUNTIME_CHAIN_SOURCE, WorkloadHost};
use crate::workload_runtime::types::{ControlMode, InstanceState, RuntimeError, WorkloadRuntime};

const NAME: &str = "Llama-3.2-1B";

fn fast() -> RestartPolicy {
    RestartPolicy {
        max_restarts: 2,
        base: Duration::from_millis(300),
        cap: Duration::from_secs(1),
    }
}

fn llama() -> Managed {
    let m = managed(InferFlavor::LlamaCpp, fast());
    adopt_model(&m.reg, m.tmp.path(), NAME, ModelFormat::Gguf);
    m
}

fn llama_spec(port: u16) -> super::InferenceSpec {
    let mut s = spec("small", InferFlavor::LlamaCpp, port);
    s.model = Some(NAME.into());
    s
}

#[tokio::test]
async fn launches_with_spec_args_and_reports_loading_as_degraded_until_ready() {
    let m = llama();
    adopt_model(&m.reg, m.tmp.path(), "Tiny-Draft", ModelFormat::Gguf);
    let port = free_port();
    let mut s = llama_spec(port);
    s.serve.ctx = Some(4096);
    s.serve.kv_quant = Some("q8_0".into());
    s.serve.draft_model = Some("Tiny-Draft".into());
    s.serve.extra_args = vec!["--threads".into(), "2".into()];
    let h = load(&m.rt, s).await;
    assert_eq!(m.rt.control_mode(), ControlMode::Managed);
    assert_eq!(m.rt.network_exposure(), NetworkPolicy::None);
    assert_eq!(m.rt.status(&h).await.state, InstanceState::Loaded);

    m.rt.start(&h).await.unwrap();
    let pid = script_pid(&m.script_dir).await;
    let st = m.rt.status(&h).await;
    assert_eq!(st.state, InstanceState::Degraded, "{st:?}");
    assert!(st.detail.unwrap().starts_with("starting"));

    let a = argv(&m.script_dir);
    let model_path = m.reg.resolve(NAME).unwrap().shards[0]
        .to_str()
        .unwrap()
        .to_string();
    let draft_path = m.reg.resolve("Tiny-Draft").unwrap().shards[0]
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(
        a[0], model_path,
        "weights come from the registry, not the spec"
    );
    assert_eq!(
        a[1..].join(" "),
        format!(
            "--port {port} --host 127.0.0.1 --ctx 4096 --kv q8_0 --draft {draft_path} --threads 2"
        )
    );

    let server = server_on(port).await;
    mount_llama(&server, 503, NAME).await;
    let st = m.rt.status(&h).await;
    assert_eq!(st.detail.as_deref(), Some("loading model"), "{st:?}");
    server.reset().await;
    mount_llama(&server, 200, NAME).await;
    assert_eq!(m.rt.status(&h).await.state, InstanceState::Running);
    assert!(alive(pid));
    m.rt.unload(h).await.unwrap();
}

#[tokio::test]
async fn deep_health_asks_for_one_token() {
    let m = llama();
    let port = free_port();
    let h = load(&m.rt, llama_spec(port)).await;
    m.rt.start(&h).await.unwrap();
    let server = server_on(port).await;
    mount_llama(&server, 200, NAME).await;
    assert!(
        m.rt.deep_health(&h).await.is_err(),
        "no completion route yet"
    );
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"choices": []})))
        .mount(&server)
        .await;
    m.rt.deep_health(&h).await.unwrap();
    let req = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.method.as_str() == "POST")
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
    assert_eq!(body["max_tokens"], 1);
    m.rt.unload(h).await.unwrap();
}

#[tokio::test]
async fn stop_terminates_only_the_process_it_started_and_returns_evidence() {
    let m = llama();
    let port = free_port();
    let h = load(&m.rt, llama_spec(port)).await;
    m.rt.start(&h).await.unwrap();
    let pid = script_pid(&m.script_dir).await;
    assert!(alive(pid));
    let ev = m.rt.stop(&h, Duration::from_secs(3)).await.unwrap();
    assert_eq!(ev.runtime, "infer.llamacpp");
    assert_eq!(ev.instance_id, h.instance_id);
    assert!(ev.signal.is_some() || ev.exit_code.is_some(), "{ev:?}");
    assert!(!alive(pid), "the launcher is gone");
    assert_eq!(m.rt.status(&h).await.state, InstanceState::Exited);
    assert!(matches!(
        m.rt.stop(&h, Duration::from_secs(1)).await,
        Err(RuntimeError::InvalidState(_))
    ));
    m.rt.unload(h).await.unwrap();
}

#[tokio::test]
async fn unload_stops_a_running_server_and_removes_its_directory() {
    let m = llama();
    let port = free_port();
    let h = load(&m.rt, llama_spec(port)).await;
    m.rt.start(&h).await.unwrap();
    let pid = script_pid(&m.script_dir).await;
    let dir = m.data.join(&h.instance_id);
    assert!(dir.is_dir());
    m.rt.unload(h.clone()).await.unwrap();
    assert!(!alive(pid));
    assert!(!dir.exists());
    assert_eq!(m.rt.status(&h).await.state, InstanceState::Unknown);
}

#[tokio::test]
async fn it_refuses_to_manage_over_a_server_already_on_the_port() {
    let m = llama();
    let taken = MockServer::start().await;
    let e =
        m.rt.admit(&workload(llama_spec(port_of(&taken))))
            .await
            .unwrap_err();
    assert!(
        matches!(e, RuntimeError::AdmissionRefused(ref s) if s.contains("already listening")),
        "{e:?}"
    );
    // The check is a TCP connect: no HTTP request reached the other server.
    assert!(taken.received_requests().await.unwrap().is_empty());
    assert!(
        !m.script_dir.join("pid.txt").exists(),
        "nothing was launched"
    );

    // A port taken between load and start is refused at start too.
    let port = free_port();
    let h = load(&m.rt, llama_spec(port)).await;
    let _squatter = server_on(port).await;
    let e = m.rt.start(&h).await.unwrap_err();
    assert!(
        matches!(e, RuntimeError::Backend(ref s) if s.contains("already taken")),
        "{e:?}"
    );
    assert!(!m.script_dir.join("pid.txt").exists());
}

#[tokio::test]
async fn an_unregistered_or_tampered_model_is_refused_before_anything_launches() {
    let m = llama();
    let mut s = llama_spec(free_port());
    s.model = Some("Never-Adopted".into());
    let e = m.rt.admit(&workload(s)).await.unwrap_err();
    assert!(matches!(e, RuntimeError::AdmissionRefused(_)), "{e:?}");

    // Change the adopted weights on disk: the lazy re-hash refuses them.
    let dir = m.tmp.path().join(NAME);
    std::fs::write(dir.join(format!("{NAME}.gguf")), vec![8u8; 4096]).unwrap();
    let e =
        m.rt.admit(&workload(llama_spec(free_port())))
            .await
            .unwrap_err();
    assert!(
        matches!(e, RuntimeError::AdmissionRefused(ref s) if s.contains("refused") || s.contains("degraded")),
        "{e:?}"
    );
    assert!(!m.script_dir.join("pid.txt").exists());
}

#[tokio::test]
async fn a_model_in_the_wrong_format_is_refused() {
    let m = managed(InferFlavor::LlamaCpp, fast());
    adopt_model(&m.reg, m.tmp.path(), "Mlx-Model", ModelFormat::Mlx);
    let mut s = spec("r", InferFlavor::LlamaCpp, free_port());
    s.model = Some("Mlx-Model".into());
    let e = m.rt.admit(&workload(s)).await.unwrap_err();
    assert!(
        matches!(e, RuntimeError::AdmissionRefused(ref s) if s.contains("cannot load a mlx")),
        "{e:?}"
    );
}

#[tokio::test]
async fn managed_mlx_passes_the_model_directory() {
    let m = managed(InferFlavor::MlxLm, fast());
    let dir = adopt_model(&m.reg, m.tmp.path(), "Quant-4bit", ModelFormat::Mlx);
    let port = free_port();
    let mut s = spec("coder-daily", InferFlavor::MlxLm, port);
    s.model = Some("Quant-4bit".into());
    let h = load(&m.rt, s).await;
    m.rt.start(&h).await.unwrap();
    script_pid(&m.script_dir).await;
    let a = argv(&m.script_dir);
    assert_eq!(a[0], dir.canonicalize().unwrap().to_str().unwrap());
    assert_eq!(a[1..].join(" "), format!("--port {port} --host 127.0.0.1"));
    m.rt.unload(h).await.unwrap();
}

#[tokio::test]
async fn a_crashed_server_is_restarted_with_backoff_then_given_up_on() {
    let m = llama();
    let port = free_port();
    let h = load(&m.rt, llama_spec(port)).await;
    m.rt.start(&h).await.unwrap();
    let pid1 = script_pid(&m.script_dir).await;
    assert_eq!(m.rt.reconcile(&h).await.unwrap(), Reconcile::Healthy);

    crash(pid1);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let st = m.rt.status(&h).await;
    assert_eq!(st.state, InstanceState::Exited, "{st:?}");
    assert!(matches!(
        m.rt.reconcile(&h).await.unwrap(),
        Reconcile::Backoff { .. }
    ));
    std::fs::remove_file(m.script_dir.join("pid.txt")).unwrap();
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert_eq!(
        m.rt.reconcile(&h).await.unwrap(),
        Reconcile::Restarted { attempt: 1 }
    );
    let pid2 = script_pid(&m.script_dir).await;
    assert_ne!(pid1, pid2);

    crash(pid2);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(matches!(
        m.rt.reconcile(&h).await.unwrap(),
        Reconcile::Backoff { .. }
    ));
    std::fs::remove_file(m.script_dir.join("pid.txt")).unwrap();
    tokio::time::sleep(Duration::from_millis(650)).await;
    assert_eq!(
        m.rt.reconcile(&h).await.unwrap(),
        Reconcile::Restarted { attempt: 2 }
    );
    let pid3 = script_pid(&m.script_dir).await;
    crash(pid3);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        m.rt.reconcile(&h).await.unwrap(),
        Reconcile::GaveUp { attempts: 2 }
    );
    assert_eq!(m.rt.status(&h).await.state, InstanceState::Exited);
    m.rt.unload(h).await.unwrap();
}

#[tokio::test]
async fn an_operator_stop_is_not_a_fault_and_restart_replaces_the_process() {
    let m = llama();
    let port = free_port();
    let h = load(&m.rt, llama_spec(port)).await;
    m.rt.start(&h).await.unwrap();
    let pid1 = script_pid(&m.script_dir).await;
    m.rt.stop(&h, Duration::from_secs(3)).await.unwrap();
    assert_eq!(
        m.rt.reconcile(&h).await.unwrap(),
        Reconcile::Healthy,
        "stopped on purpose"
    );
    assert!(!alive(pid1));
    std::fs::remove_file(m.script_dir.join("pid.txt")).unwrap();
    m.rt.restart(&h, Duration::from_secs(3)).await.unwrap();
    let pid2 = script_pid(&m.script_dir).await;
    assert_ne!(pid1, pid2);
    assert!(alive(pid2));
    m.rt.unload(h).await.unwrap();
}

#[tokio::test]
async fn two_instances_cannot_share_a_port_and_a_second_start_is_refused() {
    let m = llama();
    let port = free_port();
    let h = load(&m.rt, llama_spec(port)).await;
    let mut other = llama_spec(port);
    other.role = "other".into();
    assert!(m.rt.load(&workload(other), &wl_cfg()).await.is_err());
    m.rt.start(&h).await.unwrap();
    assert!(matches!(
        m.rt.start(&h).await,
        Err(RuntimeError::InvalidState(_))
    ));
    m.rt.unload(h).await.unwrap();
}

#[tokio::test]
async fn capabilities_come_from_the_launcher_and_a_version_probe() {
    let m = llama();
    assert!(m.rt.provides().is_empty());
    let caps = m.rt.probe_capabilities().await;
    let ids: Vec<&str> = caps.iter().map(|c| c.id.as_str()).collect();
    assert!(
        ids.contains(&"runtime.infer.llamacpp") && ids.contains(&"format.gguf"),
        "{ids:?}"
    );

    // A launcher that is not executable is not a runtime.
    let tmp = tempfile::tempdir().unwrap();
    let reg = Arc::new(crate::model_manifest::ModelRegistry::in_memory());
    let cfg = super::ManagedConfig::new(reg, tmp.path().join("d"))
        .with_serve_program(tmp.path().join("missing"));
    let rt = super::InferRuntime::new(super::InferConfig::managed(InferFlavor::LlamaCpp, cfg));
    assert!(rt.probe_capabilities().await.is_empty());

    // A version probe that fails is a missing runtime; one that works is recorded.
    let script = write_script(tmp.path());
    let mut cfg = super::ManagedConfig::new(
        Arc::new(crate::model_manifest::ModelRegistry::in_memory()),
        tmp.path().join("d"),
    )
    .with_serve_program(&script);
    cfg.version_probe = Some((
        std::path::PathBuf::from("/bin/sh"),
        vec!["-c".into(), "echo llama-server fake-1.0".into()],
    ));
    let rt = super::InferRuntime::new(super::InferConfig::managed(
        InferFlavor::LlamaCpp,
        cfg.clone(),
    ));
    let caps = rt.probe_capabilities().await;
    assert!(!caps.is_empty(), "version probe passed");
    assert!(
        format!("{:?}", caps[0]).contains("llama-server fake-1.0"),
        "{caps:?}"
    );
    cfg.version_probe = Some((
        std::path::PathBuf::from("/bin/sh"),
        vec!["-c".into(), "exit 3".into()],
    ));
    let rt = super::InferRuntime::new(super::InferConfig::managed(InferFlavor::LlamaCpp, cfg));
    assert!(rt.probe_capabilities().await.is_empty());
}

#[tokio::test]
async fn through_the_host_a_managed_instance_is_chained_end_to_end() {
    let m = llama();
    let chain = Arc::new(ChainManager::new(0, 1000));
    let gate = WorkloadGate::exempt(0.8, false, "test")
        .with_chain(chain.clone())
        .with_permit(infer_permit())
        .unwrap();
    let host = WorkloadHost::new(
        m.rt.clone(),
        Arc::new(gate),
        "operator",
        NodeTrustTier::Paired,
    )
    .with_chain(chain.clone());
    let port = free_port();
    let h = host
        .load(&workload(llama_spec(port)), &wl_cfg())
        .await
        .unwrap();
    host.start(&h).await.unwrap();
    let pid = script_pid(&m.script_dir).await;
    host.stop(&h, Duration::from_secs(3)).await.unwrap();
    host.unload(h).await.unwrap();
    assert!(!alive(pid));
    let kinds: Vec<String> = chain
        .tail(0)
        .into_iter()
        .filter(|e| e.source == RUNTIME_CHAIN_SOURCE)
        .map(|e| e.kind)
        .collect();
    assert_eq!(
        kinds,
        [
            "workload.load",
            "workload.start",
            "workload.stop",
            "workload.unload"
        ]
    );
}

#[tokio::test]
async fn a_server_listening_beyond_loopback_is_stopped_and_chained_by_the_caller() {
    if super::exposure::local_addrs().is_empty() {
        eprintln!("no non-loopback interface address on this machine; skipping");
        return;
    }
    let m = llama();
    let port = free_port();
    let h = load(&m.rt, llama_spec(port)).await;
    m.rt.start(&h).await.unwrap();
    let pid = script_pid(&m.script_dir).await;
    // The "server" binds the wildcard address despite --host 127.0.0.1.
    let _wild = std::net::TcpListener::bind(("0.0.0.0", port)).unwrap();
    let st = m.rt.status(&h).await;
    assert_eq!(st.state, InstanceState::Exited, "{st:?}");
    assert!(
        st.detail.as_deref().unwrap().contains("beyond loopback"),
        "{st:?}"
    );
    assert!(!alive(pid), "the launcher was stopped");
    assert!(matches!(
        m.rt.reconcile(&h).await.unwrap(),
        Reconcile::StoppedExposed { ref reachable_on } if !reachable_on.is_empty()
    ));
    let e = m.rt.start(&h).await.unwrap_err();
    assert!(matches!(e, RuntimeError::InvalidState(_)), "{e:?}");
    m.rt.unload(h).await.unwrap();
}

#[tokio::test]
async fn reconcile_also_catches_an_exposed_server() {
    if super::exposure::local_addrs().is_empty() {
        return;
    }
    let m = llama();
    let port = free_port();
    let h = load(&m.rt, llama_spec(port)).await;
    m.rt.start(&h).await.unwrap();
    let pid = script_pid(&m.script_dir).await;
    let _wild = std::net::TcpListener::bind(("0.0.0.0", port)).unwrap();
    assert!(matches!(
        m.rt.reconcile(&h).await.unwrap(),
        Reconcile::StoppedExposed { .. }
    ));
    assert!(!alive(pid));
    m.rt.unload(h).await.unwrap();
}

#[tokio::test]
async fn a_loopback_only_server_is_left_alone() {
    let m = llama();
    let port = free_port();
    let h = load(&m.rt, llama_spec(port)).await;
    m.rt.start(&h).await.unwrap();
    let pid = script_pid(&m.script_dir).await;
    let server = server_on(port).await; // bound to 127.0.0.1 only
    mount_llama(&server, 200, NAME).await;
    assert_eq!(m.rt.status(&h).await.state, InstanceState::Running);
    assert_eq!(m.rt.reconcile(&h).await.unwrap(), Reconcile::Healthy);
    assert!(alive(pid));
    assert!(
        super::exposure::reachable_beyond_loopback(port)
            .await
            .is_empty()
    );
    m.rt.unload(h).await.unwrap();
}

#[tokio::test]
async fn stop_and_reap_take_the_whole_process_group() {
    let m = llama();
    write_forking_script(&m.script_dir.join("fake-serve"));
    let port = free_port();
    let h = load(&m.rt, llama_spec(port)).await;
    m.rt.start(&h).await.unwrap();
    let pid = script_pid(&m.script_dir).await;
    let gpid: i32 = std::fs::read_to_string(m.script_dir.join("gpid.txt"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(alive(pid) && alive(gpid));
    m.rt.stop(&h, Duration::from_secs(3)).await.unwrap();
    assert!(!alive(pid) && !alive(gpid), "stop left a grandchild behind");

    // A leader that dies on its own leaves no grandchild after the reap.
    std::fs::remove_file(m.script_dir.join("pid.txt")).unwrap();
    m.rt.start(&h).await.unwrap();
    let pid = script_pid(&m.script_dir).await;
    let gpid: i32 = std::fs::read_to_string(m.script_dir.join("gpid.txt"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    crash(pid);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(m.rt.status(&h).await.state, InstanceState::Exited);
    for _ in 0..40 {
        if !alive(gpid) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!alive(gpid), "the reap left a grandchild behind");

    // Dropping the adapter with a server running takes the group too.
    std::fs::remove_file(m.script_dir.join("pid.txt")).unwrap();
    m.rt.start(&h).await.unwrap();
    let pid = script_pid(&m.script_dir).await;
    let gpid: i32 = std::fs::read_to_string(m.script_dir.join("gpid.txt"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    drop(m.rt);
    for _ in 0..40 {
        if !alive(pid) && !alive(gpid) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!alive(pid) && !alive(gpid), "drop left processes behind");
}

#[tokio::test]
async fn the_callers_path_wins_over_the_supervisors_default() {
    let default = managed(InferFlavor::LlamaCpp, fast());
    adopt_model(&default.reg, default.tmp.path(), NAME, ModelFormat::Gguf);
    let h = load(&default.rt, llama_spec(free_port())).await;
    default.rt.start(&h).await.unwrap();
    script_pid(&default.script_dir).await;
    let p = std::fs::read_to_string(default.script_dir.join("path.txt")).unwrap();
    assert_eq!(
        p, "/usr/local/bin:/usr/bin:/bin",
        "nothing forwarded: the default"
    );
    default.rt.unload(h).await.unwrap();

    let custom = managed_with(InferFlavor::LlamaCpp, fast(), |c| {
        c.env
            .push(("PATH".into(), "/opt/custom/bin:/usr/bin:/bin".into()));
    });
    adopt_model(&custom.reg, custom.tmp.path(), NAME, ModelFormat::Gguf);
    let h = load(&custom.rt, llama_spec(free_port())).await;
    custom.rt.start(&h).await.unwrap();
    script_pid(&custom.script_dir).await;
    let p = std::fs::read_to_string(custom.script_dir.join("path.txt")).unwrap();
    assert_eq!(
        p, "/opt/custom/bin:/usr/bin:/bin",
        "the configured PATH wins"
    );
    custom.rt.unload(h).await.unwrap();
}
