//! Operator verbs for inference placement (card mesh-placement-19):
//!
//! - `infer.status` (Read): roles, proxy state, where each role resolves,
//!   exposure, and why the mesh is or is not available.
//! - `infer.expose {role, exposed}` (Admin): let a role leave loopback for
//!   mesh peers (or stop). Audited on the chain.
//! - `infer.allow {role, node, direction, allowed}` (Admin): `direction` is
//!   `serve` (this node serves `node` the role) or `consume` (this node may
//!   use `node`'s instance). Default deny both ways. Audited on the chain.
//!
//! Changes are in effect until restart; `<runtime>/inference.json` holds
//! the persistent allowlists. In service mode the mesh verbs refuse with
//! the reason (see [`crate::infer_wire`]).

use clawft_kernel::infer_proxy::Target;
use clawft_kernel::workload_pkg::manifest::valid_token;
use clawft_rpc::Response;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::infer_wire::{InferState, state};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExposeParams {
    role: String,
    exposed: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AllowParams {
    role: String,
    node: String,
    direction: String,
    allowed: bool,
}

fn off() -> Response {
    Response::error("inference placement is off (no inference.json in the runtime directory)".to_string())
}

fn require_mesh(st: &InferState) -> Result<(), Response> {
    if st.hub.is_none() {
        return Err(Response::error(format!("mesh serving is {}", st.mesh_note)));
    }
    Ok(())
}

fn role_known(st: &InferState, role: &str) -> bool {
    st.roles.iter().any(|r| r.cfg.role == role)
}

/// Dispatch one `infer.*` verb.
pub async fn handle(method: &str, params: Value) -> Response {
    let Some(st) = state() else { return off() };
    match method {
        "infer.status" => Response::success(status(st)),
        "infer.expose" => {
            let p: ExposeParams = match serde_json::from_value(params) {
                Ok(p) => p,
                Err(e) => return Response::error(format!("invalid params: {e}")),
            };
            if let Err(r) = require_mesh(st) {
                return r;
            }
            if !role_known(st, &p.role) {
                return Response::error(format!("unknown role {:?}", p.role));
            }
            st.table.expose_to_mesh(&p.role, p.exposed);
            Response::success(json!({"role": p.role, "exposed": p.exposed}))
        }
        "infer.allow" => {
            let p: AllowParams = match serde_json::from_value(params) {
                Ok(p) => p,
                Err(e) => return Response::error(format!("invalid params: {e}")),
            };
            if let Err(r) = require_mesh(st) {
                return r;
            }
            if !role_known(st, &p.role) || !valid_token(&p.node, 128) {
                return Response::error("unknown role or malformed node id".to_string());
            }
            match p.direction.as_str() {
                "serve" => st.table.allow_mesh_peer(&p.role, &p.node, p.allowed),
                "consume" => st.table.allow_remote_node(&p.role, &p.node, p.allowed),
                _ => return Response::error("direction is serve or consume".to_string()),
            }
            Response::success(json!({
                "role": p.role, "node": p.node, "direction": p.direction, "allowed": p.allowed
            }))
        }
        _ => Response::error(format!("unknown method {method}")),
    }
}

fn status(st: &InferState) -> Value {
    let exposed = st.table.exposed_roles();
    let roles: Vec<Value> = st
        .roles
        .iter()
        .map(|r| {
            let target = match st.table.resolve(&r.cfg.role) {
                Some(Target::Local { .. }) => json!("local"),
                Some(Target::Remote { node_id }) => json!({"remote": node_id}),
                None => Value::Null,
            };
            json!({
                "role": r.cfg.role,
                "flavor": r.cfg.flavor,
                "instance_port": r.cfg.instance_port,
                "proxy": &*r.proxy.lock().unwrap(),
                "serves": target,
                "exposed": exposed.contains(&r.cfg.role),
            })
        })
        .collect();
    json!({
        "node_id": st.node_id,
        "mesh": {"available": st.hub.is_some(), "note": st.mesh_note},
        "peers": st.hub.as_ref().map(|h| h.qualifying_peers()).unwrap_or_default(),
        "roles": roles,
    })
}
