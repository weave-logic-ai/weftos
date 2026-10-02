//! `auth.token.{issue,revoke,list,validate}` (ADR-102 D3, cards 01).
//!
//! Registered in `rpc_ext::ROUTES` with explicit capabilities:
//!
//! | method | capability | why |
//! |---|---|---|
//! | `issue`, `revoke`, `list` | `Admin` | only the local socket owner (or an in-process admin) may mint, kill or enumerate tokens |
//! | `validate` | `Read` | the gateway and any caller may test a secret; a 256-bit secret is not guessable, and the reply carries only public metadata |
//!
//! A caller presenting a token secret (`wft_...`) is refused `issue`,
//! `revoke` and `list` even though a token carries owner scope: a token
//! must not be able to mint longer-lived tokens. This also covers a token
//! arriving through the TCP relay, which cannot be told apart from a
//! local caller by transport (the relay byte-copies into the unix socket).
//!
//! `validate` has no per-connection rate limit: `ExtCtx` carries no
//! connection identity. Brute force is infeasible at 256 bits; revisit
//! when a connection id exists.
//!
//! The authority lives per chain manager (one per kernel), built on first
//! use and rebuilt from the chain at that point, which is daemon start for
//! the real daemon and kernel creation for tests.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use chrono::Duration;
use clawft_kernel::boot::Kernel;
use clawft_kernel::token_authority::{Issuer, MAX_TTL, SECRET_PREFIX, TokenAuthority, TokenInfo};
use clawft_platform::NativePlatform;
use clawft_rpc::Response;
use serde_json::{Value, json};
use tokio::sync::RwLock;

use crate::rpc_ext::{ExtCall, ExtFuture};

/// Error kind when a token bearer tries to mint, revoke or list.
pub const TOKEN_CANNOT_MINT_KIND: &str = "token_cannot_manage_tokens";

static AUTHORITIES: OnceLock<Mutex<HashMap<usize, Arc<TokenAuthority>>>> = OnceLock::new();

/// The authority for this kernel's chain, created (and replayed) on first
/// use. `None` when the kernel has no chain.
///
/// Keyed by the chain manager's address. The authority holds a strong
/// reference to its chain, so an address cannot be reused while its entry
/// exists; entries whose chain nothing else references (the kernel is
/// gone) are pruned on each lookup.
pub async fn authority_for(
    kernel: &Arc<RwLock<Kernel<NativePlatform>>>,
) -> Option<Arc<TokenAuthority>> {
    let k = kernel.read().await;
    authority_for_kernel(&k)
}

/// [`authority_for`] for a caller that already holds the kernel (the
/// project supervisor is built from `&Kernel` at boot).
pub fn authority_for_kernel(k: &Kernel<NativePlatform>) -> Option<Arc<TokenAuthority>> {
    let chain = Arc::clone(k.chain_manager()?);
    let key = Arc::as_ptr(&chain) as usize;
    let mut map = AUTHORITIES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    map.retain(|_, a| a.chain_refs() > 1);
    if let Some(a) = map.get(&key) {
        return Some(Arc::clone(a));
    }
    let node_id = k.cluster_membership().local_node_id().to_owned();
    // The journal sits beside the chain checkpoint (see `with_journal`).
    let journal = k
        .kernel_config()
        .chain
        .clone()
        .unwrap_or_default()
        .effective_checkpoint_path()
        .and_then(|p| {
            std::path::Path::new(&p)
                .parent()
                .map(|d| d.join("auth-tokens.jsonl"))
        });
    let a = Arc::new(TokenAuthority::with_journal(chain, node_id, journal));
    map.insert(key, Arc::clone(&a));
    Some(a)
}

fn info_json(i: &TokenInfo) -> Value {
    json!({
        "id": i.id,
        "label": i.label,
        "issued_at": i.issued_at.to_rfc3339(),
        "expires_at": i.expires_at.to_rfc3339(),
        "scope": i.scope,
        "project": i.project,
    })
}

