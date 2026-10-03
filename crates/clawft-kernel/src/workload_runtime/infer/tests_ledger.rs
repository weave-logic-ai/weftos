//! The residency ledger must hold exactly what is resident: released when a
//! server gives up restarting, is stopped for listening beyond loopback,
//! fails to load (Ollama) or stops, and never before the process is gone.
//! Ollama never unloads a model it did not load. Fake launchers and
//! servers on random loopback ports only.

use std::sync::Arc;
use std::time::Duration;

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::config::RestartPolicy;
use super::fakes::*;
use super::lifecycle::Reconcile;
use super::residency::{GB, ResidencyLedger};
use super::spec::InferFlavor;
use crate::model_manifest::ModelFormat;
use crate::workload_runtime::types::{InstanceState, WorkloadRuntime};

const NAME: &str = "Model-L";

fn fast() -> RestartPolicy {
    RestartPolicy { max_restarts: 1, base: Duration::from_millis(200), cap: Duration::from_millis(400) }
}

fn llama(ledger: &Arc<ResidencyLedger>) -> Managed {
    let l = ledger.clone();
    let m = managed_with(InferFlavor::LlamaCpp, fast(), move |c| c.ledger = Some(l));
    adopt_model(&m.reg, m.tmp.path(), NAME, ModelFormat::Gguf);
    m
}

fn lspec(port: u16) -> super::InferenceSpec {
    let mut s = spec("coder", InferFlavor::LlamaCpp, port);
    s.model = Some(NAME.into());
    s.memory.weights_bytes = 6 * GB;
    s
}

#[tokio::test]
async fn a_server_that_gave_up_restarting_holds_no_memory_and_can_be_started_again() {
    let ledger = Arc::new(ResidencyLedger::new(Some(10 * GB)));
    let m = llama(&ledger);
    let h = load(&m.rt, lspec(free_port())).await;
    m.rt.start(&h).await.unwrap();
    assert_eq!(ledger.used(), 6 * GB);
    // Crash, restart once, crash again: the budget is spent.
    crash(script_pid(&m.script_dir).await);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(m.rt.status(&h).await.state, InstanceState::Exited);
    std::fs::remove_file(m.script_dir.join("pid.txt")).unwrap();
    tokio::time::sleep(Duration::from_millis(450)).await;
    assert_eq!(m.rt.reconcile(&h).await.unwrap(), Reconcile::Restarted { attempt: 1 });
    assert_eq!(ledger.used(), 6 * GB, "a restart keeps the reservation");
    crash(script_pid(&m.script_dir).await);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(m.rt.reconcile(&h).await.unwrap(), Reconcile::GaveUp { attempts: 1 });
    assert_eq!(ledger.used(), 0, "nothing is running, so nothing is held");
    // A plain start (what an automatic drive does) does not give the budget
    // back: the server dies again and has given up at once.
    std::fs::remove_file(m.script_dir.join("pid.txt")).unwrap();
    m.rt.start(&h).await.unwrap();
    assert_eq!(ledger.used(), 6 * GB);
    crash(script_pid(&m.script_dir).await);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(m.rt.reconcile(&h).await.unwrap(), Reconcile::GaveUp { attempts: 1 }, "the budget was not reset");
    assert_eq!(ledger.used(), 0);
    // The operator's start resets it explicitly: fresh reservation, fresh budget.
    std::fs::remove_file(m.script_dir.join("pid.txt")).unwrap();
    m.rt.reset_restarts(&h).await;
    m.rt.start(&h).await.unwrap();
    assert_eq!(ledger.used(), 6 * GB);
    script_pid(&m.script_dir).await;
    assert_eq!(m.rt.reconcile(&h).await.unwrap(), Reconcile::Healthy);
    m.rt.unload(h).await.unwrap();
    assert_eq!(ledger.used(), 0);
}

#[tokio::test]
async fn a_server_stopped_for_listening_beyond_loopback_holds_no_memory() {
    if super::exposure::local_addrs().is_empty() {
        eprintln!("no non-loopback interface address on this machine; skipping");
        return;
    }
    for via_reconcile in [false, true] {
        let ledger = Arc::new(ResidencyLedger::new(Some(10 * GB)));
        let m = llama(&ledger);
        let port = free_port();
        let h = load(&m.rt, lspec(port)).await;
        m.rt.start(&h).await.unwrap();
        script_pid(&m.script_dir).await;
        let _wild = std::net::TcpListener::bind(("0.0.0.0", port)).unwrap();
        if via_reconcile {
            assert!(matches!(m.rt.reconcile(&h).await.unwrap(), Reconcile::StoppedExposed { .. }));
        } else {
            assert_eq!(m.rt.status(&h).await.state, InstanceState::Exited);
        }
        assert_eq!(ledger.used(), 0, "via_reconcile={via_reconcile}");
        // Reconciling again, now on the recorded exposure, stays released.
        assert!(matches!(m.rt.reconcile(&h).await.unwrap(), Reconcile::StoppedExposed { .. }));
        assert_eq!(ledger.used(), 0);
        m.rt.unload(h).await.unwrap();
    }
}

