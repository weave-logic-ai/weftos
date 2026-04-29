# Phase S1: Foundation + Core Views

> **Status (2026-04-28): Shipped.** All 37 S1 items (S1.1 backend API +
> S1.2 frontend scaffold + S1.3 core views) shipped between
> 2026-02-23 and 2026-02-24, verified by step-7 phase gate (11/11
> PASS, `.planning/development_notes/step7-phase-gate.md`). The
> per-row `Status: TODO` markers below were never refreshed; see
> the audit doc
> (`.planning/reviews/0.7.0-release-gate/09-clawft-agent-dashboard.md`)
> for current ground truth, and Plane (label
> `ws09-clawft-dashboard`, items WEFT-292..321) for the deferred
> follow-up work. Source location moved from `ui/` to `clawft-ui/`
> in CHANGELOG 0.6.19 (2026-04-22); the toolchain rename completed
> in 0.7.0 wave M1-E (WEFT-292/293/294/295/296/297/318).

> **Element:** UI -- Web Dashboard + Live Canvas
> **Phase:** S1 (S1.1, S1.2, S1.3)
> **Timeline:** Weeks 1-3
> **Priority:** P0 (Critical Path)
> **Crates:** `crates/clawft-services/` (new `api/` module), `crates/clawft-cli/` (`weft ui` command), `clawft-ui/` (standalone frontend)
> **Dependencies IN:** B5 (shared tool registry builder), existing `GatewayConfig` (port 18790), `Session` type, `AgentDefinition`, `ToolRegistry`, `MessageBus`
> **Blocks:** S2.1 (Live Canvas), S2.2 (Skill Browser), S2.3 (Memory Explorer), S2.4 (Config Editor), S2.5 (Cron+Channels), S3.x (all Sprint 3 phases)
> **Status:** Shipped (see banner above)
> **Orchestrator Ref:** `ui/00-orchestrator.md` Sections 2 (Phases), 3 (Exit Criteria), 4 (Security)

---

## 1. Overview

Phase S1 establishes the foundation for the ClawFT web dashboard: a backend REST + WebSocket API layer in Axum, a standalone React/TypeScript frontend scaffolded with Vite + shadcn/ui, and the core views (Dashboard, Agent Management, WebChat, Session Explorer, Tool Registry). The UI is a fully standalone application with its own build pipeline, connecting to any clawft gateway instance via configurable API URL. It can be developed, tested, and deployed without the Rust backend running (mock API via MSW). Optionally embeddable into the `weft` binary for single-binary distribution.

The backend API extends the existing gateway with Axum routes for agent CRUD, sessions, tools, auth, and WebSocket real-time events. All new Rust code lives in `clawft-services/src/api/` under a new `api` feature flag. The API router is mounted as a nested Axum service on port 18789 (configurable via `GatewayConfig.api_port`), running alongside the existing channel gateway. Handler functions bridge to existing core types (`AgentRegistry`, `SessionManager`, `ToolRegistry`, `MessageBus`) through trait-object handles that decouple the API layer from concrete Platform generics.

The frontend scaffolding (S1.2) creates a standalone Vite + React + TypeScript project in `ui/` with Tailwind CSS v4, shadcn/ui components, TanStack Router for file-based type-safe routing, TanStack Query for server state, and Zustand for client state. MSW (Mock Service Worker) provides a complete mock API layer so frontend development proceeds independently of backend readiness. The core views (S1.3) build on this scaffolding to deliver Dashboard Home, Agent Management, WebChat with streaming, Session Explorer, Tool Registry browser, dark/light theme toggle, and Cmd+K command palette.

---

## 2. Current Code

### Existing Gateway Infrastructure

The gateway command lives in `crates/clawft-cli/src/commands/gateway.rs` and initializes an `AppContext` (bus, sessions, tools, pipeline), starts channels, and runs the agent loop. There is currently **no HTTP API** exposed -- only channel-based message ingestion.

```rust
// crates/clawft-cli/src/commands/gateway.rs (current)
pub struct GatewayArgs {
    #[arg(short, long)]
    pub config: Option<String>,
    #[arg(long)]
    pub intelligent_routing: bool,
}
```

### Existing Configuration

`GatewayConfig` in `crates/clawft-types/src/config/mod.rs` defines `host` (default `"0.0.0.0"`) and `port` (default `18790`). No CORS or UI-specific fields exist yet.

```rust
// crates/clawft-types/src/config/mod.rs (current)
pub struct GatewayConfig {
    pub host: String,
    pub port: u16,
    pub heartbeat_interval_minutes: u64,
    pub heartbeat_prompt: String,
}
```

### Existing Tool Registry

`crates/clawft-core/src/tools/registry.rs` defines the `Tool` trait, `ToolRegistry`, `ToolMetadata`, and `ToolError`. The registry holds `Arc<dyn Tool>` by name and provides `schemas()` for OpenAI function-calling format and `schemas_for_tools()` for filtered subsets. The API will expose this registry via REST endpoints.

### Existing Session Manager

`crates/clawft-core/src/session.rs` provides `SessionManager<P>` for conversation persistence with JSONL format. Sessions are identified by `"{channel}:{chat_id}"` keys. The API will wrap this for session listing and history retrieval.

### Existing Agent System

`crates/clawft-core/src/agent/agents.rs` defines `AgentDefinition` (name, description, model, system_prompt, skills, allowed_tools, max_turns, variables) and `AgentRegistry` with 3-level discovery (workspace > user > built-in).

### Existing Message Bus

`crates/clawft-core/src/bus.rs` provides `MessageBus` with bounded MPSC channels for `InboundMessage` and `OutboundMessage`. The WebSocket handler will bridge these to the frontend.

### Existing Event Types

`crates/clawft-types/src/event.rs` defines `InboundMessage` (channel, sender_id, chat_id, content, timestamp, media, metadata) and `OutboundMessage` (channel, chat_id, content, reply_to, media, metadata).

### Existing AppContext

`crates/clawft-core/src/bootstrap.rs` provides `AppContext<P>` which initializes bus, sessions, tools, pipeline, context, memory, and skills. The API layer will receive shared references from `AppContext` before it is consumed by `into_agent_loop()`.

### No Existing Frontend

There is no `ui/` directory today. The entire frontend is created from scratch.

---

## 3. Deliverables

### 3.1 Phase S1.1: Backend API Foundation (Week 1)

#### 3.1.1 API Dependencies -- `clawft-services/Cargo.toml`

Add `axum`, `axum-extra`, `tower-http`, and `tokio-tungstenite` under a new `api` feature flag.

```toml
# crates/clawft-services/Cargo.toml -- additions

[features]
default = []
delegate = ["regex"]
rvf = []
test-utils = []
clawhub = []
api = ["axum", "axum-extra", "tower-http", "tokio-tungstenite", "futures"]

[dependencies]
# ... existing deps ...
axum = { version = "0.8", features = ["ws", "json", "macros"], optional = true }
axum-extra = { version = "0.10", features = ["typed-header"], optional = true }
tower-http = { version = "0.6", features = ["cors", "fs", "trace"], optional = true }
tokio-tungstenite = { version = "0.24", optional = true }
futures = { version = "0.3", optional = true }
```

#### 3.1.2 GatewayConfig Extension -- `clawft-types/src/config/mod.rs`

Add UI-specific fields to `GatewayConfig`:

```rust
// crates/clawft-types/src/config/mod.rs -- additions to GatewayConfig

pub struct GatewayConfig {
    // ... existing fields ...

    /// API port (separate from channel gateway port). Default 18789.
    #[serde(default = "default_api_port", alias = "apiPort")]
    pub api_port: u16,

    /// Allowed CORS origins for cross-origin UI access.
    /// Default: ["http://localhost:5173"] (Vite dev server).
    #[serde(default = "default_cors_origins", alias = "corsOrigins")]
    pub cors_origins: Vec<String>,

    /// Enable the API server. Default: true.
    #[serde(default = "default_true", alias = "apiEnabled")]
    pub api_enabled: bool,
}

fn default_api_port() -> u16 {
    18789
}

fn default_cors_origins() -> Vec<String> {
    vec!["http://localhost:5173".to_string()]
}
```

#### 3.1.3 API Router Factory -- `clawft-services/src/api/mod.rs`

**New file:** `crates/clawft-services/src/api/mod.rs`

The API module provides a router factory that accepts shared application state and returns a ready-to-serve `axum::Router`.

```rust
// crates/clawft-services/src/api/mod.rs

pub mod auth;
pub mod agents;
pub mod sessions;
pub mod tools;
pub mod ws;

use std::sync::Arc;
use axum::Router;
use tower_http::cors::{CorsLayer, Any};
use tower_http::trace::TraceLayer;

/// Shared state accessible by all API handlers.
///
/// Wraps references to the core subsystems. Clone-cheap (all Arc).
#[derive(Clone)]
pub struct ApiState {
    /// Tool registry for listing and schema introspection.
    pub tools: Arc<dyn ToolRegistryAccess>,
    /// Session manager for history queries.
    pub sessions: Arc<dyn SessionAccess>,
    /// Agent manager for CRUD and lifecycle.
    pub agents: Arc<dyn AgentAccess>,
    /// Message bus for WebSocket event broadcasting.
    pub bus: Arc<dyn BusAccess>,
    /// Auth token store.
    pub auth: Arc<auth::TokenStore>,
}

/// Trait for tool registry access from API handlers.
pub trait ToolRegistryAccess: Send + Sync {
    /// List all registered tool names with descriptions.
    fn list_tools(&self) -> Vec<ToolInfo>;
    /// Get JSON Schema for a specific tool by name.
    fn tool_schema(&self, name: &str) -> Option<serde_json::Value>;
}

/// Summary info for a registered tool.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
}

/// Trait for session access from API handlers.
pub trait SessionAccess: Send + Sync {
    /// List all session keys with metadata.
    fn list_sessions(&self) -> Vec<SessionSummary>;
    /// Get full history for a session key.
    fn get_session(&self, key: &str) -> Option<SessionDetail>;
    /// Delete a session by key.
    fn delete_session(&self, key: &str) -> bool;
}

/// Session summary for list endpoint.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionSummary {
    pub key: String,
    pub message_count: usize,
    pub created_at: String,
    pub updated_at: String,
}

/// Full session detail with messages.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionDetail {
    pub key: String,
    pub messages: Vec<SessionMessage>,
    pub created_at: String,
    pub updated_at: String,
}

/// A single message in a session.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionMessage {
    pub role: String,
    pub content: String,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallRecord>>,
}

/// Record of a tool call within a message.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolCallRecord {
    pub tool_name: String,
    pub arguments: serde_json::Value,
    pub result: Option<String>,
}

/// Trait for agent lifecycle access from API handlers.
pub trait AgentAccess: Send + Sync {
    /// List all configured agents.
    fn list_agents(&self) -> Vec<AgentSummary>;
    /// Get agent detail by ID.
    fn get_agent(&self, id: &str) -> Option<AgentDetail>;
    /// Update agent configuration.
    fn update_agent(&self, id: &str, patch: AgentPatch) -> Result<AgentDetail, String>;
    /// Start an agent.
    fn start_agent(&self, id: &str) -> Result<(), String>;
    /// Stop an agent.
    fn stop_agent(&self, id: &str) -> Result<(), String>;
}

/// Agent summary for list endpoint.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentSummary {
    pub id: String,
    pub name: String,
    pub status: AgentStatus,
    pub model: String,
}

/// Agent status enum.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    Running,
    Stopped,
    Error,
}

/// Full agent detail.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentDetail {
    pub id: String,
    pub name: String,
    pub status: AgentStatus,
    pub model: String,
    pub workspace: String,
    pub max_tokens: i32,
    pub temperature: f64,
    pub max_tool_iterations: i32,
    pub memory_window: i32,
    pub allowed_tools: Vec<String>,
    pub denied_tools: Vec<String>,
}

/// Patch object for agent updates.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tool_iterations: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_window: Option<i32>,
}

/// Trait for message bus access from WS handlers.
pub trait BusAccess: Send + Sync {
    /// Subscribe to events. Returns a receiver for JSON-encoded WS events.
    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<String>;
}

/// Build the full API router with all routes and middleware.
pub fn build_router(state: ApiState, cors_origins: &[String]) -> Router {
    let cors = if cors_origins.is_empty() || cors_origins.contains(&"*".to_string()) {
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any)
    } else {
        let origins: Vec<_> = cors_origins
            .iter()
            .filter_map(|o| o.parse().ok())
            .collect();
        CorsLayer::new()
            .allow_origin(origins)
            .allow_methods(Any)
            .allow_headers(Any)
    };

    Router::new()
        .nest("/api/auth", auth::routes())
        .nest("/api/agents", agents::routes())
        .nest("/api/sessions", sessions::routes())
        .nest("/api/tools", tools::routes())
        .route("/ws", axum::routing::get(ws::ws_handler))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
```

#### 3.1.4 Auth Middleware -- `clawft-services/src/api/auth.rs`

**New file:** `crates/clawft-services/src/api/auth.rs`

Token-based local auth: `weft ui` generates a one-time token, opens browser with `?token=xxx`. Token stored with configurable TTL. Bearer middleware validates on all `/api/*` routes.

```rust
// crates/clawft-services/src/api/auth.rs

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::{
    Router,
    routing::{get, post},
    extract::{State, Json},
    http::{Request, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::ApiState;

/// In-memory token store with expiry.
pub struct TokenStore {
    tokens: Mutex<HashMap<String, TokenEntry>>,
    default_ttl: Duration,
}

struct TokenEntry {
    created_at: Instant,
    ttl: Duration,
}

impl TokenStore {
    pub fn new(default_ttl: Duration) -> Self {
        Self {
            tokens: Mutex::new(HashMap::new()),
            default_ttl,
        }
    }

    pub fn generate(&self) -> String {
        let token = Uuid::new_v4().to_string();
        let mut map = self.tokens.lock().unwrap();
        map.insert(token.clone(), TokenEntry {
            created_at: Instant::now(),
            ttl: self.default_ttl,
        });
        token
    }

    pub fn validate(&self, token: &str) -> bool {
        let map = self.tokens.lock().unwrap();
        match map.get(token) {
            Some(entry) => entry.created_at.elapsed() < entry.ttl,
            None => false,
        }
    }

    pub fn cleanup(&self) {
        let mut map = self.tokens.lock().unwrap();
        map.retain(|_, entry| entry.created_at.elapsed() < entry.ttl);
    }
}

#[derive(Deserialize)]
pub struct TokenRequest {
    #[serde(default)]
    pub ttl_seconds: Option<u64>,
}

#[derive(Serialize)]
pub struct TokenResponse {
    pub token: String,
    pub expires_in_seconds: u64,
}

#[derive(Serialize)]
pub struct VerifyResponse {
    pub valid: bool,
}

async fn generate_token(State(state): State<ApiState>) -> Json<TokenResponse> {
    let token = state.auth.generate();
    Json(TokenResponse {
        token,
        expires_in_seconds: 86400,
    })
}

async fn verify_token(
    State(state): State<ApiState>,
    req: Request<axum::body::Body>,
) -> Json<VerifyResponse> {
    let valid = extract_bearer(&req)
        .map(|t| state.auth.validate(&t))
        .unwrap_or(false);
    Json(VerifyResponse { valid })
}

fn extract_bearer<B>(req: &Request<B>) -> Option<String> {
    req.headers()
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(|s| s.to_string())
}

/// Auth middleware: rejects requests without a valid Bearer token.
pub async fn require_auth(
    State(state): State<ApiState>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    if req.uri().path() == "/api/auth/token" {
        return next.run(req).await;
    }

    match extract_bearer(&req) {
        Some(token) if state.auth.validate(&token) => next.run(req).await,
        _ => (StatusCode::UNAUTHORIZED, "invalid or missing token").into_response(),
    }
}

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/token", post(generate_token))
        .route("/verify", get(verify_token))
}
```

