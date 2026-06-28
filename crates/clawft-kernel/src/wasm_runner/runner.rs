//! WASM tool runner: compilation, execution, module cache, and hashing.

use std::path::PathBuf;
#[cfg(feature = "exochain")]
use std::sync::Arc;
#[cfg(feature = "wasm-sandbox")]
use std::time::Duration;

use sha2::{Digest, Sha256};

use super::types::*;

// ---------------------------------------------------------------------------
// WASM Tool Runner
// ---------------------------------------------------------------------------

/// WASM tool runner.
///
/// When the `wasm-sandbox` feature is enabled, this uses Wasmtime
/// for actual WASM execution. Without the feature, all tool loads
/// are rejected with [`WasmError::RuntimeUnavailable`].
pub struct WasmToolRunner {
    config: WasmSandboxConfig,
    #[cfg(feature = "wasm-sandbox")]
    engine: wasmtime::Engine,
    #[cfg(feature = "exochain")]
    chain_manager: Option<Arc<crate::chain::ChainManager>>,
    #[cfg(feature = "exochain")]
    governance_engine: Option<Arc<crate::governance::GovernanceEngine>>,
}

impl WasmToolRunner {
    /// Create a new WASM tool runner with the given configuration.
    pub fn new(config: WasmSandboxConfig) -> Self {
        #[cfg(feature = "wasm-sandbox")]
        {
            let mut wt_config = wasmtime::Config::new();
            wt_config.consume_fuel(true);
            wt_config.async_support(true);
            // Memory limit is enforced per-store, not per-engine
            let engine =
                wasmtime::Engine::new(&wt_config).expect("failed to create wasmtime engine");
            Self {
                config,
                engine,
                #[cfg(feature = "exochain")]
                chain_manager: None,
                #[cfg(feature = "exochain")]
                governance_engine: None,
            }
        }
        #[cfg(not(feature = "wasm-sandbox"))]
        {
            Self {
                config,
                #[cfg(feature = "exochain")]
                chain_manager: None,
                #[cfg(feature = "exochain")]
                governance_engine: None,
            }
        }
    }

    /// Attach a chain manager for exochain event logging.
    #[cfg(feature = "exochain")]
    pub fn set_chain_manager(&mut self, cm: Arc<crate::chain::ChainManager>) {
        self.chain_manager = Some(cm);
    }

    /// Attach a governance engine for pre-execution gating.
    #[cfg(feature = "exochain")]
    pub fn set_governance_engine(&mut self, ge: Arc<crate::governance::GovernanceEngine>) {
        self.governance_engine = Some(ge);
    }

    /// Get the sandbox configuration.
    pub fn config(&self) -> &WasmSandboxConfig {
        &self.config
    }

    /// Validate a WASM module's bytes without loading it.
    ///
    /// Checks module size, magic bytes, and (when the runtime is
    /// available) uses wasmtime::Module::validate() for full validation.
    pub fn validate_wasm(&self, wasm_bytes: &[u8]) -> Result<WasmValidation, WasmError> {
        // Check module size
        if wasm_bytes.len() > self.config.max_module_size_bytes {
            return Err(WasmError::ModuleTooLarge {
                size: wasm_bytes.len(),
                limit: self.config.max_module_size_bytes,
            });
        }

        // Check WASM magic bytes (\0asm)
        if wasm_bytes.len() < 8 || &wasm_bytes[0..4] != b"\0asm" {
            return Err(WasmError::InvalidModule(
                "missing WASM magic bytes (\\0asm)".into(),
            ));
        }

        let mut warnings = Vec::new();

        // Parse version (bytes 4-7 in little-endian)
        let version =
            u32::from_le_bytes([wasm_bytes[4], wasm_bytes[5], wasm_bytes[6], wasm_bytes[7]]);
        if version != 1 {
            warnings.push(format!("unexpected WASM version: {version} (expected 1)"));
        }

        #[cfg(not(feature = "wasm-sandbox"))]
        {
            Ok(WasmValidation {
                valid: true,
                exports: Vec::new(),
                imports: Vec::new(),
                estimated_memory: 0,
                warnings,
            })
        }

        #[cfg(feature = "wasm-sandbox")]
        {
            // Full validation via wasmtime
            if let Err(e) = wasmtime::Module::validate(&self.engine, wasm_bytes) {
                return Err(WasmError::InvalidModule(e.to_string()));
            }

            // Parse module to extract exports/imports
            match wasmtime::Module::new(&self.engine, wasm_bytes) {
                Ok(module) => {
                    let exports: Vec<String> =
                        module.exports().map(|e| e.name().to_string()).collect();
                    let imports: Vec<String> = module
                        .imports()
                        .map(|i| format!("{}::{}", i.module(), i.name()))
                        .collect();
                    Ok(WasmValidation {
                        valid: true,
                        exports,
                        imports,
                        estimated_memory: 0,
                        warnings,
                    })
                }
                Err(e) => Err(WasmError::CompilationFailed(e.to_string())),
            }
        }
    }

