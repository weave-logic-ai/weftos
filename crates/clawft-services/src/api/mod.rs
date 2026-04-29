//! REST + WebSocket API for the ClawFT web dashboard.
//!
//! Provides HTTP endpoints for agent management, session browsing,
//! tool inspection, and real-time WebSocket events.

pub mod auth;
pub mod bridge;
pub mod broadcaster;
pub mod channels_api;
pub mod chat;
pub mod config_api;
pub mod cron_api;
pub mod delegation;
pub mod handlers;
pub mod memory_api;
pub mod monitoring;
pub mod skills;
pub mod voice_api;
pub mod ws;

use std::sync::Arc;

use axum::Router;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

/// Shared state accessible by all API handlers.
#[derive(Clone)]
pub struct ApiState {
    /// Tool registry access.
    pub tools: Arc<dyn ToolRegistryAccess>,
    /// Session manager access.
    pub sessions: Arc<dyn SessionAccess>,
    /// Agent registry access.
    pub agents: Arc<dyn AgentAccess>,
    /// Message bus for WebSocket broadcasting.
    pub bus: Arc<dyn BusAccess>,
    /// Auth token store.
    pub auth: Arc<auth::TokenStore>,
    /// Skills access.
    pub skills: Arc<dyn SkillAccess>,
    /// Memory access.
    pub memory: Arc<dyn MemoryAccess>,
    /// Config access.
    pub config: Arc<dyn ConfigAccess>,
    /// Channel status access.
    pub channels: Arc<dyn ChannelAccess>,
    /// Voice configuration access.
    pub voice: Arc<dyn VoiceAccess>,
    /// Topic-based broadcaster for real-time WebSocket events.
    pub broadcaster: Arc<broadcaster::TopicBroadcaster>,
}

/// Trait for tool registry access (decouples API from Platform generics).
pub trait ToolRegistryAccess: Send + Sync {
    /// List all registered tools.
    fn list_tools(&self) -> Vec<ToolInfo>;
    /// Get the JSON schema for a named tool.
    fn tool_schema(&self, name: &str) -> Option<serde_json::Value>;
}

/// Summary info for a registered tool.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
}

/// Trait for session access.
pub trait SessionAccess: Send + Sync {
    /// List all active sessions.
    fn list_sessions(&self) -> Vec<SessionInfo>;
    /// Get details of a specific session.
    fn get_session(&self, key: &str) -> Option<SessionDetail>;
    /// Delete a session by key. Returns true if it existed.
    fn delete_session(&self, key: &str) -> bool;
}

/// Summary info for a session.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionInfo {
    pub key: String,
    pub message_count: usize,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

/// Full detail for a session, including message history.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionDetail {
    pub key: String,
    pub messages: Vec<serde_json::Value>,
}

/// Trait for agent registry access.
pub trait AgentAccess: Send + Sync {
    /// List all registered agents.
    fn list_agents(&self) -> Vec<AgentInfo>;
    /// Get details for a specific agent by name.
    fn get_agent(&self, name: &str) -> Option<AgentInfo>;
}

/// Summary info for a registered agent.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentInfo {
    pub name: String,
    pub description: String,
    pub model: String,
    pub skills: Vec<String>,
}

/// Trait for message bus access (used by WebSocket broadcasting).
pub trait BusAccess: Send + Sync {
    /// Send a message to a specific channel and chat.
    fn send_message(&self, channel: &str, chat_id: &str, content: &str);
}

/// Trait for skills access.
pub trait SkillAccess: Send + Sync {
    /// List all installed skills.
    fn list_skills(&self) -> Vec<SkillInfo>;
    /// Install a skill by registry ID.
    fn install_skill(&self, id: &str) -> Result<(), String>;
    /// Uninstall a skill by name.
    fn uninstall_skill(&self, name: &str) -> Result<(), String>;
}

/// Summary info for an installed skill.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub version: String,
    pub user_invocable: bool,
}

/// Trait for memory access.
pub trait MemoryAccess: Send + Sync {
    /// List all memory entries.
    fn list_entries(&self) -> Vec<MemoryEntryInfo>;
    /// Search memory entries by query.
    fn search(&self, query: &str, max_results: usize) -> Vec<MemoryEntryInfo>;
    /// Store a new memory entry.
    fn store(
        &self,
        key: &str,
        value: &str,
        namespace: &str,
        tags: &[String],
    ) -> Result<MemoryEntryInfo, String>;
    /// Delete a memory entry by key.
    fn delete(&self, key: &str) -> bool;
}

/// Info for a memory entry.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MemoryEntryInfo {
    pub key: String,
    pub value: String,
    pub namespace: String,
    pub tags: Vec<String>,
    #[serde(default)]
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub similarity: Option<f64>,
}

/// Trait for config access.
pub trait ConfigAccess: Send + Sync {
    /// Get the current configuration as a JSON value.
    fn get_config(&self) -> serde_json::Value;
    /// Save a new configuration.
    fn save_config(&self, config: serde_json::Value) -> Result<(), String>;
}