#### 3.1.5 Agent CRUD Endpoints -- `clawft-services/src/api/agents.rs`

**New file:** `crates/clawft-services/src/api/agents.rs`

```rust
// crates/clawft-services/src/api/agents.rs

use axum::{
    Router,
    routing::{get, post, patch},
    extract::{State, Path, Json},
    http::StatusCode,
    response::IntoResponse,
};

use super::{ApiState, AgentSummary, AgentDetail, AgentPatch};

async fn list_agents(State(state): State<ApiState>) -> Json<Vec<AgentSummary>> {
    Json(state.agents.list_agents())
}

async fn get_agent(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.agents.get_agent(&id) {
        Some(agent) => (StatusCode::OK, Json(agent)).into_response(),
        None => (StatusCode::NOT_FOUND, "agent not found").into_response(),
    }
}

async fn update_agent(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(patch): Json<AgentPatch>,
) -> impl IntoResponse {
    match state.agents.update_agent(&id, patch) {
        Ok(agent) => (StatusCode::OK, Json(agent)).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

async fn start_agent(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.agents.start_agent(&id) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

async fn stop_agent(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.agents.stop_agent(&id) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/", get(list_agents))
        .route("/{id}", get(get_agent).patch(update_agent))
        .route("/{id}/start", post(start_agent))
        .route("/{id}/stop", post(stop_agent))
}
```

#### 3.1.6 Session Endpoints -- `clawft-services/src/api/sessions.rs`

**New file:** `crates/clawft-services/src/api/sessions.rs`

```rust
// crates/clawft-services/src/api/sessions.rs

use axum::{
    Router,
    routing::{get, delete},
    extract::{State, Path, Json},
    http::StatusCode,
    response::IntoResponse,
};

use super::{ApiState, SessionSummary, SessionDetail};

async fn list_sessions(State(state): State<ApiState>) -> Json<Vec<SessionSummary>> {
    Json(state.sessions.list_sessions())
}

async fn get_session(
    State(state): State<ApiState>,
    Path(key): Path<String>,
) -> impl IntoResponse {
    match state.sessions.get_session(&key) {
        Some(detail) => (StatusCode::OK, Json(detail)).into_response(),
        None => (StatusCode::NOT_FOUND, "session not found").into_response(),
    }
}

async fn delete_session(
    State(state): State<ApiState>,
    Path(key): Path<String>,
) -> impl IntoResponse {
    if state.sessions.delete_session(&key) {
        StatusCode::NO_CONTENT.into_response()
    } else {
        (StatusCode::NOT_FOUND, "session not found").into_response()
    }
}

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/", get(list_sessions))
        .route("/{key}", get(get_session).delete(delete_session))
}
```

#### 3.1.7 Tool Listing Endpoints -- `clawft-services/src/api/tools.rs`

**New file:** `crates/clawft-services/src/api/tools.rs`

```rust
// crates/clawft-services/src/api/tools.rs

use axum::{
    Router,
    routing::get,
    extract::{State, Path, Json},
    http::StatusCode,
    response::IntoResponse,
};

use super::{ApiState, ToolInfo};

async fn list_tools(State(state): State<ApiState>) -> Json<Vec<ToolInfo>> {
    Json(state.tools.list_tools())
}

async fn tool_schema(
    State(state): State<ApiState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    match state.tools.tool_schema(&name) {
        Some(schema) => (StatusCode::OK, Json(schema)).into_response(),
        None => (StatusCode::NOT_FOUND, "tool not found").into_response(),
    }
}

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/", get(list_tools))
        .route("/{name}/schema", get(tool_schema))
}
```

#### 3.1.8 WebSocket Upgrade Handler -- `clawft-services/src/api/ws.rs`

**New file:** `crates/clawft-services/src/api/ws.rs`

WebSocket upgrade with topic-based subscription pub/sub. Clients send `subscribe`/`unsubscribe` commands; server fans out bus events matching subscribed topics.

```rust
// crates/clawft-services/src/api/ws.rs

use std::collections::HashSet;

use axum::{
    extract::{State, WebSocketUpgrade, ws::{Message, WebSocket}},
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use super::ApiState;

/// Server -> client event types.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsEvent {
    AgentStatus { agent_id: String, status: String },
    MessageInbound { session_key: String, role: String, content: String, timestamp: String },
    MessageOutbound { session_key: String, role: String, content: String, timestamp: String },
    ToolCall { session_key: String, tool_name: String, args: serde_json::Value },
    ToolResult { session_key: String, tool_name: String, result: String },
    ChannelStatus { channel: String, status: String },
    MemoryUpdate { key: String, namespace: String },
}

/// Client -> server command types.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsCommand {
    ChatSend { session_key: String, content: String },
    Subscribe { topics: Vec<String> },
    Unsubscribe { topics: Vec<String> },
}

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<ApiState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: ApiState) {
    let (mut sender, mut receiver) = socket.split();
    let mut bus_rx = state.bus.subscribe();
    let subscribed_topics: std::sync::Arc<tokio::sync::Mutex<HashSet<String>>> =
        std::sync::Arc::new(tokio::sync::Mutex::new(HashSet::new()));

    let topics_for_broadcast = subscribed_topics.clone();

    let mut send_task = tokio::spawn(async move {
        while let Ok(event_json) = bus_rx.recv().await {
            let topics = topics_for_broadcast.lock().await;
            if !topics.is_empty() {
                if let Ok(event) = serde_json::from_str::<serde_json::Value>(&event_json) {
                    if let Some(event_type) = event.get("type").and_then(|t| t.as_str()) {
                        let topic = event_type.split(':').next().unwrap_or(event_type);
                        if !topics.contains(topic) && !topics.contains(event_type) {
                            continue;
                        }
                    }
                }
            }
            drop(topics);

            if sender.send(Message::Text(event_json.into())).await.is_err() {
                break;
            }
        }
    });

    let topics_for_reader = subscribed_topics.clone();
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            if let Message::Text(text) = msg {
                if let Ok(cmd) = serde_json::from_str::<WsCommand>(&text) {
                    match cmd {
                        WsCommand::Subscribe { topics } => {
                            let mut subs = topics_for_reader.lock().await;
                            for topic in topics { subs.insert(topic); }
                        }
                        WsCommand::Unsubscribe { topics } => {
                            let mut subs = topics_for_reader.lock().await;
                            for topic in &topics { subs.remove(topic); }
                        }
                        WsCommand::ChatSend { session_key, content } => {
                            tracing::debug!(session = %session_key, "ws chat: {}", content);
                        }
                    }
                }
            }
        }
    });

    tokio::select! {
        _ = &mut send_task => recv_task.abort(),
        _ = &mut recv_task => send_task.abort(),
    }
}
```

#### 3.1.9 Gateway Wiring

Wire API router into existing gateway startup with CORS middleware:

```rust
// crates/clawft-cli/src/commands/gateway.rs -- addition after agent loop spawn

#[cfg(feature = "api")]
if config.gateway.api_enabled {
    use clawft_services::api;

    let api_state = api::ApiState {
        tools: /* wrap ToolRegistry as Arc<dyn ToolRegistryAccess> */,
        sessions: /* wrap SessionManager as Arc<dyn SessionAccess> */,
        agents: /* wrap agent manager as Arc<dyn AgentAccess> */,
        bus: /* wrap bus broadcast as Arc<dyn BusAccess> */,
        auth: Arc::new(api::auth::TokenStore::new(Duration::from_secs(86400))),
    };

    let router = api::build_router(api_state, &config.gateway.cors_origins);
    let api_addr = format!("{}:{}", config.gateway.host, config.gateway.api_port);
    let listener = tokio::net::TcpListener::bind(&api_addr).await?;
    info!(addr = %api_addr, "API server listening");

    let api_cancel = cancel.clone();
    tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move { api_cancel.cancelled().await })
            .await
            .ok();
    });
}
```

#### 3.1.10 `weft ui` CLI Command -- `clawft-cli/src/commands/ui.rs`

**New file:** `crates/clawft-cli/src/commands/ui.rs`

```rust
// crates/clawft-cli/src/commands/ui.rs

use clap::Args;
use tracing::info;

#[derive(Args)]
pub struct UiArgs {
    #[arg(short, long)]
    pub config: Option<String>,
    #[arg(long, default_value = "18789")]
    pub port: u16,
    #[arg(long)]
    pub no_open: bool,
}

pub async fn run(args: UiArgs) -> anyhow::Result<()> {
    info!("starting weft ui");
    let platform = std::sync::Arc::new(clawft_platform::NativePlatform::new());
    let mut config = super::load_config(&*platform, args.config.as_deref()).await?;
    config.gateway.api_port = args.port;
    config.gateway.api_enabled = true;

    let token_store = std::sync::Arc::new(
        clawft_services::api::auth::TokenStore::new(std::time::Duration::from_secs(86400)),
    );
    let token = token_store.generate();
    let ui_url = format!("http://localhost:{}?token={}", args.port, token);

    if !args.no_open {
        info!(url = %ui_url, "opening browser");
        if let Err(e) = open::that(&ui_url) {
            eprintln!("failed to open browser: {e}");
            eprintln!("open manually: {ui_url}");
        }
    } else {
        eprintln!("UI available at: {ui_url}");
    }

    super::gateway::run(super::gateway::GatewayArgs {
        config: args.config,
        intelligent_routing: false,
    }).await
}
```

#### 3.1.11 Static File Serving

When `--features ui` is enabled, serve `ui/dist/` from disk or via `rust-embed`:

```rust
// crates/clawft-services/src/api/mod.rs -- addition to build_router

#[cfg(feature = "ui")]
fn static_file_routes() -> Router<ApiState> {
    use tower_http::services::ServeDir;
    let dist_path = std::path::Path::new("ui/dist");
    if dist_path.exists() {
        Router::new().fallback_service(ServeDir::new(dist_path))
    } else {
        Router::new()
    }
}
```

---

### 3.2 Phase S1.2: Frontend Scaffolding (Week 1, parallel)

#### 3.2.1 Vite + React + TypeScript Project -- `ui/package.json`

```json
{
  "name": "@clawft/ui",
  "private": true,
  "version": "0.1.0",
  "type": "module",
  "scripts": {
    "dev": "vite",
    "dev:mock": "VITE_MOCK_API=true vite",
    "build": "tsc -b && vite build",
    "preview": "vite preview",
    "lint": "eslint . --ext ts,tsx --report-unused-disable-directives --max-warnings 0",
    "type-check": "tsc --noEmit",
    "test": "vitest run",
    "test:watch": "vitest",
    "test:e2e": "playwright test"
  },
  "dependencies": {
    "@hookform/resolvers": "^3.9.0",
    "@radix-ui/react-dialog": "^1.1.0",
    "@radix-ui/react-dropdown-menu": "^2.1.0",
    "@radix-ui/react-scroll-area": "^1.2.0",
    "@radix-ui/react-select": "^2.1.0",
    "@radix-ui/react-separator": "^1.1.0",
    "@radix-ui/react-slot": "^1.1.0",
    "@radix-ui/react-switch": "^1.1.0",
    "@radix-ui/react-tabs": "^1.1.0",
    "@radix-ui/react-toast": "^1.2.0",
    "@radix-ui/react-tooltip": "^1.1.0",
    "@tanstack/react-query": "^5.60.0",
    "@tanstack/react-router": "^1.80.0",
    "class-variance-authority": "^0.7.0",
    "clsx": "^2.1.0",
    "cmdk": "^1.0.0",
    "lucide-react": "^0.460.0",
    "react": "^19.0.0",
    "react-dom": "^19.0.0",
    "react-hook-form": "^7.54.0",
    "sonner": "^1.7.0",
    "tailwind-merge": "^2.6.0",
    "zod": "^3.24.0",
    "zustand": "^5.0.0"
  },
  "devDependencies": {
    "@playwright/test": "^1.49.0",
    "@testing-library/jest-dom": "^6.6.0",
    "@testing-library/react": "^16.1.0",
    "@types/react": "^19.0.0",
    "@types/react-dom": "^19.0.0",
    "@vitejs/plugin-react": "^4.3.0",
    "autoprefixer": "^10.4.0",
    "eslint": "^9.15.0",
    "eslint-plugin-react-hooks": "^5.0.0",
    "eslint-plugin-react-refresh": "^0.4.0",
    "msw": "^2.7.0",
    "postcss": "^8.4.0",
    "tailwindcss": "^4.0.0",
    "typescript": "^5.7.0",
    "vite": "^6.0.0",
    "vitest": "^2.1.0"
  },
  "msw": {
    "workerDirectory": "public"
  }
}
```

#### 3.2.2 Tailwind CSS v4 + shadcn/ui -- `ui/src/styles/globals.css`

Full Tailwind v4 configuration with oklch color system, light/dark theme variables, and shadcn design tokens.

**Install shadcn core components (19 total):**

```bash
npx shadcn@latest init --style new-york --base-color zinc --css-variables
npx shadcn@latest add button card badge table tabs dialog sidebar toast \
  scroll-area select switch separator tooltip dropdown-menu command input \
  textarea popover skeleton
```

**`ui/src/styles/globals.css`:**

