//! HTTP API for WeftOS task execution.
//!
//! Provides a lightweight HTTP endpoint that Paperclip (and other
//! orchestrators) can call to submit tasks to the WeftOS kernel.
//!
//! # Endpoints
//!
//! - `POST /api/v1/execute` -- submit a task for agent execution
//! - `POST /api/v1/govern`  -- evaluate a governance decision
//! - `GET  /api/v1/health`  -- liveness probe
//!
//! # Feature Gate
//!
//! This module is gated behind the `http-api` feature flag and is
//! **not** included in the default feature set.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::governance::{
    EffectVector, GovernanceDecision, GovernanceEngine, GovernanceRequest, GovernanceResult,
};

// ── Request / Response types ──────────────────────────────────

/// Request body for `POST /api/v1/execute`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteRequest {
    /// Identifier of the agent to execute the task.
    pub agent_id: String,

    /// Task description or prompt.
    pub task: String,

    /// Additional context passed to the agent pipeline.
    #[serde(default)]
    pub context: HashMap<String, serde_json::Value>,
}

/// Response body for `POST /api/v1/execute`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteResponse {
    /// Execution result (agent output).
    pub result: String,

    /// Confidence or quality score (0.0 -- 1.0).
    pub score: f64,

    /// Tokens consumed during execution.
    pub tokens_used: u64,

    /// Unique execution ID for traceability.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
}

/// Request body for `POST /api/v1/govern`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernRequest {
    /// Action being proposed.
    pub action: String,

    /// Agent requesting the action.
    pub agent_id: String,

    /// Additional context for governance evaluation.
    #[serde(default)]
    pub context: HashMap<String, serde_json::Value>,
}

/// Response body for `POST /api/v1/govern`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernResponse {
    /// Governance decision: `"permit"`, `"deny"`, or `"escalate"`.
    pub decision: String,

    /// Cryptographic attestation (ExoChain hash, if available).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attestation: Option<String>,

    /// Chain hash for audit trail linkage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain_hash: Option<String>,

    /// Rules that were evaluated.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evaluated_rules: Vec<String>,

    /// Effect vector magnitude that was scored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effect_magnitude: Option<f64>,
}

/// Health check response for `GET /api/v1/health`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    /// Service status: `"ok"` or `"degraded"`.
    pub status: String,

    /// Kernel version.
    pub version: String,

    /// Uptime in seconds (if tracked).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uptime_secs: Option<u64>,
}

/// Error response returned on failure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResponse {
    /// Machine-readable error code.
    pub code: String,

    /// Human-readable error message.
    pub message: String,
}

// ── HTTP API handler ──────────────────────────────────────────

/// The kernel-side HTTP API handler.
///
/// This is a transport-agnostic dispatcher: it accepts parsed request
/// types and returns serializable response types. The actual HTTP
/// server binding (hyper, axum, warp, etc.) is left to the binary
/// crate or daemon that embeds the kernel.
///
/// # Example
///
/// ```rust,ignore
/// let handler = HttpApiHandler::new(governance_engine);
/// let resp = handler.handle_execute(request).await;
/// ```
pub struct HttpApiHandler {
    governance: GovernanceEngine,
}

impl HttpApiHandler {
    /// Create a new handler backed by the given governance engine.
    pub fn new(governance: GovernanceEngine) -> Self {
        Self { governance }
    }

    /// Handle `POST /api/v1/execute`.
    ///
    /// In a full integration this would:
    /// 1. Look up the agent by `agent_id` in the process table.
    /// 2. Feed the task through the 7-stage pipeline.
    /// 3. Return the pipeline result.
    ///
    /// This stub validates the request and returns a structured
    /// response so the adapter wire format is exercised end-to-end.
    pub fn handle_execute(&self, req: &ExecuteRequest) -> Result<ExecuteResponse, ErrorResponse> {
        if req.agent_id.is_empty() {
            return Err(ErrorResponse {
                code: "INVALID_AGENT_ID".into(),
                message: "agent_id must not be empty".into(),
            });
        }
        if req.task.is_empty() {
            return Err(ErrorResponse {
                code: "INVALID_TASK".into(),
                message: "task must not be empty".into(),
            });
        }

        // In production, this dispatches to the kernel's agent pipeline.
        // For now, return a well-typed stub so the adapter protocol is
        // fully exercisable.
        let execution_id = uuid::Uuid::new_v4().to_string();
        Ok(ExecuteResponse {
            result: format!("Task accepted for agent '{}'", req.agent_id),
            score: 0.0,
            tokens_used: 0,
            execution_id: Some(execution_id),
        })
    }

