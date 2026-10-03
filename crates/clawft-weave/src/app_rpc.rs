//! Daemon handlers for the `app.*` RPC family sent by `weaver app`
//! (card mesh-placement-06, ADR-099 gap table).
//!
//! Before this module the daemon dispatcher had no `app.*` arm, so every
//! `weaver app` subcommand failed with `unknown method: app.*` against a
//! live daemon. The handlers route to the kernel's [`AppManager`]
//! (`Kernel::app_manager`) and ask the kernel governance gate first,
//! with the same action names and effect context `AppManager::install`
//! uses (`app.rs`, action `app.install`). The daemon's `AppManager` is
//! built at boot without a gate or chain attached, so the gate check and
//! the chain events live here.

use std::path::Path;

use clawft_kernel::{AppManager, AppManifest};
use clawft_rpc::Response;
use serde_json::{Value, json};

use crate::rpc_gate::Audit;

/// Upper bound on a manifest file read from disk.
pub const MAX_MANIFEST_BYTES: u64 = 256 * 1024;

/// Manifest filenames looked up inside an app directory, in order.
pub const MANIFEST_NAMES: [&str; 2] = ["weftapp.toml", "weftapp.json"];

// Chain kinds: identical strings to `clawft_kernel::chain::EVENT_KIND_APP_*`
// (asserted in tests) but usable without the `exochain` feature.
/// Chain kind for an install.
pub const APP_INSTALL: &str = "app.install";
/// Chain kind for a start.
pub const APP_START: &str = "app.start";
/// Chain kind for a stop.
pub const APP_STOP: &str = "app.stop";
/// Chain kind for a removal.
pub const APP_REMOVE: &str = "app.remove";

/// Gate handle: the kernel gate with `exochain`, absent otherwise.
#[cfg(feature = "exochain")]
pub type AppGate<'a> = Option<&'a dyn clawft_kernel::GateBackend>;
/// Gate handle: the kernel gate with `exochain`, absent otherwise.
#[cfg(not(feature = "exochain"))]
pub type AppGate<'a> = Option<&'a std::convert::Infallible>;

/// `app.*` keeps `AppManager`'s posture: with no gate, permit.
pub fn gate_check(gate: AppGate<'_>, action: &str, ctx: &Value) -> Result<(), String> {
    #[cfg(feature = "exochain")]
    {
        crate::rpc_gate::decide(gate, action, ctx, false)
    }
    #[cfg(not(feature = "exochain"))]
    {
        let _ = (gate, action, ctx);
        Ok(())
    }
}

/// Load and validate an app manifest from an absolute path.
///
/// `path` is a directory holding `weftapp.toml` / `weftapp.json`, or one of
/// those two files directly. Relative paths are refused because the
/// daemon's working directory is not the caller's. Only files named in
/// [`MANIFEST_NAMES`] are read, after symlinks are resolved, so the RPC
/// cannot be used to read or probe arbitrary `.toml` / `.json` files.
/// Parse errors are redacted to a location: the daemon never echoes file
/// content back to the caller.
pub fn load_manifest(path: &str) -> Result<AppManifest, String> {
    if path.trim().is_empty() || path.contains('\0') {
        return Err("app.install requires a non-empty 'path'".into());
    }
    let p = Path::new(path);
    if !p.is_absolute() {
        return Err(format!("app.install path must be absolute: {path}"));
    }
    let resolved = std::fs::canonicalize(p).map_err(|e| format!("cannot read {path}: {}", e.kind()))?;
    let file = if resolved.is_dir() {
        MANIFEST_NAMES
            .iter()
            .map(|n| resolved.join(n))
            .find(|f| f.is_file())
            .ok_or_else(|| format!("no weftapp.toml or weftapp.json in {path}"))?
    } else {
        resolved
    };
    // Re-canonicalize so a `weftapp.toml` symlink to another file is judged by
    // its real name, not the link's.
    let file = std::fs::canonicalize(&file).map_err(|e| format!("cannot read manifest: {}", e.kind()))?;
    let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if !MANIFEST_NAMES.contains(&name) {
        return Err("manifest file must be named weftapp.toml or weftapp.json".into());
    }
    let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("");
    let len = std::fs::metadata(&file).map_err(|e| format!("cannot read manifest: {}", e.kind()))?.len();
    if len > MAX_MANIFEST_BYTES {
        return Err(format!("manifest too large ({len} bytes, max {MAX_MANIFEST_BYTES})"));
    }
    let src = std::fs::read_to_string(&file).map_err(|e| format!("cannot read manifest: {}", e.kind()))?;
    let parsed = if ext == "toml" {
        AppManifest::from_toml_str(&src)
    } else {
        AppManifest::from_json_str(&src)
    };
    parsed.map_err(|e| redact_parse_error(&e.to_string()))
}

