//! Chat session and messaging API routes.
//!
//! Provides endpoints for creating chat sessions, sending messages,
//! exporting session histories, and streaming session events via SSE.

use std::convert::Infallible;

use axum::{
    Json, Router,
    extract::{Path, State},
    response::sse::{Event, Sse},
    routing::{get, post},
};
use futures_util::stream::unfold;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use super::ApiState;

/// Build chat API routes.
pub fn chat_routes() -> Router<ApiState> {
    Router::new()
        .route("/sessions/{key}/messages", post(send_message))
        .route("/sessions", post(create_session))
        .route("/sessions/{key}/export", get(export_session))
        .route("/sessions/{key}/stream", get(stream_session))
}

// ── Types ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct SendMessageRequest {
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct ChatMessageResponse {
    pub role: String,
    pub content: String,
    pub timestamp: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    pub agent_id: String,
}

#[derive(Debug, Serialize)]
pub struct SessionSummaryResponse {
    pub key: String,
    pub agent_id: String,
    pub message_count: usize,
    pub updated_at: String,
}

#[derive(Debug, Serialize)]
pub struct ExportResponse {
    pub messages: Vec<ChatMessageResponse>,
}

// ── Handlers ───────────────────────────────────────────────────

async fn send_message(
    State(state): State<ApiState>,
    Path(key): Path<String>,
    Json(payload): Json<SendMessageRequest>,
) -> Json<ChatMessageResponse> {
    // Publish inbound message to the bus for agent processing.
    state.bus.send_message("web", &key, &payload.content);

    // Return the user message immediately.
    // The agent response will arrive via WebSocket or SSE.
    let timestamp = chrono::Utc::now().to_rfc3339();
    Json(ChatMessageResponse {
        role: "user".into(),
        content: payload.content,
        timestamp,
    })
}

async fn create_session(
    State(_state): State<ApiState>,
    Json(payload): Json<CreateSessionRequest>,
) -> Json<SessionSummaryResponse> {
    let session_key = format!("web:{}", uuid::Uuid::new_v4());
    let now = chrono::Utc::now().to_rfc3339();

    Json(SessionSummaryResponse {
        key: session_key,
        agent_id: payload.agent_id,
        message_count: 0,
        updated_at: now,
    })
}

async fn export_session(
    State(state): State<ApiState>,
    Path(key): Path<String>,
) -> Json<ExportResponse> {
    let detail = state.sessions.get_session(&key);
    match detail {
        Some(d) => {
            let messages: Vec<ChatMessageResponse> = d
                .messages
                .into_iter()
                .map(|m| ChatMessageResponse {
                    role: m
                        .get("role")
                        .and_then(|v| v.as_str())
                        .unwrap_or("user")
                        .to_string(),
                    content: m
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    timestamp: m
                        .get("timestamp")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                })
                .collect();
            Json(ExportResponse { messages })
        }
        None => Json(ExportResponse {
            messages: Vec::new(),
        }),
    }
}

/// Stream session events via Server-Sent Events.
///
/// Subscribes to the `sessions:{key}` topic on the broadcaster and forwards
/// each published message as an SSE data frame. Clients can use
/// `EventSource` in the browser to receive real-time updates for a chat
/// session.
///
/// `GET /api/sessions/{key}/stream`
async fn stream_session(
    State(state): State<ApiState>,
    Path(key): Path<String>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let topic = format!("sessions:{}", key);
    let rx = state.broadcaster.subscribe(&topic).await;

    // Use `unfold` to convert the broadcast receiver into an SSE stream.
    // Each successful recv yields an SSE Event; lagged messages are skipped;
    // channel closure terminates the stream.
    let stream = unfold(rx, |mut rx| async {
        loop {
            match rx.recv().await {
                Ok(msg) => {
                    return Some((Ok(Event::default().data(msg)), rx));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    // Slow consumer -- skip missed messages and keep going.
                    continue;
                }
                Err(_) => {
                    // Channel closed.
                    return None;
                }
            }
        }
    });

    Sse::new(stream)
}