```css
/* ui/src/styles/globals.css */
@import "tailwindcss";

@theme inline {
  --color-background: oklch(1 0 0);
  --color-foreground: oklch(0.145 0 0);
  --color-card: oklch(1 0 0);
  --color-card-foreground: oklch(0.145 0 0);
  --color-popover: oklch(1 0 0);
  --color-popover-foreground: oklch(0.145 0 0);
  --color-primary: oklch(0.205 0 0);
  --color-primary-foreground: oklch(0.985 0 0);
  --color-secondary: oklch(0.97 0 0);
  --color-secondary-foreground: oklch(0.205 0 0);
  --color-muted: oklch(0.97 0 0);
  --color-muted-foreground: oklch(0.556 0 0);
  --color-accent: oklch(0.97 0 0);
  --color-accent-foreground: oklch(0.205 0 0);
  --color-destructive: oklch(0.577 0.245 27.325);
  --color-border: oklch(0.922 0 0);
  --color-input: oklch(0.922 0 0);
  --color-ring: oklch(0.708 0 0);

  --color-sidebar: oklch(0.985 0 0);
  --color-sidebar-foreground: oklch(0.145 0 0);
  --color-sidebar-primary: oklch(0.205 0 0);
  --color-sidebar-primary-foreground: oklch(0.985 0 0);
  --color-sidebar-accent: oklch(0.97 0 0);
  --color-sidebar-accent-foreground: oklch(0.205 0 0);
  --color-sidebar-border: oklch(0.922 0 0);
  --color-sidebar-ring: oklch(0.708 0 0);

  --color-agent-running: oklch(0.723 0.219 149.579);
  --color-agent-stopped: oklch(0.556 0 0);
  --color-agent-error: oklch(0.577 0.245 27.325);

  --radius-lg: 0.5rem;
  --radius-md: calc(var(--radius-lg) - 2px);
  --radius-sm: calc(var(--radius-lg) - 4px);

  --font-sans: "Inter", ui-sans-serif, system-ui, sans-serif;
  --font-mono: "JetBrains Mono", ui-monospace, monospace;

  --sidebar-width: 16rem;
  --sidebar-width-collapsed: 3.5rem;
}

.dark {
  --color-background: oklch(0.145 0 0);
  --color-foreground: oklch(0.985 0 0);
  --color-card: oklch(0.205 0 0);
  --color-card-foreground: oklch(0.985 0 0);
  --color-popover: oklch(0.205 0 0);
  --color-popover-foreground: oklch(0.985 0 0);
  --color-primary: oklch(0.985 0 0);
  --color-primary-foreground: oklch(0.205 0 0);
  --color-secondary: oklch(0.269 0 0);
  --color-secondary-foreground: oklch(0.985 0 0);
  --color-muted: oklch(0.269 0 0);
  --color-muted-foreground: oklch(0.708 0 0);
  --color-accent: oklch(0.269 0 0);
  --color-accent-foreground: oklch(0.985 0 0);
  --color-destructive: oklch(0.704 0.191 22.216);
  --color-border: oklch(0.269 0 0);
  --color-input: oklch(0.269 0 0);
  --color-ring: oklch(0.556 0 0);

  --color-sidebar: oklch(0.175 0 0);
  --color-sidebar-foreground: oklch(0.985 0 0);
  --color-sidebar-primary: oklch(0.985 0 0);
  --color-sidebar-primary-foreground: oklch(0.205 0 0);
  --color-sidebar-accent: oklch(0.269 0 0);
  --color-sidebar-accent-foreground: oklch(0.985 0 0);
  --color-sidebar-border: oklch(0.269 0 0);
  --color-sidebar-ring: oklch(0.556 0 0);
}

@layer base {
  * {
    @apply border-border;
  }
  body {
    @apply bg-background text-foreground font-sans antialiased;
  }
}

/* Scrollbar styling */
::-webkit-scrollbar { width: 6px; height: 6px; }
::-webkit-scrollbar-track { background: transparent; }
::-webkit-scrollbar-thumb { background: oklch(0.7 0 0); border-radius: 3px; }
.dark ::-webkit-scrollbar-thumb { background: oklch(0.4 0 0); }

/* Animations */
@keyframes fadeIn { from { opacity: 0; } to { opacity: 1; } }
@keyframes slideInRight { from { transform: translateX(100%); opacity: 0; } to { transform: translateX(0); opacity: 1; } }
.animate-fade-in { animation: fadeIn 0.2s ease-in; }
.animate-slide-in-right { animation: slideInRight 0.3s ease-out; }
```

**`ui/components.json`:**

```json
{
  "$schema": "https://ui.shadcn.com/schema.json",
  "style": "new-york",
  "rsc": false,
  "tsx": true,
  "tailwind": {
    "config": "",
    "css": "src/styles/globals.css",
    "baseColor": "zinc",
    "cssVariables": true,
    "prefix": ""
  },
  "aliases": {
    "components": "@/components",
    "utils": "@/lib/utils",
    "ui": "@/components/ui",
    "lib": "@/lib",
    "hooks": "@/hooks"
  },
  "iconLibrary": "lucide"
}
```

#### 3.2.3 TypeScript Types -- `ui/src/lib/types.ts`

Mirror Rust API types exactly:

```typescript
// ui/src/lib/types.ts

export interface TokenResponse { token: string; expires_in_seconds: number; }
export interface VerifyResponse { valid: boolean; }

export type AgentStatus = "running" | "stopped" | "error";
export interface AgentSummary { id: string; name: string; status: AgentStatus; model: string; }
export interface AgentDetail extends AgentSummary {
  workspace: string; max_tokens: number; temperature: number;
  max_tool_iterations: number; memory_window: number;
  allowed_tools: string[]; denied_tools: string[];
}
export interface AgentPatch {
  name?: string; model?: string; workspace?: string;
  max_tokens?: number; temperature?: number;
  max_tool_iterations?: number; memory_window?: number;
}

export interface SessionSummary { key: string; message_count: number; created_at: string; updated_at: string; }
export interface SessionDetail { key: string; messages: SessionMessage[]; created_at: string; updated_at: string; }
export interface SessionMessage {
  role: "user" | "assistant" | "system" | "tool";
  content: string; timestamp: string; tool_calls?: ToolCallRecord[];
}
export interface ToolCallRecord { tool_name: string; arguments: Record<string, unknown>; result?: string; }

export interface ToolInfo { name: string; description: string; }

export type WsEvent =
  | { type: "agent_status"; agentId: string; status: AgentStatus }
  | { type: "message_inbound"; sessionKey: string; role: string; content: string; timestamp: string }
  | { type: "message_outbound"; sessionKey: string; role: string; content: string; timestamp: string }
  | { type: "tool_call"; sessionKey: string; toolName: string; args: Record<string, unknown> }
  | { type: "tool_result"; sessionKey: string; toolName: string; result: string }
  | { type: "channel_status"; channel: string; status: string }
  | { type: "memory_update"; key: string; namespace: string };

export type WsCommand =
  | { type: "chat_send"; sessionKey: string; content: string }
  | { type: "subscribe"; topics: string[] }
  | { type: "unsubscribe"; topics: string[] };
```

#### 3.2.4 API Client -- `ui/src/lib/api-client.ts`

Fetch wrapper with Bearer auth, configurable API URL, typed API methods:

```typescript
// ui/src/lib/api-client.ts

import { config } from "./config";

function getToken(): string | null { return localStorage.getItem("clawft-token"); }
export function setToken(token: string): void { localStorage.setItem("clawft-token", token); }
export function clearToken(): void { localStorage.removeItem("clawft-token"); }

export class ApiError extends Error {
  constructor(public status: number, message: string) { super(message); this.name = "ApiError"; }
}

export async function apiFetch<T>(path: string, options: RequestInit = {}): Promise<T> {
  const token = getToken();
  const headers: Record<string, string> = {
    "Content-Type": "application/json",
    ...(options.headers as Record<string, string>),
  };
  if (token) { headers["Authorization"] = `Bearer ${token}`; }

  const url = `${config.apiUrl}${path}`;
  const res = await fetch(url, { ...options, headers });

  if (!res.ok) {
    const text = await res.text().catch(() => "unknown error");
    throw new ApiError(res.status, text);
  }
  if (res.status === 204) { return undefined as T; }
  return res.json();
}

import type { AgentSummary, AgentDetail, AgentPatch, SessionSummary, SessionDetail, ToolInfo, TokenResponse, VerifyResponse } from "./types";

export const api = {
  generateToken: () => apiFetch<TokenResponse>("/api/auth/token", { method: "POST" }),
  verifyToken: () => apiFetch<VerifyResponse>("/api/auth/verify"),
  listAgents: () => apiFetch<AgentSummary[]>("/api/agents"),
  getAgent: (id: string) => apiFetch<AgentDetail>(`/api/agents/${id}`),
  updateAgent: (id: string, patch: AgentPatch) => apiFetch<AgentDetail>(`/api/agents/${id}`, { method: "PATCH", body: JSON.stringify(patch) }),
  startAgent: (id: string) => apiFetch<void>(`/api/agents/${id}/start`, { method: "POST" }),
  stopAgent: (id: string) => apiFetch<void>(`/api/agents/${id}/stop`, { method: "POST" }),
  listSessions: () => apiFetch<SessionSummary[]>("/api/sessions"),
  getSession: (key: string) => apiFetch<SessionDetail>(`/api/sessions/${key}`),
  deleteSession: (key: string) => apiFetch<void>(`/api/sessions/${key}`, { method: "DELETE" }),
  listTools: () => apiFetch<ToolInfo[]>("/api/tools"),
  toolSchema: (name: string) => apiFetch<Record<string, unknown>>(`/api/tools/${name}/schema`),
};
```

#### 3.2.5 WebSocket Client -- `ui/src/lib/ws-client.ts`

Reconnecting WebSocket with exponential backoff (1s base, 30s max) and topic subscription:

```typescript
// ui/src/lib/ws-client.ts

import { config } from "./config";

type WsEventHandler = (event: WsEvent) => void;
export interface WsEvent { type: string; [key: string]: unknown; }

export class WsClient {
  private ws: WebSocket | null = null;
  private handlers = new Map<string, Set<WsEventHandler>>();
  private globalHandlers = new Set<WsEventHandler>();
  private reconnectAttempts = 0;
  private maxReconnectDelay = 30_000;
  private baseDelay = 1_000;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private subscribedTopics = new Set<string>();
  private token: string | null = null;
  private _connected = false;

  get connected(): boolean { return this._connected; }

  connect(token?: string): void { if (token) this.token = token; this.doConnect(); }

  disconnect(): void {
    if (this.reconnectTimer) { clearTimeout(this.reconnectTimer); this.reconnectTimer = null; }
    if (this.ws) { this.ws.close(); this.ws = null; }
    this._connected = false;
  }

  on(eventType: string, handler: WsEventHandler): () => void {
    if (!this.handlers.has(eventType)) { this.handlers.set(eventType, new Set()); }
    this.handlers.get(eventType)!.add(handler);
    return () => this.handlers.get(eventType)?.delete(handler);
  }

  onAny(handler: WsEventHandler): () => void {
    this.globalHandlers.add(handler);
    return () => this.globalHandlers.delete(handler);
  }

  subscribe(...topics: string[]): void {
    topics.forEach((t) => this.subscribedTopics.add(t));
    this.send({ type: "subscribe", topics });
  }

  unsubscribe(...topics: string[]): void {
    topics.forEach((t) => this.subscribedTopics.delete(t));
    this.send({ type: "unsubscribe", topics });
  }

  sendChat(sessionKey: string, content: string): void {
    this.send({ type: "chat_send", sessionKey, content });
  }

  send(command: Record<string, unknown>): void {
    if (this.ws?.readyState === WebSocket.OPEN) { this.ws.send(JSON.stringify(command)); }
  }

  private doConnect(): void {
    const wsUrl = this.token ? `${config.wsUrl}/ws?token=${this.token}` : `${config.wsUrl}/ws`;
    this.ws = new WebSocket(wsUrl);
    this.ws.onopen = () => {
      this._connected = true; this.reconnectAttempts = 0;
      if (this.subscribedTopics.size > 0) {
        this.send({ type: "subscribe", topics: Array.from(this.subscribedTopics) });
      }
    };
    this.ws.onmessage = (msg) => {
      try {
        const event: WsEvent = JSON.parse(msg.data);
        this.globalHandlers.forEach((h) => h(event));
        this.handlers.get(event.type)?.forEach((h) => h(event));
      } catch { /* ignore malformed */ }
    };
    this.ws.onclose = () => { this._connected = false; this.scheduleReconnect(); };
    this.ws.onerror = () => { this.ws?.close(); };
  }

  private scheduleReconnect(): void {
    const delay = Math.min(this.baseDelay * Math.pow(2, this.reconnectAttempts), this.maxReconnectDelay);
    this.reconnectAttempts++;
    this.reconnectTimer = setTimeout(() => this.doConnect(), delay);
  }
}

export const wsClient = new WsClient();
```

#### 3.2.6 Auth Hook -- `ui/src/hooks/use-auth.ts`

Token extraction from URL param, localStorage persistence, verification:

```typescript
// ui/src/hooks/use-auth.ts

import { useEffect, useState } from "react";
import { setToken, clearToken, api } from "@/lib/api-client";

export function useAuth() {
  const [authenticated, setAuthenticated] = useState(false);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    async function init() {
      const params = new URLSearchParams(window.location.search);
      const urlToken = params.get("token");
      if (urlToken) {
        setToken(urlToken);
        const url = new URL(window.location.href);
        url.searchParams.delete("token");
        window.history.replaceState({}, "", url.toString());
      }
      try {
        const { valid } = await api.verifyToken();
        setAuthenticated(valid);
      } catch { setAuthenticated(false); }
      finally { setLoading(false); }
    }
    init();
  }, []);

  const logout = () => { clearToken(); setAuthenticated(false); };
  return { authenticated, loading, logout };
}
```

#### 3.2.7 TanStack Router Setup -- `ui/src/main.tsx`

File-based type-safe routing with MSW conditional enablement:

```typescript
// ui/src/main.tsx

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { RouterProvider, createRouter } from "@tanstack/react-router";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { routeTree } from "./routeTree.gen";
import "./styles/globals.css";

async function enableMocking() {
  if (import.meta.env.VITE_MOCK_API !== "true") return;
  const { worker } = await import("./mocks/browser");
  return worker.start({ onUnhandledRequest: "bypass" });
}

const queryClient = new QueryClient({ defaultOptions: { queries: { staleTime: 5_000, retry: 1 } } });
const router = createRouter({ routeTree });

declare module "@tanstack/react-router" { interface Register { router: typeof router; } }

enableMocking().then(() => {
  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
      </QueryClientProvider>
    </StrictMode>,
  );
});
```

#### 3.2.8 MSW Mock Handlers -- `ui/src/mocks/`

Complete mock handlers for agents, sessions, and tools with realistic fixture data.

**`ui/src/mocks/browser.ts`:**

