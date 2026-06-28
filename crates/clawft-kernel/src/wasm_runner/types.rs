//! WASM runner types: configuration, errors, tool specs, signing, and sandbox.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::governance::EffectVector;

/// Serde support for [u8; 64] as hex strings (used for Ed25519 signatures).
pub(crate) mod sig_serde {
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(hash: &[u8; 64], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
        serializer.serialize_str(&hex)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<[u8; 64], D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let bytes: Vec<u8> = (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(serde::de::Error::custom))
            .collect::<Result<Vec<u8>, _>>()?;
        let mut arr = [0u8; 64];
        if bytes.len() != 64 {
            return Err(serde::de::Error::custom(format!(
                "expected 64 bytes, got {}",
                bytes.len()
            )));
        }
        arr.copy_from_slice(&bytes);
        Ok(arr)
    }
}

// ---------------------------------------------------------------------------
// Sandbox configuration
// ---------------------------------------------------------------------------

/// Configuration for the WASM sandbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmSandboxConfig {
    /// Maximum fuel units (roughly equivalent to instructions).
    /// Default: 1,000,000 (~100ms on modern hardware).
    #[serde(default = "default_max_fuel")]
    pub max_fuel: u64,

    /// Maximum memory in bytes the WASM module may allocate.
    /// Default: 16 MiB.
    #[serde(default = "default_max_memory")]
    pub max_memory_bytes: usize,

    /// Wall-clock timeout for execution.
    /// Default: 30 seconds.
    #[serde(default = "default_max_execution_secs", alias = "maxExecutionTimeSecs")]
    pub max_execution_time_secs: u64,

    /// Host function calls the WASM module is allowed to make.
    /// Empty means no host calls permitted.
    #[serde(default)]
    pub allowed_host_calls: Vec<String>,

    /// Whether to enable WASI (basic I/O, no filesystem).
    #[serde(default)]
    pub wasi_enabled: bool,

    /// Maximum WASM module size in bytes before loading.
    /// Default: 10 MiB.
    #[serde(default = "default_max_module_size")]
    pub max_module_size_bytes: usize,
}

fn default_max_fuel() -> u64 {
    1_000_000
}

fn default_max_memory() -> usize {
    16 * 1024 * 1024 // 16 MiB
}

fn default_max_execution_secs() -> u64 {
    30
}

fn default_max_module_size() -> usize {
    10 * 1024 * 1024 // 10 MiB
}

impl Default for WasmSandboxConfig {
    fn default() -> Self {
        Self {
            max_fuel: default_max_fuel(),
            max_memory_bytes: default_max_memory(),
            max_execution_time_secs: default_max_execution_secs(),
            allowed_host_calls: Vec::new(),
            wasi_enabled: false,
            max_module_size_bytes: default_max_module_size(),
        }
    }
}

impl WasmSandboxConfig {
    /// Get the execution timeout as a Duration.
    pub fn execution_timeout(&self) -> Duration {
        Duration::from_secs(self.max_execution_time_secs)
    }
}

// ---------------------------------------------------------------------------
// Per-execution state
// ---------------------------------------------------------------------------

/// Per-execution state for a WASM tool.
#[derive(Debug, Clone, Default)]
pub struct ToolState {
    /// Name of the tool being executed.
    pub tool_name: String,

    /// Input data (stdin equivalent).
    pub stdin: Vec<u8>,

    /// Output data (stdout equivalent).
    pub stdout: Vec<u8>,

    /// Error output data (stderr equivalent).
    pub stderr: Vec<u8>,

    /// Environment variables available to the tool.
    pub env: HashMap<String, String>,
}

/// Result of a WASM tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmToolResult {
    /// Standard output from the tool.
    pub stdout: String,

    /// Standard error from the tool.
    pub stderr: String,

    /// Exit code (0 = success).
    pub exit_code: i32,

    /// Fuel units consumed during execution.
    pub fuel_consumed: u64,

    /// Peak memory usage in bytes.
    pub memory_peak: usize,

    /// Actual execution duration.
    #[serde(with = "duration_millis")]
    pub execution_time: Duration,
}

/// Serialization helper for Duration as milliseconds.
mod duration_millis {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        d.as_millis().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        let ms = u64::deserialize(d)?;
        Ok(Duration::from_millis(ms))
    }
}

