//! Native wasmtime host for clawft WASM plugins (WEFT-398).
//!
//! This crate owns the **host-side** plugin runtime that previously lived
//! under `clawft-wasm` behind the `wasm-plugins` feature:
//!
//! | Module | Role |
//! |--------|------|
//! | [`sandbox`] | Permission / SSRF / path / rate-limit enforcement |
//! | [`engine`] | [`engine::WasmPluginEngine`] — compile, link, fuel-meter, invoke |
//! | [`audit`] | Per-plugin host-call audit log |
//! | [`permission_store`] | Persisted user approval of permission upgrades |
//!
//! ## Why a separate crate?
//!
//! `clawft-wasm` is the **browser / wasip2 entrypoint** (`cdylib` +
//! wasm-bindgen). The wasmtime host is native-only, pulls a multi-megabyte
//! JIT, and is orthogonal to W-BROWSER. Keeping both in one crate forced
//! browser consumers to carry host feature flags and obscured the boundary.
//!
//! ## Compatibility
//!
//! Historical paths `clawft_wasm::{sandbox,engine,audit,permission_store}`
//! remain available when `clawft-wasm` is built with `--features wasm-plugins`
//! (thin re-exports of this crate). Prefer depending on
//! `clawft-wasm-host` directly for new code.
//!
//! ## WIT contract
//!
//! The host↔guest interface lives at `wit/plugin.wit` in this crate.

#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod audit;
pub mod engine;
pub mod permission_store;
pub mod sandbox;
pub mod sandboxed_fs;
pub mod sandboxed_http;

// Convenience re-exports for the primary host surface.
pub use audit::{AuditEntry, AuditLog};
pub use engine::{
    HostFunctionDispatcher, HostState, MAX_FUEL_HARD, MAX_MEMORY_HARD, MAX_TABLE_ELEMENTS_HARD,
    PluginConfig, PluginExecutionResult, WasmPluginEngine, validate_module_binary,
};
pub use permission_store::{
    ApprovedRecord, AutoApprover, MockApprover, PermissionApprover, PermissionStore,
};
pub use sandbox::{
    FileValidationError, HttpValidationError, MAX_LOG_MESSAGE_SIZE, MAX_PLUGIN_DIR_SIZE,
    MAX_READ_SIZE, MAX_WASM_SIZE_GZIPPED, MAX_WASM_SIZE_UNCOMPRESSED, MAX_WRITE_SIZE,
    NetworkAllowlist, PluginSandbox, RateCounter, is_private_ip, validate_env_access,
    validate_file_access, validate_http_request, validate_log_message, validate_wasm_size,
};
pub use sandboxed_fs::SandboxedFileSystem;
pub use sandboxed_http::{HttpResponse as SandboxedHttpResponse, SandboxedHttpClient};

/// Crate version (mirrors workspace package version).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Verification helper for a supervisor adopting a persistent WASM project.
#[cfg(feature = "project-kernel")]
#[path = "project_kernel/adoption.rs"]
pub mod project_kernel;
