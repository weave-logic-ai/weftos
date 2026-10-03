//! The inference workload spec (ADR-101 section 1) and its boundary
//! validation.

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

use crate::workload_pkg::manifest::valid_token;
use crate::workload_runtime::types::{
    InferencePayload, RuntimeError, VerifiedWorkload, WorkloadSource,
};

/// Workload kind string for inference endpoints.
pub const KIND_INFERENCE: &str = "inference";

/// Largest `extra_args` list and longest single argument.
const MAX_EXTRA_ARGS: usize = 32;
const MAX_ARG_LEN: usize = 256;

/// Which server software an adapter drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum InferFlavor {
    /// `llama-server` (OpenAI-compatible, GGUF), usually through `serve-llamacpp`.
    #[serde(rename = "infer.llamacpp")]
    LlamaCpp,
    /// `mlx_lm.server` (MLX quantisation directories), usually through `serve`.
    #[serde(rename = "infer.mlx-lm")]
    MlxLm,
    /// Ollama, which owns its own process and model load/unload.
    #[serde(rename = "infer.ollama")]
    Ollama,
}

impl InferFlavor {
    /// Runtime id (`infer.llamacpp`).
    pub fn id(self) -> &'static str {
        match self {
            Self::LlamaCpp => "infer.llamacpp",
            Self::MlxLm => "infer.mlx-lm",
            Self::Ollama => "infer.ollama",
        }
    }

    /// The capability a node advertises when this runtime works.
    pub fn capability(self) -> &'static str {
        match self {
            Self::LlamaCpp => "runtime.infer.llamacpp",
            Self::MlxLm => "runtime.infer.mlx-lm",
            Self::Ollama => "runtime.infer.ollama",
        }
    }

    /// Conventional port of a hand-started server. A default only: every
    /// test and every managed instance sets its own.
    pub fn default_port(self) -> u16 {
        match self {
            Self::LlamaCpp => 8090,
            Self::MlxLm => 8081,
            Self::Ollama => 11434,
        }
    }

    /// `format.*` capability suffixes this runtime can load.
    pub fn formats(self) -> &'static [&'static str] {
        match self {
            Self::LlamaCpp => &["gguf"],
            Self::MlxLm => &["mlx", "safetensors"],
            Self::Ollama => &["ollama", "gguf"],
        }
    }
}

/// `interactive` roles carry a latency bound; `batch` roles do not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LatencyClass {
    /// Voice and agent loops.
    Interactive,
    /// Throughput work.
    #[default]
    Batch,
}

/// Memory the instance is expected to hold (placement input).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryBudget {
    /// Weights.
    pub weights_bytes: u64,
    /// KV cache.
    pub kv_budget_bytes: u64,
}

/// Serve arguments. The adapter owns host and port; everything else is
/// mapped to the runtime's flags or passed through `extra_args`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServeArgs {
    /// Bind / probe address. Loopback only in v1 (a wider bind is a
    /// governed decision, ADR-101 section 7). Default `127.0.0.1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// Port; the runtime's conventional port when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Context length.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ctx: Option<u32>,
    /// KV quantisation (`q8_0`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kv_quant: Option<String>,
    /// Draft model for speculative decoding (a model registry name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft_model: Option<String>,
    /// Further runtime arguments, passed as plain argv (never a shell).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_args: Vec<String>,
}

/// An inference endpoint to adopt or run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InferenceSpec {
    /// Stable role name consumers use (`coder-daily`).
    pub role: String,
    /// Model registry name or package id. Required to manage an instance;
    /// optional when adopting (then only the server is observed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Server software.
    pub runtime: InferFlavor,
    /// Serve arguments.
    #[serde(default)]
    pub serve: ServeArgs,
    /// Expected memory.
    #[serde(default)]
    pub memory: MemoryBudget,
    /// Co-residency exclusions (`role:planner`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excludes: Vec<String>,
    /// Latency class.
    #[serde(default)]
    pub latency_class: LatencyClass,
    /// No automatic migration (warm KV).
    #[serde(default)]
    pub sticky: bool,
    /// API dialect.
    #[serde(default = "default_api")]
    pub api: String,
}

