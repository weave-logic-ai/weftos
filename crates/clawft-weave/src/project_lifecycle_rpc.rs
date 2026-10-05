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
use clawft_types::project::{
    ProjectManifest, ProjectState, find_by_id, list_manifests, read_project_toml, validate_id,
};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;

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
        let Some(sup) = global() else {
            return no_supervisor();
        };
        if call.method.starts_with("project.nested.") {
            return nested(&sup, &call).await;
        }
        if call.method == "project.token.refresh" {
            return refresh(&sup, &call).await;
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
                    crate::project_supervisor::adopt::Found::AdoptedContainer { id, container_id, host_pid, .. } => {
                        json!({"project_id": id, "container_id": container_id, "pid": host_pid, "reason": "adopted"})
                    }
                    crate::project_supervisor::adopt::Found::UnverifiableContainer { id, container_id, reason } => {
                        json!({"project_id": id, "container_id": container_id, "reason": reason})
                    }
                })
                .collect();
            return Response::success(json!({ "children": children, "unverifiable": leftovers }));
        }
        let Some(raw) = raw else {
            return invalid(format!("{} needs `id`", call.method));
        };
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

/// Only a live project token belonging to the registered master can use the
/// nested lifecycle surface. A literal `admin` scope and an owner token do
/// not impersonate the master here.
async fn master_for(call: &ExtCall, sup: &Supervisor) -> Result<ProjectManifest, Response> {
    let secret = call.ctx.auth.as_deref().ok_or_else(|| {
        Response::error_with_kind(
            "nested_master_required",
            "present the master project's token",
        )
    })?;
    let authority = crate::token_rpc::authority_for(&call.ctx.kernel)
        .await
        .ok_or_else(|| {
            Response::error_with_kind(
                "nested_master_required",
                "project token authority is unavailable",
            )
        })?;
    let info = authority.validate(secret).ok_or_else(|| {
        Response::error_with_kind("nested_master_required", "invalid master project token")
    })?;
    if info.scope != clawft_kernel::token_authority::TokenScope::Project {
        return Err(Response::error_with_kind(
            "nested_master_required",
            "a project token is required",
        ));
    }
    let id = info.project.ok_or_else(|| {
        Response::error_with_kind("nested_master_required", "token has no project binding")
    })?;
    let master = find_by_id(&sup.config().manifests_dir, &id)
        .map_err(|e| Response::error_with_kind("nested_master_required", e.to_string()))?
        .ok_or_else(|| {
            Response::error_with_kind("nested_master_required", "master is not registered")
        })?;
    if master.state != ProjectState::Active {
        return Err(Response::error_with_kind(
            "nested_master_required",
            "master is not active",
        ));
    }
    let pt = read_project_toml(&master.root)
        .map_err(|e| Response::error_with_kind("nested_master_required", e.to_string()))?
        .ok_or_else(|| {
            Response::error_with_kind("nested_master_required", "master has no project.toml")
        })?;
    if pt.id != id || !pt.is_weave_master() {
        return Err(Response::error_with_kind(
            "nested_master_required",
            "project has not enabled weave.master",
        ));
    }
    Ok(master)
}

// `Response` is this module's error currency (it is what the RPC returns); boxing it here
// alone would only add a deref at every call site.
#[allow(clippy::result_large_err)]
fn child_under_master(master: &ProjectManifest, root: &Path) -> Result<String, Response> {
    let parent = master.root.canonicalize().map_err(|e| {
        Response::error_with_kind("nested_project_refused", format!("master root: {e}"))
    })?;
    let child = root.canonicalize().map_err(|e| {
        Response::error_with_kind("nested_project_refused", format!("child root: {e}"))
    })?;
    if child == parent || !child.starts_with(&parent) {
        return Err(Response::error_with_kind(
            "nested_project_refused",
            "child root is outside the master",
        ));
    }
    let pt = read_project_toml(&child)
        .map_err(|e| Response::error_with_kind("nested_project_refused", e.to_string()))?
        .ok_or_else(|| {
            Response::error_with_kind("nested_project_refused", "child needs project.toml")
        })?;
    if pt.parent.as_deref() != Some(master.id.as_str()) || pt.id == master.id {
        return Err(Response::error_with_kind(
            "nested_project_refused",
            "child does not name this master",
        ));
    }
    validate_id(&pt.id)
        .map_err(|e| Response::error_with_kind("nested_project_refused", e.to_string()))?;
    Ok(pt.id)
}

