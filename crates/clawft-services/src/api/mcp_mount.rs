//! MCP over HTTP inside the gateway (ADR-102 D2).
//!
//! `POST /mcp` carries JSON-RPC (`initialize`, `tools/list`, `tools/call`)
//! for the same origin and the same bearer as the REST API: the gateway's
//! daemon-token middleware authenticates, then every call runs with
//! [`SessionScopes::owner`] (ADR-102 D4: a token is owner-equivalent).
//! The tool surface is whatever [`McpServerShell`] the gateway was built
//! with (the `full` profile in production).
//!
//! Unlike `weft mcp-server --listen`, there is no second token store and no
//! SSE stream here; JSON-RPC over POST only.

use std::sync::{Arc, RwLock};

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use super::ApiState;
use super::auth::TokenMeta;
use crate::mcp::server::McpServerShell;
use crate::mcp::session_cap::SessionScopes;

/// Largest accepted request body.
const MAX_BODY: usize = 1 << 20;

/// The shell mounted at `/mcp`, with the facts `/api/health` reports.
pub struct McpMount {
    shell: Mutex<McpServerShell>,
    /// Handle the audit middleware reads the client label from.
    audit_label: Arc<RwLock<String>>,
    /// Serve profile name (e.g. `full`).
    pub profile: String,
    /// Number of tools the shell lists.
    pub tool_count: usize,
}

impl McpMount {
    /// Mount `shell`; `audit_label` is the handle its `AuditLog` exposes.
    pub fn new(
        shell: McpServerShell,
        audit_label: Arc<RwLock<String>>,
        profile: impl Into<String>,
        tool_count: usize,
    ) -> Self {
        Self {
            shell: Mutex::new(shell),
            audit_label,
            profile: profile.into(),
            tool_count,
        }
    }
}

/// `/mcp` routes. The caller wraps them in the auth middleware.
pub fn mcp_routes() -> Router<ApiState> {
    Router::new().route("/mcp", post(mcp_post))
}

async fn mcp_post(State(state): State<ApiState>, request: axum::extract::Request) -> Response {
    let Some(mount) = state.mcp.clone() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let label = request.extensions().get::<TokenMeta>().map(|m| m.label.clone());
    let body = match axum::body::to_bytes(request.into_body(), MAX_BODY).await {
        Ok(b) => b,
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    let msg: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "jsonrpc": "2.0", "id": null,
                    "error": { "code": -32700, "message": format!("parse error: {e}") },
                })),
            )
                .into_response();
        }
    };
    if let (Some(label), Ok(mut g)) = (label, mount.audit_label.write()) {
        *g = label;
    }
    let mut shell = mount.shell.lock().await;
    match shell.handle_message(msg, Some(&SessionScopes::owner())).await {
        Some(resp) => (StatusCode::OK, Json(resp)).into_response(),
        None => StatusCode::ACCEPTED.into_response(),
    }
}