fn default_api() -> String {
    "openai-v1".into()
}

fn bad(m: impl Into<String>) -> RuntimeError {
    RuntimeError::InvalidConfig(m.into())
}

/// Flags a spec may pass through, with how many value tokens each takes.
/// An allowlist, exact spellings only: a denylist loses to argparse prefix
/// abbreviations (`--hos`), short aliases (`-md`, `-hf`, `-mu`), synonyms
/// (`--ctx-size`) and every flag that reads or writes files or opens a
/// listener (`--lora*`, `--mmproj`, `--api-key`, `--ssl-*`, `--path`,
/// `--log-file`, `--slot-save-path`). Host, port, model, draft, ctx and kv
/// are owned by the adapter and are not on any list.
fn allowed_flags(flavor: InferFlavor) -> &'static [(&'static str, usize)] {
    match flavor {
        InferFlavor::LlamaCpp => &[
            ("--threads", 1),
            ("-t", 1),
            ("--threads-batch", 1),
            ("--batch-size", 1),
            ("-b", 1),
            ("--ubatch-size", 1),
            ("-ub", 1),
            ("--parallel", 1),
            ("-np", 1),
            ("--n-gpu-layers", 1),
            ("-ngl", 1),
            ("--flash-attn", 0),
            ("-fa", 0),
            ("--cont-batching", 0),
            ("--mlock", 0),
            ("--no-mmap", 0),
            ("--jinja", 0),
            ("--metrics", 0),
            ("--no-webui", 0),
            ("--temp", 1),
            ("--top-p", 1),
            ("--top-k", 1),
            ("--repeat-penalty", 1),
            ("--seed", 1),
        ],
        InferFlavor::MlxLm => &[
            ("--max-tokens", 1),
            ("--temp", 1),
            ("--top-p", 1),
            ("--top-k", 1),
            ("--log-level", 1),
        ],
        // Ollama is configured through its own environment, not argv.
        InferFlavor::Ollama => &[],
    }
}

/// A value token: a plain word, or a number (which may be negative). Never
/// something that could be read as another flag, and no whitespace.
fn valid_flag_value(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= MAX_ARG_LEN
        && (!v.starts_with('-') || v.parse::<f64>().is_ok())
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:+-/".contains(&b))
}

fn validate_extra_args(flavor: InferFlavor, args: &[String]) -> Result<(), RuntimeError> {
    if args.len() > MAX_EXTRA_ARGS {
        return Err(bad("too many extra_args"));
    }
    let allowed = allowed_flags(flavor);
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a.bytes().any(|b| b.is_ascii_whitespace() || b == 0) {
            return Err(bad(format!(
                "extra_args entries are single tokens without whitespace: {a:?}"
            )));
        }
        let Some(&(_, arity)) = allowed.iter().find(|(f, _)| f == a) else {
            return Err(bad(format!(
                "extra_args {a:?} is not an allowed {} flag (exact spelling, value as a separate entry)",
                flavor.id()
            )));
        };
        for v in args.iter().skip(i + 1).take(arity) {
            if !valid_flag_value(v) {
                return Err(bad(format!(
                    "extra_args value {v:?} for {a} is not a plain word or number"
                )));
            }
        }
        if i + arity >= args.len() && arity > 0 {
            return Err(bad(format!("extra_args {a} needs {arity} value(s)")));
        }
        i += 1 + arity;
    }
    Ok(())
}

/// Largest memory figure accepted (4 TiB): keeps cost arithmetic honest.
const MAX_MEMORY_BYTES: u64 = 1 << 42;

impl InferenceSpec {
    /// A spec with defaults for everything but its identity.
    pub fn new(role: &str, runtime: InferFlavor) -> Self {
        Self {
            role: role.into(),
            model: None,
            runtime,
            serve: ServeArgs::default(),
            memory: MemoryBudget::default(),
            excludes: Vec::new(),
            latency_class: LatencyClass::Batch,
            sticky: false,
            api: default_api(),
        }
    }

