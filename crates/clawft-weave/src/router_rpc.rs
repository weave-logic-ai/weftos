//! `route.list` and `route.reload` (ADR-116 §5), behind the user daemon's RPC.
//! Both are local: the list is the `/_weftos/routes.json` document and the
//! reload re-reads the owner's own `compose/ports.yaml` files, which the
//! poller does on its own anyway.

use serde_json::json;

use crate::protocol::Response;
use crate::rpc_ext::{ExtCall, ExtFuture};

/// Method names.
pub const LIST: &str = "route.list";
/// See [`LIST`].
pub const RELOAD: &str = "route.reload";

const OFF: &str = "the tailnet router is not enabled on this node ([router] enabled = true in ~/.weftos/weave.toml)";

/// Route handler for `route.list` and `route.reload`.
pub fn handle(call: ExtCall) -> ExtFuture {
    Box::pin(async move {
        let Some(h) = crate::router_state::global() else {
            return if call.method == LIST { Response::success(json!({ "enabled": false })) } else { Response::error(OFF.to_owned()) };
        };
        match call.method.as_str() {
            LIST => Response::success(crate::router_index::snapshot(&h).await),
            RELOAD => {
                let worker = h.clone();
                match tokio::task::spawn_blocking(move || worker.reload()).await {
                    Ok(t) => Response::success(json!({
                        "reloaded_at": h.reloaded_at(),
                        "generation": h.generation(),
                        "routes": t.routes.len(),
                        "refused": t.refused.len(),
                    })),
                    Err(e) => Response::error(format!("reload failed: {e}")),
                }
            }
            other => Response::error(format!("unknown route method {other}")),
        }
    })
}
