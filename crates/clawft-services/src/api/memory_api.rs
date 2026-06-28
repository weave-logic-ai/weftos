//! Memory management API routes.
//!
//! Provides endpoints for listing, searching, creating, and deleting
//! memory entries.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{delete, get, post},
};
use serde::Deserialize;

use super::{ApiState, MemoryEntryInfo};

/// Build memory API routes.
pub fn memory_routes() -> Router<ApiState> {
    Router::new()
        .route("/memory", get(list_memory))
        .route("/memory", post(create_memory))
        .route("/memory/search", get(search_memory))
        .route("/memory/{key}", delete(delete_memory))
}

// ── Types ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct MemorySearchQuery {
    pub q: Option<String>,
    pub threshold: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct CreateMemoryRequest {
    pub key: String,
    pub value: String,
    #[serde(default)]
    pub namespace: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

// ── Handlers ───────────────────────────────────────────────────

async fn list_memory(State(state): State<ApiState>) -> Json<Vec<MemoryEntryInfo>> {
    Json(state.memory.list_entries())
}

async fn create_memory(
    State(state): State<ApiState>,
    Json(payload): Json<CreateMemoryRequest>,
) -> Json<serde_json::Value> {
    match state.memory.store(
        &payload.key,
        &payload.value,
        &payload.namespace,
        &payload.tags,
    ) {
        Ok(entry) => Json(serde_json::to_value(entry).unwrap_or_default()),
        Err(e) => Json(serde_json::json!({ "error": e })),
    }
}

async fn search_memory(
    State(state): State<ApiState>,
    Query(params): Query<MemorySearchQuery>,
) -> Json<Vec<MemoryEntryInfo>> {
    let query = params.q.unwrap_or_default();
    let _threshold = params.threshold.unwrap_or(0.0);
    let results = state.memory.search(&query, 50);
    Json(results)
}

async fn delete_memory(
    State(state): State<ApiState>,
    Path(key): Path<String>,
) -> Json<serde_json::Value> {
    let deleted = state.memory.delete(&key);
    Json(serde_json::json!({ "success": deleted }))
}
