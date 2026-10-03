//! [`InferRuntime`]: the `infer.llamacpp`, `infer.mlx-lm` and
//! `infer.ollama` adapters (ADR-101 section 4) behind one type.
//!
//! Two constructions of the same adapter:
//!
//! - **Adopted** ([`InferConfig::adopted`], id `infer.<x>.adopted`): the
//!   adapter registers and health-checks a server somebody else started.
//!   It never starts, stops or signals it: `start` only confirms the
//!   server answers, `stop` is refused, `unload` forgets the registration.
//! - **Managed** ([`InferConfig::managed`], id `infer.<x>`): llama.cpp and
//!   mlx-lm are launched through the model lab's launcher under
//!   [`Supervised`]; Ollama is driven through its API. Weights come only
//!   from the model registry ([`crate::model_manifest::ModelRegistry`]),
//!   never from a path in the spec.
//!
//! Adapters make no governance decisions; wrap one in
//! [`crate::workload_runtime::WorkloadHost`] to gate and chain it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use clawft_types::placement::Capability;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use super::capabilities;
use super::config::{InferConfig, InferMode, ManagedConfig};
use super::exposure::port_in_use;
use super::launch::{Launch, build_launch, check_format};
use super::ollama::SharedLoad;
use super::probe::{ServerClient, ServerReport};
use super::spec::{InferFlavor, InferenceSpec};
use crate::model_manifest::{ModelError, ResolvedModel};
use crate::workload_governance::NetworkPolicy;
use crate::workload_pkg::manifest::valid_token;
use crate::workload_runtime::evidence::RunEvidence;
use crate::workload_runtime::supervise::Supervised;
use crate::workload_runtime::types::{
    Admission, ControlMode, InstanceHandle, InstanceStatus, RuntimeError, VerifiedWorkload,
    WorkloadConfig, WorkloadRuntime,
};

/// How a managed instance is run.
pub(super) enum ManagedPlan {
    /// A launcher script under supervision.
    Process(Launch),
    /// Ollama, driven through its API; the tag is what it calls the model.
    Ollama { tag: String },
}

/// Managed-mode state of one instance.
pub(super) struct Managed {
    pub plan: ManagedPlan,
    /// Display name of the model (registry name).
    pub model_name: String,
    pub proc: Option<Supervised>,
    /// Set by `start`, cleared by `stop`: whether a dead server is a fault.
    pub wanted: bool,
    pub restarts: u32,
    pub exited_at: Option<Instant>,
    pub last: Option<RunEvidence>,
    /// Set when the server was found listening beyond loopback and was
    /// stopped for it; the instance will not start again until reloaded.
    pub exposed: Option<Vec<std::net::IpAddr>>,
    pub load: Option<(SharedLoad, JoinHandle<()>)>,
    /// Ollama: this adapter brought the model into memory (it was not
    /// resident when asked), so only then may it take it out again.
    pub we_loaded: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// One registered instance.
pub(super) struct Instance {
    pub spec: InferenceSpec,
    pub client: ServerClient,
    pub managed: Option<Managed>,
}

/// The result of planning a workload (shared by admit and load).
struct Planned {
    spec: InferenceSpec,
    managed: Option<(ManagedPlan, String)>,
    notes: Vec<String>,
}

/// The adapter.
pub struct InferRuntime {
    pub(super) cfg: InferConfig,
    id: String,
    caps: RwLock<Vec<Capability>>,
    pub(super) instances: Mutex<HashMap<String, Instance>>,
}

fn refuse(m: impl Into<String>) -> RuntimeError {
    RuntimeError::AdmissionRefused(m.into())
}

fn model_err(what: &str, e: ModelError) -> RuntimeError {
    refuse(format!("{what}: {e}"))
}

impl InferRuntime {
    /// A new adapter. It claims no capabilities until
    /// [`probe_capabilities`](Self::probe_capabilities) has run.
    pub fn new(cfg: InferConfig) -> Self {
        let id = match cfg.mode {
            InferMode::Adopted => format!("{}.adopted", cfg.flavor.id()),
            InferMode::Managed(_) => cfg.flavor.id().to_string(),
        };
        Self {
            cfg,
            id,
            caps: RwLock::new(Vec::new()),
            instances: Mutex::new(HashMap::new()),
        }
    }