```typescript
// ui/src/mocks/browser.ts
import { setupWorker } from "msw/browser";
import { handlers } from "./handlers";

export const worker = setupWorker(...handlers);
```

**`ui/src/mocks/handlers/index.ts`:**

```typescript
// ui/src/mocks/handlers/index.ts
export { agentHandlers } from "./agents";
export { sessionHandlers } from "./sessions";
export { toolHandlers } from "./tools";
export { authHandlers } from "./auth";

import { agentHandlers } from "./agents";
import { sessionHandlers } from "./sessions";
import { toolHandlers } from "./tools";
import { authHandlers } from "./auth";

export const handlers = [
  ...authHandlers,
  ...agentHandlers,
  ...sessionHandlers,
  ...toolHandlers,
];
```

**`ui/src/mocks/handlers/agents.ts`:**

```typescript
// ui/src/mocks/handlers/agents.ts
import { http, HttpResponse, delay } from "msw";
import type { AgentSummary, AgentDetail, AgentPatch } from "@/lib/types";

const agents: AgentDetail[] = [
  {
    id: "default", name: "Default Agent", status: "running", model: "claude-sonnet-4-20250514",
    workspace: ".", max_tokens: 4096, temperature: 0.7, max_tool_iterations: 10,
    memory_window: 20, allowed_tools: ["*"], denied_tools: [],
  },
  {
    id: "researcher", name: "Research Agent", status: "stopped", model: "claude-sonnet-4-20250514",
    workspace: "./research", max_tokens: 8192, temperature: 0.3, max_tool_iterations: 5,
    memory_window: 50, allowed_tools: ["web_search", "memory_*"], denied_tools: [],
  },
  {
    id: "coder", name: "Coding Agent", status: "running", model: "claude-sonnet-4-20250514",
    workspace: "./src", max_tokens: 4096, temperature: 0.2, max_tool_iterations: 15,
    memory_window: 30, allowed_tools: ["file_*", "shell_*", "memory_*"], denied_tools: ["shell_rm_rf"],
  },
];

export const agentHandlers = [
  http.get("/api/agents", async () => {
    await delay(100);
    const summaries: AgentSummary[] = agents.map(({ id, name, status, model }) => ({ id, name, status, model }));
    return HttpResponse.json(summaries);
  }),

  http.get("/api/agents/:id", async ({ params }) => {
    await delay(50);
    const agent = agents.find((a) => a.id === params.id);
    if (!agent) return new HttpResponse("agent not found", { status: 404 });
    return HttpResponse.json(agent);
  }),

  http.patch("/api/agents/:id", async ({ params, request }) => {
    await delay(100);
    const idx = agents.findIndex((a) => a.id === params.id);
    if (idx === -1) return new HttpResponse("agent not found", { status: 404 });
    const patch = (await request.json()) as AgentPatch;
    Object.assign(agents[idx], patch);
    return HttpResponse.json(agents[idx]);
  }),

  http.post("/api/agents/:id/start", async ({ params }) => {
    await delay(200);
    const agent = agents.find((a) => a.id === params.id);
    if (!agent) return new HttpResponse("agent not found", { status: 404 });
    agent.status = "running";
    return new HttpResponse(null, { status: 204 });
  }),

  http.post("/api/agents/:id/stop", async ({ params }) => {
    await delay(200);
    const agent = agents.find((a) => a.id === params.id);
    if (!agent) return new HttpResponse("agent not found", { status: 404 });
    agent.status = "stopped";
    return new HttpResponse(null, { status: 204 });
  }),
];
```

**`ui/src/mocks/handlers/sessions.ts`:**

```typescript
// ui/src/mocks/handlers/sessions.ts
import { http, HttpResponse, delay } from "msw";
import type { SessionSummary, SessionDetail, SessionMessage } from "@/lib/types";

const mockMessages: SessionMessage[] = [
  { role: "user", content: "Hello, can you help me?", timestamp: "2026-02-23T10:00:00Z" },
  { role: "assistant", content: "Of course! What do you need help with?", timestamp: "2026-02-23T10:00:01Z" },
  {
    role: "assistant", content: "Let me search for that.",
    timestamp: "2026-02-23T10:00:05Z",
    tool_calls: [{ tool_name: "web_search", arguments: { query: "clawft documentation" }, result: "Found 3 results..." }],
  },
  { role: "user", content: "Great, thanks!", timestamp: "2026-02-23T10:00:10Z" },
];

const sessions: { key: string; messages: SessionMessage[]; created_at: string; updated_at: string }[] = [
  { key: "discord:user-123", messages: mockMessages, created_at: "2026-02-23T10:00:00Z", updated_at: "2026-02-23T10:00:10Z" },
  { key: "slack:channel-456", messages: mockMessages.slice(0, 2), created_at: "2026-02-22T15:30:00Z", updated_at: "2026-02-22T15:30:01Z" },
  { key: "web:session-789", messages: [], created_at: "2026-02-21T08:00:00Z", updated_at: "2026-02-21T08:00:00Z" },
];

export const sessionHandlers = [
  http.get("/api/sessions", async () => {
    await delay(100);
    const summaries: SessionSummary[] = sessions.map((s) => ({
      key: s.key, message_count: s.messages.length, created_at: s.created_at, updated_at: s.updated_at,
    }));
    return HttpResponse.json(summaries);
  }),

  http.get("/api/sessions/:key", async ({ params }) => {
    await delay(50);
    const session = sessions.find((s) => s.key === params.key);
    if (!session) return new HttpResponse("session not found", { status: 404 });
    const detail: SessionDetail = { key: session.key, messages: session.messages, created_at: session.created_at, updated_at: session.updated_at };
    return HttpResponse.json(detail);
  }),

  http.delete("/api/sessions/:key", async ({ params }) => {
    await delay(100);
    const idx = sessions.findIndex((s) => s.key === params.key);
    if (idx === -1) return new HttpResponse("session not found", { status: 404 });
    sessions.splice(idx, 1);
    return new HttpResponse(null, { status: 204 });
  }),
];
```

**`ui/src/mocks/handlers/tools.ts`:**

```typescript
// ui/src/mocks/handlers/tools.ts
import { http, HttpResponse, delay } from "msw";
import type { ToolInfo } from "@/lib/types";

const tools: (ToolInfo & { schema: Record<string, unknown> })[] = [
  {
    name: "web_search", description: "Search the web for information",
    schema: {
      type: "object",
      properties: { query: { type: "string", description: "Search query" }, max_results: { type: "integer", default: 5 } },
      required: ["query"],
    },
  },
  {
    name: "file_read", description: "Read contents of a file",
    schema: {
      type: "object",
      properties: { path: { type: "string", description: "File path to read" } },
      required: ["path"],
    },
  },
  {
    name: "memory_store", description: "Store a value in persistent memory",
    schema: {
      type: "object",
      properties: {
        key: { type: "string" }, value: { type: "string" },
        namespace: { type: "string", default: "default" }, ttl: { type: "integer" },
      },
      required: ["key", "value"],
    },
  },
  {
    name: "shell_exec", description: "Execute a shell command",
    schema: {
      type: "object",
      properties: { command: { type: "string" }, cwd: { type: "string" }, timeout_ms: { type: "integer", default: 30000 } },
      required: ["command"],
    },
  },
];

export const toolHandlers = [
  http.get("/api/tools", async () => {
    await delay(100);
    const infos: ToolInfo[] = tools.map(({ name, description }) => ({ name, description }));
    return HttpResponse.json(infos);
  }),

  http.get("/api/tools/:name/schema", async ({ params }) => {
    await delay(50);
    const tool = tools.find((t) => t.name === params.name);
    if (!tool) return new HttpResponse("tool not found", { status: 404 });
    return HttpResponse.json(tool.schema);
  }),
];
```

**`ui/src/mocks/handlers/auth.ts`:**

```typescript
// ui/src/mocks/handlers/auth.ts
import { http, HttpResponse, delay } from "msw";

export const authHandlers = [
  http.post("/api/auth/token", async () => {
    await delay(50);
    return HttpResponse.json({ token: "mock-token-" + Date.now(), expires_in_seconds: 86400 });
  }),

  http.get("/api/auth/verify", async ({ request }) => {
    await delay(50);
    const auth = request.headers.get("Authorization");
    const valid = auth?.startsWith("Bearer ") ?? false;
    return HttpResponse.json({ valid });
  }),
];
```

#### 3.2.9 Zustand Stores

**New file:** `ui/src/stores/agent-store.ts`

```typescript
// ui/src/stores/agent-store.ts

import { create } from "zustand";
import type { AgentStatus } from "@/lib/types";

interface AgentStatusMap { [agentId: string]: AgentStatus; }

interface AgentStore {
  statuses: AgentStatusMap;
  setStatus: (agentId: string, status: AgentStatus) => void;
  wsConnected: boolean;
  setWsConnected: (connected: boolean) => void;
}

export const useAgentStore = create<AgentStore>((set) => ({
  statuses: {},
  setStatus: (agentId, status) =>
    set((state) => ({ statuses: { ...state.statuses, [agentId]: status } })),
  wsConnected: false,
  setWsConnected: (connected) => set({ wsConnected: connected }),
}));
```

**New file:** `ui/src/stores/chat-store.ts`

```typescript
// ui/src/stores/chat-store.ts

import { create } from "zustand";
import type { SessionMessage } from "@/lib/types";

interface ChatStore {
  selectedSession: string | null;
  messages: SessionMessage[];
  selectSession: (key: string | null) => void;
  setMessages: (messages: SessionMessage[]) => void;
  appendMessage: (message: SessionMessage) => void;
  clearMessages: () => void;
}

export const useChatStore = create<ChatStore>((set) => ({
  selectedSession: null,
  messages: [],
  selectSession: (key) => set({ selectedSession: key, messages: [] }),
  setMessages: (messages) => set({ messages }),
  appendMessage: (message) => set((state) => ({ messages: [...state.messages, message] })),
  clearMessages: () => set({ messages: [] }),
}));
```

**New file:** `ui/src/stores/theme-store.ts`

```typescript
// ui/src/stores/theme-store.ts

import { create } from "zustand";
import { persist } from "zustand/middleware";

type Theme = "light" | "dark";

interface ThemeStore {
  theme: Theme;
  toggleTheme: () => void;
  setTheme: (theme: Theme) => void;
}

export const useThemeStore = create<ThemeStore>()(
  persist(
    (set) => ({
      theme: window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light",
      toggleTheme: () => set((state) => ({ theme: state.theme === "dark" ? "light" : "dark" })),
      setTheme: (theme) => set({ theme }),
    }),
    { name: "clawft-theme" },
  ),
);
```

#### 3.2.10 Environment Config -- `ui/src/lib/config.ts`

Centralized environment configuration with defaults:

```typescript
// ui/src/lib/config.ts

export const config = {
  apiUrl: import.meta.env.VITE_API_URL || "http://localhost:18789",
  wsUrl: (import.meta.env.VITE_WS_URL || "ws://localhost:18789"),
  mockApi: import.meta.env.VITE_MOCK_API === "true",
  appName: "ClawFT Dashboard",
  appVersion: import.meta.env.VITE_APP_VERSION || "0.1.0",
} as const;
```

**`ui/.env`:**

```env
VITE_API_URL=http://localhost:18789
VITE_WS_URL=ws://localhost:18789
VITE_MOCK_API=false
```

**`ui/.env.mock`:**

```env
VITE_API_URL=http://localhost:18789
VITE_WS_URL=ws://localhost:18789
VITE_MOCK_API=true
```

#### 3.2.11 Vite Config -- `ui/vite.config.ts`

Path aliases, dev server proxy, build optimization:

```typescript
// ui/vite.config.ts
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { resolve } from "path";
import { TanStackRouterVite } from "@tanstack/router-plugin/vite";

export default defineConfig({
  plugins: [TanStackRouterVite(), react()],
  resolve: {
    alias: { "@": resolve(__dirname, "./src") },
  },
  server: {
    port: 5173,
    proxy: {
      "/api": { target: "http://localhost:18789", changeOrigin: true },
      "/ws": { target: "ws://localhost:18789", ws: true },
    },
  },
  build: {
    outDir: "dist",
    sourcemap: false,
    rollupOptions: {
      output: {
        manualChunks: {
          vendor: ["react", "react-dom"],
          router: ["@tanstack/react-router"],
          query: ["@tanstack/react-query"],
          ui: ["cmdk", "sonner", "lucide-react"],
        },
      },
    },
  },
});
```

#### 3.2.12 TypeScript Config -- `ui/tsconfig.json`

```json
{
  "compilerOptions": {
    "target": "ES2020",
    "useDefineForClassFields": true,
    "lib": ["ES2020", "DOM", "DOM.Iterable"],
    "module": "ESNext",
    "skipLibCheck": true,
    "moduleResolution": "bundler",
    "allowImportingTsExtensions": true,
    "isolatedModules": true,
    "moduleDetection": "force",
    "noEmit": true,
    "jsx": "react-jsx",
    "strict": true,
    "noUnusedLocals": true,
    "noUnusedParameters": true,
    "noFallthroughCasesInSwitch": true,
    "noUncheckedIndexedAccess": true,
    "paths": { "@/*": ["./src/*"] },
    "baseUrl": "."
  },
  "include": ["src"]
}
```

#### 3.2.13 Dockerfile -- `ui/Dockerfile`

Multi-stage build: node:22-alpine for build, nginx:1.27-alpine for serving:

```dockerfile
# ui/Dockerfile
# Stage 1: Build
FROM node:22-alpine AS build
WORKDIR /app
RUN corepack enable
COPY package.json pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile
COPY . .
RUN pnpm build

# Stage 2: Serve
FROM nginx:1.27-alpine
COPY --from=build /app/dist /usr/share/nginx/html
COPY nginx.conf /etc/nginx/conf.d/default.conf
EXPOSE 80
CMD ["nginx", "-g", "daemon off;"]
```

**`ui/nginx.conf`:**

```nginx
server {
    listen 80;
    server_name _;
    root /usr/share/nginx/html;
    index index.html;

    # Gzip compression
    gzip on;
    gzip_types text/plain text/css application/json application/javascript text/xml;
    gzip_min_length 256;

    # Cache static assets
    location /assets/ {
        expires 1y;
        add_header Cache-Control "public, immutable";
    }

    # SPA fallback
    location / {
        try_files $uri $uri/ /index.html;
    }

    # API proxy (optional -- for co-located deployment)
    location /api/ {
        proxy_pass http://localhost:18789;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
    }

    location /ws {
        proxy_pass http://localhost:18789;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
    }
}
```

---

### 3.3 Phase S1.3: Core Views (Weeks 2-3)

