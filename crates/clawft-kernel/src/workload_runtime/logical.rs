//! The `logical` adapter: a per-project child kernel as a workload
//! (ADR-103 A6, Phase 2 package G).
//!
//! `logical` owns no process. It admits a [`WorkloadSource::Project`]
//! workload and delegates the OS side to a [`ChildLauncher`] that the weave
//! supervisor implements, so the kernel crate never depends on weave. Every
//! transition still goes through [`super::WorkloadHost`], i.e. the workload
//! gate and the chain.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use clawft_types::placement::{AttrValue, Capability, CapabilityId, Provenance};
use tokio::sync::Mutex;

use super::evidence::RunEvidence;
use super::types::{
    Admission, ControlMode, InstanceHandle, InstanceState, InstanceStatus, RuntimeError,
    VerifiedWorkload, WorkloadConfig, WorkloadRuntime, WorkloadSource,
};
use crate::workload_governance::NetworkPolicy;

/// Adapter id.
pub const LOGICAL_ID: &str = "logical";
/// The capability the adapter advertises and the `project` kind requires.
pub const CAP_PROJECT_LOGICAL: &str = "runtime.project.logical";

/// What a launcher needs to start one child kernel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildSpec {
    /// Immutable adapter selection; restart cannot reinterpret the manifest.
    pub adapter: String,
    /// Project id (ULID).
    pub project_id: String,
    /// Canonical project root.
    pub root: PathBuf,
    /// Key id of the project key the certificate names.
    pub key_id: String,
    /// Serial of the certificate in force.
    pub cert_serial: u64,
}

/// A running (or adopted) child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildRef {
    /// Project id.
    pub project_id: String,
    /// Identity selected by the launcher; a PID is never proof of a container.
    pub identity: ChildIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildIdentity {
    Native { host_pid: u32 },
    Container { engine: String, immutable_container_id: String, host_pid: u32 },
}

impl ChildIdentity {
    pub fn host_pid(&self) -> u32 {
        match self { Self::Native { host_pid } | Self::Container { host_pid, .. } => *host_pid }
    }
}

/// What a launcher can say about a child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildProbe {
    /// Alive.
    Running {
        identity: ChildIdentity,
    },
    /// Gone, with how it ended.
    Exited {
        /// Exit code, when it exited normally.
        code: Option<i32>,
        /// Signal, when it was killed.
        signal: Option<i32>,
    },
    /// Engine identity or liveness could not be established. Callers must
    /// neither report readiness nor start a replacement from this state.
    Unverifiable { reason: String },
    /// No child was ever started for this project.
    NotStarted,
}

/// The OS side of the `logical` adapter.
#[async_trait]
pub trait ChildLauncher: Send + Sync {
    /// Launch a child kernel for `spec`. Refuses a project that already has
    /// a live child.
    async fn spawn(&self, spec: &ChildSpec) -> Result<ChildRef, RuntimeError>;
    /// Stop it: graceful shutdown first, then terminate after `grace`.
    /// Returns the exit code when known.
    async fn terminate(&self, child: &ChildRef, grace: Duration)
    -> Result<Option<i32>, RuntimeError>;
    /// Current state of the project's child.
    async fn probe(&self, project_id: &str) -> ChildProbe;
}

struct Instance {
    spec: ChildSpec,
    child: Option<ChildRef>,
}

/// The adapter.
pub struct LogicalRuntime {
    adapter: &'static str,
    capability: &'static str,
    launcher: Arc<dyn ChildLauncher>,
    instances: Mutex<HashMap<String, Instance>>,
}

impl LogicalRuntime {
    /// Persistent Wasmtime project lifecycle, using a dedicated runner launcher.
    /// Shares process bookkeeping only; admission, capability and evidence carry
    /// the explicit Wasmtime identity and reject logical payloads.
    pub fn wasmtime_project(launcher: Arc<dyn ChildLauncher>) -> Self {
        Self {
            adapter: "wasmtime-project-v1",
            capability: "runtime.project.wasmtime",
            launcher,
            instances: Mutex::new(HashMap::new()),
        }
    }

    /// An adapter over `launcher`.
    pub fn new(launcher: Arc<dyn ChildLauncher>) -> Self {
        Self {
            adapter: LOGICAL_ID,
            capability: CAP_PROJECT_LOGICAL,
            launcher,
            instances: Mutex::new(HashMap::new()),
        }
    }
}

fn instance_id(project_id: &str) -> String {
    format!("project-{project_id}")
}

fn spec_of(w: &VerifiedWorkload) -> Result<ChildSpec, RuntimeError> {
    match &w.source {
        WorkloadSource::Project(p) => Ok(ChildSpec {
            adapter: p.adapter.clone(),
            project_id: p.project_id.clone(),
            root: p.root.clone(),
            key_id: p.key_id.clone(),
            cert_serial: p.cert_serial,
        }),
        _ => Err(RuntimeError::AdmissionRefused(format!(
            "{LOGICAL_ID} runs only project workloads"
        ))),
    }
}

#[async_trait]
impl WorkloadRuntime for LogicalRuntime {
    fn id(&self) -> &str {
        self.adapter
    }

    fn provides(&self) -> Vec<Capability> {
        let Ok(id) = CapabilityId::new(self.capability) else {
            return Vec::new();
        };
        vec![Capability::new(id, Provenance::Probed).with_attr("os", AttrValue::from(std::env::consts::OS))]
    }

