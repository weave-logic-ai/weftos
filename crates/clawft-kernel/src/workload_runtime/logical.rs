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
    Admission, ControlMode, InstanceHandle, InstanceState, InstanceStatus, RuntimeError, VerifiedWorkload,
    WorkloadConfig, WorkloadRuntime, WorkloadSource,
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
    /// OS process id.
    pub pid: u32,
}

/// What a launcher can say about a child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildProbe {
    /// Alive.
    Running {
        /// OS process id.
        pid: u32,
    },
    /// Gone, with how it ended.
    Exited {
        /// Exit code, when it exited normally.
        code: Option<i32>,
        /// Signal, when it was killed.
        signal: Option<i32>,
    },
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
    async fn terminate(&self, child: &ChildRef, grace: Duration) -> Result<Option<i32>, RuntimeError>;
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

    async fn load(&self, w: &VerifiedWorkload, _cfg: &WorkloadConfig) -> Result<InstanceHandle, RuntimeError> {
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
            ChildProbe::Running { .. }
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
                ChildProbe::Running { pid } => Some(ChildRef {
                    project_id: inst.spec.project_id.clone(),
                    pid,
                }),
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
            ChildProbe::Running { .. }
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
            ChildProbe::Running { pid } => InstanceStatus {
                state: InstanceState::Running,
                exit_code: None,
                detail: Some(format!("pid {pid}")),
            },
            ChildProbe::Exited { code, signal } => InstanceStatus {
                state: InstanceState::Exited,
                exit_code: code,
                detail: signal.map(|s| format!("signal {s}")),
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
        Err(RuntimeError::Unsupported(
            "a project kernel has no console cycle".into(),
        ))
    }
}

#[cfg(test)]
mod wasmtime_lifecycle_tests {
    use super::super::{HostContract, ProjectPayload, RunMode};
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    #[derive(Default)]
    struct Runner {
        live: AtomicBool,
    }
    #[async_trait]
    impl ChildLauncher for Runner {
        async fn spawn(&self, spec: &ChildSpec) -> Result<ChildRef, RuntimeError> {
            assert_eq!(spec.adapter, "wasmtime-project-v1");
            self.live.store(true, Ordering::SeqCst);
            Ok(ChildRef {
                project_id: spec.project_id.clone(),
                pid: 4242,
            })
        }
        async fn terminate(&self, _: &ChildRef, _: Duration) -> Result<Option<i32>, RuntimeError> {
            self.live.store(false, Ordering::SeqCst);
            Ok(Some(0))
        }
        async fn probe(&self, _: &str) -> ChildProbe {
            if self.live.load(Ordering::SeqCst) {
                ChildProbe::Running { pid: 4242 }
            } else {
                ChildProbe::NotStarted
            }
        }
    }
    #[tokio::test]
    async fn wasmtime_identity_survives_load_start_stop_and_rejects_logical_admission() {
        let launcher = Arc::new(Runner::default());
        let wasm = LogicalRuntime::wasmtime_project(launcher.clone());
        let logical = LogicalRuntime::new(launcher);
        let w = VerifiedWorkload {
            kind: "project".into(),
            id: "p".into(),
            version: "cert-1".into(),
            source: WorkloadSource::Project(ProjectPayload {
                adapter: "wasmtime-project-v1".into(),
                project_id: "p".into(),
                key_id: "k".into(),
                cert_serial: 1,
                user_key_id: "u".into(),
                policy_hash: String::new(),
                root: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")),
            }),
        };
        assert!(logical.admit(&w).await.is_err());
        assert_eq!(wasm.admit(&w).await.unwrap().arch, "wasm32-wasip1");
        let cfg = WorkloadConfig {
            mode: RunMode::Listener,
            args: vec![],
            host: HostContract::default_feed(),
            node_id: "p".into(),
        };
        let h = wasm.load(&w, &cfg).await.unwrap();
        assert_eq!(h.runtime, "wasmtime-project-v1");
        wasm.start(&h).await.unwrap();
        assert_eq!(wasm.status(&h).await.state, InstanceState::Running);
        assert!(wasm.start(&h).await.is_err());
        let evidence = wasm.stop(&h, Duration::ZERO).await.unwrap();
        assert_eq!(evidence.runtime, "wasmtime-project-v1");
        assert_eq!(evidence.exit_code, Some(0));
        wasm.unload(h).await.unwrap();
    }
}
