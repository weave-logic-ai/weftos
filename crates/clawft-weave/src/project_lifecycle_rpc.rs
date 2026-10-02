//! `project.start|stop|restart|status|ensure_running|stop_all` and
//! `project.token.refresh` (ADR-103 A6, Phase 2 package G).
//!
//! | method | capability | what it does |
//! |---|---|---|
//! | `project.start` `{id}` | `Admin` | start the project's child kernel (idempotent) |
//! | `project.ensure_running` `{id}` | `Admin` | same, for the resolver: returns the child socket |
//! | `project.stop` `{id}` | `Admin` | graceful stop; not restarted; `unmanaged_pid` names a live kernel the supervisor does not manage |
//! | `project.restart` `{id}` | `Admin` | stop, clear a `failed` state, start |
//! | `project.status` `{id?}` | `Admin` | one project, or every child plus unverifiable leftovers |
//! | `project.stop_all` | `Admin` | stop every child (user-daemon stop cascade) |
//! | `project.token.refresh` `{id}` | `Write` | a child renews its project token |
//!
//! `id` may also be the project's registered name when it is unique. A
//! project token (Write only) is refused by every `Admin` method by the
//! capability check; `project.token.refresh` is the one thing a token may do
//! here, and only for its own project.

use clawft_rpc::Response;
use clawft_types::project::{list_manifests, validate_id};
use serde_json::{Value, json};

use crate::project_supervisor::{Running, SupError, Supervisor, global};
use crate::rpc_ext::{ExtCall, ExtFuture};

fn no_supervisor() -> Response {
    Response::error_with_kind(
        "not_user_daemon",
        "project supervision runs only in the user daemon (`weaver kernel start --profile user`)",
    )
}

fn invalid(msg: impl Into<String>) -> Response {
    Response::error_with_kind("invalid_params", msg)
}

/// A ULID, or the unique registered name resolved to its ULID.
pub fn resolve_id(sup: &Supervisor, raw: &str) -> Result<String, SupError> {
    if validate_id(raw).is_ok() {
        return Ok(raw.to_owned());
    }
    let listing = list_manifests(&sup.config().manifests_dir)
        .map_err(|e| SupError::Identity(e.to_string()))?;
    let mut hits = listing.manifests.iter().filter(|m| m.name == raw);
    match (hits.next(), hits.next()) {
        (Some(m), None) => Ok(m.id.clone()),
        (None, _) => Err(SupError::NotRegistered(raw.to_owned())),
        (Some(_), Some(_)) => Err(SupError::InvalidId(format!(
            "{raw:?} names more than one project; use its id"
        ))),
    }
}

fn running_json(id: &str, r: &Running) -> Value {
    json!({
        "project_id": id,
        "socket": r.socket,
        "pid": r.pid,
        "started": r.started,
        "state": "running",
    })
}

/// Handler for every method in the module table.
pub fn handle(call: ExtCall) -> ExtFuture {
    Box::pin(async move {
        let Some(sup) = global() else { return no_supervisor() };
        if call.method == "project.token.refresh" {
            return refresh(&sup, &call);
        }
        if call.method == "project.stop_all" {
            let stopped = sup.stop_all().await;
            return Response::success(json!({ "stopped": stopped }));
        }
        let raw = call
            .params
            .get("id")
            .or_else(|| call.params.get("project_id"))
            .and_then(Value::as_str);
        if call.method == "project.status" && raw.is_none() {
            let children: Vec<Value> = sup.status_all().await.iter().map(|s| s.to_json()).collect();
            let leftovers: Vec<Value> = sup
                .unverifiable()
                .iter()
                .map(|f| match f {
                    crate::project_supervisor::adopt::Found::Unverifiable { id, pid, reason } => {
                        json!({"project_id": id, "pid": pid, "reason": reason.to_string()})
                    }
                    crate::project_supervisor::adopt::Found::Adopted { id, pid } => {
                        json!({"project_id": id, "pid": pid, "reason": "adopted"})
                    }
                })
                .collect();
            return Response::success(json!({ "children": children, "unverifiable": leftovers }));
        }
        let Some(raw) = raw else { return invalid(format!("{} needs `id`", call.method)) };
        let id = match resolve_id(&sup, raw) {
            Ok(id) => id,
            Err(e) => return e.response(),
        };
        let out = match call.method.as_str() {
            "project.start" | "project.ensure_running" => {
                sup.ensure_running(&id).await.map(|r| running_json(&id, &r))
            }
            "project.restart" => sup.restart(&id).await.map(|r| running_json(&id, &r)),
            "project.stop" => sup.stop(&id).await.map(|was| {
                let mut v = json!({"project_id": id, "stopped": was});
                // Not running under the supervisor is not the same as not
                // running: a verified-but-refused leftover is still there.
                if !was && let Some((pid, why)) = sup.unmanaged(&id) {
                    v["unmanaged_pid"] = json!(pid);
                    v["unmanaged_reason"] = json!(why);
                }
                v
            }),
            "project.status" => Ok(sup.status(&id).await.to_json()),
            other => return invalid(format!("unknown method: {other}")),
        };
        match out {
            Ok(v) => Response::success(v),
            Err(e) => e.response(),
        }
    })
}

fn refresh(sup: &Supervisor, call: &ExtCall) -> Response {
    let Some(id) = call
        .params
        .get("id")
        .or_else(|| call.params.get("project_id"))
        .and_then(Value::as_str)
    else {
        return invalid("project.token.refresh needs `id`");
    };
    let Some(auth) = call.ctx.auth.as_deref().map(str::trim) else {
        return Response::error_with_kind("token_required", "present the project token to refresh it");
    };
    if !auth.starts_with(clawft_kernel::token_authority::SECRET_PREFIX) {
        return Response::error_with_kind("token_required", "present the project token to refresh it");
    }
    match sup.launcher().refresh_token(id, auth) {
        Ok((token, expires)) => Response::success(json!({
            "token": token,
            "expires_at": expires.to_rfc3339(),
        })),
        Err(m) => Response::error_with_kind("token_refresh_refused", m),
    }
}
