//! `chain.subscribe`: streaming chain events (ADR-103 D, P2).
//!
//! `chain.subscribe {chain, kind_prefix?, from_seq?}` takes over the
//! connection like `kernel.logs_stream`: one ack, then line-delimited JSON
//! frames until the client disconnects.
//!
//! * `chain: "user"`: the daemon's own chain. Requires `Read`, which the
//!   capability table enforces before this handler runs.
//! * `chain: "project/<id>"`: a project's chain. The caller must be `Admin`
//!   or hold a [`VerifiedProject`] for that id (a validated project-scoped
//!   token, or this daemon's own bound project); a bare `Request.project`
//!   claim is not enough. Reading it means proxying to the project's
//!   daemon, which packages G/H wire; until then an authorized caller gets
//!   the typed `not_yet_supported`.
//!
//! Frames: `{"frame":"event", ...}` (hashes hex, `rule_hash` null when the
//! event predates ADR-103 A7) and `{"frame":"lagged","missed":n}` when the
//! subscriber fell behind (resubscribe with `from_seq` to recover).

use serde::Deserialize;

use crate::rpc_ext::VerifiedProject;

/// Parameters of `chain.subscribe`.
#[derive(Debug, Deserialize)]
pub struct SubscribeParams {
    /// `"user"` or `"project/<id>"`.
    pub chain: String,
    #[serde(default)]
    pub kind_prefix: Option<String>,
    #[serde(default)]
    pub from_seq: Option<u64>,
}

/// Which chain a request names.
#[derive(Debug, PartialEq, Eq)]
pub enum ChainTarget<'a> {
    User,
    Project(&'a str),
}

/// Parse the `chain` selector. `Err` is the user-facing reason.
pub fn parse_target(chain: &str) -> Result<ChainTarget<'_>, String> {
    match chain {
        "user" => Ok(ChainTarget::User),
        s => match s.strip_prefix("project/") {
            Some(id) if !id.is_empty() && !id.contains('/') => Ok(ChainTarget::Project(id)),
            _ => Err(format!(
                "unknown chain {chain:?}: expected \"user\" or \"project/<id>\""
            )),
        },
    }
}

/// May a caller read project `id`'s chain? `Admin`, or a verified
/// project equal to `id`. Never a bare claim.
pub fn project_read_allowed(is_admin: bool, verified: &[VerifiedProject], id: &str) -> bool {
    is_admin || verified.iter().any(|v| v.as_str() == id)
}

#[cfg(feature = "exochain")]
pub use stream::handle_chain_subscribe;

/// Without the `exochain` feature there is no chain to stream.
#[cfg(not(feature = "exochain"))]
pub async fn handle_chain_subscribe(
    _params: serde_json::Value,
    _caller: &crate::rpc_ext::CallerCtx,
    _caps: &crate::capability::CallerCapabilities,
    _kernel: std::sync::Arc<
        tokio::sync::RwLock<clawft_kernel::boot::Kernel<clawft_platform::NativePlatform>>,
    >,
) -> Result<
    (
        clawft_rpc::Response,
        String,
        tokio::sync::mpsc::Receiver<Vec<u8>>,
        Box<dyn FnOnce() + Send>,
    ),
    clawft_rpc::Response,
> {
    Err(clawft_rpc::Response::error("exochain feature not enabled"))
}

#[cfg(feature = "exochain")]
mod stream {
    use std::sync::Arc;

    use clawft_kernel::boot::Kernel;
    use clawft_kernel::chain::ChainEvent;
    use clawft_kernel::chain_subscribe::{ChainFilter, ChainItem};
    use clawft_platform::NativePlatform;
    use clawft_rpc::Response;
    use tokio::sync::RwLock;

    use super::{ChainTarget, SubscribeParams, parse_target, project_read_allowed};
    use crate::capability::{CallerCapabilities, Capability};
    use crate::rpc_ext::{CallerCtx, VerifiedProject};

    /// Stream hookup the daemon pipes into the socket after the ack.
    pub type StreamHookup = (
        Response,
        String,
        tokio::sync::mpsc::Receiver<Vec<u8>>,
        Box<dyn FnOnce() + Send>,
    );