#[tokio::test]
async fn stop_gives_the_memory_back_only_after_the_process_has_exited() {
    let ledger = Arc::new(ResidencyLedger::new(Some(10 * GB)));
    let m = llama(&ledger);
    // A launcher that ignores SIGTERM, so a stop takes its whole grace.
    let script = m.script_dir.join("fake-serve");
    std::fs::write(&script, "#!/bin/sh\nd=$(dirname \"$0\")\necho $$ > \"$d/pid.txt\"\ntrap '' TERM\nexec sleep 600\n").unwrap();
    let h = load(&m.rt, lspec(free_port())).await;
    m.rt.start(&h).await.unwrap();
    let pid = script_pid(&m.script_dir).await;
    let rt = m.rt.clone();
    let h2 = h.clone();
    let stopping = tokio::spawn(async move { rt.stop(&h2, Duration::from_millis(1500)).await });
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(alive(pid), "the server is still shutting down");
    assert_eq!(ledger.used(), 6 * GB, "its memory is not free yet");
    stopping.await.unwrap().unwrap();
    assert!(!alive(pid));
    assert_eq!(ledger.used(), 0);
    m.rt.unload(h).await.unwrap();
}

fn ollama_with(ledger: &Arc<ResidencyLedger>) -> Managed {
    let l = ledger.clone();
    let m = managed_with(InferFlavor::Ollama, RestartPolicy::default(), move |c| c.ledger = Some(l));
    adopt_model(&m.reg, m.tmp.path(), "orpheus-tts", ModelFormat::Ollama);
    m
}

fn ospec(port: u16) -> super::InferenceSpec {
    let mut s = spec("tts", InferFlavor::Ollama, port);
    s.model = Some("orpheus-tts".into());
    s.memory.weights_bytes = 4 * GB;
    s
}

#[tokio::test]
async fn a_failed_ollama_load_gives_the_memory_back() {
    let ledger = Arc::new(ResidencyLedger::new(Some(10 * GB)));
    let m = ollama_with(&ledger);
    let server = MockServer::start().await;
    mount_ollama(&server, &["orpheus-tts:latest"], &[]).await;
    Mock::given(method("POST"))
        .and(path("/api/generate"))
        .respond_with(ResponseTemplate::new(500).set_body_string("out of memory"))
        .mount(&server)
        .await;
    let h = load(&m.rt, ospec(port_of(&server))).await;
    m.rt.start(&h).await.unwrap();
    assert_eq!(ledger.used(), 4 * GB, "reserved while loading");
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(m.rt.status(&h).await.detail.unwrap().contains("load failed"));
    assert_eq!(ledger.used(), 0, "a failed load holds nothing");
    m.rt.unload(h).await.unwrap();
}

async fn unloads(s: &MockServer) -> usize {
    s.received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path() == "/api/generate")
        .filter(|r| serde_json::from_slice::<serde_json::Value>(&r.body).is_ok_and(|v| v["keep_alive"] == 0))
        .count()
}

async fn loads(s: &MockServer) -> usize {
    s.received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path() == "/api/generate")
        .count()
        - unloads(s).await
}

#[tokio::test]
async fn ollama_only_unloads_a_model_it_loaded() {
    // Resident before we asked: not ours to load or to drop.
    let ledger = Arc::new(ResidencyLedger::new(None));
    let m = ollama_with(&ledger);
    let server = MockServer::start().await;
    mount_ollama(&server, &["orpheus-tts:latest"], &["orpheus-tts:latest"]).await;
    Mock::given(method("POST")).and(path("/api/generate")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
    let h = load(&m.rt, ospec(port_of(&server))).await;
    m.rt.start(&h).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    m.rt.stop(&h, Duration::from_secs(1)).await.unwrap();
    m.rt.unload(h).await.unwrap();
    assert_eq!(loads(&server).await, 0, "it did not reload a resident model");
    assert_eq!(unloads(&server).await, 0, "another client's model was left in memory");

    // Not resident: this adapter loads it, so it unloads it, on stop and on unload.
    for via_unload in [false, true] {
        let m = ollama_with(&ledger);
        let server = MockServer::start().await;
        mount_ollama(&server, &["orpheus-tts:latest"], &[]).await;
        Mock::given(method("POST")).and(path("/api/generate")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
        let h = load(&m.rt, ospec(port_of(&server))).await;
        m.rt.start(&h).await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(loads(&server).await, 1);
        if via_unload {
            m.rt.unload(h).await.unwrap();
        } else {
            m.rt.stop(&h, Duration::from_secs(1)).await.unwrap();
            m.rt.unload(h).await.unwrap();
        }
        assert_eq!(unloads(&server).await, 1, "via_unload={via_unload}: exactly one unload");
    }
}

#[tokio::test]
async fn an_aborted_ollama_load_is_still_unloaded_at_stop() {
    let ledger = Arc::new(ResidencyLedger::new(None));
    let m = ollama_with(&ledger);
    let server = MockServer::start().await;
    mount_ollama(&server, &["orpheus-tts:latest"], &[]).await;
    // The unload answers at once; the load is still in flight when the
    // operator stops the role.
    Mock::given(method("POST"))
        .and(path("/api/generate"))
        .and(wiremock::matchers::body_partial_json(serde_json::json!({"keep_alive": 0})))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/generate"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(5)))
        .mount(&server)
        .await;
    let h = load(&m.rt, ospec(port_of(&server))).await;
    m.rt.start(&h).await.unwrap();
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(loads(&server).await, 1, "the load request was sent");
    m.rt.stop(&h, Duration::from_secs(1)).await.unwrap();
    assert_eq!(unloads(&server).await, 1, "what we asked Ollama to load is unloaded even if the load was cut short");
    m.rt.unload(h).await.unwrap();
}