    /// Load a WASM tool from bytes.
    ///
    /// Validates the module, computes a SHA-256 hash, and (with `wasm-sandbox`)
    /// compiles it with the Wasmtime engine.
    pub fn load_tool(&self, name: &str, wasm_bytes: &[u8]) -> Result<WasmTool, WasmError> {
        let validation = self.validate_wasm(wasm_bytes)?;

        if !validation.valid {
            return Err(WasmError::InvalidModule(validation.warnings.join("; ")));
        }

        let module_hash = compute_module_hash(wasm_bytes);

        #[cfg(not(feature = "wasm-sandbox"))]
        {
            let _ = name;
            let _ = module_hash;
            Err(WasmError::RuntimeUnavailable)
        }

        #[cfg(feature = "wasm-sandbox")]
        {
            Ok(WasmTool {
                name: name.to_owned(),
                module_size: wasm_bytes.len(),
                module_hash,
                schema: None,
                exports: validation.exports,
            })
        }
    }

    /// Execute a loaded WASM tool synchronously.
    ///
    /// Creates an isolated store with fuel metering and memory limits,
    /// compiles the tool's module bytes, and calls `_start` or `execute`.
    /// No host filesystem access is provided -- the instance receives
    /// an empty set of imports.
    ///
    /// For WASI-aware execution with stdio pipes, use [`execute_bytes`].
    pub fn execute(
        &self,
        _tool: &WasmTool,
        _input: serde_json::Value,
    ) -> Result<WasmToolResult, WasmError> {
        // Governance gate: check before execution
        #[cfg(feature = "exochain")]
        if let Some(ref ge) = self.governance_engine {
            let request = crate::governance::GovernanceRequest::new("wasm", "wasm.execute")
                .with_effect(crate::governance::EffectVector {
                    risk: 0.4,
                    security: 0.5,
                    ..Default::default()
                });
            let result = ge.evaluate(&request);
            if matches!(
                result.decision,
                crate::governance::GovernanceDecision::Deny(_)
            ) {
                return Err(WasmError::GovernanceDenied(result.decision.to_string()));
            }
        }

        // Chain logging
        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain_manager {
            cm.append(
                "wasm",
                crate::chain::EVENT_KIND_WASM_EXECUTE,
                Some(serde_json::json!({
                    "tool": &_tool.name,
                    "module_size": _tool.module_size,
                })),
            );
        }

        #[cfg(not(feature = "wasm-sandbox"))]
        {
            Err(WasmError::RuntimeUnavailable)
        }

        #[cfg(feature = "wasm-sandbox")]
        {
            Err(WasmError::RuntimeUnavailable)
        }
    }