async fn nested(sup: &Arc<Supervisor>, call: &ExtCall) -> Response {
    let master = match master_for(call, sup).await {
        Ok(m) => m,
        Err(r) => return r,
    };
    if call.method == "project.nested.register" {
        let Some(root) = call.params.get("root").and_then(Value::as_str) else {
            return invalid("project.nested.register needs absolute `root`");
        };
        if !Path::new(root).is_absolute() {
            return invalid("project.nested.register needs absolute `root`");
        }
        let child = match clawft_types::project::register_existing_nested(
            Path::new(root),
            &sup.config().manifests_dir,
            &master.id,
        ) {
            Ok(m) => m,
            Err(e) => return Response::error_with_kind("nested_project_refused", e.to_string()),
        };
        #[cfg(any(unix, windows))]
        crate::shared_state::invalidate(&child.id);
        sup.record_nested_registration(&master.id, &child.id);
        return Response::success(json!({"project": child, "registration_level": "isolated"}));
    }
    let Some(id) = call.params.get("id").and_then(Value::as_str) else {
        return invalid(format!("{} needs `id`", call.method));
    };
    if validate_id(id).is_err() {
        return invalid("invalid child project id");
    }
    let child = match find_by_id(&sup.config().manifests_dir, id) {
        Ok(Some(m)) if m.state == ProjectState::Active => m,
        Ok(_) => {
            return Response::error_with_kind(
                "nested_project_refused",
                "child is not registered and active",
            );
        }
        Err(e) => return Response::error_with_kind("nested_project_refused", e.to_string()),
    };
    match child_under_master(&master, &child.root) {
        Ok(found) if found == id => {}
        Ok(_) => return Response::error_with_kind("nested_project_refused", "child id changed"),
        Err(r) => return r,
    }
    match call.method.as_str() {
        "project.nested.start" => match sup.ensure_running(id).await {
            Ok(r) => Response::success(running_json(id, &r)),
            Err(e) => e.response(),
        },
        "project.nested.stop" => match sup.stop(id).await {
            Ok(stopped) => Response::success(json!({"project_id": id, "stopped": stopped})),
            Err(e) => e.response(),
        },
        _ => invalid("unknown nested project method"),
    }
}

async fn refresh(sup: &Supervisor, call: &ExtCall) -> Response {
    let Some(id) = call
        .params
        .get("id")
        .or_else(|| call.params.get("project_id"))
        .and_then(Value::as_str)
    else {
        return invalid("project.token.refresh needs `id`");
    };
    let Some(auth) = call.ctx.auth.as_deref().map(str::trim) else {
        return Response::error_with_kind(
            "token_required",
            "present the project token to refresh it",
        );
    };
    if !auth.starts_with(clawft_kernel::token_authority::SECRET_PREFIX) {
        return Response::error_with_kind(
            "token_required",
            "present the project token to refresh it",
        );
    }
    match sup.launcher().refresh_token(id, auth).await {
        Ok((token, expires)) => Response::success(json!({
            "token": token,
            "expires_at": expires.to_rfc3339(),
        })),
        Err(m) => Response::error_with_kind("token_refresh_refused", m),
    }
}

#[cfg(test)]
mod nested_tests {
    use super::*;
    use clawft_types::project::{adopt_or_init, write_project_toml};

    #[test]
    fn nested_child_must_be_inside_and_name_its_master() {
        let temp = tempfile::tempdir().unwrap();
        let manifests = temp.path().join("manifests");
        let root = temp.path().join("master");
        let child = root.join("inner");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let master = adopt_or_init(&root, &manifests, Some("master")).unwrap();
        let inner = adopt_or_init(&child, &manifests, Some("inner")).unwrap();
        let mut pt = read_project_toml(&child).unwrap().unwrap();
        assert!(child_under_master(&master, &child).is_err());
        pt.parent = Some(master.id.clone());
        write_project_toml(&child, &pt).unwrap();
        assert_eq!(child_under_master(&master, &child).unwrap(), inner.id);
        write_project_toml(&outside, &pt).unwrap();
        assert!(child_under_master(&master, &outside).is_err());
    }
}
