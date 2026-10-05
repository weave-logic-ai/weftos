//! Project read routes (dashboard project console, slice 5).
//!
//! | route | daemon RPC | notes |
//! |---|---|---|
//! | `GET /api/projects` | `project.list` | a project-bound token sees only its own project |
//! | `GET /api/projects/{ulid}` | `project.show` + `project.status` | merged; status is sanitised |
//! | `GET /api/fleet/snapshot[?project=<ulid>]` | `fleet.snapshot` | instances filtered to one project |
//!
//! All three are open to read-scoped tokens (see [`super::auth::READ_TOKEN_PATHS`]).
//! A read token bound to a project (ADR-102 D4 amendment) is confined to that
//! project here and in the auth middleware: another project's id answers 403,
//! the list holds only its project, and the snapshot is forced to it whatever
//! `?project=` says.

use axum::{
    Json, Router,
    extract::{Path, Query, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use clawft_kernel::http_facade::FacadeResponse;
use serde::Deserialize;
use serde_json::{Value, json};

use super::ApiState;
use super::auth::TokenMeta;

/// Nest-relative routes (mounted under `/api`).
pub fn project_routes() -> Router<ApiState> {
    Router::new()
        .route("/projects", get(list_projects))
        .route("/projects/{id}", get(show_project))
        .route("/fleet/snapshot", get(fleet_snapshot))
}

/// The project the calling token is confined to, if any.
fn bound_project(request: &Request) -> Option<String> {
    request.extensions().get::<TokenMeta>().and_then(|m| m.project.clone())
}

fn valid_ulid(id: &str) -> bool {
    clawft_types::project::validate_id(id).is_ok()
}

fn reply(status: StatusCode, error: &str) -> Response {
    (status, Json(json!({ "error": error }))).into_response()
}

fn facade_reply(resp: FacadeResponse) -> Response {
    let status = StatusCode::from_u16(resp.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(resp.body)).into_response()
}

/// `GET /api/projects`.
async fn list_projects(State(state): State<ApiState>, request: Request) -> Response {
    let bound = bound_project(&request);
    let resp = state.kernel_facade.call_rpc("project.list", json!({})).await;
    if resp.status != 200 {
        return facade_reply(resp);
    }
    let mut body = resp.body;
    if let Some(own) = bound {
        // Confined caller: only its project, and none of the store's
        // filesystem diagnostics.
        if let Some(rows) = body["projects"].as_array_mut() {
            rows.retain(|m| m["id"].as_str() == Some(own.as_str()));
        }
        if let Some(obj) = body.as_object_mut() {
            obj.remove("skipped");
        }
    }
    Json(body).into_response()
}

/// Fields of `project.status` the console may see. The pid, socket path,
/// container identity and failure text stay on the daemon.
const STATUS_FIELDS: &[&str] = &[
    "state",
    "restarts",
    "last_exit_code",
    "kernel_sha",
    "kernel_version",
    "stale_build",
    "unregistered_secs",
];

fn sanitise_status(status: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for k in STATUS_FIELDS {
        if let Some(v) = status.get(*k) {
            out.insert((*k).to_owned(), v.clone());
        }
    }
    Value::Object(out)
}

/// `GET /api/projects/{ulid}`: the manifest plus the supervisor's status.
async fn show_project(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    if !valid_ulid(&id) {
        return reply(StatusCode::BAD_REQUEST, "project id must be a ULID");
    }
    if bound_project(&request).is_some_and(|own| own != id) {
        return reply(StatusCode::FORBIDDEN, "this token is confined to another project");
    }
    let shown = state.kernel_facade.call_rpc("project.show", json!({ "id": id })).await;
    if shown.status != 200 {
        return facade_reply(shown);
    }
    // Status is best-effort: a project that is registered but never started,
    // or a daemon without a supervisor, still has a manifest to show.
    let status = state.kernel_facade.call_rpc("project.status", json!({ "id": id })).await;
    let status = (status.status == 200).then(|| sanitise_status(&status.body));
    Json(json!({
        "project": shown.body["project"],
        "status": status,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
struct SnapshotQuery {
    project: Option<String>,
}

/// Keep, in every node's `instances`, only the rows placed for `project`.
/// An instance with no project belongs to the machine and is dropped too
/// (same rule as the daemon's own project-scoped snapshot).
pub fn filter_snapshot_to_project(snapshot: &mut Value, project: &str) {
    let Some(nodes) = snapshot["nodes"].as_array_mut() else { return };
    for node in nodes {
        let Some(obj) = node.as_object_mut() else { continue };
        let empty = match obj.get_mut("instances").and_then(|i| i.get_mut("value")) {
            Some(Value::Array(rows)) => {
                rows.retain(|r| r["placement"]["project_id"].as_str() == Some(project));
                rows.is_empty()
            }
            _ => false,
        };
        if empty {
            obj.remove("instances");
        }
    }
}

/// `GET /api/fleet/snapshot[?project=<ulid>]`.
async fn fleet_snapshot(
    State(state): State<ApiState>,
    Query(q): Query<SnapshotQuery>,
    request: Request,
) -> Response {
    let wanted = match (bound_project(&request), q.project) {
        (Some(own), Some(asked)) if own != asked => {
            return reply(StatusCode::FORBIDDEN, "this token is confined to another project");
        }
        (Some(own), _) => Some(own),
        (None, Some(asked)) if valid_ulid(&asked) => Some(asked),
        (None, Some(_)) => return reply(StatusCode::BAD_REQUEST, "project must be a ULID"),
        (None, None) => None,
    };
    let resp = state.kernel_facade.call_rpc("fleet.snapshot", json!({})).await;
    if resp.status != 200 {
        return facade_reply(resp);
    }
    let mut body = resp.body;
    if let Some(project) = wanted {
        filter_snapshot_to_project(&mut body, &project);
    }
    Json(body).into_response()
}