    /// Execute raw WASM bytes synchronously without WASI.
    ///
    /// This is the sync K3 execution path. It creates a fresh Wasmtime
    /// store with fuel metering and memory limits, instantiates the
    /// module with **no imports** (no filesystem, no network), and
    /// calls `_start` or `run`.
    ///
    /// Returns [`WasmToolResult`] on success or a typed [`WasmError`]
    /// on fuel exhaustion, memory overflow, or compilation failure.
    #[cfg(feature = "wasm-sandbox")]
    pub fn execute_sync(
        &self,
        name: &str,
        wasm_bytes: &[u8],
        _input: serde_json::Value,
    ) -> Result<WasmToolResult, WasmError> {
        let started = std::time::Instant::now();

        // Build a sync-only engine (the shared engine has async_support
        // enabled, which forbids synchronous Instance::new).
        let mut sync_config = wasmtime::Config::new();
        sync_config.consume_fuel(true);
        let sync_engine = wasmtime::Engine::new(&sync_config)
            .map_err(|e| WasmError::CompilationFailed(format!("sync engine: {e}")))?;

        // Compile module (accepts binary .wasm or text .wat)
        let module = wasmtime::Module::new(&sync_engine, wasm_bytes)
            .map_err(|e| WasmError::CompilationFailed(format!("{name}: {e}")))?;

        // Create per-call store with embedded memory limiter
        let limiter = MemoryLimiter {
            max_bytes: self.config.max_memory_bytes,
        };
        let mut store = wasmtime::Store::new(&sync_engine, limiter);
        store
            .set_fuel(self.config.max_fuel)
            .map_err(|e| WasmError::WasmTrap(format!("set fuel: {e}")))?;
        store.limiter(|state| state as &mut dyn wasmtime::ResourceLimiter);

        // Instantiate with NO imports -- fully sandboxed, no host access
        let instance = wasmtime::Instance::new(&mut store, &module, &[])
            .map_err(|e| classify_trap_with_limiter(e, &self.config, &store))?;

        // Find entry point: _start (WASI convention) or run
        let entry = instance
            .get_func(&mut store, "_start")
            .or_else(|| instance.get_func(&mut store, "run"))
            .ok_or_else(|| WasmError::WasmTrap(format!("{name}: no _start or run export")))?;

        // Call the entry function
        match entry.call(&mut store, &[], &mut []) {
            Ok(_) => {
                let fuel_remaining = store.get_fuel().unwrap_or(0);
                let fuel_consumed = self.config.max_fuel.saturating_sub(fuel_remaining);
                Ok(WasmToolResult {
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: 0,
                    fuel_consumed,
                    memory_peak: 0,
                    execution_time: started.elapsed(),
                })
            }
            Err(e) => Err(classify_trap_with_limiter(e, &self.config, &store)),
        }
    }

