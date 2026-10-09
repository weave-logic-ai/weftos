//! Local RPC `project.fetch` (Admin): what `git-remote-weftos` calls on the
//! user daemon. `{node, body}` is forwarded as the node-admin method
//! `project.fetch` to peer `node` over the signed `workload.ctl` wire
//! (ADR-108 P3b); the answer comes back verbatim. The peer must be in
//! `workload-peers.json` (tier `pinned` for a primary) and must list this node
//! as a fetch peer with a grant for the project.
//!
//! Opens (`list`, `refs`, `bundle.open`, `tar.open`) are chained here as
//! `project.fetch`; chunk reads are not.

use clawft_kernel::workload_ctl::{PLANE_CHAIN_SOURCE, msg::method};
use clawft_rpc::Response;
use serde_json::{Value, json};

use crate::project_fetch_serve::EVENT_PROJECT_FETCH;
use crate::rpc_ext::{ExtCall, ExtFuture, KernelRef};

async fn handle_inner(params: Value, kernel: &KernelRef) -> Result<Value, String> {
    let node = params["node"].as_str().filter(|n| !n.is_empty() && n.len() <= 128).ok_or("`node` must be a node id")?;
    let body = params.get("body").filter(|b| b.is_object()).cloned().ok_or("`body` must be an object")?;
    if crate::workload_place_rpc::local_mesh_node_id().is_some_and(|me| me == node) {
        return Err("project.fetch is for another node's primary, not this node".into());
    }
    let op = body["op"].as_str().unwrap_or("").to_owned();
    let out = crate::workload_place_rpc::node_admin(kernel, node, method::PROJECT_FETCH, body.clone()).await;
    if op != "chunk" && op != "close"
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
