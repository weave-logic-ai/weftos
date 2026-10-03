//! WEFT-219: integration tests for `/api/voice/*` against a real
//! [`VoiceBridge`] (not the stub used by middleware smoke tests).
//!
//! Covers the four MSW-parity endpoints (status, settings, test-mic,
//! test-speaker) plus TTS config, camelCase UI contract, auth gate, and
//! settings round-trip through the assembled router.

#![cfg(feature = "api")]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use clawft_services::api::bridge::VoiceBridge;
use clawft_services::api::{
    AgentAccess, AgentInfo, ApiState, BusAccess, ChannelAccess, ChannelStatusInfo, ConfigAccess,
    InMemoryKernelFacade, MemoryAccess, MemoryEntryInfo, SessionAccess, SessionDetail, SessionInfo,
    SkillAccess, SkillInfo, ToolInfo, ToolRegistryAccess, auth::MemoryTokenValidator,
    broadcaster::TopicBroadcaster, build_router,
};
use clawft_types::config::ProvidersConfig;
use clawft_types::config::voice::VoiceConfig;
use http_body_util::BodyExt;
use tower::ServiceExt;

// ─── Minimal stubs for non-voice slices ─────────────────────────────────

struct StubTools;
impl ToolRegistryAccess for StubTools {
    fn list_tools(&self) -> Vec<ToolInfo> {
        vec![]
    }
    fn tool_schema(&self, _: &str) -> Option<serde_json::Value> {
        None
    }
}

struct StubSessions;
impl SessionAccess for StubSessions {
    fn list_sessions(&self) -> Vec<SessionInfo> {
        vec![]
    }
    fn get_session(&self, _: &str) -> Option<SessionDetail> {
        None
    }
    fn delete_session(&self, _: &str) -> bool {
        false
    }
}

struct StubAgents;
impl AgentAccess for StubAgents {
    fn list_agents(&self) -> Vec<AgentInfo> {
        vec![]
    }
    fn get_agent(&self, _: &str) -> Option<AgentInfo> {
        None
    }
}

struct StubBus;
impl BusAccess for StubBus {
    fn send_message(&self, _: &str, _: &str, _: &str) {}
}