    /// Handle `POST /api/v1/govern`.
    ///
    /// Evaluates the proposed action through the WeftOS governance
    /// engine and returns the decision with optional ExoChain
    /// attestation metadata.
    pub fn handle_govern(&self, req: &GovernRequest) -> Result<GovernResponse, ErrorResponse> {
        if req.agent_id.is_empty() {
            return Err(ErrorResponse {
                code: "INVALID_AGENT_ID".into(),
                message: "agent_id must not be empty".into(),
            });
        }
        if req.action.is_empty() {
            return Err(ErrorResponse {
                code: "INVALID_ACTION".into(),
                message: "action must not be empty".into(),
            });
        }

        // Build governance request from the HTTP payload.
        let mut gov_req = GovernanceRequest::new(&req.agent_id, &req.action);

        // Forward string context entries to governance context.
        for (k, v) in &req.context {
            if let Some(s) = v.as_str() {
                gov_req = gov_req.with_context_entry(k, s);
            }
        }

        // Extract effect dimensions from context if provided.
        let effect = extract_effect_vector(&req.context);
        gov_req = gov_req.with_effect(effect);

        let result: GovernanceResult = self.governance.evaluate(&gov_req);

        let decision_str = match &result.decision {
            GovernanceDecision::Permit | GovernanceDecision::PermitWithWarning(_) => {
                "permit".to_string()
            }
            GovernanceDecision::EscalateToHuman(_) => "escalate".to_string(),
            GovernanceDecision::Deny(_) => "deny".to_string(),
        };

        Ok(GovernResponse {
            decision: decision_str,
            attestation: None, // Populated when ExoChain feature is active.
            chain_hash: None,
            evaluated_rules: result.evaluated_rules,
            effect_magnitude: Some(result.effect.magnitude()),
        })
    }

    /// Handle `GET /api/v1/health`.
    pub fn handle_health(&self) -> HealthResponse {
        HealthResponse {
            status: "ok".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            uptime_secs: None,
        }
    }
}

/// Extract an [`EffectVector`] from the context map.
///
/// Looks for keys `"risk"`, `"fairness"`, `"privacy"`, `"novelty"`,
/// `"security"` and parses their values as f64.
fn extract_effect_vector(context: &HashMap<String, serde_json::Value>) -> EffectVector {
    let f = |key: &str| -> f64 { context.get(key).and_then(|v| v.as_f64()).unwrap_or(0.0) };
    EffectVector {
        risk: f("risk"),
        fairness: f("fairness"),
        privacy: f("privacy"),
        novelty: f("novelty"),
        security: f("security"),
    }
}

// ── Route dispatch helper ─────────────────────────────────────

/// Route identifier for the HTTP API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// POST /api/v1/execute
    Execute,
    /// POST /api/v1/govern
    Govern,
    /// GET /api/v1/health
    Health,
    /// Unknown route.
    NotFound,
}

/// Parse a method + path pair into a [`Route`].
pub fn match_route(method: &str, path: &str) -> Route {
    match (method, path) {
        ("POST", "/api/v1/execute") => Route::Execute,
        ("POST", "/api/v1/govern") => Route::Govern,
        ("GET", "/api/v1/health") => Route::Health,
        _ => Route::NotFound,
    }
}

