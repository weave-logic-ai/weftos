//! Managed Ollama: the adapter drives Ollama's API (load, unload, state)
//! and never owns, starts, signals, pulls or deletes anything. The
//! "Ollama" is a fake HTTP server on a random loopback port.

use std::time::Duration;

use serde_json::Value;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::fakes::*;
use super::lifecycle::Reconcile;
use super::spec::InferFlavor;
use crate::model_manifest::ModelFormat;
use crate::workload_runtime::types::{InstanceState, RuntimeError, WorkloadRuntime};

const NAME: &str = "orpheus-tts";

fn ollama_spec(port: u16) -> super::InferenceSpec {
    let mut s = spec("tts", InferFlavor::Ollama, port);
    s.model = Some(NAME.into());
    s
}

fn setup() -> Managed {
    let m = managed(InferFlavor::Ollama, super::RestartPolicy::default());
    adopt_model(&m.reg, m.tmp.path(), NAME, ModelFormat::Ollama);
    m
}

async fn bodies(s: &MockServer, p: &str) -> Vec<Value> {
    s.received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.url.path() == p)
        .map(|r| serde_json::from_slice(&r.body).unwrap_or(Value::Null))
        .collect()
}

#[tokio::test]
async fn load_start_loading_running_stop_unload_through_the_api() {
    let m = setup();
    let server = MockServer::start().await;
    mount_ollama(&server, &["orpheus-tts:latest"], &[]).await;
    Mock::given(method("POST"))
        .and(path("/api/generate"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(700)))
        .mount(&server)
        .await;
    let h = load(&m.rt, ollama_spec(port_of(&server))).await;
    assert_eq!(
        m.rt.status(&h).await.state,
        InstanceState::Loaded,
        "model known, not resident"
    );

    m.rt.start(&h).await.unwrap();
    let st = m.rt.status(&h).await;
    assert_eq!(st.state, InstanceState::Degraded, "{st:?}");
    assert_eq!(st.detail.as_deref(), Some("loading model"));
    let load_req = bodies(&server, "/api/generate").await;
    assert!(load_req.is_empty() || load_req[0]["model"] == "orpheus-tts:latest");

    tokio::time::sleep(Duration::from_millis(900)).await;
    // Ollama now reports it resident.
    server.reset().await;
    mount_ollama(&server, &["orpheus-tts:latest"], &["orpheus-tts:latest"]).await;
    Mock::given(method("POST"))
        .and(path("/api/generate"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    assert_eq!(m.rt.status(&h).await.state, InstanceState::Running);

    let ev = m.rt.stop(&h, Duration::from_secs(1)).await.unwrap();
    assert_eq!(ev.runtime, "infer.ollama");
    let sent = bodies(&server, "/api/generate").await;
    assert_eq!(
        sent.last().unwrap()["keep_alive"],
        0,
        "stop asks Ollama to drop the model"
    );

    m.rt.unload(h).await.unwrap();
    let reqs = server.received_requests().await.unwrap();
    for r in &reqs {
        let p = r.url.path();
        assert!(
            matches!(
                p,
                "/api/version" | "/api/tags" | "/api/ps" | "/api/generate"
            ),
            "adapter must not call {p}"
        );
        assert_ne!(r.method.as_str(), "DELETE");
    }
}

#[tokio::test]
async fn a_failed_load_is_degraded_with_the_reason() {
    let m = setup();
    let server = MockServer::start().await;
    mount_ollama(&server, &["orpheus-tts:latest"], &[]).await;
    Mock::given(method("POST"))
        .and(path("/api/generate"))
        .respond_with(ResponseTemplate::new(500).set_body_string("out of memory"))
        .mount(&server)
        .await;
    let h = load(&m.rt, ollama_spec(port_of(&server))).await;
    m.rt.start(&h).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let st = m.rt.status(&h).await;
    assert_eq!(st.state, InstanceState::Degraded, "{st:?}");
    assert!(st.detail.unwrap().contains("load failed"));
    m.rt.unload(h).await.unwrap();
}

#[tokio::test]
async fn it_never_downloads_and_never_starts_ollama() {
    let m = setup();
    let server = MockServer::start().await;
    mount_ollama(&server, &["something-else:latest"], &[]).await;
    let e =
        m.rt.admit(&workload(ollama_spec(port_of(&server))))
            .await
            .unwrap_err();
    assert!(
        matches!(e, RuntimeError::AdmissionRefused(ref s) if s.contains("pull it first")),
        "{e:?}"
    );
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| r.method.as_str() == "GET"),
        "admission only reads"
    );
    let e =
        m.rt.admit(&workload(ollama_spec(free_port())))
            .await
            .unwrap_err();
    assert!(
        matches!(e, RuntimeError::AdmissionRefused(ref s) if s.contains("does not start it")),
        "{e:?}"
    );
}

#[tokio::test]
async fn ollama_is_not_a_supervised_process() {
    let m = setup();
    let server = MockServer::start().await;
    mount_ollama(&server, &["orpheus-tts:latest"], &[]).await;
    let h = load(&m.rt, ollama_spec(port_of(&server))).await;
    assert_eq!(m.rt.reconcile(&h).await.unwrap(), Reconcile::NotManaged);
    m.rt.unload(h).await.unwrap();
}

#[tokio::test]
async fn capabilities_need_a_reachable_ollama() {
    let server = MockServer::start().await;
    mount_ollama(&server, &[], &[]).await;
    let mut cfg = super::ManagedConfig::new(
        std::sync::Arc::new(crate::model_manifest::ModelRegistry::in_memory()),
        std::env::temp_dir().join("infer-ollama-caps-unused"),
    );
    cfg.env_passthrough = vec![];
    let rt = super::InferRuntime::new(
        super::InferConfig::managed(InferFlavor::Ollama, cfg.clone())
            .with_probe_port(port_of(&server)),
    );
    let caps = rt.probe_capabilities().await;
    let ids: Vec<&str> = caps.iter().map(|c| c.id.as_str()).collect();
    assert!(
        ids.contains(&"runtime.infer.ollama") && ids.contains(&"format.ollama"),
        "{ids:?}"
    );
    let rt = super::InferRuntime::new(
        super::InferConfig::managed(InferFlavor::Ollama, cfg).with_probe_port(free_port()),
    );
    assert!(rt.probe_capabilities().await.is_empty());
}