#### 3.3.1 MainLayout -- `ui/src/components/layout/main-layout.tsx`

Collapsible sidebar with nav items (Dashboard, Agents, Chat, Sessions, Tools), top bar with Cmd+K search trigger, theme toggle, and command palette integration.

```tsx
// ui/src/components/layout/main-layout.tsx

import { useState, useEffect, useCallback } from "react";
import { Link, useLocation } from "@tanstack/react-router";
import {
  LayoutDashboard, Bot, MessageSquare, History, Wrench,
  ChevronsLeft, ChevronsRight, Search, Moon, Sun,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useThemeStore } from "@/stores/theme-store";
import { useAgentStore } from "@/stores/agent-store";
import { CommandPalette } from "./command-palette";
import { cn } from "@/lib/utils";

const navItems = [
  { path: "/", label: "Dashboard", icon: LayoutDashboard },
  { path: "/agents", label: "Agents", icon: Bot },
  { path: "/chat", label: "Chat", icon: MessageSquare },
  { path: "/sessions", label: "Sessions", icon: History },
  { path: "/tools", label: "Tools", icon: Wrench },
] as const;

interface MainLayoutProps { children: React.ReactNode; }

export function MainLayout({ children }: MainLayoutProps) {
  const [collapsed, setCollapsed] = useState(false);
  const [commandOpen, setCommandOpen] = useState(false);
  const location = useLocation();
  const { theme, toggleTheme } = useThemeStore();
  const wsConnected = useAgentStore((s) => s.wsConnected);

  // Apply theme class to document
  useEffect(() => {
    document.documentElement.classList.toggle("dark", theme === "dark");
  }, [theme]);

  // Cmd+K keyboard shortcut
  const handleKeyDown = useCallback((e: KeyboardEvent) => {
    if ((e.metaKey || e.ctrlKey) && e.key === "k") {
      e.preventDefault();
      setCommandOpen((prev) => !prev);
    }
  }, []);

  useEffect(() => {
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [handleKeyDown]);

  const isActive = (path: string) =>
    path === "/" ? location.pathname === "/" : location.pathname.startsWith(path);

  return (
    <div className="flex h-screen">
      {/* Sidebar */}
      <aside
        className={cn(
          "flex flex-col border-r bg-sidebar transition-all duration-200",
          collapsed ? "w-[var(--sidebar-width-collapsed)]" : "w-[var(--sidebar-width)]",
        )}
      >
        {/* Logo */}
        <div className="flex h-14 items-center gap-2 border-b px-3">
          <Bot className="h-6 w-6 text-sidebar-primary" />
          {!collapsed && <span className="font-semibold text-sidebar-foreground">ClawFT</span>}
        </div>

        {/* Navigation */}
        <nav className="flex-1 space-y-1 p-2">
          {navItems.map((item) => (
            <Tooltip key={item.path} delayDuration={collapsed ? 0 : 1000}>
              <TooltipTrigger asChild>
                <Link
                  to={item.path}
                  className={cn(
                    "flex items-center gap-3 rounded-md px-3 py-2 text-sm transition-colors",
                    isActive(item.path)
                      ? "bg-sidebar-accent text-sidebar-accent-foreground font-medium"
                      : "text-sidebar-foreground/70 hover:bg-sidebar-accent/50 hover:text-sidebar-foreground",
                  )}
                >
                  <item.icon className="h-4 w-4 shrink-0" />
                  {!collapsed && <span>{item.label}</span>}
                </Link>
              </TooltipTrigger>
              {collapsed && <TooltipContent side="right">{item.label}</TooltipContent>}
            </Tooltip>
          ))}
        </nav>

        <Separator />

        {/* Footer */}
        <div className="space-y-1 p-2">
          <Button variant="ghost" size="sm" onClick={toggleTheme} className="w-full justify-start gap-3 px-3">
            {theme === "dark" ? <Sun className="h-4 w-4" /> : <Moon className="h-4 w-4" />}
            {!collapsed && <span>{theme === "dark" ? "Light" : "Dark"} mode</span>}
          </Button>
          <Button variant="ghost" size="sm" onClick={() => setCollapsed(!collapsed)} className="w-full justify-start gap-3 px-3">
            {collapsed ? <ChevronsRight className="h-4 w-4" /> : <ChevronsLeft className="h-4 w-4" />}
            {!collapsed && <span>Collapse</span>}
          </Button>
        </div>
      </aside>

      {/* Main content */}
      <div className="flex flex-1 flex-col overflow-hidden">
        {/* Top bar */}
        <header className="flex h-14 items-center justify-between border-b px-4">
          <Button variant="outline" size="sm" onClick={() => setCommandOpen(true)} className="gap-2 text-muted-foreground">
            <Search className="h-4 w-4" />
            <span>Search...</span>
            <kbd className="ml-2 rounded border bg-muted px-1.5 py-0.5 text-xs">Cmd+K</kbd>
          </Button>
          <div className="flex items-center gap-2">
            <div className={cn("h-2 w-2 rounded-full", wsConnected ? "bg-agent-running" : "bg-agent-error")} />
            <span className="text-xs text-muted-foreground">{wsConnected ? "Connected" : "Disconnected"}</span>
          </div>
        </header>

        {/* Page content */}
        <main className="flex-1 overflow-auto p-6">{children}</main>
      </div>

      <CommandPalette open={commandOpen} onOpenChange={setCommandOpen} />
    </div>
  );
}
```

#### 3.3.2 Dashboard Home -- `ui/src/routes/index.tsx`

4 summary cards (Agents, Sessions, Tools, System Health) using TanStack Query. Recent sessions list. Skeleton loading states.

```tsx
// ui/src/routes/index.tsx

import { useQuery } from "@tanstack/react-query";
import { Bot, History, Wrench, Activity } from "lucide-react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Skeleton } from "@/components/ui/skeleton";
import { api } from "@/lib/api-client";
import { useAgentStore } from "@/stores/agent-store";
import type { AgentSummary, SessionSummary, ToolInfo } from "@/lib/types";

interface SummaryCardProps {
  title: string;
  value: string | number;
  subtitle?: string;
  icon: React.ElementType;
  loading?: boolean;
}

function SummaryCard({ title, value, subtitle, icon: Icon, loading }: SummaryCardProps) {
  return (
    <Card>
      <CardHeader className="flex flex-row items-center justify-between pb-2">
        <CardTitle className="text-sm font-medium text-muted-foreground">{title}</CardTitle>
        <Icon className="h-4 w-4 text-muted-foreground" />
      </CardHeader>
      <CardContent>
        {loading ? (
          <Skeleton className="h-8 w-16" />
        ) : (
          <>
            <div className="text-2xl font-bold">{value}</div>
            {subtitle && <p className="text-xs text-muted-foreground mt-1">{subtitle}</p>}
          </>
        )}
      </CardContent>
    </Card>
  );
}

export default function DashboardPage() {
  const wsConnected = useAgentStore((s) => s.wsConnected);

  const { data: agents, isLoading: agentsLoading } = useQuery<AgentSummary[]>({
    queryKey: ["agents"],
    queryFn: api.listAgents,
  });

  const { data: sessions, isLoading: sessionsLoading } = useQuery<SessionSummary[]>({
    queryKey: ["sessions"],
    queryFn: api.listSessions,
  });

  const { data: tools, isLoading: toolsLoading } = useQuery<ToolInfo[]>({
    queryKey: ["tools"],
    queryFn: api.listTools,
  });

  const runningCount = agents?.filter((a) => a.status === "running").length ?? 0;
  const totalAgents = agents?.length ?? 0;

  return (
    <div className="space-y-6">
      <h1 className="text-3xl font-bold">Dashboard</h1>

      {/* Summary cards */}
      <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
        <SummaryCard
          title="Agents" value={totalAgents} icon={Bot} loading={agentsLoading}
          subtitle={`${runningCount} running`}
        />
        <SummaryCard
          title="Sessions" value={sessions?.length ?? 0} icon={History} loading={sessionsLoading}
        />
        <SummaryCard
          title="Tools" value={tools?.length ?? 0} icon={Wrench} loading={toolsLoading}
        />
        <SummaryCard
          title="System Health" value={wsConnected ? "Healthy" : "Degraded"} icon={Activity}
          subtitle={wsConnected ? "WebSocket connected" : "WebSocket disconnected"}
        />
      </div>

      {/* Recent sessions */}
      <Card>
        <CardHeader>
          <CardTitle className="text-lg">Recent Sessions</CardTitle>
        </CardHeader>
        <CardContent>
          {sessionsLoading ? (
            <div className="space-y-2">
              {Array.from({ length: 3 }).map((_, i) => (
                <Skeleton key={i} className="h-12 w-full" />
              ))}
            </div>
          ) : sessions?.length === 0 ? (
            <p className="text-muted-foreground text-sm">No sessions yet.</p>
          ) : (
            <div className="space-y-2">
              {sessions?.slice(0, 5).map((s) => (
                <div key={s.key} className="flex items-center justify-between rounded-md border p-3">
                  <div>
                    <span className="font-mono text-sm">{s.key}</span>
                    <p className="text-xs text-muted-foreground">{s.message_count} messages</p>
                  </div>
                  <Badge variant="secondary">{new Date(s.updated_at).toLocaleDateString()}</Badge>
                </div>
              ))}
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
```

#### 3.3.3 Agent Management -- `ui/src/routes/agents.tsx`

Card grid with status badges (green=running, gray=stopped, red=error), start/stop mutation buttons, configure link.

```tsx
// ui/src/routes/agents.tsx

import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { Bot, Play, Square, Settings } from "lucide-react";
import { Card, CardContent, CardHeader, CardTitle, CardFooter } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { toast } from "sonner";
import { api } from "@/lib/api-client";
import type { AgentSummary, AgentStatus } from "@/lib/types";
import { cn } from "@/lib/utils";

const statusColor: Record<AgentStatus, string> = {
  running: "bg-agent-running text-white",
  stopped: "bg-agent-stopped text-white",
  error: "bg-agent-error text-white",
};

export default function AgentsPage() {
  const queryClient = useQueryClient();

  const { data: agents, isLoading } = useQuery<AgentSummary[]>({
    queryKey: ["agents"],
    queryFn: api.listAgents,
  });

  const startMutation = useMutation({
    mutationFn: (id: string) => api.startAgent(id),
    onSuccess: (_, id) => {
      queryClient.invalidateQueries({ queryKey: ["agents"] });
      toast.success(`Agent ${id} started`);
    },
    onError: (err, id) => toast.error(`Failed to start ${id}: ${err.message}`),
  });

  const stopMutation = useMutation({
    mutationFn: (id: string) => api.stopAgent(id),
    onSuccess: (_, id) => {
      queryClient.invalidateQueries({ queryKey: ["agents"] });
      toast.success(`Agent ${id} stopped`);
    },
    onError: (err, id) => toast.error(`Failed to stop ${id}: ${err.message}`),
  });

  if (isLoading) {
    return (
      <div className="space-y-6">
        <h1 className="text-3xl font-bold">Agents</h1>
        <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
          {Array.from({ length: 3 }).map((_, i) => (
            <Skeleton key={i} className="h-48 w-full" />
          ))}
        </div>
      </div>
    );
  }

  return (
    <div className="space-y-6">
      <h1 className="text-3xl font-bold">Agents</h1>
      <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
        {agents?.map((agent) => (
          <Card key={agent.id}>
            <CardHeader className="flex flex-row items-center justify-between">
              <div className="flex items-center gap-2">
                <Bot className="h-5 w-5" />
                <CardTitle className="text-base">{agent.name}</CardTitle>
              </div>
              <Badge className={cn("text-xs", statusColor[agent.status])}>{agent.status}</Badge>
            </CardHeader>
            <CardContent>
              <p className="text-sm text-muted-foreground">Model: {agent.model}</p>
              <p className="text-sm text-muted-foreground font-mono">ID: {agent.id}</p>
            </CardContent>
            <CardFooter className="flex gap-2">
              {agent.status === "running" ? (
                <Button
                  size="sm" variant="outline"
                  onClick={() => stopMutation.mutate(agent.id)}
                  disabled={stopMutation.isPending}
                >
                  <Square className="mr-1 h-3 w-3" /> Stop
                </Button>
              ) : (
                <Button
                  size="sm" variant="outline"
                  onClick={() => startMutation.mutate(agent.id)}
                  disabled={startMutation.isPending}
                >
                  <Play className="mr-1 h-3 w-3" /> Start
                </Button>
              )}
              <Button size="sm" variant="ghost" asChild>
                <Link to="/agents/$id" params={{ id: agent.id }}>
                  <Settings className="mr-1 h-3 w-3" /> Configure
                </Link>
              </Button>
            </CardFooter>
          </Card>
        ))}
      </div>
    </div>
  );
}
```

#### 3.3.4 Agent Detail -- `ui/src/routes/agents.$id.tsx`

Config form with `react-hook-form` + `zod` validation. Save via PATCH mutation.

