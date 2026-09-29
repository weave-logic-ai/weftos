//! Daemon handlers for the `workload.*` RPC family (ADR-099 decision 2,
//! card mesh-placement-06).
//!
//! Implemented on this node today:
//!
//! | method              | capability | governance action  | chain kind          |
//! |---------------------|------------|--------------------|---------------------|
//! | `workload.list`     | Read       | —                  | —                   |
//! | `workload.inspect`  | Read       | —                  | —                   |
//! | `workload.install`  | Write      | `workload.install` | `workload.install`  |
//! | `workload.unload`   | Write      | `workload.unload`  | `workload.unload`   |
//!
//! A refused mutation is chained as `workload.refuse`. Every `workload.*`
//! mutation fails closed when no governance gate is configured (ADR-099
//! section 4 default-deny). The remaining verbs (`place`, `load`, `start`,
//! `stop`, `migrate`, `revoke`, `node.bind`) are classified in
//! `capability.rs` but answered "not available on this node" until the
//! runtime adapters and mesh control plane land (cards 09 and 12).

use std::path::Path;
use std::sync::{Arc, OnceLock};

use clawft_kernel::GateBackend;
use clawft_rpc::Response;
use serde_json::{Value, json};

use crate::rpc_gate::{Audit, decide};
use crate::workload_registry::{
    InstallRequest, WorkloadRecord, WorkloadRegistry, WorkloadState, validate_name,
};

// Governance action and chain event names, verbatim from ADR-099
// sections 4 and 7. Card 05 owns the kernel-side constants; switch to
// those once it merges.
/// `workload.install` action / event kind.
pub const WORKLOAD_INSTALL: &str = "workload.install";
/// `workload.unload` action / event kind.
pub const WORKLOAD_UNLOAD: &str = "workload.unload";
/// `workload.refuse` event kind.
pub const WORKLOAD_REFUSE: &str = "workload.refuse";

/// Verbs named by ADR-099 whose handlers belong to later cards.
const NOT_YET: &[&str] = &[
    "workload.place",
    "workload.load",
    "workload.start",
    "workload.stop",
    "workload.migrate",
    "workload.revoke",
    "workload.node.bind",
];

static REGISTRY: OnceLock<Arc<WorkloadRegistry>> = OnceLock::new();

/// Install the persisted daemon catalog (call once at daemon boot).
/// Later calls are ignored; returns whether this call won.
pub fn init_registry(path: &Path) -> bool {
    REGISTRY
        .set(Arc::new(WorkloadRegistry::with_persist_path(path)))
        .is_ok()
}

/// The daemon catalog (in-memory if boot never initialised it).
pub fn registry() -> Arc<WorkloadRegistry> {
    REGISTRY
        .get_or_init(|| Arc::new(WorkloadRegistry::new()))
        .clone()
}

fn name_param(params: &Value, method: &str) -> Result<String, String> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{method} requires a string 'name'"))?;
    validate_name(name)?;
    Ok(name.to_owned())
}

/// `workload.list` → `[WorkloadRecord, ...]` sorted by name.
pub fn handle_list(reg: &WorkloadRegistry) -> Response {
    Response::success(json!(reg.list()))
}

/// `workload.inspect {name}` → one record.
pub fn handle_inspect(reg: &WorkloadRegistry, params: &Value) -> Response {
    let name = match name_param(params, "workload.inspect") {
        Ok(n) => n,
        Err(e) => return Response::error(e),
    };
    match reg.get(&name) {
        Some(rec) => Response::success(json!(rec)),
        None => Response::error(format!("workload not found: {name}")),
    }
}

fn refuse(audit: Audit<'_>, action: &str, name: &str, reason: String) -> Response {
    audit(
        WORKLOAD_REFUSE,
        json!({ "action": action, "name": name, "reason": &reason }),
    );
    Response::error(reason)
}

