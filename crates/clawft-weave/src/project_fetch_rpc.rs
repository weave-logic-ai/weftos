//! Local RPC `project.fetch` (Admin): what `git-remote-weftos` calls on the
//! user daemon. `{uri, body}` names a project repository
//! (`weftos://<mesh>/projects/<ULID>[/repos/<dir>]`, ADR-114); the daemon
//! checks the authority is its own mesh, resolves the project's paired
//! primary from `mesh-pairings.json` and forwards `body` as the node-admin
//! method `project.fetch` over the signed `workload.ctl` wire (ADR-108 P3b).
//! A name it cannot resolve, for whatever reason, gets the one refusal
//! [`UNKNOWN_NAME`].
//!
//! Chunk reads reuse signed sessions: each call takes an idle session to that
//! node from a small per-node pool (or opens one), uses it, and puts it back,
//! so a helper that keeps several local connections open gets as many mesh
//! connections kept open on the primary. Control calls still go through the
//! plane's ordinary node-admin path and are chained here as `project.fetch`.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use clawft_kernel::workload_ctl::{CtlSession, PLANE_CHAIN_SOURCE, msg::method};
use clawft_rpc::Response;
use serde_json::{Value, json};

use crate::mesh_names::{MeshNames, UNKNOWN_NAME};
use crate::project_fetch_serve::EVENT_PROJECT_FETCH;
use crate::rpc_ext::{ExtCall, ExtFuture, KernelRef};
use crate::weftos_uri::WeftosUri;

/// Idle sessions kept per node.
const POOL_PER_NODE: usize = 8;
/// An idle session older than this is dropped rather than reused.
const POOL_IDLE: Duration = Duration::from_secs(60);
const CHUNK_TIMEOUT: Duration = Duration::from_secs(120);

static POOL: Mutex<Vec<(String, Instant, CtlSession)>> = Mutex::new(Vec::new());

fn take_session(node: &str) -> Option<CtlSession> {
    let mut p = POOL.lock().ok()?;
    p.retain(|(_, t, _)| t.elapsed() < POOL_IDLE);
    let i = p.iter().position(|(n, _, _)| n == node)?;
    Some(p.swap_remove(i).2)
}

fn give_back(node: &str, s: CtlSession) {
    if let Ok(mut p) = POOL.lock()
        && p.iter().filter(|(n, _, _)| n == node).count() < POOL_PER_NODE
    {
        p.push((node.to_owned(), Instant::now(), s));
    }
}

/// A chunk read on a pooled session. The mesh leg carries the bytes as a raw
/// frame; the helper's JSON leg gets them base64 in `data`.
async fn chunk_call(node: &str, mut body: Value) -> Result<Value, String> {
    let mut s = match take_session(node) {
        Some(s) => s,
        None => {
            let plane = crate::workload_place_rpc::plane_if_built().ok_or("placement is not initialised on this node")?;
            plane.open_session(node).await.map_err(|e| e.to_string())?.with_timeout(CHUNK_TIMEOUT)
        }
    };
    body["raw"] = json!(true);
    let out = s.call_raw(method::PROJECT_FETCH, body).await;
    match &out {
        Ok(_) => give_back(node, s),
        Err(_) => s.close().await,
    }
    let (mut v, frame) = out.map_err(|e| e.to_string())?;
    if let (Some(bytes), Some(o)) = (frame, v.as_object_mut()) {
        use base64::Engine;
        o.insert("data".into(), json!(base64::engine::general_purpose::STANDARD.encode(&bytes)));
        o.remove("raw");
    }
    Ok(v)
}

/// Idle pooled sessions (tests and `doctor`).
pub fn pooled() -> usize {
    POOL.lock().map(|p| p.len()).unwrap_or(0)
}

/// The project, repository dir and primary node a repository name resolves
/// to on this node, or [`UNKNOWN_NAME`]. The daemon's own mesh is `names`;
/// pairings are read from `runtime_dir`.
pub fn resolve(uri: &str, names: &MeshNames, runtime_dir: Option<&std::path::Path>) -> Result<(String, String, String), String> {
    let u = WeftosUri::parse(uri).map_err(|e| e.to_string())?;
    if !names.accepts(&u.authority) {
        return Err(UNKNOWN_NAME.into());
    }
    let (project, dir) = u.project_repo().ok_or(UNKNOWN_NAME)?;
    let dir = dir.to_owned();
    let node = runtime_dir
        .and_then(|d| crate::mesh_pairings::primary_for(d, project).ok().flatten())
        .ok_or(UNKNOWN_NAME)?;
    Ok((project.to_owned(), dir, node))
}

async fn handle_inner(params: Value, kernel: &KernelRef) -> Result<Value, String> {
    let uri = params["uri"].as_str().filter(|u| !u.is_empty() && u.len() <= 1024).ok_or("`uri` must be a weftos:// name")?;
    let mut body = params.get("body").filter(|b| b.is_object()).cloned().ok_or("`body` must be an object")?;
    let runtime_dir = crate::workload_place_rpc::runtime_dir();
    let names = MeshNames::local(runtime_dir.as_deref());
    let (project, dir, node) = resolve(uri, &names, runtime_dir.as_deref())?;
    if crate::workload_place_rpc::local_mesh_node_id().is_some_and(|me| me == node) {
        return Err(UNKNOWN_NAME.into());
    }
    // The name decides the project and repository; the body may not.
    let op = body["op"].as_str().unwrap_or("").to_owned();
    if matches!(op.as_str(), "list" | "refs" | "bundle.open" | "tar.open") {
        body["project"] = json!(project);
        if op != "list" {
            body["dir"] = json!(dir);
        }
    }
    if op == "chunk" {
        return chunk_call(&node, body).await;
    }
    let out = crate::workload_place_rpc::node_admin(kernel, &node, method::PROJECT_FETCH, body.clone()).await;
    if op != "close"
        && let Some(chain) = kernel.read().await.chain_manager().cloned()
    {
        chain.append(
            PLANE_CHAIN_SOURCE,
            EVENT_PROJECT_FETCH,
            Some(json!({ "node": node, "project": project, "op": op, "dir": body["dir"],
                "ok": out.is_ok(), "error": out.as_ref().err() })),
        );
    }
    out
}

/// Route handler.
pub fn handle(call: ExtCall) -> ExtFuture {
    Box::pin(async move {
        match handle_inner(call.params, &call.ctx.kernel).await {
            Ok(v) => Response::success(v),
            Err(e) => Response::error(e),
        }
    })
}

#[cfg(test)]
#[path = "project_fetch_rpc_tests.rs"]
mod tests;
