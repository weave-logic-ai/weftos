//! `remote.api` adapter for a Cognitum Seed (COG-001 section 5, option b):
//! drives the Seed's own HTTP API instead of running WeftOS on it.
//!
//! Seed constraints honored here:
//! - only store cogs the operator pinned by id and version install
//!   (our signed packages go to nodes we control, never the store path);
//! - at most [`SEED_CONCURRENCY_CAP`] cogs run at once;
//! - installed cogs auto-start, so `load` stops a cog it just installed and
//!   `start` is the explicit transition;
//! - a console run needs the feed to itself (UDP 5006 contention): the
//!   adapter lists running cogs as [`Preemption`]s, the host gates and
//!   stops each, runs the console cycle, then restarts them;
//! - `unload` uninstalls only a cog this adapter installed.
//!
//! Governance runs in WeftOS at the adapter ([`super::WorkloadHost`]).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use clawft_types::placement::{AttrValue, Capability, CapabilityId, Provenance};
use serde_json::{Value, json};
use tokio::sync::Mutex;

#[path = "seed_install.rs"]
mod install;

use super::evidence::RunEvidence;
use super::seed_http::{Method, SeedCredentials, SeedTransport};
use super::types::{
    Admission, ControlMode, InstanceHandle, InstanceState, InstanceStatus, Preemption,
    RuntimeError, VerifiedWorkload, WorkloadConfig, WorkloadRuntime, WorkloadSource,
};
use crate::workload_governance::NetworkPolicy;
use crate::workload_pkg::manifest::{valid_cog_id, valid_token};

use super::seed_types::lines;
pub use super::seed_types::{
    API_TIMEOUT, CONSOLE_TIMEOUT, InstalledCog, LOG_LINES, SEED_CONCURRENCY_CAP, SEED_ID,
    SEED_REGISTRY, SeedConfig, SeedPin,
};

struct Instance {
    cog_id: String,
    installed_here: bool,
    console_commands: Vec<String>,
    last: Option<RunEvidence>,
}

/// The Seed runtime adapter.
pub struct SeedApiRuntime {
    pub(super) cfg: SeedConfig,
    pub(super) transport: Arc<dyn SeedTransport>,
    pub(super) creds: Arc<dyn SeedCredentials>,
    instances: Mutex<HashMap<String, Instance>>,
}

impl SeedApiRuntime {
    /// New adapter. Refuses a malformed node id or pin.
    pub fn new(
        cfg: SeedConfig,
        transport: Arc<dyn SeedTransport>,
        creds: Arc<dyn SeedCredentials>,
    ) -> Result<Self, RuntimeError> {
        if !valid_token(&cfg.node_id, 64) {
            return Err(RuntimeError::InvalidConfig(
                "seed node id must be a plain token".into(),
            ));
        }
        for p in &cfg.pins {
            VerifiedWorkload::store_pin(SEED_REGISTRY, &p.id, &p.version, p.sha256.as_deref())?;
        }
        Ok(Self {
            cfg,
            transport,
            creds,
            instances: Mutex::new(HashMap::new()),
        })
    }

    fn pin_for(&self, w: &VerifiedWorkload) -> Result<&SeedPin, RuntimeError> {
        let WorkloadSource::StorePin { registry, sha256 } = &w.source else {
            return Err(RuntimeError::AdmissionRefused(
                "the Seed store path installs only operator-pinned store cogs; \
                 signed packages go to native or container nodes"
                    .into(),
            ));
        };
        if registry != SEED_REGISTRY {
            return Err(RuntimeError::AdmissionRefused(format!(
                "unknown registry {registry:?}"
            )));
        }
        let pin = self
            .cfg
            .pins
            .iter()
            .find(|p| p.id == w.id && p.version == w.version)
            .ok_or_else(|| {
                RuntimeError::AdmissionRefused(format!(
                    "{}@{} is not pinned by the operator for this Seed",
                    w.id, w.version
                ))
            })?;
        if let (Some(a), Some(b)) = (sha256, &pin.sha256)
            && a != b
        {
            return Err(RuntimeError::AdmissionRefused(
                "store pin sha256 differs from the operator pin".into(),
            ));
        }
        Ok(pin)
    }

