//! Monitoring API routes.
//!
//! Provides endpoints for token usage tracking, cost breakdowns,
//! and pipeline run telemetry.

use axum::{Json, Router, extract::State, routing::get};
use serde::{Deserialize, Serialize};

use super::ApiState;

/// Build monitoring API routes.
pub fn monitoring_routes() -> Router<ApiState> {
    Router::new()
        .route("/monitoring/token-usage", get(token_usage))
        .route("/monitoring/costs", get(cost_breakdown))
        .route("/monitoring/pipeline-runs", get(pipeline_runs))
}

// ── Types ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    pub provider: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub request_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsageSummary {
    pub total_input: u64,
    pub total_output: u64,
    pub total_requests: u64,
    pub by_provider: Vec<TokenUsage>,
    pub by_session: Vec<SessionTokenUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionTokenUsage {
    pub session_key: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub request_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostBreakdown {
    pub total_cost_usd: f64,
    pub by_provider: Vec<ProviderCost>,
    pub by_tier: Vec<TierCost>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderCost {
    pub provider: String,
    pub model: String,
    pub input_cost_usd: f64,
    pub output_cost_usd: f64,
    pub total_cost_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierCost {
    pub tier: u32,
    pub label: String,
    pub request_count: u64,
    pub total_cost_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineRun {
    pub id: String,
    pub session_key: String,
    pub model: String,
    pub complexity: f64,
    pub latency_ms: u64,
    pub status: PipelineRunStatus,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PipelineRunStatus {
    Success,
    Error,
}

// ── Handlers ───────────────────────────────────────────────────

async fn token_usage(State(_state): State<ApiState>) -> Json<TokenUsageSummary> {
    // Mock data; will be wired to actual metrics collector later.
    let by_provider = vec![
        TokenUsage {
            provider: "anthropic".into(),
            model: "claude-sonnet-4".into(),
            input_tokens: 245_000,
            output_tokens: 82_000,
            total_tokens: 327_000,
            request_count: 142,
        },
        TokenUsage {
            provider: "anthropic".into(),
            model: "claude-haiku-3.5".into(),
            input_tokens: 89_000,
            output_tokens: 31_000,
            total_tokens: 120_000,
            request_count: 318,
        },
    ];

    let by_session = vec![
        SessionTokenUsage {
            session_key: "sess-abc-123".into(),
            input_tokens: 180_000,
            output_tokens: 65_000,
            request_count: 95,
        },
        SessionTokenUsage {
            session_key: "sess-def-456".into(),
            input_tokens: 98_000,
            output_tokens: 32_000,
            request_count: 210,
        },
        SessionTokenUsage {
            session_key: "sess-ghi-789".into(),
            input_tokens: 56_000,
            output_tokens: 16_000,
            request_count: 155,
        },
    ];

    let total_input = by_provider.iter().map(|p| p.input_tokens).sum();
    let total_output = by_provider.iter().map(|p| p.output_tokens).sum();
    let total_requests = by_provider.iter().map(|p| p.request_count).sum();

    Json(TokenUsageSummary {
        total_input,
        total_output,
        total_requests,
        by_provider,
        by_session,
    })
}

async fn cost_breakdown(State(_state): State<ApiState>) -> Json<CostBreakdown> {
    let by_provider = vec![
        ProviderCost {
            provider: "anthropic".into(),
            model: "claude-sonnet-4".into(),
            input_cost_usd: 0.735,
            output_cost_usd: 1.230,
            total_cost_usd: 1.965,
        },
        ProviderCost {
            provider: "anthropic".into(),
            model: "claude-haiku-3.5".into(),
            input_cost_usd: 0.022,
            output_cost_usd: 0.031,
            total_cost_usd: 0.053,
        },
    ];

    let by_tier = vec![
        TierCost {
            tier: 1,
            label: "Agent Booster (WASM)".into(),
            request_count: 1240,
            total_cost_usd: 0.0,
        },
        TierCost {
            tier: 2,
            label: "Haiku".into(),
            request_count: 318,
            total_cost_usd: 0.053,
        },
        TierCost {
            tier: 3,
            label: "Sonnet/Opus".into(),
            request_count: 142,
            total_cost_usd: 1.965,
        },
    ];

    let total_cost_usd = by_provider.iter().map(|p| p.total_cost_usd).sum();

    Json(CostBreakdown {
        total_cost_usd,
        by_provider,
        by_tier,
    })
}

async fn pipeline_runs(State(_state): State<ApiState>) -> Json<Vec<PipelineRun>> {
    let runs = vec![
        PipelineRun {
            id: "run-001".into(),
            session_key: "sess-abc-123".into(),
            model: "claude-sonnet-4".into(),
            complexity: 0.72,
            latency_ms: 3200,
            status: PipelineRunStatus::Success,
            timestamp: "2026-02-24T10:30:00Z".into(),
        },
        PipelineRun {
            id: "run-002".into(),
            session_key: "sess-def-456".into(),
            model: "claude-haiku-3.5".into(),
            complexity: 0.18,
            latency_ms: 480,
            status: PipelineRunStatus::Success,
            timestamp: "2026-02-24T10:31:00Z".into(),
        },
        PipelineRun {
            id: "run-003".into(),
            session_key: "sess-abc-123".into(),
            model: "agent-booster".into(),
            complexity: 0.05,
            latency_ms: 2,
            status: PipelineRunStatus::Success,
            timestamp: "2026-02-24T10:31:30Z".into(),
        },
        PipelineRun {
            id: "run-004".into(),
            session_key: "sess-ghi-789".into(),
            model: "claude-sonnet-4".into(),
            complexity: 0.88,
            latency_ms: 5000,
            status: PipelineRunStatus::Error,
            timestamp: "2026-02-24T10:32:00Z".into(),
        },
        PipelineRun {
            id: "run-005".into(),
            session_key: "sess-def-456".into(),
            model: "claude-haiku-3.5".into(),
            complexity: 0.22,
            latency_ms: 620,
            status: PipelineRunStatus::Success,
            timestamp: "2026-02-24T10:33:00Z".into(),
        },
    ];
    Json(runs)
}
