//! Local RPC `project.fetch` (Admin): what `git-remote-weftos` calls on the
//! user daemon. `{node, body}` is forwarded as the node-admin method
//! `project.fetch` to peer `node` over the signed `workload.ctl` wire
//! (ADR-108 P3b); the answer comes back verbatim. The peer must be in
//! `workload-peers.json` (tier `pinned` for a primary) and must list this node
//! as a fetch peer with a grant for the project.
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

use crate::project_fetch_serve::EVENT_PROJECT_FETCH;
use crate::rpc_ext::{ExtCall, ExtFuture, KernelRef};

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

async fn handle_inner(params: Value, kernel: &KernelRef) -> Result<Value, String> {
    let node = params["node"].as_str().filter(|n| !n.is_empty() && n.len() <= 128).ok_or("`node` must be a node id")?;
    let body = params.get("body").filter(|b| b.is_object()).cloned().ok_or("`body` must be an object")?;
    if crate::workload_place_rpc::local_mesh_node_id().is_some_and(|me| me == node) {
        return Err("project.fetch is for another node's primary, not this node".into());
    }
    let op = body["op"].as_str().unwrap_or("").to_owned();
    if op == "chunk" {
        return chunk_call(node, body).await;
    }
    let out = crate::workload_place_rpc::node_admin(kernel, node, method::PROJECT_FETCH, body.clone()).await;
    if op != "close"
        && let Some(chain) = kernel.read().await.chain_manager().cloned()
    {
        chain.append(
            PLANE_CHAIN_SOURCE,
            EVENT_PROJECT_FETCH,
            Some(json!({ "node": node, "project": body["project"], "op": op, "dir": body["dir"],
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
