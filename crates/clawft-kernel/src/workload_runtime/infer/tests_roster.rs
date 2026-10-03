//! Roster import and the residency ledger through the managed adapter.
//! The roster is a fixture copy in this repo; the real `~/llm` is never
//! read, and the launcher is a fake script.

use std::sync::Arc;
use std::time::Duration;

use super::config::RestartPolicy;
use super::fakes::*;
use super::residency::{GB, ResidencyLedger};
use super::roster::{RosterOverlay, flavor_for_backend, import_roster};
use super::spec::{InferFlavor, LatencyClass};
use crate::chain::ChainManager;
use crate::model_manifest::ModelFormat;
use crate::workload_governance::{NodeTrustTier, WorkloadGate};
use crate::workload_runtime::host::{RUNTIME_CHAIN_SOURCE, WorkloadHost};
use crate::workload_runtime::types::{RuntimeError, WorkloadRuntime};

const YAML: &str = include_str!("fixtures/model-lab-roster.yaml");

fn overlay() -> RosterOverlay {
    serde_yaml::from_str(
        r#"
excludes:
  planner: ["role:coder-daily"]
latency_class:
  coder-small: interactive
sticky:
  swarm-micro: false
"#,
    )
    .unwrap()
}

#[test]
fn imports_memory_ports_flavors_and_the_overlay() {
    let r = import_roster(YAML, &overlay()).unwrap();
    let ids: Vec<_> = r.specs.iter().map(|s| s.spec.role.as_str()).collect();
    assert_eq!(
        ids,
        ["coder-daily", "coder-compact", "coder-small", "planner", "swarm-worker", "swarm-micro", "voice-llm"]
    );
    let daily = r.get("coder-daily").unwrap();
    assert_eq!(daily.spec.runtime, InferFlavor::MlxLm);
    assert_eq!(daily.spec.serve.port, Some(8081));
    assert_eq!(daily.spec.model.as_deref(), Some("Qwen3-Coder-Next-80B-A3B"));
    assert_eq!(daily.spec.memory.weights_bytes, 55 * GB);
    assert!(daily.spec.sticky, "inference is sticky by default (ADR-101 section 6)");
    assert_eq!(daily.spec.latency_class, LatencyClass::Batch);
    assert_eq!(daily.status, "live");
    assert_eq!(r.get("coder-small").unwrap().spec.memory.weights_bytes, 9_700_000_000);
    assert_eq!(r.get("coder-compact").unwrap().spec.runtime, InferFlavor::Ollama);
    // Overlay: excludes, latency, sticky.
    assert_eq!(r.get("planner").unwrap().spec.excludes, ["role:coder-daily"]);
    assert_eq!(r.get("coder-small").unwrap().spec.latency_class, LatencyClass::Interactive);
    assert!(!r.get("swarm-micro").unwrap().spec.sticky);
    // The role text can say voice.
    assert_eq!(r.get("voice-llm").unwrap().spec.latency_class, LatencyClass::Interactive);
    // "a or b" backends take the first.
    assert_eq!(r.get("swarm-worker").unwrap().spec.runtime, InferFlavor::Ollama);
    assert_eq!(r.get("swarm-micro").unwrap().spec.runtime, InferFlavor::MlxLm);
    // Every imported spec validates.
    assert!(r.specs.iter().all(|s| s.spec.validate().is_ok()));
}

#[test]
fn what_is_not_imported_is_reported_with_a_reason() {
    let r = import_roster(YAML, &RosterOverlay::default()).unwrap();
    let why = |id: &str| r.skipped.iter().find(|(i, _)| i == id).map(|(_, w)| w.clone()).unwrap_or_default();
    assert!(why("coder-next").contains("alias of coder-daily"), "{}", why("coder-next"));
    assert!(why("vlm").contains("no inference adapter"));
    assert!(why("tts").contains("no inference adapter"));
    assert!(why("broken-mem").contains("ram_gb"));
    assert!(why("bad id!").contains("plain token"));
    assert_eq!(r.specs.len() + r.skipped.len(), 12, "nothing vanishes");
}

#[test]
fn bad_input_is_refused_at_the_boundary() {
    assert!(import_roster("roster: [", &RosterOverlay::default()).is_err());
    assert!(import_roster(&"x".repeat(2 * 1024 * 1024), &RosterOverlay::default()).is_err());
    let many: String = std::iter::once("roster:\n".to_string())
        .chain((0..300).map(|i| format!("- id: r{i}\n  backend: mlx_lm\n  port: 9000\n  ram_gb: 1\n  repo: a{i}\n")))
        .collect();
    assert!(import_roster(&many, &RosterOverlay::default()).is_err());
    // An overlay with an unknown key is refused.
    assert!(serde_yaml::from_str::<RosterOverlay>("bogus: 1").is_err());
    // No roster key at all is an empty import, not an error.
    assert_eq!(import_roster("meta: {}", &RosterOverlay::default()).unwrap().specs.len(), 0);
    assert_eq!(flavor_for_backend("llama.cpp"), Some(InferFlavor::LlamaCpp));
    assert_eq!(flavor_for_backend(""), None);
}

