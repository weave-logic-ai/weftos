//! Configuration management API routes.
//!
//! Provides endpoints for reading and writing the application configuration.

use axum::{
    Json, Router,
    extract::State,
    routing::{get, put},
};

use super::ApiState;

/// Build config API routes.
pub fn config_routes() -> Router<ApiState> {
    Router::new()
        .route("/config", get(get_config))
        .route("/config", put(save_config))
}

// ── Handlers ───────────────────────────────────────────────────

async fn get_config(State(state): State<ApiState>) -> Json<serde_json::Value> {
    Json(state.config.get_config())
}

async fn save_config(
    State(state): State<ApiState>,
    Json(payload): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    match state.config.save_config(payload) {
        Ok(()) => Json(serde_json::json!({ "success": true })),
        Err(e) => Json(serde_json::json!({ "success": false, "error": e })),
    }
}
