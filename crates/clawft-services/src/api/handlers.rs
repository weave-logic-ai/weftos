//! HTTP request handlers for the REST API.

use axum::{
    extract::{Path, State},
    routing::{delete, get, post},
    Json, Router,
};

use super::ApiState;

/// Build all API routes.
pub fn api_routes() -> Router<ApiState> {
    Router::new()
        // Agent endpoints
        .route("/agents", get(list_agents))
        .route("/agents/{name}", get(get_agent))
        .route("/agents/{name}/start", post(start_agent))
        .route("/agents/{name}/stop", post(stop_agent))
        // Session endpoints
        .route("/sessions", get(list_sessions))
        .route("/sessions/{key}", get(get_session))
        .route("/sessions/{key}", delete(delete_session))
        // Tool endpoints
        .route("/tools", get(list_tools))
        .route("/tools/{name}/schema", get(get_tool_schema))
        // Auth
        .route("/auth/token", post(create_token))
        // Health check
        .route("/health", get(health_check))
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

async fn delete_session(
    State(state): State<ApiState>,
    Path(key): Path<String>,
) -> Json<bool> {
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

async fn create_token(
    State(state): State<ApiState>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let token = state
        .auth
        .generate_token(86400) // 24h TTL
        .ok_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::json!({ "token": token })))
}

/// Server start time, set once at process start.
static START_TIME: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// Returns basic health status, version, and uptime.
async fn health_check() -> Json<serde_json::Value> {
    let start = START_TIME.get_or_init(std::time::Instant::now);
    let uptime_secs = start.elapsed().as_secs();
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_secs": uptime_secs
    }))
}

// CSP, CORS deny-by-default, per-IP rate limiting, and Bearer-token
// auth are all implemented in `super::middleware` and `super::auth`,
// and wired in `super::build_router`. See WEFT-99/100/101/298.