    async fn admit(&self, w: &VerifiedWorkload) -> Result<Admission, RuntimeError> {
        let spec = spec_of(w)?;
        if spec.adapter != self.adapter {
            return Err(RuntimeError::AdmissionRefused("project adapter mismatch".into()));
        }
        if !spec.root.is_dir() {
            return Err(RuntimeError::AdmissionRefused(format!(
                "project root {} is gone",
                spec.root.display()
            )));
        }
        Ok(Admission {
            runtime: self.adapter.into(),
            arch: if self.adapter == LOGICAL_ID {
                std::env::consts::ARCH
            } else {
                "wasm32-wasip1"
            }
            .into(),
            emulated: false,
            notes: vec![format!("project {} root {}", spec.project_id, spec.root.display())],
        })
    }

    async fn load(
        &self,
        w: &VerifiedWorkload,
        _cfg: &WorkloadConfig,
    ) -> Result<InstanceHandle, RuntimeError> {
        self.admit(w).await?;
        let spec = spec_of(w)?;
        let iid = instance_id(&spec.project_id);
        let mut map = self.instances.lock().await;
        if map.contains_key(&iid) {
            return Err(RuntimeError::InvalidState(format!("{iid} is already loaded")));
        }
        let workload_id = spec.project_id.clone();
        map.insert(iid.clone(), Instance { spec, child: None });
        Ok(InstanceHandle {
            runtime: self.adapter.into(),
            instance_id: iid,
            workload_id,
            store_installed: false,
        })
    }

    async fn start(&self, h: &InstanceHandle) -> Result<(), RuntimeError> {
        let mut map = self.instances.lock().await;
        let inst = map
            .get_mut(&h.instance_id)
            .ok_or_else(|| RuntimeError::UnknownInstance(h.instance_id.clone()))?;
        if matches!(
            self.launcher.probe(&inst.spec.project_id).await,
            ChildProbe::Running { .. } | ChildProbe::Unverifiable { .. }
        ) {
            return Err(RuntimeError::InvalidState("already running".into()));
        }
        if !inst.spec.root.is_dir() {
            return Err(RuntimeError::AdmissionRefused(format!(
                "project root {} is gone",
                inst.spec.root.display()
            )));
        }
        inst.child = Some(self.launcher.spawn(&inst.spec).await?);
        Ok(())
    }

    async fn stop(&self, h: &InstanceHandle, grace: Duration) -> Result<RunEvidence, RuntimeError> {
        let mut map = self.instances.lock().await;
        let inst = map
            .get_mut(&h.instance_id)
            .ok_or_else(|| RuntimeError::UnknownInstance(h.instance_id.clone()))?;
        let started = std::time::Instant::now();
        let mut ev = RunEvidence {
            runtime: self.adapter.into(),
            instance_id: h.instance_id.clone(),
            ..RunEvidence::default()
        };
        // An adopted child was not started by this instance: stop whatever
        // the launcher says is running for the project.
        let child = match inst.child.take() {
            Some(c) => Some(c),
            None => match self.launcher.probe(&inst.spec.project_id).await {
                ChildProbe::Running { identity } => Some(ChildRef {
                    project_id: inst.spec.project_id.clone(),
                    identity,
                }),
                ChildProbe::Unverifiable { reason } => {
                    return Err(RuntimeError::InvalidState(format!("child unverifiable: {reason}")));
                }
                _ => None,
            },
        };
        if let Some(child) = child {
            ev.exit_code = self.launcher.terminate(&child, grace).await?;
        }
        ev.elapsed_ms = started.elapsed().as_millis() as u64;
        Ok(ev)
    }

    async fn unload(&self, h: InstanceHandle) -> Result<(), RuntimeError> {
        let mut map = self.instances.lock().await;
        let inst = map
            .get(&h.instance_id)
            .ok_or_else(|| RuntimeError::UnknownInstance(h.instance_id.clone()))?;
        if matches!(
            self.launcher.probe(&inst.spec.project_id).await,
            ChildProbe::Running { .. } | ChildProbe::Unverifiable { .. }
        ) {
            return Err(RuntimeError::InvalidState("stop the child before unloading".into()));
        }
        map.remove(&h.instance_id);
        Ok(())
    }

    async fn status(&self, h: &InstanceHandle) -> InstanceStatus {
        let map = self.instances.lock().await;
        let Some(inst) = map.get(&h.instance_id) else {
            return InstanceStatus::of(InstanceState::Unknown);
        };
        match self.launcher.probe(&inst.spec.project_id).await {
            ChildProbe::Running { identity } => InstanceStatus {
                state: InstanceState::Running,
                exit_code: None,
                detail: Some(match identity {
                    ChildIdentity::Native { host_pid } => format!("pid {host_pid}"),
                    ChildIdentity::Container { engine, immutable_container_id, .. } => format!("{engine} container {immutable_container_id}"),
                }),
            },
            ChildProbe::Exited { code, signal } => InstanceStatus {
                state: InstanceState::Exited,
                exit_code: code,
                detail: signal.map(|s| format!("signal {s}")),
            },
            ChildProbe::Unverifiable { reason } => InstanceStatus {
                state: InstanceState::Unknown,
                exit_code: None,
                detail: Some(reason),
            },
            ChildProbe::NotStarted => InstanceStatus::of(InstanceState::Loaded),
        }
    }

    fn control_mode(&self) -> ControlMode {
        ControlMode::Managed
    }

    fn network_exposure(&self) -> NetworkPolicy {
        NetworkPolicy::None
    }

    async fn console(&self, _h: &InstanceHandle, _command: &str) -> Result<RunEvidence, RuntimeError> {
        Err(RuntimeError::Unsupported("a project kernel has no console cycle".into()))
    }
}