/// Dispatch a raw JSON body to the appropriate handler and return
/// the JSON response bytes.
///
/// This is a convenience function for embedding in a minimal HTTP
/// server without pulling in a framework dependency.
pub fn dispatch(handler: &HttpApiHandler, route: &Route, body: &[u8]) -> Result<Vec<u8>, Vec<u8>> {
    match route {
        Route::Execute => {
            let req: ExecuteRequest = serde_json::from_slice(body).map_err(|e| {
                serde_json::to_vec(&ErrorResponse {
                    code: "PARSE_ERROR".into(),
                    message: format!("Invalid JSON: {e}"),
                })
                .unwrap_or_default()
            })?;
            match handler.handle_execute(&req) {
                Ok(resp) => serde_json::to_vec(&resp).map_err(|e| {
                    serde_json::to_vec(&ErrorResponse {
                        code: "INTERNAL".into(),
                        message: e.to_string(),
                    })
                    .unwrap_or_default()
                }),
                Err(err) => Err(serde_json::to_vec(&err).unwrap_or_default()),
            }
        }
        Route::Govern => {
            let req: GovernRequest = serde_json::from_slice(body).map_err(|e| {
                serde_json::to_vec(&ErrorResponse {
                    code: "PARSE_ERROR".into(),
                    message: format!("Invalid JSON: {e}"),
                })
                .unwrap_or_default()
            })?;
            match handler.handle_govern(&req) {
                Ok(resp) => serde_json::to_vec(&resp).map_err(|e| {
                    serde_json::to_vec(&ErrorResponse {
                        code: "INTERNAL".into(),
                        message: e.to_string(),
                    })
                    .unwrap_or_default()
                }),
                Err(err) => Err(serde_json::to_vec(&err).unwrap_or_default()),
            }
        }
        Route::Health => {
            let resp = handler.handle_health();
            serde_json::to_vec(&resp).map_err(|e| {
                serde_json::to_vec(&ErrorResponse {
                    code: "INTERNAL".into(),
                    message: e.to_string(),
                })
                .unwrap_or_default()
            })
        }
        Route::NotFound => Err(serde_json::to_vec(&ErrorResponse {
            code: "NOT_FOUND".into(),
            message: "Unknown route".into(),
        })
        .unwrap_or_default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_handler() -> HttpApiHandler {
        HttpApiHandler::new(GovernanceEngine::open())
    }

    #[test]
    fn test_execute_success() {
        let handler = make_handler();
        let req = ExecuteRequest {
            agent_id: "agent-1".into(),
            task: "summarise document".into(),
            context: HashMap::new(),
        };
        let resp = handler.handle_execute(&req).unwrap();
        assert!(resp.result.contains("agent-1"));
        assert!(resp.execution_id.is_some());
    }

    #[test]
    fn test_execute_empty_agent_id() {
        let handler = make_handler();
        let req = ExecuteRequest {
            agent_id: String::new(),
            task: "do something".into(),
            context: HashMap::new(),
        };
        let err = handler.handle_execute(&req).unwrap_err();
        assert_eq!(err.code, "INVALID_AGENT_ID");
    }

    #[test]
    fn test_execute_empty_task() {
        let handler = make_handler();
        let req = ExecuteRequest {
            agent_id: "a".into(),
            task: String::new(),
            context: HashMap::new(),
        };
        let err = handler.handle_execute(&req).unwrap_err();
        assert_eq!(err.code, "INVALID_TASK");
    }

    #[test]
    fn test_govern_permit() {
        let handler = make_handler();
        let req = GovernRequest {
            action: "read_file".into(),
            agent_id: "agent-1".into(),
            context: HashMap::new(),
        };
        let resp = handler.handle_govern(&req).unwrap();
        assert_eq!(resp.decision, "permit");
    }

    #[test]
    fn test_govern_empty_action() {
        let handler = make_handler();
        let req = GovernRequest {
            action: String::new(),
            agent_id: "a".into(),
            context: HashMap::new(),
        };
        let err = handler.handle_govern(&req).unwrap_err();
        assert_eq!(err.code, "INVALID_ACTION");
    }

    #[test]
    fn test_health() {
        let handler = make_handler();
        let resp = handler.handle_health();
        assert_eq!(resp.status, "ok");
        assert!(!resp.version.is_empty());
    }

    #[test]
    fn test_route_matching() {
        assert_eq!(match_route("POST", "/api/v1/execute"), Route::Execute);
        assert_eq!(match_route("POST", "/api/v1/govern"), Route::Govern);
        assert_eq!(match_route("GET", "/api/v1/health"), Route::Health);
        assert_eq!(match_route("GET", "/api/v1/execute"), Route::NotFound);
        assert_eq!(match_route("DELETE", "/unknown"), Route::NotFound);
    }

    #[test]
    fn test_dispatch_execute() {
        let handler = make_handler();
        let body = serde_json::to_vec(&ExecuteRequest {
            agent_id: "a1".into(),
            task: "hello".into(),
            context: HashMap::new(),
        })
        .unwrap();
        let result = dispatch(&handler, &Route::Execute, &body);
        assert!(result.is_ok());
        let resp: ExecuteResponse = serde_json::from_slice(&result.unwrap()).unwrap();
        assert!(resp.result.contains("a1"));
    }

    #[test]
    fn test_dispatch_invalid_json() {
        let handler = make_handler();
        let result = dispatch(&handler, &Route::Execute, b"not json");
        assert!(result.is_err());
        let err: ErrorResponse = serde_json::from_slice(&result.unwrap_err()).unwrap();
        assert_eq!(err.code, "PARSE_ERROR");
    }

    #[test]
    fn test_dispatch_not_found() {
        let handler = make_handler();
        let result = dispatch(&handler, &Route::NotFound, b"{}");
        assert!(result.is_err());
    }

    #[test]
    fn test_effect_vector_extraction() {
        let mut ctx = HashMap::new();
        ctx.insert("risk".into(), serde_json::json!(0.7));
        ctx.insert("security".into(), serde_json::json!(0.3));
        let ev = extract_effect_vector(&ctx);
        assert!((ev.risk - 0.7).abs() < f64::EPSILON);
        assert!((ev.security - 0.3).abs() < f64::EPSILON);
        assert!((ev.fairness - 0.0).abs() < f64::EPSILON);
    }
}