    async fn instance_cog(&self, h: &InstanceHandle) -> Result<String, RuntimeError> {
        self.instances
            .lock()
            .await
            .get(&h.instance_id)
            .map(|i| i.cog_id.clone())
            .ok_or_else(|| RuntimeError::UnknownInstance(h.instance_id.clone()))
    }
}

#[async_trait]
impl WorkloadRuntime for SeedApiRuntime {
    fn id(&self) -> &str {
        SEED_ID
    }

    fn provides(&self) -> Vec<Capability> {
        // Adapter-attested: provenance `claimed`, never `probed`.
        let mut out = Vec::new();
        if let Ok(id) = CapabilityId::new("runtime.remote.api") {
            out.push(
                Capability::new(id, Provenance::Claimed)
                    .with_attr("vendor", "cognitum")
                    .with_attr("device", "seed")
                    .with_attr("concurrency_cap", self.cfg.concurrency_cap as i64)
                    .with_attr("arches_native", AttrValue::List(vec!["armv7".into()])),
            );
        }
        if let Ok(id) = CapabilityId::new("node.class.cognitum-seed") {
            out.push(Capability::new(id, Provenance::Claimed));
        }
        out
    }

    async fn admit(&self, w: &VerifiedWorkload) -> Result<Admission, RuntimeError> {
        let pin = self.pin_for(w)?.clone();
        let store = self
            .api(Method::Get, "/api/v1/apps/available", None, API_TIMEOUT)
            .await?;
        let entry = store
            .get("cogs")
            .and_then(Value::as_array)
            .and_then(|c| {
                c.iter()
                    .find(|e| e.get("id").and_then(Value::as_str) == Some(&w.id))
            })
            .ok_or_else(|| {
                RuntimeError::AdmissionRefused(format!(
                    "the Seed store registry does not list {}",
                    w.id
                ))
            })?;
        let version = entry.get("version").and_then(Value::as_str).unwrap_or("");
        if version != pin.version {
            return Err(RuntimeError::AdmissionRefused(format!(
                "store has {}@{version}, operator pinned {}",
                w.id, pin.version
            )));
        }
        if let Some(want) = &pin.sha256
            && entry.get("sha256").and_then(Value::as_str) != Some(want.as_str())
        {
            return Err(RuntimeError::AdmissionRefused(
                "store sha256 differs from the pin".into(),
            ));
        }
        Ok(Admission {
            runtime: SEED_ID.into(),
            arch: "armv7".into(),
            emulated: false,
            notes: vec![format!("store {}@{version} matches operator pin", w.id)],
        })
    }

