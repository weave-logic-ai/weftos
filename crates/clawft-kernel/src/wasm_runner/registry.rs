//! Tool registry: trait, hierarchical lookup, WASM adapter, and signing.

use std::collections::HashMap;
use std::sync::Arc;

#[cfg(feature = "wasm-sandbox")]
use super::runner::WasmToolRunner;
use super::types::*;
#[cfg(feature = "wasm-sandbox")]
use crate::governance::EffectVector;

// ---------------------------------------------------------------------------
// Built-in tool trait
// ---------------------------------------------------------------------------

/// Trait for built-in kernel tools.
///
/// Each tool has a spec and an execute method. Tools hold their own
/// dependencies (e.g. `Arc<ProcessTable>` for agent tools).
pub trait BuiltinTool: Send + Sync {
    /// Return the tool name (e.g. "fs.read_file").
    fn name(&self) -> &str;
    /// Return the tool specification.
    fn spec(&self) -> &BuiltinToolSpec;
    /// Execute the tool with the given JSON arguments.
    fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError>;
}

// ---------------------------------------------------------------------------
// Tool Registry
// ---------------------------------------------------------------------------

/// Registry of available tools for dispatch.
///
/// Supports hierarchical lookup: a child registry can overlay a parent.
/// The parent chain is walked when a tool is not found locally.
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn BuiltinTool>>,
    /// Optional parent registry for hierarchical lookup.
    parent: Option<Arc<ToolRegistry>>,
    /// When true, only signed tools may be registered.
    require_signatures: bool,
    /// Trusted public keys for signature verification (32-byte Ed25519 keys).
    trusted_keys: Vec<[u8; 32]>,
    /// Signatures for registered tools (tool_name -> ToolSignature).
    signatures: HashMap<String, ToolSignature>,
}

impl ToolRegistry {
    /// Create an empty registry with no parent.
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            parent: None,
            require_signatures: false,
            trusted_keys: Vec::new(),
            signatures: HashMap::new(),
        }
    }

    /// Create a child registry that delegates to `parent` for missing tools.
    pub fn with_parent(parent: Arc<ToolRegistry>) -> Self {
        Self {
            tools: HashMap::new(),
            parent: Some(parent),
            require_signatures: false,
            trusted_keys: Vec::new(),
            signatures: HashMap::new(),
        }
    }

    /// Register a tool (local to this registry level).
    ///
    /// When `require_signatures` is enabled, this rejects unsigned tools
    /// with [`ToolError::SignatureRequired`]. Use [`register_signed`]
    /// to supply a signature, or disable the requirement.
    pub fn register(&mut self, tool: Arc<dyn BuiltinTool>) {
        if self.require_signatures {
            tracing::warn!(
                tool = tool.name(),
                "unsigned tool registration rejected (require_signatures=true)"
            );
            return;
        }
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Register a tool, checking signatures when required.
    ///
    /// Returns `Err(SignatureRequired)` when `require_signatures` is on
    /// and no signature is provided. Returns `Ok(())` otherwise.
    pub fn try_register(&mut self, tool: Arc<dyn BuiltinTool>) -> Result<(), ToolError> {
        if self.require_signatures {
            return Err(ToolError::SignatureRequired(tool.name().to_string()));
        }
        self.tools.insert(tool.name().to_string(), tool);
        Ok(())
    }

    /// Register a tool with a cryptographic signature.
    ///
    /// Verifies the signature against trusted keys before allowing registration.
    /// The signature is stored and the tool is chain-logged if ExoChain is available.
    pub fn register_signed(
        &mut self,
        tool: Arc<dyn BuiltinTool>,
        signature: ToolSignature,
    ) -> Result<(), ToolError> {
        // Verify the signature against at least one trusted key.
        if !self.verify_tool_signature(&signature) {
            return Err(ToolError::InvalidSignature(format!(
                "no trusted key verified signature for tool '{}'",
                signature.tool_name,
            )));
        }
        let name = tool.name().to_string();
        self.tools.insert(name.clone(), tool);
        self.signatures.insert(name, signature);
        Ok(())
    }

    /// Check whether a tool signature is valid against any trusted key.
    pub fn verify_tool_signature(&self, signature: &ToolSignature) -> bool {
        self.trusted_keys.iter().any(|key| signature.verify(key))
    }

    /// Enable or disable mandatory signature verification for tool registration.
    pub fn set_require_signatures(&mut self, require: bool) {
        self.require_signatures = require;
    }

    /// Whether signatures are required for tool registration.
    pub fn requires_signatures(&self) -> bool {
        self.require_signatures
    }

    /// Add a trusted Ed25519 public key for signature verification.
    pub fn add_trusted_key(&mut self, key: [u8; 32]) {
        self.trusted_keys.push(key);
    }

    /// Get the signature for a registered tool, if any.
    pub fn get_signature(&self, tool_name: &str) -> Option<&ToolSignature> {
        self.signatures.get(tool_name)
    }

    /// Look up a tool by name, walking the parent chain.
    pub fn get(&self, name: &str) -> Option<&Arc<dyn BuiltinTool>> {
        self.tools.get(name).or_else(|| {
            // Walk parent chain -- returns &Arc from parent, which is valid
            // because `self` borrows the parent via `Arc`.
            self.parent.as_ref().and_then(|p| p.get(name))
        })
    }

    /// Execute a tool by name, walking the parent chain.
    pub fn execute(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, ToolError> {
        let tool = self
            .get(name)
            .ok_or_else(|| ToolError::NotFound(name.to_string()))?;
        tool.execute(args)
    }

    /// List all registered tool names (merges parent + local, local wins).
    pub fn list(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        // Local tools first
        for name in self.tools.keys() {
            seen.insert(name.clone());
        }
        // Parent tools (only if not overridden locally)
        if let Some(ref parent) = self.parent {
            for name in parent.list() {
                seen.insert(name);
            }
        }
        let mut result: Vec<String> = seen.into_iter().collect();
        result.sort();
        result
    }

    /// Number of registered tools (parent + local, deduplicated).
    pub fn len(&self) -> usize {
        if self.parent.is_none() {
            return self.tools.len();
        }
        self.list().len()
    }

    /// Whether the registry has no tools (including parent).
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty() && self.parent.as_ref().is_none_or(|p| p.is_empty())
    }

    /// Get a reference to the parent registry, if any.
    pub fn parent(&self) -> Option<&Arc<ToolRegistry>> {
        self.parent.as_ref()
    }

    /// Register a WASM tool that executes through a [`WasmToolRunner`].
    ///
    /// The WASM bytes are stored inside the adapter and compiled on each
    /// execution (K3). Compiled module caching is deferred to K4.
    ///
    /// The tool is dispatched synchronously via [`BuiltinTool::execute`],
    /// which spawns a blocking thread internally to run the async Wasmtime
    /// execution. For fully async dispatch, call
    /// [`WasmToolRunner::execute_bytes`] directly.
    #[cfg(feature = "wasm-sandbox")]
    pub fn register_wasm_tool(
        &mut self,
        name: &str,
        description: &str,
        wasm_bytes: Vec<u8>,
        runner: Arc<WasmToolRunner>,
    ) -> Result<(), WasmError> {
        // Validate the module by attempting compilation (handles both
        // binary WASM and WAT text format).
        wasmtime::Module::new(runner.engine(), &wasm_bytes)
            .map_err(|e| WasmError::InvalidModule(e.to_string()))?;

        let adapter = WasmToolAdapter {
            tool_name: name.to_owned(),
            spec: BuiltinToolSpec {
                name: name.to_owned(),
                category: ToolCategory::User,
                description: description.to_owned(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "input": {"type": "object", "description": "JSON input passed to WASM stdin"}
                    }
                }),
                gate_action: format!("tool.wasm.{name}"),
                effect: EffectVector {
                    risk: 0.5,
                    ..Default::default()
                },
                native: false,
            },
            wasm_bytes: Arc::new(wasm_bytes),
            runner,
        };
        self.register(Arc::new(adapter));
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// WASM tool adapter (bridges BuiltinTool to WasmToolRunner)
// ---------------------------------------------------------------------------