/// Validation result for a WASM module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmValidation {
    /// Whether the module is valid.
    pub valid: bool,

    /// Exported function names.
    pub exports: Vec<String>,

    /// Required import names.
    pub imports: Vec<String>,

    /// Estimated initial memory requirement.
    pub estimated_memory: usize,

    /// Warnings about the module (non-fatal issues).
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// WASM runner errors.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum WasmError {
    /// The WASM runtime is not available (feature not enabled).
    #[error("WASM runtime unavailable: compile with --features wasm-sandbox")]
    RuntimeUnavailable,

    /// The WASM module bytes are invalid.
    #[error("invalid WASM module: {0}")]
    InvalidModule(String),

    /// Module compilation failed.
    #[error("compilation failed: {0}")]
    CompilationFailed(String),

    /// The tool exhausted its fuel budget.
    #[error("fuel exhausted after {consumed} units (limit: {limit})")]
    FuelExhausted { consumed: u64, limit: u64 },

    /// Memory allocation exceeded the configured limit.
    #[error("memory limit exceeded: {allocated} bytes (limit: {limit} bytes)")]
    MemoryLimitExceeded { allocated: usize, limit: usize },

    /// Execution exceeded the wall-clock timeout.
    #[error("execution timeout after {0:?}")]
    ExecutionTimeout(Duration),

    /// A WASM trap occurred during execution.
    #[error("WASM trap: {0}")]
    WasmTrap(String),

    /// A host function call was denied by sandbox policy.
    #[error("host call denied: {0}")]
    HostCallDenied(String),

    /// The module exceeds the maximum allowed size.
    #[error("module too large: {size} bytes (limit: {limit} bytes)")]
    ModuleTooLarge { size: usize, limit: usize },

    /// Execution denied by governance gate.
    #[error("governance denied: {0}")]
    GovernanceDenied(String),
}

/// Tool execution errors.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("tool not found: {0}")]
    NotFound(String),
    #[error("invalid arguments: {0}")]
    InvalidArgs(String),
    #[error("execution failed: {0}")]
    ExecutionFailed(String),
    #[error("file not found: {0}")]
    FileNotFound(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("file too large: {size} bytes (limit: {limit} bytes)")]
    FileTooLarge { size: u64, limit: u64 },
    #[error("signature required: {0}")]
    SignatureRequired(String),
    #[error("invalid signature: {0}")]
    InvalidSignature(String),
    #[error("wasm error: {0}")]
    Wasm(#[from] WasmError),
}

// ---------------------------------------------------------------------------
// Built-in tool catalog types
// ---------------------------------------------------------------------------

/// Category of a built-in kernel tool.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolCategory {
    Filesystem,
    Agent,
    System,
    /// ECC cognitive substrate tools (behind `ecc` feature).
    Ecc,
    User,
}

/// Specification of a built-in kernel tool.
///
/// Named `BuiltinToolSpec` to distinguish from [`crate::app::ToolSpec`]
/// which describes application-provided tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuiltinToolSpec {
    /// Dotted tool name (e.g. "fs.read_file").
    pub name: String,
    /// Category (Filesystem, Agent, System, User).
    pub category: ToolCategory,
    /// Human-readable description.
    pub description: String,
    /// JSON Schema for parameters.
    pub parameters: serde_json::Value,
    /// GovernanceGate action string (e.g. "tool.fs.read").
    pub gate_action: String,
    /// Effect vector for governance scoring.
    pub effect: EffectVector,
    /// Whether this tool can run natively (without WASM).
    pub native: bool,
}

/// A deployed version of a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolVersion {
    /// Version number (monotonically increasing per tool).
    pub version: u32,
    /// SHA-256 hash of the WASM module bytes.
    pub module_hash: [u8; 32],
    /// Ed25519 signature over module_hash (zero if unsigned).
    #[serde(with = "sig_serde")]
    pub signature: [u8; 64],
    /// When this version was deployed.
    pub deployed_at: DateTime<Utc>,
    /// Whether this version has been revoked.
    pub revoked: bool,
    /// Chain sequence number of the deploy event.
    pub chain_seq: u64,
}

/// A tool with its spec and version history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployedTool {
    /// Tool specification.
    pub spec: BuiltinToolSpec,
    /// Version history (ordered by version number).
    pub versions: Vec<ToolVersion>,
    /// Currently active version number.
    pub active_version: u32,
}

/// A loaded WASM tool module.
#[derive(Debug, Clone)]
pub struct WasmTool {
    /// Tool name.
    pub name: String,

    /// Module size in bytes.
    pub module_size: usize,

    /// SHA-256 hash of module bytes.
    pub module_hash: [u8; 32],

    /// Tool parameter schema (if exported by the module).
    pub schema: Option<serde_json::Value>,

    /// Exported function names.
    pub exports: Vec<String>,
}

// ---------------------------------------------------------------------------
// WASI filesystem scope
// ---------------------------------------------------------------------------

