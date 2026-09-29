//! [`WorkloadHost`]: governance before every transition, and every
//! transition chained as a `workload.*` event on an isolated in-memory
//! chain. The adapter is a recording mock (London school).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use clawft_types::placement::Capability;
use serde_json::Value;

use super::evidence::RunEvidence;
use super::host::{RUNTIME_CHAIN_SOURCE, WorkloadHost};
use super::host_contract::HostContract;
use super::test_support::signed_workload;
use super::types::*;
use crate::chain::{ChainEvent, ChainManager};
use crate::workload_governance::{NodeTrustTier, PackageTrust, WorkloadGate, WorkloadPermitRule};

#[derive(Default)]
struct MockRuntime {
    calls: Mutex<Vec<String>>,
    refuse_admission: bool,
    stopped_for_console: Vec<String>,
}

impl MockRuntime {
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    fn note(&self, c: &str) {
        self.calls.lock().unwrap().push(c.into());
    }
    fn handle(w: &VerifiedWorkload) -> InstanceHandle {
        InstanceHandle {
            runtime: "mock".into(),
            instance_id: format!("{}-i", w.id),
            workload_id: w.id.clone(),
        }
    }
}

#[async_trait]
impl WorkloadRuntime for MockRuntime {
    fn id(&self) -> &str {
        "mock"
    }
    fn provides(&self) -> Vec<Capability> {
        Vec::new()
    }
    async fn admit(&self, _w: &VerifiedWorkload) -> Result<Admission, RuntimeError> {
        self.note("admit");
        if self.refuse_admission {
            return Err(RuntimeError::AdmissionRefused(
                "binary arch mismatch".into(),
            ));
        }
        Ok(Admission {
            runtime: "mock".into(),
            arch: "aarch64".into(),
            emulated: false,
            notes: vec![],
        })
    }
    async fn load(
        &self,
        w: &VerifiedWorkload,
        _c: &WorkloadConfig,
    ) -> Result<InstanceHandle, RuntimeError> {
        self.note("load");
        Ok(Self::handle(w))
    }
    async fn start(&self, _h: &InstanceHandle) -> Result<(), RuntimeError> {
        self.note("start");
        Ok(())
    }
    async fn stop(&self, h: &InstanceHandle, _g: Duration) -> Result<RunEvidence, RuntimeError> {
        self.note("stop");
        Ok(RunEvidence {
            runtime: "mock".into(),
            instance_id: h.instance_id.clone(),
            exit_code: Some(0),
            stdout: "{\"secret_output\":1}\n".into(),
            ..RunEvidence::default()
        })
    }
    async fn unload(&self, _h: InstanceHandle) -> Result<(), RuntimeError> {
        self.note("unload");
        Err(RuntimeError::Backend("disk busy".into()))
    }
    async fn status(&self, _h: &InstanceHandle) -> InstanceStatus {
        InstanceStatus::of(InstanceState::Running)
    }
    fn control_mode(&self) -> ControlMode {
        ControlMode::Managed
    }
    async fn console(&self, h: &InstanceHandle, _c: &str) -> Result<RunEvidence, RuntimeError> {
        self.note("console");
        Ok(RunEvidence {
            runtime: "mock".into(),
            instance_id: h.instance_id.clone(),
            exit_code: Some(0),
            stopped_for_console: self.stopped_for_console.clone(),
            ..RunEvidence::default()
        })
    }
}

const TOML: &str = "[cog]\nid = \"anomaly-detect\"\nversion = \"1.2.0\"\n";

fn cfg() -> WorkloadConfig {
    WorkloadConfig {
        mode: RunMode::Once,
        args: vec![],
        host: HostContract::default_feed(),
        node_id: "n1".into(),
    }
}

fn host(rt: Arc<MockRuntime>, permits: &[WorkloadPermitRule]) -> (WorkloadHost, Arc<ChainManager>) {
    let chain = Arc::new(ChainManager::new(0, 1000));
    let mut gate = WorkloadGate::new(0.8, false).with_chain(chain.clone());
    for p in permits {
        gate = gate.with_permit(p.clone()).unwrap();
    }
    let h = WorkloadHost::new(rt, Arc::new(gate), "operator", NodeTrustTier::Paired)
        .with_chain(chain.clone());
    (h, chain)
}

fn events(cm: &ChainManager, source: &str) -> Vec<ChainEvent> {
    cm.tail(0)
        .into_iter()
        .filter(|e| e.source == source)
        .collect()
}

fn kinds(cm: &ChainManager, source: &str) -> Vec<String> {
    events(cm, source).into_iter().map(|e| e.kind).collect()
}

fn all_cog() -> WorkloadPermitRule {
    WorkloadPermitRule::new("permit-cog", ["workload.*"], ["cog"])
}

#[tokio::test]
async fn default_deny_stops_load_before_the_adapter_loads_and_chains_the_denial() {
    let fx = signed_workload(TOML, &[("aarch64", b"\x7fELF")]);
    let rt = Arc::new(MockRuntime::default());
    let (h, chain) = host(rt.clone(), &[]);
    let e = h.load(&fx.workload, &cfg()).await.unwrap_err();
    assert!(
        matches!(e, RuntimeError::Governance(ref m) if m.contains("default deny")),
        "{e}"
    );
    assert_eq!(
        rt.calls(),
        ["admit"],
        "admission self-check is read-only; load never ran"
    );
    let gate_events = events(&chain, "workload");
    assert_eq!(gate_events.last().unwrap().kind, "workload.load");
    assert_eq!(
        gate_events.last().unwrap().payload.as_ref().unwrap()["decision"],
        "deny"
    );
    assert!(events(&chain, RUNTIME_CHAIN_SOURCE).is_empty());
}