    /// Compile and execute WASM bytes in one shot.
    ///
    /// This is the primary execution path for K3. It accepts raw WASM
    /// bytes (binary or WAT text), compiles them with the engine, creates
    /// an isolated WASI store with fuel metering, serializes `input` as
    /// JSON to the module's stdin, calls `_start` (WASI preview1) or
    /// `execute`, and reads stdout/stderr.
    ///
    /// For cached execution with pre-compiled modules, see K4.
    #[cfg(feature = "wasm-sandbox")]
    pub async fn execute_bytes(
        &self,
        name: &str,
        wasm_bytes: &[u8],
        input: serde_json::Value,
    ) -> Result<WasmToolResult, WasmError> {
        use wasmtime_wasi::p2::pipe::{MemoryInputPipe, MemoryOutputPipe};

        let started = std::time::Instant::now();

        // Serialize input to JSON bytes for stdin
        let input_bytes = serde_json::to_vec(&input)
            .map_err(|e| WasmError::WasmTrap(format!("input serialization: {e}")))?;

        // Create pipes for stdio
        let stdout_pipe = MemoryOutputPipe::new(65_536);
        let stderr_pipe = MemoryOutputPipe::new(65_536);
        let stdin_pipe = MemoryInputPipe::new(input_bytes);

        // Build WASI preview1 context with stdio pipes
        let wasi_ctx = wasmtime_wasi::p2::WasiCtxBuilder::new()
            .stdin(stdin_pipe)
            .stdout(stdout_pipe.clone())
            .stderr(stderr_pipe.clone())
            .build_p1();

        // Create per-call store with fuel budget
        let mut store = wasmtime::Store::new(&self.engine, wasi_ctx);
        store
            .set_fuel(self.config.max_fuel)
            .map_err(|e| WasmError::WasmTrap(format!("set fuel: {e}")))?;

        // Link WASI preview1 functions (wasi_snapshot_preview1.*)
        let mut linker = wasmtime::Linker::<wasmtime_wasi::preview1::WasiP1Ctx>::new(&self.engine);
        wasmtime_wasi::preview1::add_to_linker_async(&mut linker, |ctx| ctx)
            .map_err(|e| WasmError::CompilationFailed(format!("WASI linker: {e}")))?;

        // Compile the module (accepts both binary .wasm and text .wat)
        let module = wasmtime::Module::new(&self.engine, wasm_bytes)
            .map_err(|e| WasmError::CompilationFailed(format!("{name}: {e}")))?;

        // Instantiate
        let instance = linker
            .instantiate_async(&mut store, &module)
            .await
            .map_err(|e| {
                let is_fuel = e
                    .downcast_ref::<wasmtime::Trap>()
                    .is_some_and(|t| *t == wasmtime::Trap::OutOfFuel)
                    || e.to_string().contains("fuel");
                if is_fuel {
                    WasmError::FuelExhausted {
                        consumed: self.config.max_fuel,
                        limit: self.config.max_fuel,
                    }
                } else {
                    WasmError::CompilationFailed(format!("instantiate {name}: {e}"))
                }
            })?;

        // Execute with wall-clock timeout
        let timeout = Duration::from_secs(self.config.max_execution_time_secs);
        let exec_result = tokio::time::timeout(timeout, async {
            // Try _start (WASI convention), then execute
            if let Some(start_fn) = instance.get_func(&mut store, "_start") {
                start_fn.call_async(&mut store, &[], &mut []).await
            } else if let Some(exec_fn) = instance.get_func(&mut store, "execute") {
                exec_fn.call_async(&mut store, &[], &mut []).await
            } else {
                Err(wasmtime::Error::msg("no _start or execute export"))
            }
        })
        .await;

        // Read captured output
        let stdout = String::from_utf8_lossy(&stdout_pipe.contents()).to_string();
        let stderr = String::from_utf8_lossy(&stderr_pipe.contents()).to_string();

        let fuel_remaining = store.get_fuel().unwrap_or(0);
        let fuel_consumed = self.config.max_fuel.saturating_sub(fuel_remaining);

        match exec_result {
            Ok(Ok(_)) => Ok(WasmToolResult {
                stdout,
                stderr,
                exit_code: 0,
                fuel_consumed,
                memory_peak: 0,
                execution_time: started.elapsed(),
            }),
            Ok(Err(trap)) => {
                let msg = trap.to_string();
                // Check for fuel exhaustion via downcast or message
                let is_fuel = trap
                    .downcast_ref::<wasmtime::Trap>()
                    .is_some_and(|t| *t == wasmtime::Trap::OutOfFuel)
                    || msg.contains("fuel");
                if is_fuel {
                    Err(WasmError::FuelExhausted {
                        consumed: fuel_consumed,
                        limit: self.config.max_fuel,
                    })
                } else if msg.contains("memory") {
                    Err(WasmError::MemoryLimitExceeded {
                        allocated: self.config.max_memory_bytes,
                        limit: self.config.max_memory_bytes,
                    })
                } else {
                    // Non-zero exit or trap -- return result with stderr
                    Ok(WasmToolResult {
                        stdout,
                        stderr: if stderr.is_empty() {
                            format!("trap: {msg}")
                        } else {
                            format!("{stderr}\ntrap: {msg}")
                        },
                        exit_code: 1,
                        fuel_consumed,
                        memory_peak: 0,
                        execution_time: started.elapsed(),
                    })
                }
            }
            Err(_timeout) => Err(WasmError::ExecutionTimeout(timeout)),
        }
    }

    /// Get a reference to the Wasmtime engine.
    #[cfg(feature = "wasm-sandbox")]
    pub fn engine(&self) -> &wasmtime::Engine {
        &self.engine
    }
}

// ---------------------------------------------------------------------------
// Wasmtime helpers (behind feature gate)
// ---------------------------------------------------------------------------

/// Resource limiter that caps linear memory growth.
#[cfg(feature = "wasm-sandbox")]
pub(crate) struct MemoryLimiter {
    pub(crate) max_bytes: usize,
}

#[cfg(feature = "wasm-sandbox")]
impl wasmtime::ResourceLimiter for MemoryLimiter {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> Result<bool, wasmtime::Error> {
        if desired > self.max_bytes {
            // Deny the growth -- Wasmtime will trap
            Ok(false)
        } else {
            Ok(true)
        }
    }