fn local_uid() -> Option<u32> {
    #[cfg(unix)]
    {
        Some(nix::unistd::geteuid().as_raw())
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Handler for every `auth.token.*` route.
pub fn handle(call: ExtCall) -> ExtFuture {
    Box::pin(async move {
        let Some(authority) = authority_for(&call.ctx.kernel).await else {
            return Response::error("token authority unavailable: kernel has no chain");
        };
        // On a project-bound kernel a token may only be scoped to that
        // project: a `project` param naming another is refused.
        if call.method == "auth.token.issue"
            && let (Some(v), Some(p)) = (
                call.ctx.verified_project.as_ref(),
                call.params.get("project").and_then(Value::as_str),
            )
            && p != v.as_str()
        {
            return Response::error_with_kind(
                crate::caller_principal::SCOPE_MISMATCH_KIND,
                "auth.token.issue: 'project' differs from the verified project of this kernel",
            );
        }
        run(
            &authority,
            &call.method,
            &call.params,
            call.ctx.auth.as_deref(),
        )
    })
}

/// Method logic, separated from kernel plumbing for tests.
pub fn run(
    authority: &TokenAuthority,
    method: &str,
    params: &Value,
    auth: Option<&str>,
) -> Response {
    if method != "auth.token.validate" && auth.is_some_and(|a| a.trim().starts_with(SECRET_PREFIX))
    {
        return Response::error_with_kind(
            TOKEN_CANNOT_MINT_KIND,
            "a token cannot issue, revoke or list tokens; use the local socket as the owner",
        );
    }
    match method {
        "auth.token.issue" => issue(authority, params),
        "auth.token.revoke" => match params.get("id").and_then(Value::as_str) {
            Some(id) => match authority.revoke(id) {
                Ok(revoked) => Response::success(json!({ "revoked": revoked, "id": id })),
                Err(e) => Response::error(format!("auth.token.revoke: {e}")),
            },
            None => Response::error("auth.token.revoke: missing string param 'id'"),
        },
        "auth.token.list" => {
            let tokens: Vec<Value> = authority.list().iter().map(info_json).collect();
            Response::success(json!({ "tokens": tokens }))
        }
        "auth.token.validate" => match params.get("token").and_then(Value::as_str) {
            Some(t) => match authority.validate(t) {
                Some(info) => {
                    Response::success(json!({ "valid": true, "token": info_json(&info) }))
                }
                None => Response::success(json!({ "valid": false })),
            },
            None => Response::error("auth.token.validate: missing string param 'token'"),
        },
        other => Response::error(format!("unknown method: {other}")),
    }
}

fn issue(authority: &TokenAuthority, params: &Value) -> Response {
    let label = params
        .get("label")
        .and_then(Value::as_str)
        .unwrap_or("weft-cli");
    let ttl = match params.get("ttl_secs") {
        None | Some(Value::Null) => None,
        Some(v) => match v.as_i64() {
            // Compare before `Duration::seconds`, which panics on huge values.
            Some(s) if s > MAX_TTL.num_seconds() => {
                return Response::error("auth.token.issue: ttl exceeds the 24 h maximum");
            }
            Some(s) if s > 0 => Some(Duration::seconds(s)),
            _ => return Response::error("auth.token.issue: 'ttl_secs' must be a positive integer"),
        },
    };
    let project = match params.get("project") {
        None | Some(Value::Null) => None,
        Some(v) => match v
            .as_str()
            .filter(|p| clawft_types::project::validate_id(p).is_ok())
        {
            Some(p) => Some(p.to_owned()),
            None => return Response::error("auth.token.issue: 'project' must be a ULID"),
        },
    };
    let issuer = Issuer { uid: local_uid() };
    match authority.issue(label, ttl, project, &issuer) {
        Ok((secret, info)) => {
            let mut v = info_json(&info);
            v["secret"] = json!(secret);
            Response::success(v)
        }
        Err(e) => Response::error(format!("auth.token.issue: {e}")),
    }
}

#[cfg(test)]
#[path = "token_rpc_tests.rs"]
mod tests;
