//! [`WorkloadHost`]: gates every adapter transition through governance and
//! chains what the adapter did (ADR-099 sections 4 and 5).
//!
//! The gate chains its own decision under the action's kind with source
//! `workload`; the host then chains the transition's outcome under the same
//! kind with source [`RUNTIME_CHAIN_SOURCE`]. Admission refusals and adapter
//! failures are chained as `workload.refuse`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::Mutex;

use super::evidence::RunEvidence;
use super::seed::SeedApiRuntime;
use super::seed_ops::{KIND_SEED_FIRMWARE, SeedBackup, UpgradeOutcome};
use super::types::{
    InstanceHandle, InstanceStatus, RuntimeError, VerifiedWorkload, WorkloadConfig,
    WorkloadRuntime, WorkloadSource,
};
use crate::chain::{self, ChainManager};
use crate::gate::{GateBackend, GateDecision};
use crate::workload_governance::{NetworkPolicy, NodeTrustTier};

/// Chain source for adapter lifecycle outcomes.
pub const RUNTIME_CHAIN_SOURCE: &str = "workload.runtime";

/// Governs and records one adapter.
pub struct WorkloadHost {
    runtime: Arc<dyn WorkloadRuntime>,
    gate: Arc<dyn GateBackend>,
    chain: Option<Arc<ChainManager>>,
    agent_id: String,
    node_tier: NodeTrustTier,
    network: NetworkPolicy,
    loaded: Mutex<HashMap<String, (VerifiedWorkload, bool)>>,
}

impl WorkloadHost {
    /// Host for `runtime` on a node of `node_tier`.
    pub fn new(
        runtime: Arc<dyn WorkloadRuntime>,
        gate: Arc<dyn GateBackend>,
        agent_id: impl Into<String>,
        node_tier: NodeTrustTier,
    ) -> Self {
        Self {
            runtime,
            gate,
            chain: None,
            agent_id: agent_id.into(),
            node_tier,
            network: NetworkPolicy::Lan,
            loaded: Mutex::new(HashMap::new()),
        }
    }

    /// Attach the chain.
    pub fn with_chain(mut self, cm: Arc<ChainManager>) -> Self {
        self.chain = Some(cm);
        self
    }

    /// The adapter.
    pub fn runtime(&self) -> &Arc<dyn WorkloadRuntime> {
        &self.runtime
    }

    fn context(&self, w: &VerifiedWorkload, kind: &str, emulated: bool) -> Value {
        let (trust, package_id, keys, hashes, cost) = match &w.source {
            WorkloadSource::SignedPackage(p) => (
                "pinned_signer",
                p.package_id.clone(),
                p.signer_keys.clone(),
                p.artifact_hashes.clone(),
                (f64::from(p.spec.resources.cpu_pct) / 400.0).clamp(0.0, 1.0),
            ),
            WorkloadSource::StorePin { .. } => (
                "operator_attested",
                format!("store.{}.{}", w.id, w.version),
                Vec::new(),
                Vec::new(),
                0.25,
            ),
        };
        json!({ "workload": {
            "kind": kind,
            "package_trust": trust,
            "node_tier": self.node_tier,
            "network": self.network,
            "secrets": false,
            "emulated": emulated,
            "resource_cost": cost,
            "package_id": package_id,
            "signer_keys": keys,
            "artifact_hashes": hashes,
        }})
    }

    fn check(
        &self,
        action: &str,
        w: &VerifiedWorkload,
        kind: &str,
        emulated: bool,
    ) -> Result<(), RuntimeError> {
        match self
            .gate
            .check(&self.agent_id, action, &self.context(w, kind, emulated))
        {
            GateDecision::Permit { .. } => Ok(()),
            GateDecision::Deny { reason, .. } => Err(RuntimeError::Governance(format!(
                "{action} denied: {reason}"
            ))),
            GateDecision::Defer { reason } => Err(RuntimeError::Governance(format!(
                "{action} deferred to a human: {reason}"
            ))),
        }
    }

    fn record(&self, kind: &str, payload: Value) {
        if let Some(cm) = &self.chain {
            cm.append(RUNTIME_CHAIN_SOURCE, kind, Some(payload));
        }
    }

    fn base(&self, w: &VerifiedWorkload, instance: Option<&str>) -> Value {
        json!({
            "runtime": self.runtime.id(),
            "workload_id": w.id,
            "version": w.version,
            "instance_id": instance,
        })
    }

    fn outcome<T>(&self, kind: &str, mut payload: Value, r: &Result<T, RuntimeError>) {
        match r {
            Ok(_) => {
                payload["outcome"] = json!("ok");
                self.record(kind, payload);
            }
            Err(e) => {
                payload["outcome"] = json!("error");
                payload["failed_action"] = json!(kind);
                payload["error_code"] = json!(e.code());
                payload["reason"] = json!(e.to_string());
                self.record(chain::EVENT_KIND_WORKLOAD_REFUSE, payload);
            }
        }
    }

    async fn workload_of(
        &self,
        h: &InstanceHandle,
    ) -> Result<(VerifiedWorkload, bool), RuntimeError> {
        self.loaded
            .lock()
            .await
            .get(&h.instance_id)
            .cloned()
            .ok_or_else(|| RuntimeError::UnknownInstance(h.instance_id.clone()))
    }