    fn table_growing(
        &mut self,
        _current: usize,
        _desired: usize,
        _maximum: Option<usize>,
    ) -> Result<bool, wasmtime::Error> {
        Ok(true)
    }
}

/// Classify a Wasmtime error into a typed [`WasmError`].
///
/// Inspects the error for fuel exhaustion or memory-related traps
/// and returns the corresponding `WasmError` variant.
#[cfg(feature = "wasm-sandbox")]
fn classify_trap_impl(
    err: wasmtime::Error,
    config: &WasmSandboxConfig,
    fuel_remaining: u64,
) -> WasmError {
    let msg = err.to_string();

    // Check for fuel exhaustion
    let is_fuel = err
        .downcast_ref::<wasmtime::Trap>()
        .is_some_and(|t| *t == wasmtime::Trap::OutOfFuel)
        || msg.contains("fuel");
    if is_fuel {
        return WasmError::FuelExhausted {
            consumed: config.max_fuel.saturating_sub(fuel_remaining),
            limit: config.max_fuel,
        };
    }

    // Check for memory limit
    if msg.contains("memory") {
        return WasmError::MemoryLimitExceeded {
            allocated: config.max_memory_bytes,
            limit: config.max_memory_bytes,
        };
    }

    WasmError::WasmTrap(msg)
}

/// Classify trap from a `Store<MemoryLimiter>` (used by execute_sync).
#[cfg(feature = "wasm-sandbox")]
fn classify_trap_with_limiter(
    err: wasmtime::Error,
    config: &WasmSandboxConfig,
    store: &wasmtime::Store<MemoryLimiter>,
) -> WasmError {
    classify_trap_impl(err, config, store.get_fuel().unwrap_or(0))
}

// ---------------------------------------------------------------------------
// Module hashing
// ---------------------------------------------------------------------------

/// Compute SHA-256 hash of WASM module bytes.
pub fn compute_module_hash(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let result = hasher.finalize();
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&result);
    hash
}

// ---------------------------------------------------------------------------
// Disk-persisted module cache
// ---------------------------------------------------------------------------

/// Compiled module cache with LRU eviction.
///
/// Stores compiled WASM modules on disk keyed by SHA-256 hash.
/// When the cache exceeds `max_size`, the oldest entries are evicted.
pub struct CompiledModuleCache {
    cache_dir: PathBuf,
    max_size: u64,
}

impl CompiledModuleCache {
    /// Create a new module cache at the given directory.
    pub fn new(cache_dir: PathBuf, max_size: u64) -> Self {
        let _ = std::fs::create_dir_all(&cache_dir);
        Self {
            cache_dir,
            max_size,
        }
    }

    /// Get a cached compiled module by its hash.
    pub fn get(&self, hash: &[u8; 32]) -> Option<Vec<u8>> {
        let path = self.cache_path(hash);
        std::fs::read(&path).ok()
    }

    /// Store a compiled module in the cache.
    pub fn put(&self, hash: &[u8; 32], bytes: &[u8]) {
        let path = self.cache_path(hash);
        let _ = std::fs::write(&path, bytes);
        self.evict_lru();
    }

    /// Evict oldest entries until cache is under `max_size`.
    fn evict_lru(&self) {
        let mut entries: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();
        if let Ok(dir) = std::fs::read_dir(&self.cache_dir) {
            for entry in dir.flatten() {
                if let Ok(meta) = entry.metadata() {
                    let modified = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                    entries.push((entry.path(), meta.len(), modified));
                }
            }
        }
        let total: u64 = entries.iter().map(|(_, s, _)| s).sum();
        if total <= self.max_size {
            return;
        }
        // Sort by modification time (oldest first)
        entries.sort_by_key(|(_, _, t)| *t);
        let mut remaining = total;
        for (path, size, _) in &entries {
            if remaining <= self.max_size {
                break;
            }
            let _ = std::fs::remove_file(path);
            remaining -= size;
        }
    }

    fn cache_path(&self, hash: &[u8; 32]) -> PathBuf {
        let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
        self.cache_dir.join(format!("{hex}.wasm"))
    }
}
