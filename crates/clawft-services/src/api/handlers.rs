//! HTTP request handlers for the REST API.

use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{delete, get, post},
};

use super::ApiState;

/// Build all API routes.
pub fn api_routes() -> Router<ApiState> {
    Router::new()
        // Agent endpoints
        .route("/agents", get(list_agents))
        // GET = dashboard agent detail; DELETE = WEFT-122 facade agent.stop
        // (numeric pid). Same path-param pattern — axum cannot register
        // `/agents/{name}` and `/agents/{pid}` separately.
        .route(
            "/agents/{name}",
            get(get_agent).delete(super::http_facade_api::delete_agent_by_pid),
        )
        .route("/agents/{name}/start", post(start_agent))
        .route("/agents/{name}/stop", post(stop_agent))
        // Session endpoints
        .route("/sessions", get(list_sessions))
        .route("/sessions/{key}", get(get_session))
        .route("/sessions/{key}", delete(delete_session))
        // Tool endpoints
        .route("/tools", get(list_tools))
        .route("/tools/{name}/schema", get(get_tool_schema))
        // Auth: tokens are minted by the daemon (`weft token issue`), never
        // over HTTP (ADR-102 D5); only self-revoke is exposed here.
        .route("/auth/revoke", post(revoke_token))
        // Health check (tiered by token, ADR-102 D1)
        .route("/health", get(super::health::health_check))
        // Delegation monitoring
        .merge(super::delegation::delegation_routes())
        // System monitoring
        .merge(super::monitoring::monitoring_routes())
        // Skills
        .merge(super::skills::skills_routes())
        // Memory
        .merge(super::memory_api::memory_routes())
        // Config
        .merge(super::config_api::config_routes())
        // Cron
        .merge(super::cron_api::cron_routes())
        // Channels
        .merge(super::channels_api::channel_routes())
        // Chat (session messages, create, export)
        .merge(super::chat::chat_routes())
        // Voice
        .merge(super::voice_api::voice_routes())
        // WEFT-122: kernel http_facade REST surface (status/chain/vectors/ecc/agents)
        .merge(super::http_facade_api::kernel_facade_api_routes())
}

async fn list_agents(State(state): State<ApiState>) -> Json<Vec<super::AgentInfo>> {
    Json(state.agents.list_agents())
}

async fn get_agent(
    State(state): State<ApiState>,
    Path(name): Path<String>,
) -> Json<Option<super::AgentInfo>> {
    Json(state.agents.get_agent(&name))
}

async fn start_agent(
    State(_state): State<ApiState>,
    Path(_name): Path<String>,
) -> Json<serde_json::Value> {
    // Stub: agent start will be wired to agent lifecycle management.
    Json(serde_json::json!({ "ok": true }))
}

async fn stop_agent(
    State(_state): State<ApiState>,
    Path(_name): Path<String>,
) -> Json<serde_json::Value> {
    // Stub: agent stop will be wired to agent lifecycle management.
    Json(serde_json::json!({ "ok": true }))
}

async fn list_sessions(State(state): State<ApiState>) -> Json<Vec<super::SessionInfo>> {
    Json(state.sessions.list_sessions())
}

async fn get_session(
    State(state): State<ApiState>,
    Path(key): Path<String>,
) -> Json<Option<super::SessionDetail>> {
    Json(state.sessions.get_session(&key))
}

async fn delete_session(State(state): State<ApiState>, Path(key): Path<String>) -> Json<bool> {
    Json(state.sessions.delete_session(&key))
}

async fn list_tools(State(state): State<ApiState>) -> Json<Vec<super::ToolInfo>> {
    Json(state.tools.list_tools())
}

async fn get_tool_schema(
    State(state): State<ApiState>,
    Path(name): Path<String>,
) -> Json<Option<serde_json::Value>> {
    Json(state.tools.tool_schema(&name))
}

/// `POST /api/auth/revoke` — server-side logout for the bearer used to
/// authenticate this very request. WEFT-570, ADR-102 D5.
///
/// The auth middleware already validated the bearer and left its
/// [`TokenMeta`](super::auth::TokenMeta) in the request extensions; the
/// handler forwards that token's own id to the daemon's `auth.token.revoke`.
/// There is no way to name another token here, so one caller cannot revoke
/// another's. Returns 204 on success and 503 when the daemon is down (the
/// token is then still live, and the caller is told so).
async fn revoke_token(
    State(state): State<ApiState>,
    request: axum::extract::Request,
) -> axum::http::StatusCode {
    use super::auth::{RevokeOutcome, TokenMeta};
    let Some(meta) = request.extensions().get::<TokenMeta>() else {
        return axum::http::StatusCode::UNAUTHORIZED;
    };
    match state.auth.revoke(&meta.id).await {
        RevokeOutcome::Revoked => axum::http::StatusCode::NO_CONTENT,
        RevokeOutcome::Unavailable => axum::http::StatusCode::SERVICE_UNAVAILABLE,
    }
}

// CSP, CORS deny-by-default, per-IP rate limiting, and Bearer-token
// auth are all implemented in `super::middleware` and `super::auth`,
// and wired in `super::build_router`. See WEFT-99/100/101/298.
