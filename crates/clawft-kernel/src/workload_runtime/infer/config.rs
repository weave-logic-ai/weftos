//! Adapter configuration: adopted or managed, and what managed needs.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::spec::InferFlavor;
use crate::model_manifest::ModelRegistry;

/// Backoff for restarting a managed server that died while it should run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestartPolicy {
    /// Restarts before giving up (the operator then sees `Exited`).
    pub max_restarts: u32,
    /// Delay before the first restart; doubles each time.
    pub base: Duration,
    /// Longest delay.
    pub cap: Duration,
    /// How long a process must have been up (and answering) before the
    /// restart count is cleared. A server that answers one probe and then
    /// dies is still crash-looping and must reach `GaveUp`.
    pub stable_after: Duration,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self {
            max_restarts: 5,
            base: Duration::from_secs(2),
            cap: Duration::from_secs(60),
            stable_after: Duration::from_secs(60),
        }
    }
}

impl RestartPolicy {
    /// Delay before restart number `attempt` (0-based).
    pub fn delay(&self, attempt: u32) -> Duration {
        self.base
            .saturating_mul(1u32 << attempt.min(16))
            .min(self.cap)
    }
}

/// What managed mode needs.
#[derive(Clone)]
pub struct ManagedConfig {
    /// The launcher script (`serve-llamacpp`, `serve`). Not used by Ollama,
    /// which manages its own process.
    pub serve_program: Option<PathBuf>,
    /// Optional capability probe command (`llama-server --version`); exit 0
    /// is a pass and the first stdout line is recorded as the version.
    pub version_probe: Option<(PathBuf, Vec<String>)>,
    /// Where per-instance working directories are created.
    pub data_root: PathBuf,
    /// Adopted model manifests; the only source of weight paths.
    pub models: Arc<ModelRegistry>,
    /// Extra environment for the server (the parent's is cleared).
    pub env: Vec<(String, String)>,
    /// Names of parent environment variables to forward (`HOME`,
    /// `HF_HOME`, `PATH`). The default forwards the daemon's `PATH` and
    /// `HOME` to the launcher: the server runs with the daemon's search
    /// path and home directory. A forwarded or explicit `env` `PATH` wins
    /// over the supervisor's default one.
    pub env_passthrough: Vec<String>,
    /// `(uid, gid)` to drop to when the host runs as root.
    pub run_as: Option<(u32, u32)>,
    /// Restart backoff.
    pub restart: RestartPolicy,
    /// Ollama `keep_alive` sent when loading a model (`5m`, `30m`, `-1`).
    pub keep_alive: String,
    /// Unified-memory budget and co-residency, shared by every managed
    /// adapter on the node. `None`: no check (a single adapter, or the
    /// operator did not ask for one).
    pub ledger: Option<Arc<super::residency::ResidencyLedger>>,
}

impl ManagedConfig {
    /// Defaults: no launcher, `HOME` and `PATH` forwarded, default backoff.
    pub fn new(models: Arc<ModelRegistry>, data_root: PathBuf) -> Self {
        Self {
            serve_program: None,
            version_probe: None,
            data_root,
            models,
            env: Vec::new(),
            env_passthrough: vec!["HOME".into(), "PATH".into()],
            run_as: None,
            restart: RestartPolicy::default(),
            keep_alive: "30m".into(),
            ledger: None,
        }
    }

    /// Share a residency ledger (budget and co-residency).
    pub fn with_ledger(mut self, ledger: Arc<super::residency::ResidencyLedger>) -> Self {
        self.ledger = Some(ledger);
        self
    }

    /// Launcher script.
    pub fn with_serve_program(mut self, p: impl Into<PathBuf>) -> Self {
        self.serve_program = Some(p.into());
        self
    }
}

/// The model lab's launcher for `flavor` under `home` (`<home>/llm/bin/...`).
/// Only builds the path; nothing is read or checked.
pub fn lab_serve_program(home: &Path, flavor: InferFlavor) -> Option<PathBuf> {
    let name = match flavor {
        InferFlavor::LlamaCpp => "serve-llamacpp",
        InferFlavor::MlxLm => "serve",
        InferFlavor::Ollama => return None,
    };
    Some(home.join("llm").join("bin").join(name))
}

/// Observe-only, or launch and control.
///
/// Adopted mode is observe-only. Everything it does is a read against a
/// loopback address taken from the spec, and that includes `admit`, which
/// the host calls before the governance gate (an admission self-check is
/// read-only by contract), so a spec's loopback port is probed with GETs
/// before any permit is consulted. [`InferConfig::allowed_ports`] bounds
/// which ports that can be.
#[derive(Clone)]
pub enum InferMode {
    /// Register and health-check a server already running; never start,
    /// stop or signal it.
    Adopted,
    /// Start, stop, restart and unload the server.
    Managed(Box<ManagedConfig>),
}

/// One adapter's configuration.
#[derive(Clone)]
pub struct InferConfig {
    /// Server software.
    pub flavor: InferFlavor,
    /// Adopted or managed.
    pub mode: InferMode,
    /// Per-request timeout of probes.
    pub probe_timeout: Duration,
    /// Port the capability probe asks (the flavor's conventional port when
    /// absent). Instances carry their own ports.
    pub probe_port: Option<u16>,
    /// Inclusive port range instances may use. Checked before anything is
    /// probed: a spec naming a port outside it never causes a connection.
    pub allowed_ports: Option<(u16, u16)>,
    /// Timeout of [`InferRuntime::deep_health`](super::InferRuntime::deep_health)
    /// (a cold model can take minutes to answer its first token).
    pub deep_timeout: Duration,
}

impl InferConfig {
    /// Limit instances to ports in `lo..=hi`.
    pub fn with_port_range(mut self, lo: u16, hi: u16) -> Self {
        self.allowed_ports = Some((lo, hi));
        self
    }

    /// Probe a different port for capabilities.
    pub fn with_probe_port(mut self, port: u16) -> Self {
        self.probe_port = Some(port);
        self
    }

    /// An adopting adapter.
    pub fn adopted(flavor: InferFlavor) -> Self {
        Self {
            flavor,
            mode: InferMode::Adopted,
            probe_timeout: Duration::from_secs(2),
            probe_port: None,
            allowed_ports: None,
            deep_timeout: Duration::from_secs(300),
        }
    }

    /// A managing adapter.
    pub fn managed(flavor: InferFlavor, cfg: ManagedConfig) -> Self {
        Self {
            flavor,
            mode: InferMode::Managed(Box::new(cfg)),
            probe_timeout: Duration::from_secs(2),
            probe_port: None,
            allowed_ports: None,
            deep_timeout: Duration::from_secs(300),
        }
    }
}
