//! Axum binding for `clawft_kernel::http_facade` (WEFT-122).
//!
//! The kernel crate defines transport-agnostic types, route matching,
//! RPC param building, SSE frame formatting, and `poll_events()`.
//! This module owns the axum handlers, SSE streaming loop, and a
//! pluggable [`KernelFacadeBackend`] so the gateway can forward RPC
//! to the daemon (or an in-memory stub for tests).

use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, State},
    http::{Method, StatusCode, Uri},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use clawft_kernel::console::KernelEventLog;
use clawft_kernel::http_facade::{
    CONTENT_TYPE_JSON, CONTENT_TYPE_SSE, FacadeResponse, FacadeRoute, HttpMethod,
    SSE_HEARTBEAT_INTERVAL_SECS, SseMessage, WitnessRequest, WitnessResponse, build_rpc_params,
    match_facade_route, poll_events, sse_response_headers,
};
use futures_util::stream::unfold;

use super::ApiState;

/// Poll interval for the SSE delta loop (matches http-facade plan).
const SSE_POLL_INTERVAL: Duration = Duration::from_millis(500);

// ── Backend trait ───────────────────────────────────────────────

/// Pluggable backend for kernel HTTP facade RPC + event log access.
///
/// Production gateways implement this by forwarding to the daemon's
/// Unix-socket RPC dispatcher. Tests use [`InMemoryKernelFacade`].
#[async_trait]
pub trait KernelFacadeBackend: Send + Sync {
    /// Invoke a kernel/ecc/custody RPC method with JSON params.
    async fn call_rpc(&self, method: &str, params: serde_json::Value) -> FacadeResponse;

    /// Snapshot new SSE messages since `cursor` via `poll_events`.
    fn poll_events(&self, cursor: usize) -> (Vec<SseMessage>, usize);

    /// Accept or reject an external custody witness injection.
    fn inject_witness(&self, req: WitnessRequest) -> WitnessResponse;

    /// Push an info event into the backend event log (tests / stream seed).
    fn push_info(&self, source: &str, message: &str) {
        let _ = (source, message);
    }
}

/// In-memory facade backend for smoke tests and gateway stub mode.
///
/// RPC calls return a stub envelope echoing method + params. Witness
/// inject validates non-empty signature and appends a log event.
/// SSE is driven by a real [`KernelEventLog`] via `poll_events()`.
pub struct InMemoryKernelFacade {
    event_log: KernelEventLog,
}

impl InMemoryKernelFacade {
    /// Create a new empty in-memory facade backend.
    pub fn new() -> Self {
        Self {
            event_log: KernelEventLog::new(),
        }
    }

    /// Shared access to the underlying event log (for tests).
    pub fn event_log(&self) -> &KernelEventLog {
        &self.event_log
    }
}

impl Default for InMemoryKernelFacade {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl KernelFacadeBackend for InMemoryKernelFacade {
    async fn call_rpc(&self, method: &str, params: serde_json::Value) -> FacadeResponse {
        // Record a classifiable event so SSE smoke tests can observe RPC activity.
        self.event_log
            .info("http_facade", format!("rpc {method}"));
        FacadeResponse::ok(serde_json::json!({
            "ok": true,
            "method": method,
            "params": params,
            "stub": true,
        }))
    }

    fn poll_events(&self, cursor: usize) -> (Vec<SseMessage>, usize) {
        poll_events(&self.event_log, cursor)
    }

    fn inject_witness(&self, req: WitnessRequest) -> WitnessResponse {
        if req.event_type.is_empty() {
            return WitnessResponse::rejected("event_type required");
        }
        if req.signature.is_empty() {
            return WitnessResponse::rejected("signature required");
        }
        // Stub accepts any non-empty signature without Ed25519
        // verification (full verify lives behind kernel `exochain`).
        self.event_log.info(
            "witness",
            format!("external witness accepted type={}", req.event_type),
        );
        // Deterministic stub hash so tests can assert presence without
        // depending on chain state.
        let hash = format!(
            "stub-{:x}",
            req.event_type
                .bytes()
                .fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(u64::from(b)))
        );
        WitnessResponse::accepted(hash, 1)
    }

    fn push_info(&self, source: &str, message: &str) {
        self.event_log.info(source, message);
    }
}

// ── Route table ─────────────────────────────────────────────────

/// Nest-relative facade routes (mounted under `/api`).
///
/// Paths are relative to the `/api` nest; handlers rebuild the full
/// path before calling [`match_facade_route`].
pub fn kernel_facade_api_routes() -> Router<ApiState> {
    Router::new()
        .route("/processes", get(rpc_get))
        .route("/services", get(rpc_get))
        .route("/chain/status", get(rpc_get))
        .route("/chain/events", get(rpc_get))
        .route("/vectors/status", get(rpc_get))
        .route("/vectors/search", post(rpc_post))
        .route("/ecc/calibration", get(rpc_get))
        .route("/ecc/coherence", get(rpc_get))
        .route("/custody/attest", get(rpc_get))
        // `/fleet/snapshot` lives in `projects_api` (project filter).
        .route("/agents/spawn", post(rpc_post))
    // NOTE: DELETE /agents/{pid} is registered on the existing
    // `/agents/{name}` route in `handlers.rs` (axum forbids two
    // distinct path-param names on the same pattern). See
    // [`delete_agent_by_pid`].
}

/// Top-level facade routes (SSE + witness) — not under `/api` nest.
pub fn kernel_facade_top_routes() -> Router<ApiState> {
    Router::new()
        .route("/events", get(sse_events))
        .route("/custody/witness", post(custody_witness))
}

// ── Handlers ────────────────────────────────────────────────────