struct StubSkills;
impl SkillAccess for StubSkills {
    fn list_skills(&self) -> Vec<SkillInfo> {
        vec![]
    }
    fn install_skill(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn uninstall_skill(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
}

struct StubMemory;
impl MemoryAccess for StubMemory {
    fn list_entries(&self) -> Vec<MemoryEntryInfo> {
        vec![]
    }
    fn search(&self, _: &str, _: usize) -> Vec<MemoryEntryInfo> {
        vec![]
    }
    fn store(&self, _: &str, _: &str, _: &str, _: &[String]) -> Result<MemoryEntryInfo, String> {
        Err("stub".into())
    }
    fn delete(&self, _: &str) -> bool {
        false
    }
}

struct StubConfig;
impl ConfigAccess for StubConfig {
    fn get_config(&self) -> serde_json::Value {
        serde_json::json!({})
    }
    fn save_config(&self, _: serde_json::Value) -> Result<(), String> {
        Ok(())
    }
}

struct StubChannels;
impl ChannelAccess for StubChannels {
    fn list_channels(&self) -> Vec<ChannelStatusInfo> {
        vec![]
    }
}

fn make_state_with_voice() -> (ApiState, Arc<MemoryTokenValidator>, Arc<VoiceBridge>) {
    let auth = Arc::new(MemoryTokenValidator::new());
    let voice = Arc::new(VoiceBridge::new(
        VoiceConfig::default(),
        ProvidersConfig::default(),
    ));
    let state = ApiState {
        routing_history: Arc::new(
            clawft_core::pipeline::decision_history::RoutingDecisionHistory::new(),
        ),
        rate_limiter: Arc::new(clawft_core::pipeline::rate_limiter::RateLimiter::new(60, 0)),
        health_cache: Default::default(),
        tools: Arc::new(StubTools),
        sessions: Arc::new(StubSessions),
        agents: Arc::new(StubAgents),
        bus: Arc::new(StubBus),
        auth: auth.clone(),
        skills: Arc::new(StubSkills),
        memory: Arc::new(StubMemory),
        config: Arc::new(StubConfig),
        channels: Arc::new(StubChannels),
        voice: voice.clone(),
        broadcaster: Arc::new(TopicBroadcaster::new()),
        kernel_facade: Arc::new(InMemoryKernelFacade::new()),
    };
    (state, auth, voice)
}

async fn json_body(resp: axum::response::Response) -> serde_json::Value {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::json!({}))
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn voice_status_requires_auth() {
    let (state, _auth, _) = make_state_with_voice();
    let app = build_router(state, &[], None);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/voice/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn voice_status_returns_camel_case_ui_contract() {
    let (state, auth, _) = make_state_with_voice();
    let token = auth.generate_token(3600).unwrap();
    let app = build_router(state, &[], None);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/voice/status")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_body(resp).await;

    assert_eq!(json["state"], "idle");
    assert_eq!(json["talkModeActive"], false);
    assert_eq!(json["wakeWordEnabled"], false);

    let settings = &json["settings"];
    assert_eq!(settings["enabled"], false);
    assert_eq!(settings["wakeWordEnabled"], false);
    assert_eq!(settings["language"], "en");
    assert_eq!(settings["echoCancel"], true);
    assert_eq!(settings["noiseSuppression"], true);
    assert_eq!(settings["pushToTalk"], false);

    // Snake_case must not leak into the UI payload.
    assert!(settings.get("wake_word_enabled").is_none());
    assert!(settings.get("echo_cancel").is_none());
    assert!(json.get("talk_mode_active").is_none());
}

#[tokio::test]
async fn voice_settings_put_round_trips_via_status() {
    let (state, auth, _) = make_state_with_voice();
    let token = auth.generate_token(3600).unwrap();
    let app = build_router(state, &[], None);

    let put = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/api/voice/settings")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"enabled":true,"wakeWordEnabled":true,"language":"de","echoCancel":false,"pushToTalk":true}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(put.status(), StatusCode::OK);
    let put_json = json_body(put).await;
    assert_eq!(put_json["success"], true);

    let get = app
        .oneshot(
            Request::builder()
                .uri("/api/voice/status")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(get.status(), StatusCode::OK);
    let status = json_body(get).await;
    assert_eq!(status["wakeWordEnabled"], true);
    assert_eq!(status["settings"]["enabled"], true);
    assert_eq!(status["settings"]["wakeWordEnabled"], true);
    assert_eq!(status["settings"]["language"], "de");
    assert_eq!(status["settings"]["echoCancel"], false);
    assert_eq!(status["settings"]["noiseSuppression"], true); // not in PUT
    assert_eq!(status["settings"]["pushToTalk"], true);
}

#[tokio::test]
async fn voice_test_mic_and_speaker() {
    let (state, auth, _) = make_state_with_voice();
    let token = auth.generate_token(3600).unwrap();
    let app = build_router(state, &[], None);

    let mic = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/voice/test-mic")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(mic.status(), StatusCode::OK);
    let mic_json = json_body(mic).await;
    assert_eq!(mic_json["success"], true);
    assert!(mic_json["level"].as_f64().is_some());

    let speaker = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/voice/test-speaker")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(speaker.status(), StatusCode::OK);
    let spk_json = json_body(speaker).await;
    assert_eq!(spk_json["success"], true);
}

#[tokio::test]
async fn voice_tts_config_returns_provider_without_secrets() {
    let (state, auth, _) = make_state_with_voice();
    let token = auth.generate_token(3600).unwrap();
    let app = build_router(state, &[], None);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/voice/tts/config")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_body(resp).await;
    // WEFT-238: default provider is local (not browser).
    assert_eq!(json["provider"], "local");
    assert!(json.get("model").is_some());
    assert!(json.get("voice").is_some());
    assert!(json.get("speed").is_some());
    assert!(json.get("api_key").is_none());
    assert!(json.get("apiKey").is_none());
}

#[tokio::test]
async fn voice_tts_local_default_provider_returns_clear_400() {
    let (state, auth, _) = make_state_with_voice();
    let token = auth.generate_token(3600).unwrap();
    let app = build_router(state, &[], None);

    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/voice/tts")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"text":"hello"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    // WEFT-238: default TTS provider is local → cloud proxy returns a clear
    // error (not an opaque "unsupported" or silent hang).
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = json_body(resp).await;
    let err = json["error"].as_str().unwrap_or("");
    assert!(
        err.to_lowercase().contains("local"),
        "expected local-provider error, got: {err}"
    );
    assert!(
        err.contains("openai") || err.contains("elevenlabs") || err.contains("native"),
        "expected guidance toward cloud or native path, got: {err}"
    );
}