/// Reduce a TOML/JSON parse error to its line and column. Parser messages
/// quote the offending source line or value, which would turn `app.install`
/// into a file-content oracle. Validation errors (the file parsed as a
/// manifest) pass through unchanged.
fn redact_parse_error(reason: &str) -> String {
    let kind = if reason.contains("TOML parse error") {
        "TOML"
    } else if reason.contains("JSON parse error") {
        "JSON"
    } else {
        return reason.to_owned();
    };
    let num_after = |key: &str| -> Option<u64> {
        // TOML puts the location first; serde_json appends it last.
        let at = if kind == "TOML" { reason.find(key)? } else { reason.rfind(key)? };
        let rest = &reason[at + key.len()..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok()
    };
    match (num_after("line "), num_after("column ")) {
        (Some(l), Some(c)) => format!("manifest {kind} parse error at line {l}, column {c} (details withheld)"),
        _ => format!("manifest {kind} parse error (details withheld)"),
    }
}

fn name_param(params: &Value, method: &str) -> Result<String, String> {
    match params.get("name").and_then(Value::as_str) {
        Some(n) if !n.is_empty() && n.len() <= 128 => Ok(n.to_owned()),
        _ => Err(format!("{method} requires a non-empty string 'name'")),
    }
}

/// `app.install {path}` → installed app name (string).
pub fn handle_install(mgr: &AppManager, params: &Value, gate: AppGate<'_>, audit: Audit<'_>) -> Response {
    let path = params.get("path").and_then(Value::as_str).unwrap_or_default();
    let manifest = match load_manifest(path) {
        Ok(m) => m,
        Err(e) => return Response::error(e),
    };
    let ctx = json!({
        "app_name": &manifest.name,
        "version": &manifest.version,
        "effect": { "risk": 0.3, "security": 0.3 },
    });
    if let Err(e) = gate_check(gate, APP_INSTALL, &ctx) {
        return Response::error(e);
    }
    let (version, agents) = (manifest.version.clone(), manifest.agents.len());
    match mgr.install(manifest) {
        Ok(name) => {
            audit(APP_INSTALL, json!({ "app_name": &name, "version": version, "agents": agents }));
            Response::success(json!(name))
        }
        Err(e) => Response::error(e.to_string()),
    }
}

/// `app.list` → `[{name, state, version}]` sorted by name.
pub fn handle_list(mgr: &AppManager) -> Response {
    let mut apps = mgr.list();
    apps.sort_by(|a, b| a.0.cmp(&b.0));
    let rows: Vec<Value> = apps
        .into_iter()
        .map(|(name, state, version)| json!({ "name": name, "state": state.to_string(), "version": version }))
        .collect();
    Response::success(json!(rows))
}

/// `app.inspect {name}` → full `InstalledApp`.
pub fn handle_inspect(mgr: &AppManager, params: &Value) -> Response {
    let name = match name_param(params, "app.inspect") {
        Ok(n) => n,
        Err(e) => return Response::error(e),
    };
    match mgr.inspect(&name) {
        Ok(app) => Response::success(json!(app)),
        Err(e) => Response::error(e.to_string()),
    }
}

/// `app.remove {name}`.
pub fn handle_remove(mgr: &AppManager, params: &Value, gate: AppGate<'_>, audit: Audit<'_>) -> Response {
    let name = match name_param(params, "app.remove") {
        Ok(n) => n,
        Err(e) => return Response::error(e),
    };
    let ctx = json!({ "app_name": &name, "effect": { "risk": 0.4, "security": 0.2 } });
    if let Err(e) = gate_check(gate, APP_REMOVE, &ctx) {
        return Response::error(e);
    }
    match mgr.remove(&name) {
        Ok(m) => {
            audit(APP_REMOVE, json!({ "app_name": &name, "version": m.version }));
            Response::success(json!({ "removed": name }))
        }
        Err(e) => Response::error(e.to_string()),
    }
}

#[cfg(any(unix, windows))]
#[path = "app_rpc_daemon.rs"]
mod daemon_glue;
#[cfg(any(unix, windows))]
pub use daemon_glue::dispatch;

#[cfg(test)]
#[path = "app_rpc_tests.rs"]
mod tests;
