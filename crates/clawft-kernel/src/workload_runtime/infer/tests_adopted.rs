//! Adopted mode: a server somebody else started is registered and
//! health-checked, and never controlled. Servers are fakes on random
//! loopback ports; the real :8090, :8081 and :11434 are never contacted.

use std::sync::Arc;
use std::time::Duration;

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::fakes::*;
use super::spec::InferFlavor;
use crate::chain::ChainManager;
use crate::workload_governance::NetworkPolicy;
use crate::workload_governance::{NodeTrustTier, WorkloadGate};
use crate::workload_runtime::host::{RUNTIME_CHAIN_SOURCE, WorkloadHost};
use crate::workload_runtime::types::{ControlMode, InstanceState, RuntimeError, WorkloadRuntime};

const MODEL: &str = "Llama-3.2-1B-Instruct-Q4_K_M.gguf";

#[tokio::test]
async fn adopts_a_running_server_and_reports_it_healthy() {
    let server = MockServer::start().await;
    mount_llama(&server, 200, MODEL).await;
    let rt = adopted(InferFlavor::LlamaCpp);
    let mut s = spec("hermes", InferFlavor::LlamaCpp, port_of(&server));
    s.model = Some("Llama-3.2-1B-Instruct".into());
    let w = workload(s);

    let adm = rt.admit(&w).await.unwrap();
    assert_eq!(adm.runtime, "infer.llamacpp.adopted");
    let h = rt.load(&w, &wl_cfg()).await.unwrap();
    rt.start(&h).await.unwrap();
    let st = rt.status(&h).await;
    assert_eq!(st.state, InstanceState::Running, "{st:?}");
    assert_eq!(rt.control_mode(), ControlMode::Adopted);
    let rep = rt.health(&h).await.unwrap();
    assert!(rep.lists("Llama-3.2-1B-Instruct"));
    assert_eq!(
        rt.endpoint(&h).await.unwrap(),
        format!("http://127.0.0.1:{}", port_of(&server))
    );
}

#[tokio::test]
async fn adopting_never_sends_anything_but_reads_and_unload_only_forgets() {
    let server = MockServer::start().await;
    mount_llama(&server, 200, MODEL).await;
    let rt = adopted(InferFlavor::LlamaCpp);
    let h = load(&rt, spec("hermes", InferFlavor::LlamaCpp, port_of(&server))).await;
    rt.start(&h).await.unwrap();
    let _ = rt.status(&h).await;

    let stop = rt.stop(&h, Duration::from_secs(1)).await.unwrap_err();
    assert!(matches!(stop, RuntimeError::Unsupported(_)), "{stop:?}");
    rt.unload(h.clone()).await.unwrap();
    assert_eq!(rt.status(&h).await.state, InstanceState::Unknown);

    let reqs = server.received_requests().await.unwrap();
    assert!(!reqs.is_empty());
    assert!(
        reqs.iter().all(|r| r.method.as_str() == "GET"),
        "an adopted server only ever receives reads: {:?}",
        reqs.iter()
            .map(|r| (r.method.to_string(), r.url.path().to_string()))
            .collect::<Vec<_>>()
    );
    // Still serving after the registration is gone.
    let again = adopted(InferFlavor::LlamaCpp);
    assert!(
        again
            .admit(&workload(spec(
                "x",
                InferFlavor::LlamaCpp,
                port_of(&server)
            )))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn a_503_is_degraded_loading_model() {
    let server = MockServer::start().await;
    mount_llama(&server, 503, MODEL).await;
    let rt = adopted(InferFlavor::LlamaCpp);
    let h = load(&rt, spec("hermes", InferFlavor::LlamaCpp, port_of(&server))).await;
    let st = rt.status(&h).await;
    assert_eq!(st.state, InstanceState::Degraded);
    assert_eq!(st.detail.as_deref(), Some("loading model"));
}

#[tokio::test]
async fn a_named_model_the_server_does_not_list_is_degraded() {
    let server = MockServer::start().await;
    mount_llama(&server, 200, MODEL).await;
    let rt = adopted(InferFlavor::LlamaCpp);
    let mut s = spec("hermes", InferFlavor::LlamaCpp, port_of(&server));
    s.model = Some("Some-Other-Model".into());
    let h = load(&rt, s).await;
    let st = rt.status(&h).await;
    assert_eq!(st.state, InstanceState::Degraded, "{st:?}");
    assert!(st.detail.unwrap().contains("not listed"));
}

#[tokio::test]
async fn no_server_means_admission_is_refused() {
    let rt = adopted(InferFlavor::LlamaCpp);
    let w = workload(spec("hermes", InferFlavor::LlamaCpp, free_port()));
    let e = rt.admit(&w).await.unwrap_err();
    assert!(
        matches!(e, RuntimeError::AdmissionRefused(ref m) if m.contains("no infer.llamacpp server")),
        "{e:?}"
    );
    assert!(rt.load(&w, &wl_cfg()).await.is_err());
}

#[tokio::test]
async fn a_server_that_goes_away_is_exited_and_not_restarted() {
    // An unpooled server: dropping it really closes the listener.
    let port = free_port();
    let server = server_on(port).await;
    mount_llama(&server, 200, MODEL).await;
    let rt = adopted(InferFlavor::LlamaCpp);
    let h = load(&rt, spec("hermes", InferFlavor::LlamaCpp, port)).await;
    assert_eq!(rt.status(&h).await.state, InstanceState::Running);
    drop(server);
    let st = rt.status(&h).await;
    assert_eq!(st.state, InstanceState::Exited);
    assert!(st.detail.unwrap().contains("not restarted"));
    assert!(
        rt.start(&h).await.is_err(),
        "adopted servers are never started"
    );
    assert_eq!(
        rt.reconcile(&h).await.unwrap(),
        super::Reconcile::NotManaged
    );
}

#[tokio::test]
async fn mlx_without_a_health_route_falls_back_to_the_model_list() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{"id": "mlx-community/Qwen3-Coder-Next-4bit"}]
        })))
        .mount(&server)
        .await;
    let rt = adopted(InferFlavor::MlxLm);
    let mut s = spec("coder-daily", InferFlavor::MlxLm, port_of(&server));
    s.model = Some("Qwen3-Coder-Next-4bit".into());
    let h = load(&rt, s).await;
    assert_eq!(rt.status(&h).await.state, InstanceState::Running);
}