#[tokio::test]
async fn permitted_lifecycle_chains_every_transition_without_secrets_or_output() {
    let fx = signed_workload(TOML, &[("aarch64", b"\x7fELF")]);
    let rt = Arc::new(MockRuntime {
        stopped_for_console: vec!["other-cog".into()],
        ..MockRuntime::default()
    });
    let (h, chain) = host(rt.clone(), &[all_cog()]);
    let c = cfg();
    let inst = h.load(&fx.workload, &c).await.unwrap();
    h.start(&inst).await.unwrap();
    h.console(&inst, "--once").await.unwrap();
    let ev = h.stop(&inst, Duration::from_secs(1)).await.unwrap();
    assert_eq!(ev.exit_code, Some(0));
    assert!(h.unload(inst.clone()).await.is_err());
    assert_eq!(
        rt.calls(),
        ["admit", "load", "start", "console", "stop", "unload"]
    );

    assert_eq!(
        kinds(&chain, RUNTIME_CHAIN_SOURCE),
        [
            "workload.load",
            "workload.start",
            "workload.stop",
            "workload.start",
            "workload.stop",
            "workload.refuse"
        ]
    );
    let gate_kinds = kinds(&chain, "workload");
    assert_eq!(
        gate_kinds,
        [
            "workload.load",
            "workload.start",
            "workload.start",
            "workload.stop",
            "workload.unload"
        ]
    );
    let rt_events = events(&chain, RUNTIME_CHAIN_SOURCE);
    let load = rt_events[0].payload.as_ref().unwrap();
    assert_eq!(load["outcome"], "ok");
    assert_eq!(load["runtime"], "mock");
    assert_eq!(
        load["host_contract"]["token_hash"],
        Value::String(c.host.token_hash())
    );
    assert_eq!(rt_events[3].payload.as_ref().unwrap()["phase"], "console");
    let refuse = rt_events[5].payload.as_ref().unwrap();
    assert_eq!(refuse["failed_action"], "workload.unload");
    assert_eq!(refuse["error_code"], "backend");

    let dump = serde_json::to_string(&chain.tail(0).iter().map(|e| &e.payload).collect::<Vec<_>>())
        .unwrap();
    assert!(
        !dump.contains(c.host.token.expose()),
        "token leaked to chain"
    );
    assert!(
        !dump.contains("secret_output"),
        "stdout content leaked to chain"
    );
}

#[tokio::test]
async fn admission_refusal_is_chained_as_workload_refuse() {
    let fx = signed_workload(TOML, &[("aarch64", b"\x7fELF")]);
    let rt = Arc::new(MockRuntime {
        refuse_admission: true,
        ..MockRuntime::default()
    });
    let (h, chain) = host(rt.clone(), &[all_cog()]);
    assert!(matches!(
        h.load(&fx.workload, &cfg()).await,
        Err(RuntimeError::AdmissionRefused(_))
    ));
    assert_eq!(rt.calls(), ["admit"]);
    let ev = events(&chain, RUNTIME_CHAIN_SOURCE).pop().unwrap();
    assert_eq!(ev.kind, "workload.refuse");
    assert_eq!(ev.payload.unwrap()["failed_action"], "workload.load");
}

#[tokio::test]
async fn store_pins_are_gated_as_install_and_need_an_operator_attested_permit() {
    let w = VerifiedWorkload::store_pin("cognitum", "fall-detect", "1.0.0", None).unwrap();
    let rt = Arc::new(MockRuntime::default());
    // Strict permit wants a pinned signer: an operator store pin is not one.
    let (h, _) = host(rt.clone(), &[all_cog()]);
    assert!(
        matches!(h.load(&w, &cfg()).await, Err(RuntimeError::Governance(m)) if m.contains("workload.install"))
    );

    let mut store = WorkloadPermitRule::new("permit-store", ["workload.*"], ["cog"]);
    store.min_package_trust = PackageTrust::OperatorAttested;
    let rt = Arc::new(MockRuntime::default());
    let (h, chain) = host(rt.clone(), &[store]);
    let inst = h.load(&w, &cfg()).await.unwrap();
    assert_eq!(
        kinds(&chain, RUNTIME_CHAIN_SOURCE),
        ["workload.install", "workload.load"]
    );
    assert_eq!(
        kinds(&chain, "workload"),
        ["workload.install", "workload.load"]
    );
    h.start(&inst).await.unwrap();
}

#[tokio::test]
async fn transitions_on_unknown_instances_fail_without_touching_the_adapter() {
    let rt = Arc::new(MockRuntime::default());
    let (h, _) = host(rt.clone(), &[all_cog()]);
    let ghost = InstanceHandle {
        runtime: "mock".into(),
        instance_id: "ghost".into(),
        workload_id: "x".into(),
    };
    assert!(matches!(
        h.start(&ghost).await,
        Err(RuntimeError::UnknownInstance(_))
    ));
    assert!(rt.calls().is_empty());
}