fn fast() -> RestartPolicy {
    RestartPolicy { max_restarts: 1, base: Duration::from_millis(300), cap: Duration::from_secs(1) }
}

fn managed_with_ledger(ledger: &Arc<ResidencyLedger>) -> Managed {
    let l = ledger.clone();
    let m = managed_with(InferFlavor::LlamaCpp, fast(), move |c| c.ledger = Some(l));
    for n in ["Model-A", "Model-B"] {
        adopt_model(&m.reg, m.tmp.path(), n, ModelFormat::Gguf);
    }
    m
}

fn spec_of(role: &str, model: &str, gb: u64, excludes: &[&str]) -> super::InferenceSpec {
    let mut s = spec(role, InferFlavor::LlamaCpp, free_port());
    s.model = Some(model.into());
    s.memory.weights_bytes = gb * GB;
    s.excludes = excludes.iter().map(|x| x.to_string()).collect();
    s
}

#[tokio::test]
async fn the_adapter_refuses_a_start_over_budget_and_frees_it_on_stop() {
    let ledger = Arc::new(ResidencyLedger::new(Some(10 * GB)));
    let (m1, m2) = (managed_with_ledger(&ledger), managed_with_ledger(&ledger));
    let a = load(&m1.rt, spec_of("coder", "Model-A", 6, &[])).await;
    let b = load(&m2.rt, spec_of("planner", "Model-B", 6, &[])).await;
    m1.rt.start(&a).await.unwrap();
    let e = m2.rt.start(&b).await.unwrap_err();
    assert!(matches!(e, RuntimeError::AdmissionRefused(_)), "{e:?}");
    let text = e.to_string();
    assert!(text.contains("unplaceable") && text.contains("'planner' needs 6.0 GB") && text.contains("coder 6.0 GB"), "{text}");
    // Nothing was launched for the refused one.
    assert!(!m2.script_dir.join("pid.txt").exists());
    assert_eq!(ledger.snapshot(), [("coder".to_string(), 6 * GB)]);
    // Stopping the holder gives the memory back.
    m1.rt.stop(&a, Duration::from_secs(3)).await.unwrap();
    assert_eq!(ledger.used(), 0);
    m2.rt.start(&b).await.unwrap();
    // Unload releases too, and a second start of a running instance does not double count.
    assert!(m2.rt.start(&b).await.is_err(), "already running");
    assert_eq!(ledger.used(), 6 * GB, "the failed second start kept the first reservation");
    m2.rt.unload(b).await.unwrap();
    assert_eq!(ledger.used(), 0);
    m1.rt.unload(a).await.unwrap();
}

#[tokio::test]
async fn co_residency_exclusions_hold_across_adapters() {
    let ledger = Arc::new(ResidencyLedger::new(None));
    let (m1, m2) = (managed_with_ledger(&ledger), managed_with_ledger(&ledger));
    let a = load(&m1.rt, spec_of("coder-daily", "Model-A", 1, &[])).await;
    let b = load(&m2.rt, spec_of("planner", "Model-B", 1, &["role:coder-daily"])).await;
    m1.rt.start(&a).await.unwrap();
    let e = m2.rt.start(&b).await.unwrap_err().to_string();
    assert!(e.contains("cannot be resident beside 'coder-daily'") && e.contains("'planner' excludes it"), "{e}");
    m1.rt.unload(a).await.unwrap();
    m2.rt.start(&b).await.unwrap();
    m2.rt.unload(b).await.unwrap();
}

#[tokio::test]
async fn through_the_host_an_unplaceable_start_is_chained_with_its_reason() {
    let ledger = Arc::new(ResidencyLedger::new(Some(5 * GB)));
    let m = managed_with_ledger(&ledger);
    let chain = Arc::new(ChainManager::new(0, 1000));
    let gate = WorkloadGate::exempt(0.8, false, "test")
        .with_chain(chain.clone())
        .with_permit(infer_permit())
        .unwrap();
    let host = WorkloadHost::new(m.rt.clone(), Arc::new(gate), "operator", NodeTrustTier::Paired)
        .with_chain(chain.clone());
    let h = host.load(&workload(spec_of("big", "Model-A", 8, &[])), &wl_cfg()).await.unwrap();
    let e = host.start(&h).await.unwrap_err();
    assert!(e.to_string().contains("unplaceable"), "{e}");
    let refused: Vec<_> = chain
        .tail(0)
        .into_iter()
        .filter(|e| e.source == RUNTIME_CHAIN_SOURCE && e.kind == "workload.refuse")
        .collect();
    assert_eq!(refused.len(), 1);
    let p = refused[0].payload.clone().unwrap().to_string();
    assert!(p.contains("unplaceable") && p.contains("8.0 GB") && p.contains("5.0 GB"), "{p}");
    host.unload(h).await.unwrap();
}