```tsx
// ui/src/routes/agents.$id.tsx

import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { useParams, useNavigate } from "@tanstack/react-router";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import { toast } from "sonner";
import { api } from "@/lib/api-client";
import type { AgentDetail, AgentPatch } from "@/lib/types";

const agentSchema = z.object({
  name: z.string().min(1, "Name is required"),
  model: z.string().min(1, "Model is required"),
  workspace: z.string().min(1, "Workspace is required"),
  max_tokens: z.coerce.number().int().min(1).max(200000),
  temperature: z.coerce.number().min(0).max(2),
  max_tool_iterations: z.coerce.number().int().min(0).max(100),
  memory_window: z.coerce.number().int().min(0).max(1000),
});

type AgentFormValues = z.infer<typeof agentSchema>;

const MODELS = [
  "claude-sonnet-4-20250514",
  "claude-opus-4-20250514",
  "claude-haiku-3-20250514",
  "gpt-4o",
  "gpt-4o-mini",
];

export default function AgentDetailPage() {
  const { id } = useParams({ from: "/agents/$id" });
  const navigate = useNavigate();
  const queryClient = useQueryClient();

  const { data: agent, isLoading } = useQuery<AgentDetail>({
    queryKey: ["agents", id],
    queryFn: () => api.getAgent(id),
  });

  const form = useForm<AgentFormValues>({
    resolver: zodResolver(agentSchema),
    values: agent
      ? {
          name: agent.name, model: agent.model, workspace: agent.workspace,
          max_tokens: agent.max_tokens, temperature: agent.temperature,
          max_tool_iterations: agent.max_tool_iterations, memory_window: agent.memory_window,
        }
      : undefined,
  });

  const mutation = useMutation({
    mutationFn: (patch: AgentPatch) => api.updateAgent(id, patch),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["agents"] });
      toast.success("Agent updated");
      navigate({ to: "/agents" });
    },
    onError: (err) => toast.error(`Failed to update: ${err.message}`),
  });

  const onSubmit = (values: AgentFormValues) => mutation.mutate(values);

  if (isLoading) {
    return (
      <div className="max-w-2xl space-y-4">
        <Skeleton className="h-10 w-48" />
        <Skeleton className="h-96 w-full" />
      </div>
    );
  }

  return (
    <div className="max-w-2xl space-y-6">
      <h1 className="text-3xl font-bold">Agent: {agent?.name}</h1>
      <Card>
        <CardHeader><CardTitle>Configuration</CardTitle></CardHeader>
        <CardContent>
          <form onSubmit={form.handleSubmit(onSubmit)} className="space-y-4">
            <div className="space-y-2">
              <label className="text-sm font-medium">Name</label>
              <Input {...form.register("name")} />
              {form.formState.errors.name && (
                <p className="text-xs text-destructive">{form.formState.errors.name.message}</p>
              )}
            </div>
            <div className="space-y-2">
              <label className="text-sm font-medium">Model</label>
              <Select value={form.watch("model")} onValueChange={(v) => form.setValue("model", v)}>
                <SelectTrigger><SelectValue /></SelectTrigger>
                <SelectContent>
                  {MODELS.map((m) => <SelectItem key={m} value={m}>{m}</SelectItem>)}
                </SelectContent>
              </Select>
            </div>
            <div className="space-y-2">
              <label className="text-sm font-medium">Workspace</label>
              <Input {...form.register("workspace")} />
            </div>
            <div className="grid grid-cols-2 gap-4">
              <div className="space-y-2">
                <label className="text-sm font-medium">Max Tokens</label>
                <Input type="number" {...form.register("max_tokens")} />
              </div>
              <div className="space-y-2">
                <label className="text-sm font-medium">Temperature</label>
                <Input type="number" step="0.1" {...form.register("temperature")} />
              </div>
              <div className="space-y-2">
                <label className="text-sm font-medium">Max Tool Iterations</label>
                <Input type="number" {...form.register("max_tool_iterations")} />
              </div>
              <div className="space-y-2">
                <label className="text-sm font-medium">Memory Window</label>
                <Input type="number" {...form.register("memory_window")} />
              </div>
            </div>
            <div className="flex gap-2 pt-4">
              <Button type="submit" disabled={mutation.isPending}>
                {mutation.isPending ? "Saving..." : "Save Changes"}
              </Button>
              <Button type="button" variant="outline" onClick={() => navigate({ to: "/agents" })}>
                Cancel
              </Button>
            </div>
          </form>
        </CardContent>
      </Card>
    </div>
  );
}
```

#### 3.3.5 WebChat -- `ui/src/routes/chat.tsx`

Session list sidebar + message thread + input area with real-time WebSocket display and optimistic send.

```tsx
// ui/src/routes/chat.tsx

import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Send, Bot, User } from "lucide-react";
import { Card } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Badge } from "@/components/ui/badge";
import { api } from "@/lib/api-client";
import { wsClient } from "@/lib/ws-client";
import { useChatStore } from "@/stores/chat-store";
import { ToolCallCard } from "@/components/chat/tool-call-card";
import type { SessionSummary, SessionMessage } from "@/lib/types";
import { cn } from "@/lib/utils";

export default function ChatPage() {
  const [input, setInput] = useState("");
  const scrollRef = useRef<HTMLDivElement>(null);
  const { selectedSession, messages, selectSession, setMessages, appendMessage } = useChatStore();

  const { data: sessions } = useQuery<SessionSummary[]>({
    queryKey: ["sessions"],
    queryFn: api.listSessions,
  });

  // Load session messages when selected
  useEffect(() => {
    if (!selectedSession) return;
    api.getSession(selectedSession).then((detail) => {
      if (detail) setMessages(detail.messages);
    });
  }, [selectedSession, setMessages]);

  // Subscribe to WS events for real-time messages
  useEffect(() => {
    const unsubs = [
      wsClient.on("message_inbound", (event) => {
        if (event.sessionKey === selectedSession) {
          appendMessage({
            role: event.role as SessionMessage["role"],
            content: event.content as string,
            timestamp: event.timestamp as string,
          });
        }
      }),
      wsClient.on("message_outbound", (event) => {
        if (event.sessionKey === selectedSession) {
          appendMessage({
            role: event.role as SessionMessage["role"],
            content: event.content as string,
            timestamp: event.timestamp as string,
          });
        }
      }),
      wsClient.on("tool_call", (event) => {
        if (event.sessionKey === selectedSession) {
          appendMessage({
            role: "assistant",
            content: "",
            timestamp: new Date().toISOString(),
            tool_calls: [{
              tool_name: event.toolName as string,
              arguments: event.args as Record<string, unknown>,
            }],
          });
        }
      }),
    ];
    return () => unsubs.forEach((fn) => fn());
  }, [selectedSession, appendMessage]);

  // Auto-scroll to bottom on new messages
  useEffect(() => {
    scrollRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages]);

  const handleSend = () => {
    if (!input.trim() || !selectedSession) return;
    const content = input.trim();
    setInput("");

    // Optimistic append
    appendMessage({ role: "user", content, timestamp: new Date().toISOString() });
    wsClient.sendChat(selectedSession, content);
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); handleSend(); }
  };

  return (
    <div className="flex h-[calc(100vh-7rem)] gap-4">
      {/* Session sidebar */}
      <Card className="w-64 shrink-0 flex flex-col">
        <div className="border-b p-3 font-medium text-sm">Sessions</div>
        <ScrollArea className="flex-1">
          <div className="space-y-1 p-2">
            {sessions?.map((s) => (
              <button
                key={s.key}
                onClick={() => selectSession(s.key)}
                className={cn(
                  "w-full text-left rounded-md px-3 py-2 text-sm transition-colors",
                  selectedSession === s.key
                    ? "bg-accent text-accent-foreground"
                    : "hover:bg-accent/50",
                )}
              >
                <span className="font-mono text-xs block truncate">{s.key}</span>
                <span className="text-xs text-muted-foreground">{s.message_count} msgs</span>
              </button>
            ))}
          </div>
        </ScrollArea>
      </Card>

      {/* Chat area */}
      <div className="flex flex-1 flex-col">
        {!selectedSession ? (
          <div className="flex flex-1 items-center justify-center text-muted-foreground">
            Select a session to start chatting
          </div>
        ) : (
          <>
            {/* Messages */}
            <ScrollArea className="flex-1 p-4">
              <div className="space-y-4">
                {messages.map((msg, i) => (
                  <div key={i} className={cn("flex gap-3", msg.role === "user" ? "flex-row-reverse" : "flex-row")}>
                    <div className={cn(
                      "flex h-8 w-8 shrink-0 items-center justify-center rounded-full",
                      msg.role === "user" ? "bg-primary text-primary-foreground" : "bg-muted",
                    )}>
                      {msg.role === "user" ? <User className="h-4 w-4" /> : <Bot className="h-4 w-4" />}
                    </div>
                    <div className={cn(
                      "max-w-[70%] space-y-2 rounded-lg p-3",
                      msg.role === "user" ? "bg-primary text-primary-foreground" : "bg-muted",
                    )}>
                      {msg.content && <p className="text-sm whitespace-pre-wrap">{msg.content}</p>}
                      {msg.tool_calls?.map((tc, j) => (
                        <ToolCallCard key={j} toolCall={tc} />
                      ))}
                      <span className="block text-xs opacity-60">{new Date(msg.timestamp).toLocaleTimeString()}</span>
                    </div>
                  </div>
                ))}
                <div ref={scrollRef} />
              </div>
            </ScrollArea>

            {/* Input */}
            <div className="border-t p-4">
              <div className="flex gap-2">
                <Textarea
                  value={input} onChange={(e) => setInput(e.target.value)}
                  onKeyDown={handleKeyDown}
                  placeholder="Type a message... (Enter to send, Shift+Enter for newline)"
                  className="min-h-[44px] max-h-32 resize-none"
                  rows={1}
                />
                <Button onClick={handleSend} disabled={!input.trim()} size="icon" className="shrink-0">
                  <Send className="h-4 w-4" />
                </Button>
              </div>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
```

#### 3.3.6 Tool Call Cards -- `ui/src/components/chat/tool-call-card.tsx`

Expandable card showing tool name badge, arguments JSON, and result.

```tsx
// ui/src/components/chat/tool-call-card.tsx

import { useState } from "react";
import { ChevronDown, ChevronRight, Wrench } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import type { ToolCallRecord } from "@/lib/types";
import { cn } from "@/lib/utils";

interface ToolCallCardProps { toolCall: ToolCallRecord; }

export function ToolCallCard({ toolCall }: ToolCallCardProps) {
  const [expanded, setExpanded] = useState(false);

  return (
    <div
      className={cn(
        "rounded-md border bg-background text-foreground cursor-pointer transition-all",
        expanded ? "p-3" : "p-2",
      )}
      onClick={() => setExpanded(!expanded)}
    >
      <div className="flex items-center gap-2">
        {expanded ? <ChevronDown className="h-3 w-3" /> : <ChevronRight className="h-3 w-3" />}
        <Wrench className="h-3 w-3 text-muted-foreground" />
        <Badge variant="secondary" className="text-xs font-mono">{toolCall.tool_name}</Badge>
      </div>
      {expanded && (
        <div className="mt-2 space-y-2">
          <div>
            <span className="text-xs font-medium text-muted-foreground">Arguments:</span>
            <pre className="mt-1 rounded bg-muted p-2 text-xs font-mono overflow-x-auto">
              {JSON.stringify(toolCall.arguments, null, 2)}
            </pre>
          </div>
          {toolCall.result && (
            <div>
              <span className="text-xs font-medium text-muted-foreground">Result:</span>
              <pre className="mt-1 rounded bg-muted p-2 text-xs font-mono overflow-x-auto max-h-40 overflow-y-auto">
                {toolCall.result}
              </pre>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
```

#### 3.3.7 Session Explorer -- `ui/src/routes/sessions.tsx`

DataTable with columns: session key, message count, created, updated, actions. Detail dialog with full conversation and JSON export button.

```tsx
// ui/src/routes/sessions.tsx

import { useState } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { Download, Eye, Trash2 } from "lucide-react";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Skeleton } from "@/components/ui/skeleton";
import { toast } from "sonner";
import { api } from "@/lib/api-client";
import type { SessionSummary, SessionDetail } from "@/lib/types";

export default function SessionsPage() {
  const queryClient = useQueryClient();
  const [detailKey, setDetailKey] = useState<string | null>(null);

  const { data: sessions, isLoading } = useQuery<SessionSummary[]>({
    queryKey: ["sessions"],
    queryFn: api.listSessions,
  });

  const { data: detail } = useQuery<SessionDetail>({
    queryKey: ["sessions", detailKey],
    queryFn: () => api.getSession(detailKey!),
    enabled: !!detailKey,
  });

  const deleteMutation = useMutation({
    mutationFn: (key: string) => api.deleteSession(key),
    onSuccess: (_, key) => {
      queryClient.invalidateQueries({ queryKey: ["sessions"] });
      toast.success(`Session ${key} deleted`);
    },
    onError: (err) => toast.error(`Delete failed: ${err.message}`),
  });

  const handleExport = (session: SessionDetail) => {
    const blob = new Blob([JSON.stringify(session, null, 2)], { type: "application/json" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `session-${session.key.replace(/[^a-z0-9]/gi, "-")}.json`;
    a.click();
    URL.revokeObjectURL(url);
  };

  if (isLoading) {
    return (
      <div className="space-y-6">
        <h1 className="text-3xl font-bold">Sessions</h1>
        <div className="space-y-2">
          {Array.from({ length: 5 }).map((_, i) => <Skeleton key={i} className="h-12 w-full" />)}
        </div>
      </div>
    );
  }

  return (
    <div className="space-y-6">
      <h1 className="text-3xl font-bold">Sessions</h1>

      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Session Key</TableHead>
            <TableHead>Messages</TableHead>
            <TableHead>Created</TableHead>
            <TableHead>Updated</TableHead>
            <TableHead className="w-32">Actions</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {sessions?.map((s) => (
            <TableRow key={s.key}>
              <TableCell className="font-mono text-sm">{s.key}</TableCell>
              <TableCell><Badge variant="secondary">{s.message_count}</Badge></TableCell>
              <TableCell className="text-sm">{new Date(s.created_at).toLocaleString()}</TableCell>
              <TableCell className="text-sm">{new Date(s.updated_at).toLocaleString()}</TableCell>
              <TableCell>
                <div className="flex gap-1">
                  <Button size="icon" variant="ghost" onClick={() => setDetailKey(s.key)}>
                    <Eye className="h-4 w-4" />
                  </Button>
                  <Button
                    size="icon" variant="ghost"
                    onClick={() => deleteMutation.mutate(s.key)}
                    disabled={deleteMutation.isPending}
                  >
                    <Trash2 className="h-4 w-4 text-destructive" />
                  </Button>
                </div>
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>

      {/* Detail dialog */}
      <Dialog open={!!detailKey} onOpenChange={(open) => !open && setDetailKey(null)}>
        <DialogContent className="max-w-2xl max-h-[80vh]">
          <DialogHeader>
            <DialogTitle className="font-mono text-sm">{detailKey}</DialogTitle>
          </DialogHeader>
          <ScrollArea className="max-h-[60vh]">
            <div className="space-y-3 p-1">
              {detail?.messages.map((msg, i) => (
                <div key={i} className="rounded-md border p-3">
                  <div className="flex items-center gap-2 mb-1">
                    <Badge variant={msg.role === "user" ? "default" : "secondary"}>{msg.role}</Badge>
                    <span className="text-xs text-muted-foreground">{new Date(msg.timestamp).toLocaleString()}</span>
                  </div>
                  <p className="text-sm whitespace-pre-wrap">{msg.content}</p>
                </div>
              ))}
            </div>
          </ScrollArea>
          {detail && (
            <Button variant="outline" onClick={() => handleExport(detail)} className="gap-2">
              <Download className="h-4 w-4" /> Export JSON
            </Button>
          )}
        </DialogContent>
      </Dialog>
    </div>
  );
}
```

#### 3.3.8 Tool Registry -- `ui/src/routes/tools.tsx`

Table listing all tools with name badges and descriptions. Schema viewer dialog.

