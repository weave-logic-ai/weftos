//! Channel status API routes.
//!
//! Provides endpoints for listing channel connection statuses.

use axum::{Json, Router, extract::State, routing::get};

use super::{ApiState, ChannelStatusInfo};

/// Build channel status API routes.
pub fn channel_routes() -> Router<ApiState> {
    Router::new().route("/channels", get(list_channels))
}

// ── Handlers ───────────────────────────────────────────────────

async fn list_channels(State(state): State<ApiState>) -> Json<Vec<ChannelStatusInfo>> {
    Json(state.channels.list_channels())
}
