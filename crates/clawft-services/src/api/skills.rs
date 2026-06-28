//! Skills management API routes.
//!
//! Provides endpoints for listing installed skills, installing/uninstalling
//! skills, and searching the skill registry (ClawHub).

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};

use super::ApiState;

/// Build skills API routes.
pub fn skills_routes() -> Router<ApiState> {
    Router::new()
        .route("/skills", get(list_skills))
        .route("/skills/install", post(install_skill))
        .route("/skills/{name}", delete(uninstall_skill))
        .route("/skills/registry/search", get(search_registry))
}

// ── Types ──────────────────────────────────────────────────────

/// Response shape matching SkillData in the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDataResponse {
    pub name: String,
    pub version: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    pub tags: Vec<String>,
    pub installed: bool,
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct InstallRequest {
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct RegistrySearchQuery {
    pub q: Option<String>,
}

/// Registry skill shape matching RegistrySkill in the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistrySkillResponse {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub author: String,
    pub stars: u32,
    pub tags: Vec<String>,
    pub signed: bool,
}

// ── Handlers ───────────────────────────────────────────────────

async fn list_skills(State(state): State<ApiState>) -> Json<Vec<SkillDataResponse>> {
    let skills = state.skills.list_skills();
    let responses: Vec<SkillDataResponse> = skills
        .into_iter()
        .map(|s| SkillDataResponse {
            name: s.name,
            version: s.version,
            description: s.description,
            author: None,
            tags: Vec::new(),
            installed: true,
            enabled: true,
        })
        .collect();
    Json(responses)
}

async fn install_skill(
    State(state): State<ApiState>,
    Json(payload): Json<InstallRequest>,
) -> Json<serde_json::Value> {
    match state.skills.install_skill(&payload.id) {
        Ok(()) => Json(serde_json::json!({ "success": true })),
        Err(e) => Json(serde_json::json!({ "success": false, "error": e })),
    }
}

async fn uninstall_skill(
    State(state): State<ApiState>,
    Path(name): Path<String>,
) -> Json<serde_json::Value> {
    match state.skills.uninstall_skill(&name) {
        Ok(()) => Json(serde_json::json!({ "success": true })),
        Err(e) => Json(serde_json::json!({ "success": false, "error": e })),
    }
}

async fn search_registry(
    Query(_params): Query<RegistrySearchQuery>,
) -> Json<Vec<RegistrySkillResponse>> {
    // Stub: ClawHub registry integration will be added later.
    Json(Vec::new())
}