```tsx
// ui/src/routes/tools.tsx

import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Eye } from "lucide-react";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Skeleton } from "@/components/ui/skeleton";
import { api } from "@/lib/api-client";
import type { ToolInfo } from "@/lib/types";

export default function ToolsPage() {
  const [selectedTool, setSelectedTool] = useState<string | null>(null);

  const { data: tools, isLoading } = useQuery<ToolInfo[]>({
    queryKey: ["tools"],
    queryFn: api.listTools,
  });

  const { data: schema } = useQuery<Record<string, unknown>>({
    queryKey: ["tools", selectedTool, "schema"],
    queryFn: () => api.toolSchema(selectedTool!),
    enabled: !!selectedTool,
  });

  if (isLoading) {
    return (
      <div className="space-y-6">
        <h1 className="text-3xl font-bold">Tool Registry</h1>
        <div className="space-y-2">
          {Array.from({ length: 4 }).map((_, i) => <Skeleton key={i} className="h-12 w-full" />)}
        </div>
      </div>
    );
  }

  return (
    <div className="space-y-6">
      <h1 className="text-3xl font-bold">Tool Registry</h1>

      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Name</TableHead>
            <TableHead>Description</TableHead>
            <TableHead className="w-24">Schema</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {tools?.map((tool) => (
            <TableRow key={tool.name}>
              <TableCell>
                <Badge variant="outline" className="font-mono">{tool.name}</Badge>
              </TableCell>
              <TableCell className="text-sm">{tool.description}</TableCell>
              <TableCell>
                <Button size="icon" variant="ghost" onClick={() => setSelectedTool(tool.name)}>
                  <Eye className="h-4 w-4" />
                </Button>
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>

      {/* Schema viewer dialog */}
      <Dialog open={!!selectedTool} onOpenChange={(open) => !open && setSelectedTool(null)}>
        <DialogContent className="max-w-lg max-h-[70vh]">
          <DialogHeader>
            <DialogTitle className="flex items-center gap-2">
              <Badge variant="outline" className="font-mono">{selectedTool}</Badge>
              <span>Schema</span>
            </DialogTitle>
          </DialogHeader>
          <ScrollArea className="max-h-[50vh]">
            <pre className="rounded bg-muted p-4 text-sm font-mono overflow-x-auto">
              {schema ? JSON.stringify(schema, null, 2) : "Loading..."}
            </pre>
          </ScrollArea>
        </DialogContent>
      </Dialog>
    </div>
  );
}
```

#### 3.3.9 Theme Toggle Hook -- `ui/src/hooks/use-theme.ts`

Dark/light mode effect hook that syncs `useThemeStore` state to `document.documentElement`:

```typescript
// ui/src/hooks/use-theme.ts

import { useEffect } from "react";
import { useThemeStore } from "@/stores/theme-store";

/**
 * Hook that syncs theme state to the DOM.
 * Call once in the root layout; toggleTheme() is available from useThemeStore directly.
 */
export function useThemeSync() {
  const theme = useThemeStore((s) => s.theme);

  useEffect(() => {
    const root = document.documentElement;
    if (theme === "dark") {
      root.classList.add("dark");
    } else {
      root.classList.remove("dark");
    }
  }, [theme]);

  return theme;
}
```

#### 3.3.10 Command Palette -- `ui/src/components/layout/command-palette.tsx`

`CommandDialog` from cmdk with page navigation across all route pages. Triggered by Cmd+K.

```tsx
// ui/src/components/layout/command-palette.tsx

import { useNavigate } from "@tanstack/react-router";
import { LayoutDashboard, Bot, MessageSquare, History, Wrench, Moon, Sun } from "lucide-react";
import {
  CommandDialog, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList,
} from "@/components/ui/command";
import { useThemeStore } from "@/stores/theme-store";

interface CommandPaletteProps { open: boolean; onOpenChange: (open: boolean) => void; }

const pages = [
  { path: "/", label: "Dashboard", icon: LayoutDashboard, keywords: ["home", "overview"] },
  { path: "/agents", label: "Agents", icon: Bot, keywords: ["agent", "manage", "start", "stop"] },
  { path: "/chat", label: "Chat", icon: MessageSquare, keywords: ["chat", "message", "webchat"] },
  { path: "/sessions", label: "Sessions", icon: History, keywords: ["session", "history", "conversation"] },
  { path: "/tools", label: "Tools", icon: Wrench, keywords: ["tool", "registry", "schema"] },
];

export function CommandPalette({ open, onOpenChange }: CommandPaletteProps) {
  const navigate = useNavigate();
  const { theme, toggleTheme } = useThemeStore();

  const handleSelect = (path: string) => {
    navigate({ to: path });
    onOpenChange(false);
  };

  return (
    <CommandDialog open={open} onOpenChange={onOpenChange}>
      <CommandInput placeholder="Type a command or search..." />
      <CommandList>
        <CommandEmpty>No results found.</CommandEmpty>
        <CommandGroup heading="Pages">
          {pages.map((page) => (
            <CommandItem key={page.path} onSelect={() => handleSelect(page.path)} keywords={page.keywords}>
              <page.icon className="mr-2 h-4 w-4" />
              <span>{page.label}</span>
            </CommandItem>
          ))}
        </CommandGroup>
        <CommandGroup heading="Actions">
          <CommandItem onSelect={() => { toggleTheme(); onOpenChange(false); }}>
            {theme === "dark" ? <Sun className="mr-2 h-4 w-4" /> : <Moon className="mr-2 h-4 w-4" />}
            <span>Toggle {theme === "dark" ? "Light" : "Dark"} Mode</span>
          </CommandItem>
        </CommandGroup>
      </CommandList>
    </CommandDialog>
  );
}
```

#### 3.3.11 Toast Notifications -- `ui/src/hooks/use-ws-notifications.ts`

Global WS event listener that shows toast notifications for agent status changes and channel events.

```typescript
// ui/src/hooks/use-ws-notifications.ts

import { useEffect } from "react";
import { toast } from "sonner";
import { wsClient } from "@/lib/ws-client";
import { useAgentStore } from "@/stores/agent-store";

/**
 * Hook that subscribes to WS events and shows toast notifications.
 * Should be mounted once in the root layout.
 */
export function useWsNotifications() {
  const setStatus = useAgentStore((s) => s.setStatus);
  const setWsConnected = useAgentStore((s) => s.setWsConnected);

  useEffect(() => {
    // Track connection status
    const checkConnection = setInterval(() => {
      setWsConnected(wsClient.connected);
    }, 1000);

    const unsubs = [
      wsClient.on("agent_status", (event) => {
        const agentId = event.agentId as string;
        const status = event.status as string;
        setStatus(agentId, status as "running" | "stopped" | "error");

        if (status === "error") {
          toast.error(`Agent ${agentId} encountered an error`);
        } else if (status === "running") {
          toast.success(`Agent ${agentId} is now running`);
        } else if (status === "stopped") {
          toast.info(`Agent ${agentId} stopped`);
        }
      }),

      wsClient.on("channel_status", (event) => {
        const channel = event.channel as string;
        const status = event.status as string;

        if (status === "disconnected") {
          toast.warning(`Channel ${channel} disconnected`);
        } else if (status === "connected") {
          toast.success(`Channel ${channel} connected`);
        }
      }),
    ];

    return () => {
      clearInterval(checkConnection);
      unsubs.forEach((fn) => fn());
    };
  }, [setStatus, setWsConnected]);
}
```

#### 3.3.12 Utility Functions -- `ui/src/lib/utils.ts`

shadcn utility for class merging:

```typescript
// ui/src/lib/utils.ts

import { type ClassValue, clsx } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

/**
 * Format a date string for display.
 */
export function formatDate(dateStr: string): string {
  return new Date(dateStr).toLocaleDateString("en-US", {
    month: "short", day: "numeric", hour: "2-digit", minute: "2-digit",
  });
}

/**
 * Truncate a string to maxLen characters with ellipsis.
 */
export function truncate(str: string, maxLen: number): string {
  if (str.length <= maxLen) return str;
  return str.slice(0, maxLen - 1) + "\u2026";
}
```

#### 3.3.13 Root Route + Layout Wrapper -- `ui/src/routes/__root.tsx`

TanStack Router root route that wraps all pages in MainLayout:

```tsx
// ui/src/routes/__root.tsx

import { createRootRoute, Outlet } from "@tanstack/react-router";
import { Toaster } from "sonner";
import { TooltipProvider } from "@/components/ui/tooltip";
import { MainLayout } from "@/components/layout/main-layout";
import { useAuth } from "@/hooks/use-auth";
import { useWsNotifications } from "@/hooks/use-ws-notifications";
import { wsClient } from "@/lib/ws-client";
import { useEffect } from "react";

function RootComponent() {
  const { authenticated, loading } = useAuth();

  // Initialize WebSocket connection
  useEffect(() => {
    if (authenticated) {
      const token = localStorage.getItem("clawft-token");
      wsClient.connect(token ?? undefined);
      wsClient.subscribe("agent_status", "channel_status", "message_inbound", "message_outbound", "tool_call", "tool_result");
    }
    return () => wsClient.disconnect();
  }, [authenticated]);

  useWsNotifications();

  if (loading) {
    return (
      <div className="flex h-screen items-center justify-center">
        <div className="animate-spin h-8 w-8 border-4 border-primary border-t-transparent rounded-full" />
      </div>
    );
  }

  if (!authenticated) {
    return (
      <div className="flex h-screen items-center justify-center">
        <div className="text-center space-y-2">
          <h1 className="text-2xl font-bold">ClawFT Dashboard</h1>
          <p className="text-muted-foreground">Launch with <code className="bg-muted px-1 rounded">weft ui</code> to authenticate.</p>
        </div>
      </div>
    );
  }

  return (
    <TooltipProvider>
      <MainLayout>
        <Outlet />
      </MainLayout>
      <Toaster position="bottom-right" richColors />
    </TooltipProvider>
  );
}

export const Route = createRootRoute({ component: RootComponent });
```

---

## 4. Tasks

| # | Item | Priority | Week | Status | Location | Type |
|---|------|----------|------|--------|----------|------|
| S1.1.1 | Add API deps to `clawft-services/Cargo.toml` | P0 | 1 | TODO | `crates/clawft-services/Cargo.toml` | config |
| S1.1.2 | Create `api/mod.rs` router factory + state types | P0 | 1 | TODO | `crates/clawft-services/src/api/mod.rs` | code |
| S1.1.3 | Auth token store + Bearer middleware | P0 | 1 | TODO | `crates/clawft-services/src/api/auth.rs` | code |
| S1.1.4 | Agent CRUD endpoints (list, get, update) | P0 | 1 | TODO | `crates/clawft-services/src/api/agents.rs` | code |
| S1.1.5 | Agent start/stop action endpoints | P0 | 1 | TODO | `crates/clawft-services/src/api/agents.rs` | code |
| S1.1.6 | Session endpoints (list, get, delete) | P0 | 1 | TODO | `crates/clawft-services/src/api/sessions.rs` | code |
| S1.1.7 | Tool listing + schema endpoints | P0 | 1 | TODO | `crates/clawft-services/src/api/tools.rs` | code |
| S1.1.8 | WebSocket upgrade + topic subscription handler | P0 | 1 | TODO | `crates/clawft-services/src/api/ws.rs` | code |
| S1.1.9 | Wire API router into gateway startup + CORS | P0 | 1 | TODO | `crates/clawft-cli/src/commands/gateway.rs` | wiring |
| S1.1.10 | `weft ui` CLI command | P0 | 1 | TODO | `crates/clawft-cli/src/commands/ui.rs` | code |
| S1.1.11 | Static file serving (disk or rust-embed) | P1 | 1 | TODO | `crates/clawft-services/src/api/mod.rs` | code |
| S1.2.1 | Initialize Vite + React + TS project | P0 | 1 | TODO | `ui/package.json`, `ui/vite.config.ts` | scaffold |
| S1.2.2 | Tailwind CSS v4 config + globals.css | P0 | 1 | TODO | `ui/src/styles/globals.css` | config |
| S1.2.3 | shadcn/ui init + components.json | P0 | 1 | TODO | `ui/components.json` | config |
| S1.2.4 | Install 19 shadcn core components | P0 | 1 | TODO | `ui/src/components/ui/` | scaffold |
| S1.2.5 | MainLayout with collapsible sidebar | P0 | 1 | TODO | `ui/src/components/layout/main-layout.tsx` | code |
| S1.2.6 | `api-client.ts` fetch wrapper + typed methods | P0 | 1 | TODO | `ui/src/lib/api-client.ts` | code |
| S1.2.7 | `ws-client.ts` reconnecting WebSocket | P0 | 1 | TODO | `ui/src/lib/ws-client.ts` | code |
| S1.2.8 | `use-auth.ts` hook (URL param + localStorage) | P0 | 1 | TODO | `ui/src/hooks/use-auth.ts` | code |
| S1.2.9 | TypeScript types mirroring Rust API types | P0 | 1 | TODO | `ui/src/lib/types.ts` | code |
| S1.2.10 | TanStack Router file-based routing setup | P0 | 1 | TODO | `ui/src/main.tsx`, `ui/src/routes/` | code |
| S1.2.11 | MSW mock handlers (agents, sessions, tools) | P0 | 1 | TODO | `ui/src/mocks/` | code |
| S1.2.12 | Zustand stores (agent, chat, theme) | P0 | 1 | TODO | `ui/src/stores/` | code |
| S1.2.13 | Dockerfile (multi-stage: build + nginx:alpine) | P1 | 1 | TODO | `ui/Dockerfile` | config |
| S1.3.1 | Dashboard Home (summary cards + recent sessions) | P0 | 2 | TODO | `ui/src/routes/index.tsx` | code |
| S1.3.2 | Agent Management list view with status badges | P0 | 2 | TODO | `ui/src/routes/agents.tsx` | code |
| S1.3.3 | Agent Detail config form (react-hook-form + zod) | P0 | 2 | TODO | `ui/src/routes/agents.$id.tsx` | code |
| S1.3.4 | WebChat session sidebar | P0 | 2 | TODO | `ui/src/routes/chat.tsx` | code |
| S1.3.5 | WebChat message thread + streaming display | P0 | 2 | TODO | `ui/src/routes/chat.tsx` | code |
| S1.3.6 | WebChat input + optimistic send | P0 | 2 | TODO | `ui/src/routes/chat.tsx` | code |
| S1.3.7 | Tool Call Cards (expandable) | P0 | 2 | TODO | `ui/src/components/chat/tool-call-card.tsx` | code |
| S1.3.8 | Session Explorer DataTable | P0 | 3 | TODO | `ui/src/routes/sessions.tsx` | code |
| S1.3.9 | Session Detail dialog + export | P0 | 3 | TODO | `ui/src/routes/sessions.tsx` | code |
| S1.3.10 | Tool Registry list + schema viewer | P0 | 3 | TODO | `ui/src/routes/tools.tsx` | code |
| S1.3.11 | Dark/light theme toggle | P1 | 3 | TODO | `ui/src/hooks/use-theme.ts` | code |
| S1.3.12 | Command palette (Cmd+K) | P1 | 3 | TODO | `ui/src/components/layout/command-palette.tsx` | code |
| S1.3.13 | Toast notifications for WS events | P1 | 3 | TODO | `ui/src/hooks/use-ws-notifications.ts` | code |