    /// Gate, admit and load. Store pins are also gated as `workload.install`.
    pub async fn load(
        &self,
        w: &VerifiedWorkload,
        cfg: &WorkloadConfig,
    ) -> Result<InstanceHandle, RuntimeError> {
        let admission = self.runtime.admit(w).await;
        let emulated = admission.as_ref().is_ok_and(|a| a.emulated);
        let mut payload = self.base(w, None);
        payload["admission"] = json!(admission.as_ref().ok());
        payload["host_contract"] = cfg.host.audit();
        if let Err(e) = &admission {
            self.outcome::<()>(chain::EVENT_KIND_WORKLOAD_LOAD, payload, &Err(e.clone()));
            return Err(e.clone());
        }
        let store = matches!(w.source, WorkloadSource::StorePin { .. });
        if store {
            self.check("workload.install", w, &w.kind, emulated)?;
        }
        self.check("workload.load", w, &w.kind, emulated)?;
        let r = self.runtime.load(w, cfg).await;
        if let Ok(h) = &r {
            payload["instance_id"] = json!(h.instance_id);
            self.loaded
                .lock()
                .await
                .insert(h.instance_id.clone(), (w.clone(), emulated));
            if store {
                self.record(
                    chain::EVENT_KIND_WORKLOAD_INSTALL,
                    json!({
                        "runtime": self.runtime.id(), "workload_id": w.id, "version": w.version,
                        "instance_id": h.instance_id, "outcome": "ok", "source": "store_pin",
                    }),
                );
            }
        }
        self.outcome(chain::EVENT_KIND_WORKLOAD_LOAD, payload, &r);
        r
    }

    /// Gate and start.
    pub async fn start(&self, h: &InstanceHandle) -> Result<(), RuntimeError> {
        let (w, emu) = self.workload_of(h).await?;
        self.check("workload.start", &w, &w.kind, emu)?;
        let r = self.runtime.start(h).await;
        self.outcome(
            chain::EVENT_KIND_WORKLOAD_START,
            self.base(&w, Some(&h.instance_id)),
            &r,
        );
        r
    }

    /// Gate and run one console cycle (chained as `workload.start`, phase
    /// `console`; instances stopped to free the feed as `workload.stop`).
    pub async fn console(
        &self,
        h: &InstanceHandle,
        command: &str,
    ) -> Result<RunEvidence, RuntimeError> {
        let (w, emu) = self.workload_of(h).await?;
        self.check("workload.start", &w, &w.kind, emu)?;
        let r = self.runtime.console(h, command).await;
        let mut payload = self.base(&w, Some(&h.instance_id));
        payload["phase"] = json!("console");
        if let Ok(ev) = &r {
            for id in &ev.stopped_for_console {
                self.record(
                    chain::EVENT_KIND_WORKLOAD_STOP,
                    json!({
                        "runtime": self.runtime.id(), "workload_id": id, "outcome": "ok",
                        "reason": "stopped before a console run (sensor feed contention)",
                    }),
                );
            }
            payload["evidence"] = ev.audit();
        }
        self.outcome(chain::EVENT_KIND_WORKLOAD_START, payload, &r);
        r
    }

    /// Gate and stop.
    pub async fn stop(
        &self,
        h: &InstanceHandle,
        grace: Duration,
    ) -> Result<RunEvidence, RuntimeError> {
        let (w, emu) = self.workload_of(h).await?;
        self.check("workload.stop", &w, &w.kind, emu)?;
        let r = self.runtime.stop(h, grace).await;
        let mut payload = self.base(&w, Some(&h.instance_id));
        if let Ok(ev) = &r {
            payload["evidence"] = ev.audit();
        }
        self.outcome(chain::EVENT_KIND_WORKLOAD_STOP, payload, &r);
        r
    }

    /// Gate and unload.
    pub async fn unload(&self, h: InstanceHandle) -> Result<(), RuntimeError> {
        let (w, emu) = self.workload_of(&h).await?;
        self.check("workload.unload", &w, &w.kind, emu)?;
        let payload = self.base(&w, Some(&h.instance_id));
        let iid = h.instance_id.clone();
        let r = self.runtime.unload(h).await;
        if r.is_ok() {
            self.loaded.lock().await.remove(&iid);
        }
        self.outcome(chain::EVENT_KIND_WORKLOAD_UNLOAD, payload, &r);
        r
    }

    /// Status (not gated: read-only).
    pub async fn status(&self, h: &InstanceHandle) -> InstanceStatus {
        self.runtime.status(h).await
    }

    /// Governed, backed-up Seed firmware upgrade: gated as
    /// `workload.install` with kind `seed-firmware`, outcome chained.
    pub async fn upgrade_seed_firmware(
        &self,
        seed: &SeedApiRuntime,
        backup: &SeedBackup,
    ) -> Result<UpgradeOutcome, RuntimeError> {
        let w = VerifiedWorkload::store_pin("cognitum", "seed-firmware", "current", None)?;
        self.check("workload.install", &w, KIND_SEED_FIRMWARE, false)?;
        let r = seed.upgrade_firmware(backup).await;
        let mut payload = json!({
            "runtime": seed_id(), "phase": "firmware-upgrade", "backup": backup.audit(),
        });
        if let Ok(o) = &r {
            payload["result"] = json!(format!("{o:?}"));
        }
        self.outcome(chain::EVENT_KIND_WORKLOAD_INSTALL, payload, &r);
        r
    }

    /// Governed `writes_gated` recovery (truncate-confirm after a backup).
    pub async fn recover_seed_writes(
        &self,
        seed: &SeedApiRuntime,
        backup: &SeedBackup,
    ) -> Result<(), RuntimeError> {
        let w = VerifiedWorkload::store_pin("cognitum", "seed-firmware", "current", None)?;
        self.check("workload.install", &w, KIND_SEED_FIRMWARE, false)?;
        let r = seed.recover_writes_gated(backup).await;
        let payload = json!({
            "runtime": seed_id(), "phase": "writes-gated-recovery", "backup": backup.audit(),
        });
        self.outcome(chain::EVENT_KIND_WORKLOAD_INSTALL, payload, &r);
        r
    }
}

fn seed_id() -> &'static str {
    super::seed::SEED_ID
}