/// Trait for channel status access.
pub trait ChannelAccess: Send + Sync {
    /// List all channel statuses.
    fn list_channels(&self) -> Vec<ChannelStatusInfo>;
}

/// Status info for a channel.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChannelStatusInfo {
    pub name: String,
    #[serde(rename = "type")]
    pub channel_type: String,
    pub status: String,
    pub message_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_activity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routes_to: Option<String>,
}

/// Trait for voice pipeline access.
pub trait VoiceAccess: Send + Sync {
    /// Get the current voice pipeline status.
    fn get_status(&self) -> VoiceStatusInfo;
    /// Get the current voice settings.
    fn get_settings(&self) -> VoiceSettingsInfo;
    /// Update voice settings (partial merge).
    fn update_settings(&self, update: VoiceSettingsUpdate) -> Result<(), String>;
    /// Get TTS configuration for the cloud TTS proxy.
    fn get_tts_config(&self) -> TtsProviderInfo;
}

/// TTS provider configuration exposed to the API layer.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TtsProviderInfo {
    /// Provider name: "browser", "openai", or "elevenlabs".
    pub provider: String,
    /// Model identifier (e.g. "tts-1", "tts-1-hd").
    pub model: String,
    /// Voice ID (e.g. "alloy", "nova", "shimmer").
    pub voice: String,
    /// Speaking speed (0.25 - 4.0).
    pub speed: f32,
    /// API key for the TTS provider (empty if browser-only).
    #[serde(skip_serializing)]
    pub api_key: String,
    /// Optional base URL override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_base: Option<String>,
}

/// Voice pipeline status info.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VoiceStatusInfo {
    pub state: String,
    #[serde(rename = "talkModeActive")]
    pub talk_mode_active: bool,
    #[serde(rename = "wakeWordEnabled")]
    pub wake_word_enabled: bool,
}

/// Voice settings matching the UI's VoiceSettingsData.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VoiceSettingsInfo {
    pub enabled: bool,
    #[serde(rename = "wakeWordEnabled")]
    pub wake_word_enabled: bool,
    pub language: String,
    #[serde(rename = "echoCancel")]
    pub echo_cancel: bool,
    #[serde(rename = "noiseSuppression")]
    pub noise_suppression: bool,
    #[serde(rename = "pushToTalk")]
    pub push_to_talk: bool,
}

/// Partial update for voice settings.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct VoiceSettingsUpdate {
    pub enabled: Option<bool>,
    #[serde(rename = "wakeWordEnabled")]
    pub wake_word_enabled: Option<bool>,
    pub language: Option<String>,
    #[serde(rename = "echoCancel")]
    pub echo_cancel: Option<bool>,
    #[serde(rename = "noiseSuppression")]
    pub noise_suppression: Option<bool>,
    #[serde(rename = "pushToTalk")]
    pub push_to_talk: Option<bool>,
}

/// Start the API server on the given listener with graceful shutdown.
///
/// This is a convenience wrapper around `axum::serve` that keeps the
/// axum dependency confined to clawft-services.
///
/// When `static_dir` is `Some`, the built frontend in that directory will
/// be served as an SPA fallback (any path not matched by `/api` or `/ws`
/// returns the static file or `index.html`).
pub async fn serve(
    listener: tokio::net::TcpListener,
    state: ApiState,
    cors_origins: &[String],
    static_dir: Option<&str>,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    // WEFT-102: kick off the periodic token-store sweep so revoked
    // and expired tokens do not accumulate over the server's lifetime.
    // The handle is detached -- the task observes the store via a
    // Weak ref and self-terminates when ApiState drops its Arc.
    let _cleanup = auth::spawn_cleanup_task(
        state.auth.clone(),
        auth::TOKEN_CLEANUP_INTERVAL_SECS,
    );
    let router = build_router(state, cors_origins, static_dir);
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await
}

/// Build the API router with all routes.
///
/// When `static_dir` is provided, a [`tower_http::services::ServeDir`]
/// fallback is added so that the built frontend is served for any path
/// not matched by the API or WebSocket routes.
pub fn build_router(state: ApiState, cors_origins: &[String], static_dir: Option<&str>) -> Router {
    let cors = if cors_origins.is_empty() {
        CorsLayer::permissive()
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

    let mut router = Router::new()
        .nest("/api", handlers::api_routes())
        // NOTE: To enable auth middleware on protected API routes, wrap the
        // `/api` nest with:
        //   .nest("/api", handlers::api_routes()
        //       .layer(axum::middleware::from_fn_with_state(
        //           state.clone(), auth::auth_middleware)))
        // This is intentionally disabled for now to keep the dev workflow
        // simple (no token required). Enable once the UI has a login flow.
        .route("/ws", axum::routing::get(ws::ws_handler));

    // Serve built UI as SPA fallback when a static directory is provided.
    if let Some(dir) = static_dir {
        use tower_http::services::ServeDir;
        router = router.fallback_service(
            ServeDir::new(dir).append_index_html_on_directories(true),
        );
    }

    router
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
