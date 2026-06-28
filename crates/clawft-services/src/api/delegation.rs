//! Delegation monitoring API routes.
//!
//! Provides endpoints for viewing active delegations, managing delegation
//! rules, and browsing delegation history.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{delete, get, patch},
};
use serde::{Deserialize, Serialize};

use super::ApiState;

/// Build delegation API routes.
pub fn delegation_routes() -> Router<ApiState> {
    Router::new()
        .route("/delegation/active", get(list_active_delegations))
        .route("/delegation/rules", get(list_delegation_rules))
        .route("/delegation/rules", patch(upsert_delegation_rule))
        .route("/delegation/rules/{name}", delete(delete_delegation_rule))
        .route("/delegation/history", get(delegation_history))
}

// ── Types ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveDelegation {
    pub task_id: String,
    pub session_key: String,
    pub target: String,
    pub status: DelegationStatus,
    pub started_at: String,
    pub latency_ms: Option<u64>,
    pub tool_name: String,
    pub complexity: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DelegationStatus {
    Pending,
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegationRule {
    pub name: String,
    pub pattern: String,
    pub target: String,
    pub complexity_threshold: f64,
    pub enabled: bool,
    pub priority: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegationHistoryEntry {
    pub task_id: String,
    pub session_key: String,
    pub target: String,
    pub tool_name: String,
    pub status: DelegationStatus,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub latency_ms: Option<u64>,
    pub complexity: f64,
}

#[derive(Debug, Deserialize)]
pub struct HistoryQuery {
    pub session: Option<String>,
    pub target: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct PaginatedHistory {
    pub items: Vec<DelegationHistoryEntry>,
    pub total: usize,
    pub limit: usize,
    pub offset: usize,
}

// ── Handlers ───────────────────────────────────────────────────

async fn list_active_delegations(State(_state): State<ApiState>) -> Json<Vec<ActiveDelegation>> {
    // Mock data for now; will be wired to live delegation manager later.
    let delegations = vec![
        ActiveDelegation {
            task_id: "del-001".into(),
            session_key: "sess-abc-123".into(),
            target: "claude-sonnet-4".into(),
            status: DelegationStatus::Running,
            started_at: "2026-02-24T10:30:00Z".into(),
            latency_ms: Some(1250),
            tool_name: "code-review".into(),
            complexity: 0.72,
        },
        ActiveDelegation {
            task_id: "del-002".into(),
            session_key: "sess-def-456".into(),
            target: "claude-haiku-3.5".into(),
            status: DelegationStatus::Pending,
            started_at: "2026-02-24T10:31:00Z".into(),
            latency_ms: None,
            tool_name: "file-search".into(),
            complexity: 0.18,
        },
        ActiveDelegation {
            task_id: "del-003".into(),
            session_key: "sess-abc-123".into(),
            target: "agent-booster".into(),
            status: DelegationStatus::Running,
            started_at: "2026-02-24T10:31:30Z".into(),
            latency_ms: Some(2),
            tool_name: "format-code".into(),
            complexity: 0.05,
        },
    ];
    Json(delegations)
}

async fn list_delegation_rules(State(_state): State<ApiState>) -> Json<Vec<DelegationRule>> {
    // Mock rules; will be loaded from config in production.
    let rules = vec![
        DelegationRule {
            name: "simple-transforms".into(),
            pattern: "format-*|lint-*".into(),
            target: "agent-booster".into(),
            complexity_threshold: 0.1,
            enabled: true,
            priority: 1,
        },
        DelegationRule {
            name: "low-complexity".into(),
            pattern: "search-*|list-*".into(),
            target: "claude-haiku-3.5".into(),
            complexity_threshold: 0.3,
            enabled: true,
            priority: 2,
        },
        DelegationRule {
            name: "high-complexity".into(),
            pattern: "*".into(),
            target: "claude-sonnet-4".into(),
            complexity_threshold: 1.0,
            enabled: true,
            priority: 10,
        },
    ];
    Json(rules)
}

async fn upsert_delegation_rule(
    State(_state): State<ApiState>,
    Json(rule): Json<DelegationRule>,
) -> Json<DelegationRule> {
    // Stub: in production this persists to config.
    Json(rule)
}

async fn delete_delegation_rule(
    State(_state): State<ApiState>,
    Path(name): Path<String>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "deleted": name }))
}

async fn delegation_history(
    State(_state): State<ApiState>,
    Query(params): Query<HistoryQuery>,
) -> Json<PaginatedHistory> {
    let all_entries = vec![
        DelegationHistoryEntry {
            task_id: "del-h01".into(),
            session_key: "sess-abc-123".into(),
            target: "claude-sonnet-4".into(),
            tool_name: "code-review".into(),
            status: DelegationStatus::Completed,
            started_at: "2026-02-24T09:00:00Z".into(),
            completed_at: Some("2026-02-24T09:00:03Z".into()),
            latency_ms: Some(3200),
            complexity: 0.65,
        },
        DelegationHistoryEntry {
            task_id: "del-h02".into(),
            session_key: "sess-def-456".into(),
            target: "claude-haiku-3.5".into(),
            tool_name: "file-search".into(),
            status: DelegationStatus::Completed,
            started_at: "2026-02-24T09:15:00Z".into(),
            completed_at: Some("2026-02-24T09:15:01Z".into()),
            latency_ms: Some(480),
            complexity: 0.12,
        },
        DelegationHistoryEntry {
            task_id: "del-h03".into(),
            session_key: "sess-abc-123".into(),
            target: "agent-booster".into(),
            tool_name: "format-code".into(),
            status: DelegationStatus::Completed,
            started_at: "2026-02-24T09:20:00Z".into(),
            completed_at: Some("2026-02-24T09:20:00Z".into()),
            latency_ms: Some(1),
            complexity: 0.03,
        },
        DelegationHistoryEntry {
            task_id: "del-h04".into(),
            session_key: "sess-ghi-789".into(),
            target: "claude-sonnet-4".into(),
            tool_name: "architecture-review".into(),
            status: DelegationStatus::Failed,
            started_at: "2026-02-24T09:30:00Z".into(),
            completed_at: Some("2026-02-24T09:30:05Z".into()),
            latency_ms: Some(5000),
            complexity: 0.88,
        },
        DelegationHistoryEntry {
            task_id: "del-h05".into(),
            session_key: "sess-def-456".into(),
            target: "claude-haiku-3.5".into(),
            tool_name: "summarize".into(),
            status: DelegationStatus::Completed,
            started_at: "2026-02-24T09:45:00Z".into(),
            completed_at: Some("2026-02-24T09:45:01Z".into()),
            latency_ms: Some(620),
            complexity: 0.22,
        },
    ];

    // Apply filtering.
    let filtered: Vec<_> = all_entries
        .into_iter()
        .filter(|e| params.session.as_ref().is_none_or(|s| &e.session_key == s))
        .filter(|e| params.target.as_ref().is_none_or(|t| &e.target == t))
        .collect();

    let total = filtered.len();
    let offset = params.offset.unwrap_or(0);
    let limit = params.limit.unwrap_or(50);
    let items: Vec<_> = filtered.into_iter().skip(offset).take(limit).collect();

    Json(PaginatedHistory {
        items,
        total,
        limit,
        offset,
    })
}