    fn hex(b: &[u8; 32]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    fn event_frame(e: &ChainEvent) -> serde_json::Value {
        serde_json::json!({
            "frame": "event",
            "sequence": e.sequence,
            "chain_id": e.chain_id,
            "timestamp": e.timestamp.to_rfc3339(),
            "source": e.source,
            "kind": e.kind,
            "hash": hex(&e.hash),
            "prev_hash": hex(&e.prev_hash),
            "rule_hash": e.rule_hash.as_ref().map(hex),
            "payload": e.payload,
        })
    }

    /// The projects whose identity this caller has cryptographic proof of.
    async fn verified_projects(
        caller: &CallerCtx,
        kernel: &Arc<RwLock<Kernel<NativePlatform>>>,
    ) -> Vec<VerifiedProject> {
        let mut out = Vec::new();
        out.extend(VerifiedProject::from_bound(&crate::handshake_rpc::bound()));
        let token = caller.auth.as_deref().map(str::trim).unwrap_or("");
        if token.starts_with(clawft_kernel::token_authority::SECRET_PREFIX)
            && let Some(authority) = crate::token_rpc::authority_for(kernel).await
            && let Some(info) = authority.validate(token)
        {
            out.extend(VerifiedProject::from_token(&info));
        }
        out
    }

    /// Handle `chain.subscribe`. `Err` is the refusal response.
    pub async fn handle_chain_subscribe(
        params: serde_json::Value,
        caller: &CallerCtx,
        caps: &CallerCapabilities,
        kernel: Arc<RwLock<Kernel<NativePlatform>>>,
    ) -> Result<StreamHookup, Response> {
        let p: SubscribeParams = serde_json::from_value(params)
            .map_err(|e| Response::error_with_kind("invalid_params", format!("chain.subscribe: {e}")))?;
        let target = parse_target(&p.chain)
            .map_err(|m| Response::error_with_kind("invalid_params", m))?;

        if let ChainTarget::Project(id) = target {
            let verified = verified_projects(caller, &kernel).await;
            if !project_read_allowed(caps.allows(Capability::Admin), &verified, id) {
                return Err(Response::error_with_kind(
                    "project_denied",
                    format!("permission denied: reading project/{id} needs a verified project or admin"),
                ));
            }
            return Err(Response::error_with_kind(
                "not_yet_supported",
                format!("chain.subscribe for project/{id} (proxy to the project daemon) is not wired yet"),
            ));
        }

        let chain = {
            let k = kernel.read().await;
            k.chain_manager().cloned()
        }
        .ok_or_else(|| Response::error_with_kind("chain_unavailable", "chain is not enabled"))?;

        let mut sub = chain.subscribe(ChainFilter {
            kind_prefix: p.kind_prefix,
            from_seq: p.from_seq,
        });
        let (tx, rx) = tokio::sync::mpsc::channel::<Vec<u8>>(256);
        tokio::spawn(async move {
            loop {
                let item = tokio::select! {
                    _ = tx.closed() => return,
                    item = sub.next() => item,
                };
                let frame = match item {
                    Some(ChainItem::Event(e)) => event_frame(&e),
                    Some(ChainItem::Lagged(n)) => {
                        serde_json::json!({ "frame": "lagged", "missed": n })
                    }
                    None => return,
                };
                let mut line = frame.to_string();
                line.push('\n');
                if tx.send(line.into_bytes()).await.is_err() {
                    return;
                }
            }
        });

        let ack = Response::success(serde_json::json!({
            "streaming": true,
            "chain": "user",
            "from_seq": p.from_seq,
        }));
        Ok((ack, "chain.subscribe".into(), rx, Box::new(|| {})))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "01JB8Z3Q0V6X9KQ4M2N7T5R1WD";
    const B: &str = "01JB8Z3Q0V6X9KQ4M2N7T5R1WE";

    #[test]
    fn target_parsing() {
        assert_eq!(parse_target("user"), Ok(ChainTarget::User));
        assert_eq!(parse_target("project/abc"), Ok(ChainTarget::Project("abc")));
        for bad in ["", "project/", "project/a/b", "users", "Project/x"] {
            assert!(parse_target(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn project_acl_needs_admin_or_a_matching_verified_project() {
        let va = VerifiedProject::from_verified_forward(A.into());
        assert!(project_read_allowed(true, &[], A));
        assert!(project_read_allowed(false, std::slice::from_ref(&va), A));
        assert!(!project_read_allowed(false, std::slice::from_ref(&va), B));
        assert!(!project_read_allowed(false, &[], A));
    }
}