    /// The loopback address the server is reached at.
    pub fn bind_ip(&self) -> Result<IpAddr, RuntimeError> {
        let raw = self.serve.host.as_deref().unwrap_or("127.0.0.1");
        let ip: IpAddr = if raw == "localhost" {
            IpAddr::from([127, 0, 0, 1])
        } else {
            raw.parse()
                .map_err(|_| bad(format!("serve.host {raw:?} is not an IP address")))?
        };
        if !ip.is_loopback() {
            return Err(bad(format!(
                "serve.host {raw} is not loopback: binding wider than loopback needs a governed \
                 Permit and is not supported by the inference adapters yet"
            )));
        }
        Ok(ip)
    }

    /// Port in force.
    pub fn port(&self) -> Result<u16, RuntimeError> {
        match self.serve.port {
            Some(0) => Err(bad("serve.port must not be 0")),
            Some(p) => Ok(p),
            None => Ok(self.runtime.default_port()),
        }
    }

    /// `http://<ip>:<port>` of the server.
    pub fn base_url(&self) -> Result<String, RuntimeError> {
        Ok(format!(
            "http://{}",
            std::net::SocketAddr::new(self.bind_ip()?, self.port()?)
        ))
    }

    /// Boundary validation.
    pub fn validate(&self) -> Result<(), RuntimeError> {
        if !valid_token(&self.role, 64) {
            return Err(bad("role must be a plain token (1-64 chars)"));
        }
        for m in self.model.iter().chain(self.serve.draft_model.iter()) {
            if !valid_token(m, 128) {
                return Err(bad("model names must be plain tokens (1-128 chars)"));
            }
        }
        self.bind_ip()?;
        self.port()?;
        if self.serve.ctx == Some(0) {
            return Err(bad("serve.ctx must be positive"));
        }
        if let Some(q) = &self.serve.kv_quant
            && !(valid_token(q, 16) && !q.contains(':') && !q.contains('+'))
        {
            return Err(bad("serve.kv_quant must be a plain token"));
        }
        if self.serve.draft_model.is_some() && self.runtime != InferFlavor::LlamaCpp {
            return Err(bad("draft models are supported by infer.llamacpp only"));
        }
        validate_extra_args(self.runtime, &self.serve.extra_args)?;
        if self.memory.weights_bytes > MAX_MEMORY_BYTES
            || self.memory.kv_budget_bytes > MAX_MEMORY_BYTES
        {
            return Err(bad("memory figures must be at most 4 TiB"));
        }
        for x in &self.excludes {
            if !valid_token(x, 64) {
                return Err(bad("excludes entries must be plain tokens"));
            }
        }
        if self.api != "openai-v1" {
            return Err(bad("only api \"openai-v1\" is supported"));
        }
        Ok(())
    }
}

impl VerifiedWorkload {
    /// An inference workload. Trust comes from the operator-attested model
    /// manifest (when managed) and the operator's own request; nothing
    /// signed is fetched.
    pub fn inference(spec: InferenceSpec) -> Result<Self, RuntimeError> {
        spec.validate()?;
        Ok(Self {
            kind: KIND_INFERENCE.to_string(),
            id: spec.role.clone(),
            version: spec.model.clone().unwrap_or_else(|| "adopted".into()),
            source: WorkloadSource::Inference(Box::new(InferencePayload { spec })),
        })
    }

    /// The inference spec, or an admission refusal naming the adapter.
    pub fn inference_spec(&self, runtime: &str) -> Result<&InferenceSpec, RuntimeError> {
        match &self.source {
            WorkloadSource::Inference(p) => Ok(&p.spec),
            _ => Err(RuntimeError::AdmissionRefused(format!(
                "{runtime} runs only inference workloads"
            ))),
        }
    }
}