/// `workload.install {name, kind, manifest_hash, version?}`.
///
/// Validates, asks the gate for `workload.install`, and on Permit
/// records the workload and chains `workload.install`.
pub fn handle_install(
    reg: &WorkloadRegistry,
    params: Value,
    node_id: &str,
    gate: Option<&dyn GateBackend>,
    audit: Audit<'_>,
) -> Response {
    let req: InstallRequest = match serde_json::from_value(params) {
        Ok(r) => r,
        Err(e) => return Response::error(format!("invalid workload.install params: {e}")),
    };
    if let Err(e) = req.validate() {
        return Response::error(e);
    }
    if reg.contains(&req.name) {
        return Response::error(format!("workload '{}' is already installed", req.name));
    }
    let ctx = json!({
        "name": &req.name,
        "kind": &req.kind,
        "manifest_hash": &req.manifest_hash,
        "node_id": node_id,
        "effect": { "risk": 0.4, "security": 0.4 },
    });
    if let Err(reason) = decide(gate, WORKLOAD_INSTALL, &ctx, true) {
        return refuse(audit, WORKLOAD_INSTALL, &req.name, reason);
    }
    let record = WorkloadRecord {
        name: req.name,
        kind: req.kind,
        manifest_hash: req.manifest_hash,
        version: req.version,
        state: WorkloadState::Installed,
        node_id: node_id.to_owned(),
        installed_at: chrono::Utc::now(),
    };
    if let Err(e) = reg.insert(record.clone()) {
        return Response::error(e);
    }
    audit(
        WORKLOAD_INSTALL,
        json!({
            "name": &record.name,
            "kind": &record.kind,
            "manifest_hash": &record.manifest_hash,
            "version": &record.version,
            "node_id": &record.node_id,
        }),
    );
    Response::success(json!(record))
}

/// `workload.unload {name}`: drop an installed workload from this node.
pub fn handle_unload(
    reg: &WorkloadRegistry,
    params: &Value,
    gate: Option<&dyn GateBackend>,
    audit: Audit<'_>,
) -> Response {
    let name = match name_param(params, "workload.unload") {
        Ok(n) => n,
        Err(e) => return Response::error(e),
    };
    let Some(rec) = reg.get(&name) else {
        return Response::error(format!("workload not found: {name}"));
    };
    let ctx = json!({
        "name": &rec.name,
        "kind": &rec.kind,
        "manifest_hash": &rec.manifest_hash,
        "node_id": &rec.node_id,
        "effect": { "risk": 0.2, "security": 0.1 },
    });
    if let Err(reason) = decide(gate, WORKLOAD_UNLOAD, &ctx, true) {
        return refuse(audit, WORKLOAD_UNLOAD, &name, reason);
    }
    match reg.remove(&name) {
        Some(rec) => {
            audit(
                WORKLOAD_UNLOAD,
                json!({ "name": &rec.name, "kind": &rec.kind, "node_id": &rec.node_id }),
            );
            Response::success(json!({ "unloaded": rec.name }))
        }
        None => Response::error(format!("workload not found: {name}")),
    }
}

/// Route one `workload.*` call. `gate`/`audit` come from the kernel.
pub fn route(
    method: &str,
    params: Value,
    reg: &WorkloadRegistry,
    node_id: &str,
    gate: Option<&dyn GateBackend>,
    audit: Audit<'_>,
) -> Response {
    match method {
        "workload.list" => handle_list(reg),
        "workload.inspect" => handle_inspect(reg, &params),
        "workload.install" => handle_install(reg, params, node_id, gate, audit),
        "workload.unload" => handle_unload(reg, &params, gate, audit),
        m if NOT_YET.contains(&m) => Response::error(format!(
            "{m} is not available on this node yet (ADR-099: needs runtime adapters / \
             mesh control plane)"
        )),
        other => Response::error(format!("unknown method: {other}")),
    }
}

/// Daemon entry point: pulls gate, chain and node id from the kernel.
#[cfg(any(unix, windows))]
pub async fn dispatch(
    method: &str,
    params: Value,
    kernel: Arc<
        tokio::sync::RwLock<clawft_kernel::boot::Kernel<clawft_platform::NativePlatform>>,
    >,
) -> Response {
    let k = kernel.read().await;
    let node_id = k.cluster_membership().local_node_id().to_owned();
    let gate = k.governance_gate().cloned();
    let chain = k.chain_manager().cloned();
    drop(k);
    let audit = |kind: &str, payload: Value| {
        if let Some(cm) = &chain {
            cm.append("workload", kind, Some(payload));
        }
    };
    route(method, params, &registry(), &node_id, gate.as_deref(), &audit)
}

#[cfg(test)]
#[path = "workload_rpc_tests.rs"]
mod tests;