**Total: 37 tasks** (11 backend + 13 frontend scaffold + 13 core views)

---

## 5. Tests

### 5.1 Backend API Tests (`crates/clawft-services/src/api/`)

| # | Test Name | Description | Assertions |
|---|-----------|-------------|------------|
| B1 | `test_token_generate_and_validate` | Generate token via `TokenStore`, verify it validates | Token is non-empty; `validate()` returns true; random string returns false |
| B2 | `test_token_expiry` | Generate token, advance time past TTL, verify rejection | `validate()` returns false after TTL elapsed |
| B3 | `test_token_cleanup` | Generate tokens, call `cleanup()`, verify expired removed | Only non-expired tokens remain in store |
| B4 | `test_bearer_extraction` | Construct requests with valid/invalid/missing Auth headers | Valid `Bearer xxx` extracts `xxx`; missing header returns `None`; `Basic` prefix returns `None` |
| B5 | `test_auth_middleware_rejects_no_token` | Send request to `/api/agents` without Bearer header | Response status is 401 |
| B6 | `test_auth_middleware_allows_valid_token` | Send request with valid Bearer token | Response status is 200 |
| B7 | `test_auth_middleware_skips_token_endpoint` | Send request to `/api/auth/token` without Bearer | Response status is 200 (no auth required) |
| B8 | `test_cors_allows_configured_origins` | Build router with `["http://localhost:5173"]`, send preflight | `Access-Control-Allow-Origin` header matches |
| B9 | `test_cors_allows_wildcard` | Build router with `["*"]`, send preflight from any origin | All origins allowed |
| B10 | `test_agents_list_returns_json` | Call `GET /api/agents` with mock AgentAccess | Response is JSON array of `AgentSummary` |
| B11 | `test_agents_get_found` | Call `GET /api/agents/default` with existing agent | Response 200 with `AgentDetail` fields |
| B12 | `test_agents_get_not_found` | Call `GET /api/agents/nonexistent` | Response 404 |
| B13 | `test_agents_update_patch` | Call `PATCH /api/agents/default` with model change | Response 200 with updated model |
| B14 | `test_agents_start` | Call `POST /api/agents/default/start` | Response 204 |
| B15 | `test_agents_stop` | Call `POST /api/agents/default/stop` | Response 204 |
| B16 | `test_sessions_list` | Call `GET /api/sessions` | Response is JSON array with `SessionSummary` fields |
| B17 | `test_sessions_get_detail` | Call `GET /api/sessions/:key` for existing session | Response 200 with messages array |
| B18 | `test_sessions_get_not_found` | Call `GET /api/sessions/missing` | Response 404 |
| B19 | `test_sessions_delete` | Call `DELETE /api/sessions/:key` | Response 204 on success, 404 on missing |
| B20 | `test_tools_list` | Call `GET /api/tools` | Response is JSON array with `ToolInfo` fields |
| B21 | `test_tools_schema_found` | Call `GET /api/tools/echo/schema` | Response 200 with JSON Schema object |
| B22 | `test_tools_schema_not_found` | Call `GET /api/tools/nonexistent/schema` | Response 404 |
| B23 | `test_ws_connect_and_receive` | Connect to `/ws`, subscribe to topic, push event, verify received | Client receives JSON event matching subscription |
| B24 | `test_ws_topic_filtering` | Subscribe to `agent_status` only, push `channel_status` event | Client does NOT receive unsubscribed event type |
| B25 | `test_ws_unsubscribe` | Subscribe then unsubscribe from topic | Events stop arriving for unsubscribed topic |

### 5.2 Frontend Component Tests (`ui/src/__tests__/`)

| # | Test Name | Description | Assertions |
|---|-----------|-------------|------------|
| F1 | `test_api_client_adds_auth_header` | Mock fetch, call `apiFetch`, inspect headers | Authorization header is `Bearer <token>` |
| F2 | `test_api_client_handles_204` | Mock fetch returning 204 | `apiFetch` returns undefined |
| F3 | `test_api_client_throws_on_error` | Mock fetch returning 500 | `ApiError` thrown with status 500 |
| F4 | `test_api_client_throws_on_401` | Mock fetch returning 401 | `ApiError` thrown with status 401 |
| F5 | `test_ws_client_reconnects` | Create WsClient, simulate disconnect | `scheduleReconnect` called; delay follows exponential backoff |
| F6 | `test_ws_client_topic_subscription` | Subscribe to topics, inspect sent message | Subscribe command sent via WebSocket |
| F7 | `test_ws_client_resubscribes_on_reconnect` | Connect, subscribe, disconnect, reconnect | Topics re-sent after reconnect |
| F8 | `test_use_auth_extracts_url_token` | Render hook with `?token=abc` in URL | localStorage contains token; URL param removed |
| F9 | `test_use_auth_verifies_stored_token` | Render hook with token in localStorage | `api.verifyToken` called; `authenticated` is true |
| F10 | `test_dashboard_renders_summary_cards` | Render Dashboard with mock data | 4 summary cards present with correct values |
| F11 | `test_dashboard_shows_loading_skeletons` | Render Dashboard while queries loading | Skeleton components rendered |
| F12 | `test_agent_list_shows_status_badges` | Render Agent Management with mock agents | Status badges render with correct colors |
| F13 | `test_agent_start_button_calls_mutation` | Click start button on stopped agent | `api.startAgent` called; toast shown |
| F14 | `test_agent_stop_button_calls_mutation` | Click stop button on running agent | `api.stopAgent` called; toast shown |
| F15 | `test_agent_detail_form_validates` | Submit form with invalid data | Zod errors displayed; no API call |
| F16 | `test_agent_detail_form_saves` | Fill valid data and submit | `api.updateAgent` called with form values |
| F17 | `test_chat_session_selection` | Click session in sidebar | `useChatStore.selectSession` called; messages loaded |
| F18 | `test_chat_message_rendering` | Render chat with mock messages | User messages right-aligned; assistant messages left-aligned |
| F19 | `test_chat_send_message` | Type message and press Enter | `wsClient.sendChat` called; optimistic message appended |
| F20 | `test_tool_call_card_collapsed` | Render ToolCallCard | Tool name badge visible; args/result hidden |
| F21 | `test_tool_call_card_expands` | Click ToolCallCard | Args JSON and result visible |
| F22 | `test_session_explorer_table` | Render Session Explorer with mock data | Table rows match mock session count |
| F23 | `test_session_export` | Click export button in detail dialog | Blob download triggered with correct JSON |
| F24 | `test_session_delete` | Click delete button, confirm | `api.deleteSession` called; row removed |
| F25 | `test_tool_registry_lists_tools` | Render Tool Registry with mock data | All mock tools listed in table |
| F26 | `test_tool_schema_viewer` | Click view on a tool | Dialog opens with formatted JSON schema |
| F27 | `test_theme_toggle` | Toggle theme | `dark` class toggled on `document.documentElement` |
| F28 | `test_theme_persists` | Toggle theme, reload | `localStorage` contains correct theme |
| F29 | `test_command_palette_opens` | Press Cmd+K | Command palette dialog opens |
| F30 | `test_command_palette_navigates` | Select "Agents" in palette | Navigation to `/agents` occurs; dialog closes |
| F31 | `test_main_layout_sidebar_collapse` | Click collapse button | Sidebar width changes; labels hidden |
| F32 | `test_main_layout_active_nav` | Navigate to `/agents` | Agents nav item has active styling |
| F33 | `test_ws_notification_agent_error` | Simulate agent_status error event | Toast error notification displayed |
| F34 | `test_ws_notification_channel_disconnect` | Simulate channel_status disconnect event | Toast warning notification displayed |

### 5.3 Integration Tests

| # | Test Name | Description | Assertions |
|---|-----------|-------------|------------|
| I1 | `test_msw_mock_agents_roundtrip` | Fetch agents via `api.listAgents` with MSW | Returns mock agent data matching fixtures |
| I2 | `test_msw_mock_sessions_roundtrip` | Fetch sessions via `api.listSessions` with MSW | Returns mock session data matching fixtures |
| I3 | `test_msw_mock_tools_roundtrip` | Fetch tools via `api.listTools` with MSW | Returns mock tool data matching fixtures |
| I4 | `test_api_client_retry_on_network_error` | Mock network failure, verify retry behavior | Retries once per TanStack Query config |
| I5 | `test_ws_reconnection_backoff` | Simulate 3 disconnects, verify delay progression | Delays follow 1s, 2s, 4s pattern |
| I6 | `test_full_auth_flow` | Generate token, set in localStorage, verify, make API call | All steps succeed without errors |

### 5.4 E2E Tests (Playwright)

| # | Test Name | Description | Assertions |
|---|-----------|-------------|------------|
| E1 | `test_e2e_auth_redirect` | Load app with `?token=xxx` | Token extracted; redirected to dashboard |
| E2 | `test_e2e_navigation` | Click each sidebar link | Correct page content rendered for each route |
| E3 | `test_e2e_agent_start_stop` | Navigate to agents, click start/stop | Status badge updates; toast notification shown |
| E4 | `test_e2e_session_view_export` | Navigate to sessions, view detail, export | Dialog opens; download triggered |
| E5 | `test_e2e_command_palette` | Press Cmd+K, type "agents", select | Navigation to agents page |
| E6 | `test_e2e_theme_toggle` | Click theme toggle | Background color changes; preference persists |

**Total: 71 test specifications** (25 backend + 34 frontend + 6 integration + 6 E2E)

---

## 6. Exit Criteria

### Backend (S1.1)

- [ ] `cargo build -p clawft-services --features api` compiles cleanly
- [ ] `cargo clippy -p clawft-services --features api -- -D warnings` is clean
- [ ] All API endpoints respond correctly: `/api/auth/token`, `/api/auth/verify`, `/api/agents`, `/api/agents/:id`, `/api/agents/:id/start`, `/api/agents/:id/stop`, `/api/sessions`, `/api/sessions/:key`, `/api/tools`, `/api/tools/:name/schema`
- [ ] WebSocket `/ws` accepts connections and delivers events
- [ ] WebSocket topic subscription/unsubscription works
- [ ] CORS middleware allows configured origins
- [ ] Bearer auth middleware rejects unauthenticated requests with 401
- [ ] `weft ui` generates token, opens browser, starts API server
- [ ] Static file serving works when `ui/dist/` exists on disk
- [ ] `GatewayConfig` includes `api_port`, `cors_origins`, `api_enabled` fields with defaults

### Frontend Scaffolding (S1.2)

- [ ] `pnpm install && pnpm build` succeeds in `ui/`
- [ ] `pnpm type-check` reports no type errors
- [ ] `pnpm lint` passes with zero warnings
- [ ] `VITE_MOCK_API=true pnpm dev` serves the application standalone with mock data
- [ ] All shadcn/ui core components installed: Button, Card, Badge, Table, Tabs, Dialog, Sidebar, Toast, ScrollArea, Select, Switch, Separator, Tooltip, DropdownMenu, Command, Input, Textarea, Popover, Skeleton
- [ ] MainLayout renders with collapsible sidebar and navigation
- [ ] TanStack Router routes to all core views: `/`, `/agents`, `/agents/:id`, `/chat`, `/sessions`, `/tools`
- [ ] API client handles auth token injection and error responses
- [ ] WebSocket client reconnects with exponential backoff
- [ ] Auth hook extracts token from URL param and persists to localStorage
- [ ] `pnpm build` produces optimized `ui/dist/` under 200 KB gzipped
- [ ] Dockerfile builds and serves via nginx:alpine

### Core Views (S1.3)

- [ ] Dashboard Home displays agent count, session count, tool count, health status
- [ ] Agent Management shows agents with status badges and start/stop actions
- [ ] Agent Detail shows config form with model selector, saves via PATCH
- [ ] WebChat displays session list sidebar and message thread
- [ ] WebChat supports real-time message display via WebSocket
- [ ] WebChat input sends messages and displays optimistic updates
- [ ] Tool Call Cards render expandably in chat with tool name, args, result
- [ ] Session Explorer shows DataTable with session keys, message counts, timestamps
- [ ] Session Detail dialog shows full conversation with export button
- [ ] Tool Registry lists all tools with schema viewer dialog
- [ ] Dark/light theme toggle works and persists preference
- [ ] Command palette (Cmd+K) navigates to all views
- [ ] Toast notifications fire for agent status changes and channel events

---

## 7. Risks

| Risk | Likelihood | Impact | Score | Mitigation |
|------|-----------|--------|-------|------------|
| Backend API delays block frontend integration | Low | Low | **2** | MSW mock layer enables full frontend dev without backend. Mock handlers maintained alongside Rust API types. |
| WebSocket protocol changes mid-sprint | Medium | Medium | **6** | Define protocol in shared `types.ts` + Rust types. Version WS messages from day one. Breaking changes require mock handler updates. |
| shadcn/ui or Tailwind breaking changes | Low | Low | **2** | Pin versions in `pnpm-lock.yaml`. Use `components.json` lock. Only upgrade between sprints. |
| Bundle size exceeds 200KB budget | Medium | Low | **4** | Dynamic imports for heavy views (Canvas in S2). Monitor with `vite-bundle-analyzer`. Tree-shaking via ES module imports. |
| axum 0.8 API surface changes | Low | Medium | **3** | Pin exact version. Axum 0.8 is the current stable release. |
| Token store is in-memory (lost on restart) | Low | Low | **2** | Acceptable for MVP single-operator model. Token auto-regenerated by `weft ui` on each launch. Persistent store deferred to S3.5. |
| WebSocket connection instability on unreliable networks | Medium | Medium | **6** | Reconnecting client with exponential backoff (1s, 2s, 4s ... 30s max). Missed events recovered via REST API polling on reconnect. Connection status indicator in UI header. |
| Type drift between Rust API and TypeScript types | Medium | Medium | **6** | TypeScript types manually maintained in `types.ts` mirroring Rust structs. Add CI check comparing Rust `serde_json::to_string` output with TS type assertions. Consider `ts-rs` crate for automated generation in S3. |
| XSS via user-generated content in chat messages | Low | Critical | **5** | React default escaping for all text. No `dangerouslySetInnerHTML`. Tool call results rendered in `<pre>` tags. Canvas input validation deferred to S2. |
