//! Voice pipeline API routes.
//!
//! Provides endpoints for voice status, settings, device testing,
//! and cloud TTS synthesis proxy.
//! Reads/writes real configuration via the [`VoiceAccess`] trait on ApiState.

use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use tracing::{debug, error};

use super::ApiState;
use super::VoiceSettingsUpdate;

/// Request body for the TTS synthesis endpoint.
#[derive(serde::Deserialize)]
struct TtsRequest {
    /// Text to synthesize.
    text: String,
    /// Optional voice override (e.g. "alloy", "nova", "shimmer").
    voice: Option<String>,
    /// Optional speed override (0.25 - 4.0).
    speed: Option<f32>,
}

/// Build voice API routes.
pub fn voice_routes() -> Router<ApiState> {
    Router::new()
        .route("/voice/status", get(voice_status))
        .route("/voice/settings", put(update_voice_settings))
        .route("/voice/test-mic", post(test_mic))
        .route("/voice/test-speaker", post(test_speaker))
        .route("/voice/tts", post(synthesize_tts))
        .route("/voice/tts/config", get(tts_config))
}

// ── Handlers ───────────────────────────────────────────────────

async fn voice_status(State(state): State<ApiState>) -> Json<serde_json::Value> {
    let status = state.voice.get_status();
    let settings = state.voice.get_settings();
    Json(serde_json::json!({
        "state": status.state,
        "talkModeActive": status.talk_mode_active,
        "wakeWordEnabled": status.wake_word_enabled,
        "settings": settings,
    }))
}

async fn update_voice_settings(
    State(state): State<ApiState>,
    Json(payload): Json<VoiceSettingsUpdate>,
) -> Json<serde_json::Value> {
    match state.voice.update_settings(payload) {
        Ok(()) => Json(serde_json::json!({ "success": true })),
        Err(e) => Json(serde_json::json!({ "success": false, "error": e })),
    }
}

async fn test_mic(State(_state): State<ApiState>) -> Json<serde_json::Value> {
    // Mic testing requires audio hardware access; return a placeholder level.
    // A real implementation would use the platform audio layer.
    Json(serde_json::json!({ "success": true, "level": 0.0 }))
}

async fn test_speaker(State(_state): State<ApiState>) -> Json<serde_json::Value> {
    // Speaker testing requires audio hardware access; return success.
    Json(serde_json::json!({ "success": true }))
}

/// Return the current TTS provider configuration (without API keys).
async fn tts_config(State(state): State<ApiState>) -> Json<serde_json::Value> {
    let cfg = state.voice.get_tts_config();
    Json(serde_json::json!({
        "provider": cfg.provider,
        "model": cfg.model,
        "voice": cfg.voice,
        "speed": cfg.speed,
    }))
}

/// Proxy text-to-speech synthesis through the configured cloud provider.
///
/// Accepts `{ "text": "...", "voice": "alloy", "speed": 1.0 }` and returns
/// an `audio/mpeg` stream from the cloud TTS API. This keeps the API key
/// server-side so the browser never sees it.
///
/// Returns 400 if the provider is "browser" (client should use Web Speech API).
/// Returns 500 if the upstream TTS API call fails.
async fn synthesize_tts(
    State(state): State<ApiState>,
    Json(req): Json<TtsRequest>,
) -> Result<Response, Response> {
    let cfg = state.voice.get_tts_config();

    if cfg.provider == "browser" {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "TTS provider is set to 'browser'. Use the Web Speech API directly."
            })),
        )
            .into_response());
    }

    let text = req.text.trim().to_string();
    if text.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "text is required" })),
        )
            .into_response());
    }

    if cfg.api_key.is_empty() {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": "No API key configured for TTS provider"
            })),
        )
            .into_response());
    }

    match cfg.provider.as_str() {
        "openai" => synthesize_openai(&cfg, &text, req.voice, req.speed).await,
        "elevenlabs" => synthesize_elevenlabs(&cfg, &text, req.voice).await,
        other => Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!("Unsupported TTS provider: {other}")
            })),
        )
            .into_response()),
    }
}

/// Call the OpenAI TTS API and stream the audio response back.
async fn synthesize_openai(
    cfg: &super::TtsProviderInfo,
    text: &str,
    voice_override: Option<String>,
    speed_override: Option<f32>,
) -> Result<Response, Response> {
    let base = cfg
        .api_base
        .as_deref()
        .unwrap_or("https://api.openai.com/v1");
    let url = format!("{}/audio/speech", base);

    let voice = voice_override.as_deref().unwrap_or(&cfg.voice);
    let speed = speed_override.unwrap_or(cfg.speed);

    debug!(
        model = %cfg.model,
        voice = %voice,
        speed = %speed,
        text_len = text.len(),
        "calling OpenAI TTS API"
    );

    let body = serde_json::json!({
        "model": cfg.model,
        "input": text,
        "voice": voice,
        "speed": speed,
        "response_format": "mp3",
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", cfg.api_key))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            error!(error = %e, "OpenAI TTS request failed");
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": format!("TTS request failed: {e}") })),
            )
                .into_response()
        })?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let err_text = resp.text().await.unwrap_or_default();
        error!(status, body = %err_text, "OpenAI TTS API error");
        return Err((
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({
                "error": format!("TTS API returned {status}"),
                "detail": err_text,
            })),
        )
            .into_response());
    }

    // Read the audio bytes and return them to the browser.
    let bytes = resp.bytes().await.map_err(|e| {
        error!(error = %e, "failed to read TTS response body");
        (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "error": format!("Failed to read TTS audio: {e}") })),
        )
            .into_response()
    })?;

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "audio/mpeg")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from(bytes))
        .unwrap())
}

/// Call the ElevenLabs TTS API and return the audio response.
///
/// ElevenLabs API: POST /v1/text-to-speech/{voice_id}
/// The voice_id is either a preset name or a custom voice ID.
async fn synthesize_elevenlabs(
    cfg: &super::TtsProviderInfo,
    text: &str,
    voice_override: Option<String>,
) -> Result<Response, Response> {
    let base = cfg
        .api_base
        .as_deref()
        .unwrap_or("https://api.elevenlabs.io");
    let voice = voice_override.as_deref().unwrap_or(&cfg.voice);
    let url = format!("{}/v1/text-to-speech/{}", base, voice);

    debug!(
        model = %cfg.model,
        voice = %voice,
        text_len = text.len(),
        "calling ElevenLabs TTS API"
    );

    let body = serde_json::json!({
        "text": text,
        "model_id": cfg.model,
        "voice_settings": {
            "stability": 0.5,
            "similarity_boost": 0.75,
            "style": 0.0,
            "use_speaker_boost": true,
        },
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("xi-api-key", &cfg.api_key)
        .header("Content-Type", "application/json")
        .header("Accept", "audio/mpeg")
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            error!(error = %e, "ElevenLabs TTS request failed");
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": format!("TTS request failed: {e}") })),
            )
                .into_response()
        })?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let err_text = resp.text().await.unwrap_or_default();
        error!(status, body = %err_text, "ElevenLabs TTS API error");
        return Err((
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({
                "error": format!("TTS API returned {status}"),
                "detail": err_text,
            })),
        )
            .into_response());
    }

    let bytes = resp.bytes().await.map_err(|e| {
        error!(error = %e, "failed to read ElevenLabs TTS response body");
        (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "error": format!("Failed to read TTS audio: {e}") })),
        )
            .into_response()
    })?;

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "audio/mpeg")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from(bytes))
        .unwrap())
}