/// WASI filesystem access scope for a tool.
#[non_exhaustive]
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum WasiFsScope {
    /// No filesystem access.
    #[default]
    None,
    /// Read-only access to a directory.
    ReadOnly(PathBuf),
    /// Read-write access to a directory.
    ReadWrite(PathBuf),
}

// ---------------------------------------------------------------------------
// CA chain signing
// ---------------------------------------------------------------------------

/// Tool signing authority -- identifies who signed a tool module.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolSigningAuthority {
    /// Signed by the kernel's built-in key.
    Kernel,
    /// Signed by a developer with a certificate chain.
    Developer {
        /// Certificate chain (leaf first, root last).
        cert_chain: Vec<Certificate>,
    },
}

/// A signing certificate in the CA chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Certificate {
    /// Subject name (e.g. "developer@example.com").
    pub subject: String,
    /// Ed25519 public key bytes (32 bytes).
    pub public_key: [u8; 32],
    /// Signature over subject + public_key by the issuer.
    #[serde(with = "sig_serde")]
    pub signature: [u8; 64],
    /// Issuer subject name.
    pub issuer: String,
}

// ---------------------------------------------------------------------------
// Tool Signature for ExoChain registration
// ---------------------------------------------------------------------------

/// A cryptographic signature binding a tool definition to a signer identity.
///
/// Used by [`ToolRegistry::register_signed`] to gate tool registration
/// behind signature verification when `require_signatures` is enabled.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSignature {
    /// Name of the tool being signed.
    pub tool_name: String,
    /// SHA-256 hash of the tool definition (spec JSON bytes).
    pub tool_hash: [u8; 32],
    /// Identity of the signer (e.g. public key hex or developer id).
    pub signer_id: String,
    /// Ed25519 signature bytes over `tool_hash`.
    pub signature: Vec<u8>,
    /// Timestamp when the signature was created.
    pub signed_at: DateTime<Utc>,
}

impl ToolSignature {
    /// Create a new tool signature from components.
    pub fn new(
        tool_name: impl Into<String>,
        tool_hash: [u8; 32],
        signer_id: impl Into<String>,
        signature: Vec<u8>,
    ) -> Self {
        Self {
            tool_name: tool_name.into(),
            tool_hash,
            signer_id: signer_id.into(),
            signature,
            signed_at: Utc::now(),
        }
    }

    /// Verify this signature against a 32-byte Ed25519 public key.
    ///
    /// Returns `true` if the signature is valid for `self.tool_hash`.
    /// Requires `exochain` feature; without it, always returns `false`.
    pub fn verify(&self, public_key: &[u8; 32]) -> bool {
        if self.signature.len() != 64 {
            return false;
        }
        let mut sig_bytes = [0u8; 64];
        sig_bytes.copy_from_slice(&self.signature);
        verify_tool_signature(&self.tool_hash, &sig_bytes, public_key)
    }
}

/// Verify a tool's Ed25519 signature against a public key.
///
/// Requires the `exochain` feature for real Ed25519 verification.
/// Without the feature, always returns `false`.
#[cfg(feature = "exochain")]
pub fn verify_tool_signature(
    module_hash: &[u8; 32],
    signature: &[u8; 64],
    public_key: &[u8; 32],
) -> bool {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let Ok(vk) = VerifyingKey::from_bytes(public_key) else {
        return false;
    };
    let sig = Signature::from_bytes(signature);
    vk.verify(module_hash, &sig).is_ok()
}

/// Stub: always returns `false` when `exochain` feature is disabled.
#[cfg(not(feature = "exochain"))]
pub fn verify_tool_signature(
    _module_hash: &[u8; 32],
    _signature: &[u8; 64],
    _public_key: &[u8; 32],
) -> bool {
    false
}

// ---------------------------------------------------------------------------
// Backend selection
// ---------------------------------------------------------------------------

/// Backend selection for tool execution.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackendSelection {
    /// Run natively (no isolation).
    Native,
    /// Run in WASM sandbox.
    Wasm,
    /// Auto-select based on risk score.
    Auto,
}

impl BackendSelection {
    /// Select backend based on effect vector risk score.
    ///
    /// Simple heuristic: risk > 0.3 => WASM sandbox, else native.
    pub fn from_risk(risk: f64) -> Self {
        if risk > 0.3 { Self::Wasm } else { Self::Native }
    }
}

// ---------------------------------------------------------------------------
// Multi-layer sandboxing (k3:D12)
// ---------------------------------------------------------------------------