    pub(super) fn managed_cfg(&self) -> Option<&ManagedConfig> {
        match &self.cfg.mode {
            InferMode::Managed(m) => Some(m),
            InferMode::Adopted => None,
        }
    }

    fn flavor(&self) -> InferFlavor {
        self.cfg.flavor
    }

    /// Check the runtime is there and refresh what `provides()` returns.
    pub async fn probe_capabilities(&self) -> Vec<Capability> {
        let port = self
            .cfg
            .probe_port
            .unwrap_or_else(|| self.flavor().default_port());
        let caps = capabilities::probe(&self.cfg, port).await;
        if let Ok(mut g) = self.caps.write() {
            *g = caps.clone();
        }
        caps
    }

    fn client_for(&self, spec: &InferenceSpec) -> Result<ServerClient, RuntimeError> {
        ServerClient::new(spec.base_url()?, self.cfg.probe_timeout)
    }

    fn resolve(&self, m: &ManagedConfig, name: &str) -> Result<ResolvedModel, RuntimeError> {
        m.models
            .resolve(name)
            .map_err(|e| model_err(&format!("model {name}"), e))
    }

    async fn plan(&self, w: &VerifiedWorkload) -> Result<Planned, RuntimeError> {
        let spec = w.inference_spec(&self.id)?.clone();
        spec.validate()?;
        if spec.runtime != self.flavor() {
            return Err(refuse(format!(
                "{} cannot run a {} workload",
                self.id,
                spec.runtime.id()
            )));
        }
        if let Some((lo, hi)) = self.cfg.allowed_ports {
            let p = spec.port()?;
            if !(lo..=hi).contains(&p) {
                return Err(refuse(format!(
                    "port {p} is outside this adapter's allowed range {lo}-{hi}"
                )));
            }
        }
        let client = self.client_for(&spec)?;
        let mut notes = vec![format!("endpoint {}", client.base())];
        let Some(m) = self.managed_cfg() else {
            // Adopted: the server must already answer.
            let r = client.probe(spec.runtime).await;
            if let super::probe::Health::Unreachable(why) = &r.health {
                return Err(refuse(format!(
                    "no {} server answering at {}: {why}",
                    spec.runtime.id(),
                    client.base()
                )));
            }
            notes.push(format!("adopting running server ({:?})", r.health));
            notes.push(format!("{} model(s) listed", r.models.len()));
            if let Some(v) = &r.version {
                notes.push(format!("version {v}"));
            }
            return Ok(Planned {
                spec,
                managed: None,
                notes,
            });
        };
        let name = spec
            .model
            .clone()
            .ok_or_else(|| refuse("managed mode needs a model"))?;
        let model = self.resolve(m, &name)?;
        check_format(spec.runtime, &model)?;
        notes.push(format!(
            "model {} ({}) verified",
            model.name,
            model.format.as_str()
        ));
        if spec.runtime == InferFlavor::Ollama {
            let r = client.probe(spec.runtime).await;
            if matches!(r.health, super::probe::Health::Unreachable(_)) {
                return Err(refuse(format!(
                    "ollama is not answering at {}; the adapter drives it but does not start it",
                    client.base()
                )));
            }
            let tag = model
                .body
                .source
                .ollama_tag
                .clone()
                .unwrap_or_else(|| model.name.clone());
            if !r.lists(&tag) {
                return Err(refuse(format!(
                    "ollama has no model {tag}: pull it first (the adapter never downloads)"
                )));
            }
            return Ok(Planned {
                spec,
                managed: Some((ManagedPlan::Ollama { tag }, model.name)),
                notes,
            });
        }
        let draft = match &spec.serve.draft_model {
            Some(d) => Some(self.resolve(m, d)?),
            None => None,
        };
        let prog: &PathBuf = m
            .serve_program
            .as_ref()
            .ok_or_else(|| refuse("managed mode needs a serve program"))?;
        if !capabilities_is_exec(prog) {
            return Err(refuse(format!(
                "serve program {} is not executable",
                prog.display()
            )));
        }
        let (ip, port) = (spec.bind_ip()?, spec.port()?);
        if port_in_use(ip, port).await {
            return Err(refuse(format!(
                "something is already listening on {ip}:{port}; adopt it instead of managing over it"
            )));
        }
        let probe_id = "plan";
        let launch = build_launch(spec.runtime, &spec, &model, draft.as_ref(), m, probe_id)?;
        notes.push(format!(
            "launch {} with {} args",
            launch.program.display(),
            launch.args.len()
        ));
        Ok(Planned {
            spec,
            managed: Some((ManagedPlan::Process(launch), model.name)),
            notes,
        })
    }
}

fn capabilities_is_exec(p: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// Instance id: `<role>-<8 hex of identity hash>-<node>`.
fn instance_id(spec: &InferenceSpec, mode: &str, node_id: &str) -> Result<String, RuntimeError> {
    if !valid_token(node_id, 64) {
        return Err(RuntimeError::InvalidConfig(
            "node id must be a plain token".into(),
        ));
    }
    let mut h = blake3::Hasher::new();
    h.update(spec.role.as_bytes());
    h.update(spec.runtime.id().as_bytes());
    h.update(spec.model.as_deref().unwrap_or("").as_bytes());
    h.update(spec.base_url()?.as_bytes());
    h.update(mode.as_bytes());
    Ok(format!(
        "{}-{}-{}",
        spec.role,
        &h.finalize().to_hex()[..8],
        node_id
    ))
}

#[async_trait]
impl WorkloadRuntime for InferRuntime {
    fn id(&self) -> &str {
        &self.id
    }

    fn provides(&self) -> Vec<Capability> {
        self.caps.read().map(|c| c.clone()).unwrap_or_default()
    }

    async fn admit(&self, w: &VerifiedWorkload) -> Result<Admission, RuntimeError> {
        let p = self.plan(w).await?;
        Ok(Admission {
            runtime: self.id.clone(),
            arch: std::env::consts::ARCH.into(),
            emulated: false,
            notes: p.notes,
        })
    }

    async fn load(
        &self,
        w: &VerifiedWorkload,
        cfg: &WorkloadConfig,
    ) -> Result<InstanceHandle, RuntimeError> {
        if cfg.mode != crate::workload_runtime::types::RunMode::Listener || !cfg.args.is_empty() {
            return Err(RuntimeError::InvalidConfig(
                "inference instances run as listeners and take their arguments from the spec"
                    .into(),
            ));
        }
        let mode = if self.managed_cfg().is_some() {
            "managed"
        } else {
            "adopted"
        };
        let p = self.plan(w).await?;
        let id = instance_id(&p.spec, mode, &cfg.node_id)?;
        let client = self.client_for(&p.spec)?;
        let mut managed = None;
        if let Some((plan, model_name)) = p.managed {
            let mut plan = plan;
            if let (ManagedPlan::Process(l), Some(m)) = (&mut plan, self.managed_cfg()) {
                l.dir = m.data_root.join(&id);
                std::fs::create_dir_all(&l.dir).map_err(|e| {
                    RuntimeError::Backend(format!("create {}: {e}", l.dir.display()))
                })?;
            }
            managed = Some(Managed {
                plan,
                model_name,
                proc: None,
                wanted: false,
                restarts: 0,
                exited_at: None,
                last: None,
                exposed: None,
                load: None,
                we_loaded: Default::default(),
            });
        }
        let mut g = self.instances.lock().await;
        if g.contains_key(&id) {
            return Err(RuntimeError::InvalidState(format!(
                "{id} is already loaded"
            )));
        }
        let (ip, port) = (p.spec.bind_ip()?, p.spec.port()?);
        if g.values().any(|i| {
            i.spec.bind_ip().ok() == Some(ip)
                && i.spec.port().ok() == Some(port)
                && (managed.is_some() || i.managed.is_some())
        }) {
            return Err(refuse(format!(
                "{ip}:{port} is already used by another instance"
            )));
        }
        g.insert(
            id.clone(),
            Instance {
                spec: p.spec.clone(),
                client,
                managed,
            },
        );
        Ok(InstanceHandle {
            runtime: self.id.clone(),
            instance_id: id,
            workload_id: p.spec.role,
            store_installed: false,
        })
    }

    async fn start(&self, h: &InstanceHandle) -> Result<(), RuntimeError> {
        self.start_instance(h).await
    }

    async fn stop(&self, h: &InstanceHandle, grace: Duration) -> Result<RunEvidence, RuntimeError> {
        self.stop_instance(h, grace).await
    }

    async fn unload(&self, h: InstanceHandle) -> Result<(), RuntimeError> {
        self.unload_instance(&h).await
    }

    async fn status(&self, h: &InstanceHandle) -> InstanceStatus {
        self.status_of(h).await
    }

    fn control_mode(&self) -> ControlMode {
        match self.cfg.mode {
            InferMode::Adopted => ControlMode::Adopted,
            InferMode::Managed(_) => ControlMode::Managed,
        }
    }

    /// A managed server binds loopback only (a wider bind is refused at
    /// validation). An adopted server's own binding cannot be observed
    /// from here (the model lab starts its servers with `--host 0.0.0.0`),
    /// so it is reported as LAN-reachable rather than assumed private.
    fn network_exposure(&self) -> NetworkPolicy {
        match self.cfg.mode {
            InferMode::Adopted => NetworkPolicy::Lan,
            InferMode::Managed(_) => NetworkPolicy::None,
        }
    }

    async fn adopt(&self, h: &InstanceHandle, w: &VerifiedWorkload) -> Result<(), RuntimeError> {
        if self.managed_cfg().is_some() {
            return Err(RuntimeError::Unsupported(format!(
                "{} cannot re-adopt {}: a managed server's process is not recoverable",
                self.id, h.instance_id
            )));
        }
        let p = self.plan(w).await?;
        let client = self.client_for(&p.spec)?;
        self.instances
            .lock()
            .await
            .entry(h.instance_id.clone())
            .or_insert(Instance {
                spec: p.spec,
                client,
                managed: None,
            });
        Ok(())
    }

    async fn console(&self, h: &InstanceHandle, _c: &str) -> Result<RunEvidence, RuntimeError> {
        Err(RuntimeError::Unsupported(format!(
            "{} has no console run ({})",
            self.id, h.instance_id
        )))
    }
}

impl InferRuntime {
    /// Full probe of an instance's server.
    pub async fn health(&self, h: &InstanceHandle) -> Result<ServerReport, RuntimeError> {
        let (client, flavor) = {
            let g = self.instances.lock().await;
            let i = g
                .get(&h.instance_id)
                .ok_or_else(|| RuntimeError::UnknownInstance(h.instance_id.clone()))?;
            (i.client.clone(), i.spec.runtime)
        };
        Ok(client.probe(flavor).await)
    }

    /// Base URL of an instance (`http://127.0.0.1:<port>`): what the
    /// stable-address proxy forwards to.
    pub async fn endpoint(&self, h: &InstanceHandle) -> Option<String> {
        self.instances
            .lock()
            .await
            .get(&h.instance_id)
            .map(|i| i.client.base().to_string())
    }

    /// The model id the server itself knows the instance by (the Ollama tag,
    /// the registry name for a launched server, the spec's model when
    /// adopted). The mesh-serving side pins requests to it.
    pub async fn served_model(&self, h: &InstanceHandle) -> Option<String> {
        let g = self.instances.lock().await;
        let i = g.get(&h.instance_id)?;
        match &i.managed {
            Some(Managed {
                plan: ManagedPlan::Ollama { tag },
                ..
            }) => Some(tag.clone()),
            Some(m) => Some(m.model_name.clone()),
            None => i.spec.model.clone(),
        }
    }

    /// The spec of a loaded instance.
    pub async fn spec_of(&self, h: &InstanceHandle) -> Option<InferenceSpec> {
        self.instances
            .lock()
            .await
            .get(&h.instance_id)
            .map(|i| i.spec.clone())
    }

    /// Health plus one real token (ADR-101: "a tiny completion succeeds").
    /// Costs compute, so it is separate from [`status`](WorkloadRuntime::status).
    pub async fn deep_health(&self, h: &InstanceHandle) -> Result<(), RuntimeError> {
        let (client, model, flavor) = {
            let g = self.instances.lock().await;
            let i = g
                .get(&h.instance_id)
                .ok_or_else(|| RuntimeError::UnknownInstance(h.instance_id.clone()))?;
            let model = i
                .managed
                .as_ref()
                .map(|m| m.model_name.clone())
                .or_else(|| i.spec.model.clone());
            (i.client.clone(), model, i.spec.runtime)
        };
        let client = ServerClient::new(client.base().to_string(), self.cfg.deep_timeout)?;
        let report = client.probe(flavor).await;
        let model = model
            .or_else(|| report.models.first().cloned())
            .ok_or_else(|| RuntimeError::Backend("no model to ask".into()))?;
        client
            .tiny_completion(&model)
            .await
            .map_err(|e| RuntimeError::Backend(format!("deep health: {e}")))
    }
}