/// Adapter that wraps WASM bytes + a [`WasmToolRunner`] as a [`BuiltinTool`].
///
/// When [`BuiltinTool::execute`] is called, this adapter spawns a blocking
/// thread to run [`WasmToolRunner::execute_bytes`] asynchronously. The JSON
/// args are passed as stdin to the WASM module.
#[cfg(feature = "wasm-sandbox")]
struct WasmToolAdapter {
    tool_name: String,
    spec: BuiltinToolSpec,
    wasm_bytes: Arc<Vec<u8>>,
    runner: Arc<WasmToolRunner>,
}

#[cfg(feature = "wasm-sandbox")]
impl BuiltinTool for WasmToolAdapter {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn spec(&self) -> &BuiltinToolSpec {
        &self.spec
    }

    fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        // Extract input from args, defaulting to the full args object
        let input = args.get("input").cloned().unwrap_or(args.clone());

        let runner = self.runner.clone();
        let wasm_bytes = self.wasm_bytes.clone();
        let name = self.tool_name.clone();

        // Run async execute_bytes on a blocking thread with its own runtime
        let result = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| ToolError::ExecutionFailed(format!("runtime: {e}")))?;
            rt.block_on(runner.execute_bytes(&name, &wasm_bytes, input))
                .map_err(ToolError::Wasm)
        })
        .join()
        .map_err(|_| ToolError::ExecutionFailed("WASM execution thread panicked".into()))??;

        Ok(serde_json::json!({
            "stdout": result.stdout,
            "stderr": result.stderr,
            "exit_code": result.exit_code,
            "fuel_consumed": result.fuel_consumed,
            "execution_time_ms": result.execution_time.as_millis() as u64,
        }))
    }
}

// Safety: WasmToolAdapter is Send+Sync because all its fields are Send+Sync.
// - tool_name/spec: plain data
// - wasm_bytes: Arc<Vec<u8>>
// - runner: Arc<WasmToolRunner> (Engine is Send+Sync)
#[cfg(feature = "wasm-sandbox")]
unsafe impl Send for WasmToolAdapter {}
#[cfg(feature = "wasm-sandbox")]
unsafe impl Sync for WasmToolAdapter {}

// Safety: ToolRegistry is Send+Sync because it contains Send+Sync fields.
// The `parent` is behind an Arc, and `tools` contains Arc<dyn BuiltinTool>
// which requires Send+Sync.
unsafe impl Send for ToolRegistry {}
unsafe impl Sync for ToolRegistry {}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