#[tokio::test]
async fn adopts_ollama_and_reports_model_state() {
    let server = MockServer::start().await;
    mount_ollama(&server, &["orpheus-tts:latest"], &[]).await;
    let rt = adopted(InferFlavor::Ollama);
    let mut s = spec("tts", InferFlavor::Ollama, port_of(&server));
    s.model = Some("orpheus-tts".into());
    let h = load(&rt, s).await;
    let st = rt.status(&h).await;
    assert_eq!(st.state, InstanceState::Running, "{st:?}");
    assert_eq!(
        rt.health(&h).await.unwrap().version.as_deref(),
        Some("0.0.0-test")
    );
    let mut missing = spec("tts2", InferFlavor::Ollama, port_of(&server));
    missing.model = Some("not-pulled".into());
    let h2 = load(&rt, missing).await;
    assert_eq!(rt.status(&h2).await.state, InstanceState::Degraded);
}

#[tokio::test]
async fn a_redirect_is_not_followed() {
    let elsewhere = MockServer::start().await;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("Location", format!("{}/health", elsewhere.uri())),
        )
        .mount(&server)
        .await;
    let rt = adopted(InferFlavor::LlamaCpp);
    let h = load(&rt, spec("hermes", InferFlavor::LlamaCpp, port_of(&server))).await;
    assert_eq!(rt.status(&h).await.state, InstanceState::Degraded);
    assert!(elsewhere.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn capabilities_are_claimed_only_after_a_probe_finds_the_server() {
    let server = MockServer::start().await;
    mount_llama(&server, 200, MODEL).await;
    let cfg = super::InferConfig::adopted(InferFlavor::LlamaCpp).with_probe_port(port_of(&server));
    let rt = super::InferRuntime::new(cfg);
    assert!(
        rt.provides().is_empty(),
        "nothing is claimed before a probe"
    );
    let caps = rt.probe_capabilities().await;
    let ids: Vec<&str> = caps.iter().map(|c| c.id.as_str()).collect();
    assert!(ids.contains(&"runtime.infer.llamacpp"), "{ids:?}");
    assert!(ids.contains(&"format.gguf"));
    assert_eq!(rt.provides().len(), caps.len());

    let gone = super::InferRuntime::new(
        super::InferConfig::adopted(InferFlavor::LlamaCpp).with_probe_port(free_port()),
    );
    assert!(gone.probe_capabilities().await.is_empty());
}

#[tokio::test]
async fn a_server_for_another_runtime_is_not_adoptable_here() {
    let server = MockServer::start().await;
    mount_llama(&server, 200, MODEL).await;
    let rt = adopted(InferFlavor::MlxLm);
    let w = workload(spec("hermes", InferFlavor::LlamaCpp, port_of(&server)));
    assert!(matches!(
        rt.admit(&w).await,
        Err(RuntimeError::AdmissionRefused(_))
    ));
}

#[tokio::test]
async fn config_that_is_not_a_listener_is_rejected() {
    let server = MockServer::start().await;
    mount_llama(&server, 200, MODEL).await;
    let rt = adopted(InferFlavor::LlamaCpp);
    let w = workload(spec("hermes", InferFlavor::LlamaCpp, port_of(&server)));
    let mut c = wl_cfg();
    c.args = vec!["--once".into()];
    assert!(matches!(
        rt.load(&w, &c).await,
        Err(RuntimeError::InvalidConfig(_))
    ));
    let mut c = wl_cfg();
    c.node_id = "bad id".into();
    assert!(rt.load(&w, &c).await.is_err());
}

#[tokio::test]
async fn adopted_reports_lan_exposure_and_reattaches_after_a_restart() {
    let server = MockServer::start().await;
    mount_llama(&server, 200, MODEL).await;
    let rt = adopted(InferFlavor::LlamaCpp);
    assert_eq!(rt.network_exposure(), NetworkPolicy::Lan);
    let w = workload(spec("hermes", InferFlavor::LlamaCpp, port_of(&server)));
    let h = rt.load(&w, &wl_cfg()).await.unwrap();
    let fresh = adopted(InferFlavor::LlamaCpp);
    assert_eq!(fresh.status(&h).await.state, InstanceState::Unknown);
    fresh.adopt(&h, &w).await.unwrap();
    assert_eq!(fresh.status(&h).await.state, InstanceState::Running);
}

#[tokio::test]
async fn through_the_host_every_transition_is_gated_and_chained() {
    let server = MockServer::start().await;
    mount_llama(&server, 200, MODEL).await;
    let chain = Arc::new(ChainManager::new(0, 1000));
    let gate = WorkloadGate::exempt(0.8, false, "test")
        .with_chain(chain.clone())
        .with_permit(infer_permit())
        .unwrap();
    let rt = Arc::new(adopted(InferFlavor::LlamaCpp));
    let host = WorkloadHost::new(rt, Arc::new(gate), "operator", NodeTrustTier::Paired)
        .with_chain(chain.clone());
    let w = workload(spec("hermes", InferFlavor::LlamaCpp, port_of(&server)));
    let h = host.load(&w, &wl_cfg()).await.unwrap();
    host.start(&h).await.unwrap();
    assert_eq!(host.status(&h).await.state, InstanceState::Running);
    let e = host.stop(&h, Duration::from_secs(1)).await.unwrap_err();
    assert!(matches!(e, RuntimeError::Unsupported(_)));
    host.unload(h).await.unwrap();

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
            "workload.refuse",
            "workload.unload"
        ],
        "{kinds:?}"
    );
}

#[tokio::test]
async fn without_a_permit_the_host_denies_before_the_adapter_loads() {
    let server = MockServer::start().await;
    mount_llama(&server, 200, MODEL).await;
    let chain = Arc::new(ChainManager::new(0, 1000));
    let gate = WorkloadGate::exempt(0.8, false, "test").with_chain(chain.clone());
    let rt = Arc::new(adopted(InferFlavor::LlamaCpp));
    let host = WorkloadHost::new(
        rt.clone(),
        Arc::new(gate),
        "operator",
        NodeTrustTier::Paired,
    )
    .with_chain(chain);
    let w = workload(spec("hermes", InferFlavor::LlamaCpp, port_of(&server)));
    let e = host.load(&w, &wl_cfg()).await.unwrap_err();
    assert!(matches!(e, RuntimeError::Governance(_)), "{e:?}");
    assert!(rt.instances.lock().await.is_empty());
}