/// Which sandbox layer denied (or allowed) access.
///
/// Three enforcement layers are evaluated in order (k3:D12):
/// 1. **Governance** -- gate check with tool name + effect vector context
/// 2. **Environment** -- per-environment allowed-path configuration
/// 3. **SudoOverride** -- elevated agent capability that bypasses
///    environment restrictions (logged to chain, requires `sudo` flag)
///
/// The first `Deny` short-circuits. `SudoOverride` can only bypass
/// the **Environment** layer, never the **Governance** layer.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SandboxLayer {
    /// Governance gate check (always authoritative, cannot be overridden).
    Governance,
    /// Environment-scoped path restrictions (e.g. dev=permissive, prod=strict).
    Environment,
    /// Elevated override that bypasses environment restrictions.
    /// Requires `AgentCapabilities::sudo` and is always logged to chain.
    SudoOverride,
}

impl std::fmt::Display for SandboxLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SandboxLayer::Governance => write!(f, "governance"),
            SandboxLayer::Environment => write!(f, "environment"),
            SandboxLayer::SudoOverride => write!(f, "sudo-override"),
        }
    }
}

/// Result of evaluating the multi-layer sandbox stack.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxDecision {
    /// Whether access is permitted.
    pub allowed: bool,
    /// Which layer made the decision.
    pub decided_by: SandboxLayer,
    /// Human-readable reason (for logging / chain events).
    pub reason: String,
}

impl SandboxDecision {
    /// Create a permit decision.
    pub fn permit(layer: SandboxLayer) -> Self {
        Self {
            allowed: true,
            decided_by: layer,
            reason: "access permitted".into(),
        }
    }

    /// Create a deny decision.
    pub fn deny(layer: SandboxLayer, reason: impl Into<String>) -> Self {
        Self {
            allowed: false,
            decided_by: layer,
            reason: reason.into(),
        }
    }
}

/// Filesystem sandbox configuration for built-in tools.
///
/// Controls which paths a tool is allowed to access. When `allowed_paths`
/// is non-empty, only files under those directories are permitted.
/// An empty `allowed_paths` means permissive mode (dev default).
///
/// Part of the multi-layer sandboxing stack (k3:D12):
/// governance gate -> environment config -> sudo override.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SandboxConfig {
    /// Directories the tool is allowed to access.
    /// Empty = permissive (all paths allowed).
    pub allowed_paths: Vec<PathBuf>,

    /// Whether sudo override is active for this execution.
    /// When true and path is denied by environment config, access
    /// is granted anyway (but logged to chain). Governance denials
    /// can never be overridden.
    #[serde(default)]
    pub sudo_override: bool,
}

impl SandboxConfig {
    /// Check whether a path is allowed by this sandbox config.
    ///
    /// Returns `true` if `allowed_paths` is empty (permissive mode)
    /// or the path is under at least one allowed directory.
    pub fn is_path_allowed(&self, path: &std::path::Path) -> bool {
        if self.allowed_paths.is_empty() {
            return true;
        }
        // Canonicalize the target path for comparison
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.allowed_paths.iter().any(|allowed| {
            let allowed_canon = allowed.canonicalize().unwrap_or_else(|_| allowed.clone());
            canonical.starts_with(&allowed_canon)
        })
    }

    /// Multi-layer sandbox check (k3:D12).
    ///
    /// Evaluates the environment layer and optional sudo override.
    /// The governance layer is evaluated separately by the caller
    /// (via `GovernanceEngine::evaluate`) because it requires the
    /// full `GovernanceRequest` context.
    ///
    /// Evaluation order:
    /// 1. Environment config (`allowed_paths`) -- if empty, permit.
    /// 2. If denied and `sudo_override` is true, permit with
    ///    `SandboxLayer::SudoOverride` (caller must log to chain).
    /// 3. Otherwise deny with `SandboxLayer::Environment`.
    pub fn check_path_multilayer(&self, path: &std::path::Path) -> SandboxDecision {
        // Permissive mode (dev default)
        if self.allowed_paths.is_empty() {
            return SandboxDecision::permit(SandboxLayer::Environment);
        }

        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let env_allowed = self.allowed_paths.iter().any(|allowed| {
            let allowed_canon = allowed.canonicalize().unwrap_or_else(|_| allowed.clone());
            canonical.starts_with(&allowed_canon)
        });

        if env_allowed {
            return SandboxDecision::permit(SandboxLayer::Environment);
        }

        // Environment denied -- check sudo override
        if self.sudo_override {
            return SandboxDecision {
                allowed: true,
                decided_by: SandboxLayer::SudoOverride,
                reason: format!(
                    "sudo override: path {} bypassed environment restriction",
                    path.display()
                ),
            };
        }

        SandboxDecision::deny(
            SandboxLayer::Environment,
            format!("path outside sandbox: {}", path.display()),
        )
    }
}
