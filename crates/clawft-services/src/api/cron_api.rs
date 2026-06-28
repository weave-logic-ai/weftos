//! Cron job management API routes.
//!
//! Provides endpoints for listing, creating, updating, deleting, and
//! manually running cron jobs. Currently returns stub data; CronService
//! integration will be wired in a future phase.

use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{delete, get, post, put},
};
use serde::{Deserialize, Serialize};

use super::ApiState;

/// Build cron API routes.
pub fn cron_routes() -> Router<ApiState> {
    Router::new()
        .route("/cron", get(list_cron_jobs))
        .route("/cron", post(create_cron_job))
        .route("/cron/{id}", put(update_cron_job))
        .route("/cron/{id}", delete(delete_cron_job))
        .route("/cron/{id}/run", post(run_cron_job))
}

// ── Types ──────────────────────────────────────────────────────

/// Cron job shape matching CronJob in the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJobResponse {
    pub id: String,
    pub name: String,
    pub schedule: String,
    pub enabled: bool,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_run: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateCronJobRequest {
    pub name: String,
    pub schedule: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub payload: Option<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
pub struct UpdateCronJobRequest {
    pub name: Option<String>,
    pub schedule: Option<String>,
    pub enabled: Option<bool>,
    pub payload: Option<String>,
}

// ── Handlers ───────────────────────────────────────────────────

async fn list_cron_jobs(State(_state): State<ApiState>) -> Json<Vec<CronJobResponse>> {
    // Stub: will be wired to CronService in a future phase.
    Json(Vec::new())
}

async fn create_cron_job(
    State(_state): State<ApiState>,
    Json(payload): Json<CreateCronJobRequest>,
) -> Json<CronJobResponse> {
    // Stub: returns the job as if created.
    let id = uuid::Uuid::new_v4().to_string();
    Json(CronJobResponse {
        id,
        name: payload.name,
        schedule: payload.schedule,
        enabled: payload.enabled,
        status: "idle".into(),
        last_run: None,
        next_run: None,
        payload: payload.payload,
    })
}

async fn update_cron_job(
    State(_state): State<ApiState>,
    Path(id): Path<String>,
    Json(payload): Json<UpdateCronJobRequest>,
) -> Json<CronJobResponse> {
    // Stub: returns the job as if updated.
    Json(CronJobResponse {
        id,
        name: payload.name.unwrap_or_else(|| "unnamed".into()),
        schedule: payload.schedule.unwrap_or_else(|| "0 * * * *".into()),
        enabled: payload.enabled.unwrap_or(true),
        status: "idle".into(),
        last_run: None,
        next_run: None,
        payload: payload.payload,
    })
}

async fn delete_cron_job(
    State(_state): State<ApiState>,
    Path(_id): Path<String>,
) -> Json<serde_json::Value> {
    // Stub: always returns success.
    Json(serde_json::json!({ "success": true }))
}

async fn run_cron_job(
    State(_state): State<ApiState>,
    Path(_id): Path<String>,
) -> Json<serde_json::Value> {
    // Stub: pretends to run the job.
    Json(serde_json::json!({ "success": true }))
}
