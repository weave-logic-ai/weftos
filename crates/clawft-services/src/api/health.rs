//! Tiered `GET /api/health` (ADR-102 D1).
//!
//! - No token, or an invalid one: liveness only. `{"status":"ok"}` with 200,
//!   or `{"status":"degraded"}` with 503 when the daemon cannot be reached.
//!   Nothing else is disclosed, so load balancers and uptime checks work
//!   without learning versions, component names or paths.
//! - Valid owner token: the full status document, assembled from an explicit
//!   allow-list of fields. Raw daemon replies are never passed through: they
//!   carry runtime roots, user ids and key ids that this view must not show.
//!   Providers appear by name with a configured flag, never keys or URLs.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse, response::Response};
use serde_json::{Value, json};

use super::ApiState;
use super::auth::TokenMeta;

/// How long a `chain.verify` result is reused. Verifying walks the whole
/// chain, so it is not recomputed per request.
const CHAIN_VERIFY_TTL: Duration = Duration::from_secs(60);

/// Process start, for gateway uptime. Set when the router is built.
static START_TIME: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// Record process start (idempotent). Called when the router is built.
pub fn mark_start() {
    START_TIME.get_or_init(Instant::now);
}

fn uptime_secs() -> u64 {
    START_TIME.get_or_init(Instant::now).elapsed().as_secs()
}

/// Cache for the expensive parts of the status document.
#[derive(Default)]
pub struct HealthCache {
    chain_verify: Mutex<Option<(Instant, Value)>>,
}

impl HealthCache {
    fn verify(&self) -> Option<Value> {
        let guard = self.chain_verify.lock().unwrap_or_else(|e| e.into_inner());
        match guard.as_ref() {
            Some((at, v)) if at.elapsed() < CHAIN_VERIFY_TTL => Some(v.clone()),
            _ => None,
        }
    }

    fn store_verify(&self, v: Value) {
        *self.chain_verify.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), v));
    }
}

/// `GET /api/health`.
pub async fn health_check(
    State(state): State<ApiState>,
    request: axum::extract::Request,
) -> Response {
    let caller = request.extensions().get::<TokenMeta>().cloned();

    let status = state
        .kernel_facade
        .call_rpc("kernel.status", json!({}))
        .await;
    let daemon_up = status.status == 200;

    let Some(meta) = caller else {
        return if daemon_up {
            Json(json!({ "status": "ok" })).into_response()
        } else {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "status": "degraded" })),
            )
                .into_response()
        };
    };

    let body = full_status(&state, &meta, daemon_up.then_some(status.body)).await;
    let code = if daemon_up {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (code, Json(body)).into_response()
}

async fn full_status(state: &ApiState, meta: &TokenMeta, daemon: Option<Value>) -> Value {
    let gateway_version = env!("CARGO_PKG_VERSION");

    let (daemon_section, kernel, chain) = match daemon {
        Some(status) => {
            let d = daemon_section(&status, gateway_version);
            let kernel = kernel_section(state).await;
            let chain = chain_section(state).await;
            (d, kernel, chain)
        }
        None => (
            json!({ "reachable": false }),
            Value::Null,
            Value::Null,
        ),
    };

    json!({
        "status": if daemon_section["reachable"] == true { "ok" } else { "degraded" },
        // Kept top-level for the dashboard's SystemHealth type.
        "version": gateway_version,
        "uptime_secs": uptime_secs(),
        "build": {
            "version": gateway_version,
            "binary": std::env::current_exe().ok().map(|p| p.display().to_string()),
        },
        "gateway": { "uptime_secs": uptime_secs() },
        "daemon": daemon_section,
        "kernel": kernel,
        "chain": chain,
        "mcp": mcp_section(state),
        "channels": channels_section(state),
        "providers": providers_section(state),
        "token": {
            "id": meta.id,
            "label": meta.label,
            "issued_at": meta.issued_at,
            "expires_at": meta.expires_at,
        },
    })
}

fn str_of(v: &Value, k: &str) -> Value {
    v.get(k).cloned().filter(Value::is_string).unwrap_or(Value::Null)
}