async fn rpc_get(State(state): State<ApiState>, method: Method, uri: Uri) -> Response {
    dispatch_rpc(&state, method, &uri, &Bytes::new()).await
}

async fn rpc_post(
    State(state): State<ApiState>,
    method: Method,
    uri: Uri,
    body: Bytes,
) -> Response {
    dispatch_rpc(&state, method, &uri, &body).await
}

/// `DELETE /api/agents/{pid}` — facade `agent.stop` via `match_facade_route`.
///
/// Mounted on the shared `/agents/{name}` path in `handlers.rs` so it
/// does not conflict with `GET /agents/{name}`.
pub async fn delete_agent_by_pid(
    State(state): State<ApiState>,
    Path(pid): Path<String>,
) -> Response {
    // Rebuild the full path so match_facade_route can extract the PID.
    let full_path = format!("/api/agents/{pid}");
    let route = match_facade_route(HttpMethod::Delete, &full_path);
    match route {
        FacadeRoute::Rpc {
            method: rpc_method, ..
        } => {
            let params = build_rpc_params(&route, &[], "");
            let resp = state.kernel_facade.call_rpc(rpc_method, params).await;
            facade_response(resp)
        }
        _ => facade_response(FacadeResponse::not_found()),
    }
}

async fn custody_witness(
    State(state): State<ApiState>,
    Json(req): Json<WitnessRequest>,
) -> Response {
    // Confirm the route table agrees this is the witness endpoint.
    let route = match_facade_route(HttpMethod::Post, "/custody/witness");
    if !matches!(route, FacadeRoute::CustodyWitness) {
        return facade_response(FacadeResponse::not_found());
    }
    let resp = state.kernel_facade.inject_witness(req);
    let status = if resp.accepted {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    (status, Json(resp)).into_response()
}

/// SSE loop: poll `poll_events()` every 500ms; heartbeat every 15s.
///
/// Uses facade frame formatting constants (`CONTENT_TYPE_SSE`,
/// `SSE_HEARTBEAT_INTERVAL_SECS`, `sse_response_headers`).
async fn sse_events(
    State(state): State<ApiState>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    // Touch constants so the binding stays coupled to facade types.
    let _ = (CONTENT_TYPE_SSE, CONTENT_TYPE_JSON, sse_response_headers());

    let backend = Arc::clone(&state.kernel_facade);
    // Seed an init-classifiable event so a fresh stream has content
    // for the first `poll_events` tick.
    backend.push_info("http_facade", "SSE stream ready");

    let stream = unfold(
        SseLoop {
            backend,
            cursor: 0,
            last_heartbeat: Instant::now(),
            pending: VecDeque::new(),
        },
        |mut loop_state| async move {
            // Drain any messages queued from a prior multi-message poll.
            if let Some(msg) = loop_state.pending.pop_front() {
                return Some((Ok(sse_event_from_message(msg)), loop_state));
            }

            tokio::time::sleep(SSE_POLL_INTERVAL).await;

            let (messages, new_cursor) = loop_state.backend.poll_events(loop_state.cursor);
            loop_state.cursor = new_cursor;

            let mut iter = messages.into_iter();
            if let Some(first) = iter.next() {
                loop_state.pending.extend(iter);
                return Some((Ok(sse_event_from_message(first)), loop_state));
            }

            // Heartbeat comment keep-alive (also mirrored via KeepAlive).
            if loop_state.last_heartbeat.elapsed()
                >= Duration::from_secs(SSE_HEARTBEAT_INTERVAL_SECS)
            {
                loop_state.last_heartbeat = Instant::now();
                return Some((Ok(Event::default().comment("heartbeat")), loop_state));
            }

            // Quiet tick — SSE comment clients ignore.
            Some((Ok(Event::default().comment("poll")), loop_state))
        },
    );

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(SSE_HEARTBEAT_INTERVAL_SECS))
            .text("heartbeat"),
    )
}

fn sse_event_from_message(msg: SseMessage) -> Event {
    Event::default().event(msg.event_type.as_str()).data(
        serde_json::to_string(&serde_json::json!({
            "data": msg.data,
            "timestamp": msg.timestamp,
        }))
        .unwrap_or_else(|_| "{}".into()),
    )
}

struct SseLoop {
    backend: Arc<dyn KernelFacadeBackend>,
    cursor: usize,
    last_heartbeat: Instant,
    pending: VecDeque<SseMessage>,
}

// ── Shared dispatch ─────────────────────────────────────────────

async fn dispatch_rpc(state: &ApiState, method: Method, uri: &Uri, body: &Bytes) -> Response {
    let Some(http_method) = HttpMethod::parse(method.as_str()) else {
        return facade_response(FacadeResponse::bad_request("unsupported HTTP method"));
    };

    // Nest strips `/api`; rebuild the full path for the route table.
    let path = uri.path();
    let full_path = if path.starts_with("/api/") || path == "/api" {
        path.to_string()
    } else {
        format!("/api{path}")
    };
    let query = uri.query().unwrap_or("");

    let route = match_facade_route(http_method, &full_path);
    match route {
        FacadeRoute::Rpc {
            method: rpc_method, ..
        } => {
            let params = build_rpc_params(&route, body, query);
            let resp = state.kernel_facade.call_rpc(rpc_method, params).await;
            facade_response(resp)
        }
        FacadeRoute::NotFound => facade_response(FacadeResponse::not_found()),
        FacadeRoute::Events | FacadeRoute::CustodyWitness => {
            facade_response(FacadeResponse::bad_request(
                "use dedicated handler for this route",
            ))
        }
    }
}

fn facade_response(resp: FacadeResponse) -> Response {
    let status = StatusCode::from_u16(resp.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(resp.body)).into_response()
}
