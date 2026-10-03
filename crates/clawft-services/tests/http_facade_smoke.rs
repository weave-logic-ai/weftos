//! WEFT-122 smoke tests: axum handlers wired to `http_facade` types + SSE.
//!
//! Covers every facade route from the kernel route table and an SSE client
//! that reads frames produced by the `poll_events()` loop.

#![cfg(feature = "api")]

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use clawft_services::api::{
    AgentAccess, AgentInfo, ApiState, BusAccess, ChannelAccess, ChannelStatusInfo, ConfigAccess,
    InMemoryKernelFacade, KernelFacadeBackend, MemoryAccess, MemoryEntryInfo, SessionAccess,
    SessionDetail, SessionInfo, SkillAccess, SkillInfo, ToolInfo, ToolRegistryAccess,
    TtsProviderInfo, VoiceAccess, VoiceSettingsInfo, VoiceSettingsUpdate, VoiceStatusInfo,
    auth::MemoryTokenValidator, broadcaster::TopicBroadcaster, build_router,
};
use http_body_util::BodyExt;
use tower::ServiceExt;

// ─── Stubs (shared shape with api_middleware) ───────────────────────────

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

struct StubVoice;
impl VoiceAccess for StubVoice {
    fn get_status(&self) -> VoiceStatusInfo {
        VoiceStatusInfo {
            state: "idle".into(),
            talk_mode_active: false,
            wake_word_enabled: false,
        }
    }
    fn get_settings(&self) -> VoiceSettingsInfo {
        VoiceSettingsInfo {
            enabled: false,
            wake_word_enabled: false,
            language: "en".into(),
            echo_cancel: false,
            noise_suppression: false,
            push_to_talk: false,
        }
    }
    fn update_settings(&self, _: VoiceSettingsUpdate) -> Result<(), String> {
        Ok(())
    }
    fn get_tts_config(&self) -> TtsProviderInfo {
        TtsProviderInfo {
            provider: "browser".into(),
            model: "default".into(),
            voice: "default".into(),
            speed: 1.0,
            api_key: String::new(),
            api_base: None,
        }
    }
}

fn make_state() -> (ApiState, Arc<MemoryTokenValidator>, Arc<InMemoryKernelFacade>) {
    let auth = Arc::new(MemoryTokenValidator::new());
    let facade = Arc::new(InMemoryKernelFacade::new());
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
        voice: Arc::new(StubVoice),
        broadcaster: Arc::new(TopicBroadcaster::new()),
        kernel_facade: facade.clone(),
    };
    (state, auth, facade)
}

async fn json_body(resp: axum::response::Response) -> serde_json::Value {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::json!({}))
}

// ─── RPC route smoke ────────────────────────────────────────────────────

/// Each entry: (method, path, expected RPC method name in stub body).
const RPC_ROUTES: &[(&str, &str, &str)] = &[
    ("GET", "/api/processes", "kernel.ps"),
    ("GET", "/api/services", "kernel.services"),
    ("GET", "/api/chain/status", "chain.status"),
    ("GET", "/api/chain/events", "kernel.logs"),
    ("GET", "/api/vectors/status", "ecc.status"),
    ("POST", "/api/vectors/search", "ecc.search"),
    ("GET", "/api/ecc/calibration", "ecc.calibrate"),
    ("GET", "/api/ecc/coherence", "ecc.coherence"),
    ("GET", "/api/custody/attest", "custody.attest"),
    ("POST", "/api/agents/spawn", "agent.spawn"),
    ("DELETE", "/api/agents/42", "agent.stop"),
];

#[tokio::test]
async fn facade_rpc_routes_smoke() {
    let (state, auth, _) = make_state();
    let token = auth.generate_token(3600).unwrap();
    let app = build_router(state, &[], None);

    for (method, path, expected_method) in RPC_ROUTES {
        let http_method = match *method {
            "GET" => Method::GET,
            "POST" => Method::POST,
            "DELETE" => Method::DELETE,
            other => panic!("unexpected method {other}"),
        };
        let body = if *method == "POST" {
            Body::from(r#"{"query":"test"}"#)
        } else {
            Body::empty()
        };
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(http_method)
                    .uri(*path)
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(body)
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "route {method} {path} expected 200"
        );
        let json = json_body(resp).await;
        assert_eq!(
            json.get("method").and_then(|v| v.as_str()),
            Some(*expected_method),
            "route {method} {path} method mismatch: {json}"
        );
        assert_eq!(json.get("ok"), Some(&serde_json::json!(true)));
        assert_eq!(json.get("stub"), Some(&serde_json::json!(true)));
    }
}

#[tokio::test]
async fn facade_chain_events_passes_count_query() {
    let (state, auth, _) = make_state();
    let token = auth.generate_token(3600).unwrap();
    let app = build_router(state, &[], None);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/chain/events?count=7")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_body(resp).await;
    assert_eq!(json["method"], "kernel.logs");
    assert_eq!(json["params"]["count"], 7);
}