fn daemon_section(status: &Value, gateway_version: &str) -> Value {
    let build = status.get("build").cloned().unwrap_or(Value::Null);
    let version = str_of(&build, "version");
    let skew = version.as_str().map(|v| v != gateway_version);
    json!({
        "reachable": true,
        "state": str_of(status, "state"),
        "version": version,
        "git_sha": str_of(&build, "sha"),
        "built_at": str_of(&build, "timestamp"),
        "uptime_secs": status.get("uptime_secs").cloned().filter(Value::is_number),
        "version_skew": skew,
    })
}

async fn kernel_section(state: &ApiState) -> Value {
    let processes = state.kernel_facade.call_rpc("kernel.ps", json!({})).await;
    let services = state
        .kernel_facade
        .call_rpc("kernel.services", json!({}))
        .await;
    let pick = |body: &Value, keys: &[&str]| -> Value {
        let rows = body
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|row| {
                        let mut o = serde_json::Map::new();
                        for k in keys {
                            if let Some(v) = row.get(*k) {
                                o.insert((*k).to_owned(), v.clone());
                            }
                        }
                        Value::Object(o)
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Value::Array(rows)
    };
    json!({
        "processes": if processes.status == 200 {
            pick(&processes.body, &["pid", "agent_id", "state"])
        } else { Value::Null },
        "services": if services.status == 200 {
            pick(&services.body, &["name", "service_type", "state", "health"])
        } else { Value::Null },
    })
}

async fn chain_section(state: &ApiState) -> Value {
    let status = state
        .kernel_facade
        .call_rpc("chain.status", json!({}))
        .await;
    if status.status != 200 {
        return json!({ "available": false });
    }
    let verify = match state.health_cache.verify() {
        Some(v) => v,
        None => {
            let r = state
                .kernel_facade
                .call_rpc("chain.verify", json!({}))
                .await;
            let v = if r.status == 200 {
                json!({
                    "valid": r.body.get("valid").cloned().filter(Value::is_boolean),
                    "event_count": r.body.get("event_count").cloned().filter(Value::is_number),
                    "signature_verified": r.body.get("signature_verified").cloned(),
                    // Error text can name paths; only the count is shown.
                    "error_count": r.body.get("errors").and_then(Value::as_array).map(Vec::len),
                })
            } else {
                Value::Null
            };
            state.health_cache.store_verify(v.clone());
            v
        }
    };
    json!({
        "available": true,
        "sequence": status.body.get("sequence").cloned().filter(Value::is_number),
        "head": str_of(&status.body, "last_hash"),
        "checkpoint_count": status.body.get("checkpoint_count").cloned().filter(Value::is_number),
        "events_since_checkpoint": status.body.get("events_since_checkpoint").cloned().filter(Value::is_number),
        "verify": verify,
    })
}

fn channels_section(state: &ApiState) -> Value {
    Value::Array(
        state
            .channels
            .list_channels()
            .into_iter()
            .map(|c| json!({ "name": c.name, "type": c.channel_type, "status": c.status }))
            .collect(),
    )
}

/// Provider names and whether a key is configured. Never keys or base URLs.
fn providers_section(state: &ApiState) -> Value {
    let cfg = state.config.get_config();
    let Some(providers) = cfg.get("providers").and_then(Value::as_object) else {
        return Value::Array(vec![]);
    };
    Value::Array(
        providers
            .iter()
            .map(|(name, p)| {
                json!({
                    "name": name,
                    "configured": p.get("api_key_set").and_then(Value::as_bool).unwrap_or(false),
                })
            })
            .collect(),
    )
}

/// This gateway's `/mcp` surface. Upstream servers are not listed: their
/// definitions carry commands, URLs and environment.
fn mcp_section(state: &ApiState) -> Value {
    match &state.mcp {
        Some(m) => json!({
            "mounted": true,
            "path": "/mcp",
            "profile": m.profile,
            "tool_count": m.tool_count,
        }),
        None => json!({ "mounted": false }),
    }
}