    async fn load(
        &self,
        w: &VerifiedWorkload,
        cfg: &WorkloadConfig,
    ) -> Result<InstanceHandle, RuntimeError> {
        if !cfg.args.is_empty() {
            return Err(RuntimeError::InvalidConfig(
                "Seed cogs take their settings from the Seed config API, not argv".into(),
            ));
        }
        let pin = self.pin_for(w)?.clone();
        let iid = format!("{}-seed-{}", w.id, cfg.node_id);
        if !valid_cog_id(&w.id) || !valid_token(&iid, 160) {
            return Err(RuntimeError::InvalidConfig("bad instance id".into()));
        }
        if self.instances.lock().await.contains_key(&iid) {
            return Err(RuntimeError::InvalidState(format!(
                "{iid} is already loaded"
            )));
        }
        let installed = self.installed().await?;
        let installed_here = match installed.iter().find(|c| c.id == w.id) {
            Some(c) if c.version == pin.version => false,
            Some(c) => {
                return Err(RuntimeError::AdmissionRefused(format!(
                    "{} is installed at {} but the pin is {}",
                    w.id, c.version, pin.version
                )));
            }
            None => {
                self.admit(w).await?;
                // Installed cogs auto-start, so an install is a start too:
                // it must fit under the concurrency cap.
                let running = installed.iter().filter(|c| c.running).count();
                if running >= self.cfg.concurrency_cap {
                    return Err(RuntimeError::AdmissionRefused(format!(
                        "installing {} auto-starts it, but the Seed concurrency cap {} \
                         is reached ({running} running)",
                        w.id, self.cfg.concurrency_cap
                    )));
                }
                self.install_reconciled(&w.id).await?;
                // Installed cogs auto-start; loading must not leave it running.
                if let Err(e) = self.stop_cog(&w.id).await {
                    return Err(self.undo_install(w, &iid, &pin, e).await);
                }
                true
            }
        };
        self.instances.lock().await.insert(
            iid.clone(),
            Instance {
                cog_id: w.id.clone(),
                installed_here,
                console_commands: pin.console_commands.clone(),
                last: None,
            },
        );
        Ok(InstanceHandle {
            runtime: SEED_ID.into(),
            instance_id: iid,
            workload_id: w.id.clone(),
            store_installed: installed_here,
        })
    }

    async fn start(&self, h: &InstanceHandle) -> Result<(), RuntimeError> {
        let id = self.instance_cog(h).await?;
        let installed = self.installed().await?;
        if installed.iter().any(|c| c.id == id && c.running) {
            return Ok(());
        }
        let running = installed.iter().filter(|c| c.running).count();
        if running >= self.cfg.concurrency_cap {
            return Err(RuntimeError::AdmissionRefused(format!(
                "Seed concurrency cap {} reached ({running} running)",
                self.cfg.concurrency_cap
            )));
        }
        self.start_cog(&id).await
    }

    async fn stop(
        &self,
        h: &InstanceHandle,
        _grace: Duration,
    ) -> Result<RunEvidence, RuntimeError> {
        let id = self.instance_cog(h).await?;
        let t0 = Instant::now();
        self.stop_cog(&id).await?;
        let logs = self
            .api(
                Method::Get,
                &format!("/api/v1/apps/{id}/logs?lines={LOG_LINES}"),
                None,
                API_TIMEOUT,
            )
            .await
            .unwrap_or(Value::Null);
        let (out, err) = (lines(logs.get("output")), lines(logs.get("errors")));
        let ev = RunEvidence {
            runtime: SEED_ID.into(),
            instance_id: h.instance_id.clone(),
            elapsed_ms: t0.elapsed().as_millis() as u64,
            stdout_bytes: out.len() as u64,
            stderr_bytes: err.len() as u64,
            stdout: out,
            stderr: err,
            ..RunEvidence::default()
        };
        if let Some(i) = self.instances.lock().await.get_mut(&h.instance_id) {
            i.last = Some(ev.clone());
        }
        Ok(ev)
    }

    async fn unload(&self, h: InstanceHandle) -> Result<(), RuntimeError> {
        let (cog, installed_here) = self
            .instances
            .lock()
            .await
            .get(&h.instance_id)
            .map(|i| (i.cog_id.clone(), i.installed_here))
            .ok_or_else(|| RuntimeError::UnknownInstance(h.instance_id.clone()))?;
        if installed_here {
            // Forgotten only once the uninstall succeeded, so a failed one
            // can be retried.
            self.api(
                Method::Delete,
                &format!("/api/v1/apps/{cog}"),
                None,
                API_TIMEOUT,
            )
            .await?;
        }
        self.instances.lock().await.remove(&h.instance_id);
        Ok(())
    }