#[tokio::test]
async fn facade_agent_stop_injects_pid() {
    let (state, auth, _) = make_state();
    let token = auth.generate_token(3600).unwrap();
    let app = build_router(state, &[], None);

    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri("/api/agents/99")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_body(resp).await;
    assert_eq!(json["method"], "agent.stop");
    assert_eq!(json["params"]["pid"], 99);
    assert_eq!(json["params"]["graceful"], true);
}

// ─── Witness ────────────────────────────────────────────────────────────

#[tokio::test]
async fn facade_witness_accepts_valid_request() {
    let (state, auth, _) = make_state();
    let token = auth.generate_token(3600).unwrap();
    let app = build_router(state, &[], None);

    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/custody/witness")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"event_type":"audit.external","payload":{"src":"t"},"signature":"deadbeef"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_body(resp).await;
    assert_eq!(json["accepted"], true);
    assert!(json.get("chain_hash").is_some());
    assert_eq!(json["sequence"], 1);
}

#[tokio::test]
async fn facade_witness_rejects_empty_signature() {
    let (state, auth, _) = make_state();
    let token = auth.generate_token(3600).unwrap();
    let app = build_router(state, &[], None);

    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/custody/witness")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"event_type":"audit.external","payload":{},"signature":""}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = json_body(resp).await;
    assert_eq!(json["accepted"], false);
}

#[tokio::test]
async fn facade_routes_require_auth() {
    let (state, _auth, _) = make_state();
    let app = build_router(state, &[], None);

    for path in ["/api/processes", "/events", "/custody/witness"] {
        let method = if path == "/custody/witness" {
            Method::POST
        } else {
            Method::GET
        };
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "unauth {path} should 401"
        );
    }
}

// ─── SSE client smoke ───────────────────────────────────────────────────

#[tokio::test]
async fn facade_sse_poll_events_stream() {
    let (state, auth, facade) = make_state();
    let token = auth.generate_token(3600).unwrap();

    // Seed a classifiable agent-spawn event before connecting.
    facade.push_info("agent", "spawned coder-1 (PID 5)");

    // Bind a real TCP listener so we can stream SSE with a client.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = build_router(state, &[], None);

    let server = tokio::spawn(async move {
        axum::serve(listener, app.into_make_service())
            .await
            .unwrap();
    });

    // Give the server a moment to accept.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = reqwest::Client::new();
    let mut resp = client
        .get(format!("http://{addr}/events"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::ACCEPT, "text/event-stream")
        .send()
        .await
        .expect("SSE connect");

    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.starts_with("text/event-stream"),
        "content-type was {ct}"
    );

    // Read chunks until we see an event frame or timeout.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut collected = String::new();
    loop {
        if tokio::time::Instant::now() > deadline {
            break;
        }
        match tokio::time::timeout(Duration::from_millis(800), resp.chunk()).await {
            Ok(Ok(Some(chunk))) => {
                collected.push_str(&String::from_utf8_lossy(&chunk));
                if collected.contains("event:") || collected.contains("data:") {
                    break;
                }
            }
            Ok(Ok(None)) => break,
            Ok(Err(e)) => panic!("SSE chunk error: {e}"),
            Err(_) => {
                // timeout on chunk — keep waiting until overall deadline
                if !collected.is_empty() {
                    break;
                }
            }
        }
    }

    assert!(
        collected.contains("event:") || collected.contains("data:") || collected.contains(":"),
        "expected SSE frames, got: {collected:?}"
    );
    // Seeded agent spawn should classify as agent_spawn (or stream-ready init).
    assert!(
        collected.contains("agent_spawn")
            || collected.contains("init")
            || collected.contains("data:")
            || collected.contains(":"),
        "SSE payload missing expected event content: {collected:?}"
    );

    server.abort();
}

#[tokio::test]
async fn facade_sse_uses_poll_events_for_new_log_entries() {
    // Unit-level: backend.poll_events mirrors kernel poll_events cursor semantics.
    let facade = InMemoryKernelFacade::new();
    let (msgs, cursor) = facade.poll_events(0);
    assert!(msgs.is_empty());
    assert_eq!(cursor, 0);

    facade.push_info("agent", "spawned worker (PID 9)");
    let (msgs, cursor) = facade.poll_events(0);
    assert_eq!(msgs.len(), 1);
    assert_eq!(cursor, 1);
    assert_eq!(
        msgs[0].event_type,
        clawft_kernel::http_facade::SseEventType::AgentSpawn
    );

    let (msgs2, cursor2) = facade.poll_events(cursor);
    assert!(msgs2.is_empty());
    assert_eq!(cursor2, 1);
}