    async fn status(&self, h: &InstanceHandle) -> InstanceStatus {
        let Ok(id) = self.instance_cog(h).await else {
            return InstanceStatus::of(InstanceState::Unknown);
        };
        match self.installed().await {
            Ok(list) => match list.iter().find(|c| c.id == id) {
                Some(c) if c.running => InstanceStatus::of(InstanceState::Running),
                Some(_) => InstanceStatus::of(InstanceState::Loaded),
                None => InstanceStatus::of(InstanceState::Unknown),
            },
            Err(e) => InstanceStatus {
                state: InstanceState::Degraded,
                exit_code: None,
                detail: Some(e.to_string()),
            },
        }
    }

    fn control_mode(&self) -> ControlMode {
        ControlMode::Managed
    }

    /// The Seed firmware, not WeftOS, decides what its cogs can reach; the
    /// Seed has internet access, so the gate is told `egress`.
    fn network_exposure(&self) -> NetworkPolicy {
        NetworkPolicy::Egress
    }

    /// Every running cog, including ones WeftOS did not load (UDP 5006
    /// contention: any listener on the Seed competes for the feed).
    async fn console_preemptions(
        &self,
        h: &InstanceHandle,
    ) -> Result<Vec<Preemption>, RuntimeError> {
        self.instance_cog(h).await?;
        Ok(self
            .installed()
            .await?
            .into_iter()
            .filter(|c| c.running)
            .map(|c| Preemption {
                registry: SEED_REGISTRY.into(),
                workload_id: c.id,
                version: c.version,
            })
            .collect())
    }

    async fn preempt(&self, p: &Preemption) -> Result<(), RuntimeError> {
        self.stop_cog(&p.workload_id).await
    }

    async fn resume(&self, p: &Preemption) -> Result<(), RuntimeError> {
        self.start_cog(&p.workload_id).await
    }

    async fn console(
        &self,
        h: &InstanceHandle,
        command: &str,
    ) -> Result<RunEvidence, RuntimeError> {
        let (id, allowed) = {
            let map = self.instances.lock().await;
            let i = map
                .get(&h.instance_id)
                .ok_or_else(|| RuntimeError::UnknownInstance(h.instance_id.clone()))?;
            (i.cog_id.clone(), i.console_commands.clone())
        };
        if !allowed.iter().any(|c| c == command) {
            return Err(RuntimeError::InvalidConfig(format!(
                "console command {command:?} is not pinned"
            )));
        }
        // UDP 5006 contention: the host preempts running cogs (each gated
        // as `workload.stop`) before calling this; never stop them here.
        let running: Vec<String> = self
            .installed()
            .await?
            .into_iter()
            .filter(|c| c.running)
            .map(|c| c.id)
            .collect();
        if !running.is_empty() {
            return Err(RuntimeError::InvalidState(format!(
                "cogs still running on the Seed ({}); a console run needs the feed to itself",
                running.join(", ")
            )));
        }
        let t0 = Instant::now();
        let v = self
            .api(
                Method::Post,
                &format!("/api/v1/apps/{id}/console"),
                Some(&json!({ "command": command })),
                CONSOLE_TIMEOUT,
            )
            .await?;
        let out = ["output", "stdout"]
            .iter()
            .map(|k| lines(v.get(*k)))
            .find(|s| !s.is_empty())
            .unwrap_or_else(|| v.to_string());
        let err = ["errors", "stderr"]
            .iter()
            .map(|k| lines(v.get(*k)))
            .find(|s| !s.is_empty())
            .unwrap_or_default();
        let code = ["exit_code", "code"]
            .iter()
            .find_map(|k| v.get(*k).and_then(Value::as_i64))
            .map(|c| c as i32);
        Ok(RunEvidence {
            runtime: SEED_ID.into(),
            instance_id: h.instance_id.clone(),
            args: command.split_whitespace().map(str::to_string).collect(),
            exit_code: code,
            elapsed_ms: t0.elapsed().as_millis() as u64,
            stdout_bytes: out.len() as u64,
            stderr_bytes: err.len() as u64,
            stdout: out,
            stderr: err,
            ..RunEvidence::default()
        })
    }
}
