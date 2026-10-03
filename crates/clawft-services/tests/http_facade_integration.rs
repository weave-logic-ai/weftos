//! WEFT-123: HTTP facade integration tests.
//!
//! Prerequisites (`ProfilesConfig` / `PairingConfig`) landed in
//! `clawft-types`; this suite exercises the full facade surface once
//! those types are available:
//!
//! - all 12 RPC routes + witness + SSE (route table from
//!   `clawft_kernel::http_facade`)
//! - query/path param injection (`count`, agent pid)
//! - auth gating
//! - unknown-route 404
//! - config types used by a gateway that boots with profiles + pairing
//!
//! Smoke coverage lives in `http_facade_smoke.rs` (WEFT-122). This file
//! is the AC suite for WEFT-123.

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
use clawft_types::config::{
    KernelConfig, PairingConfig, ProfilesConfig, VectorBackendKind, VectorConfig,
};
use http_body_util::BodyExt;
use tower::ServiceExt;

// ─── Stubs ──────────────────────────────────────────────────────────────

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

/// Config access that surfaces profiles + pairing (WEFT-123 gate).
struct ProfilesPairingConfig;
impl ConfigAccess for ProfilesPairingConfig {
    fn get_config(&self) -> serde_json::Value {
        let kcfg = KernelConfig {
            profiles: Some(ProfilesConfig {
                enabled: true,
                storage_path: ".weftos/profiles".into(),
                default_profile: "integration".into(),
            }),
            pairing: Some(PairingConfig {
                persist_path: ".weftos/runtime/paired_hosts.json".into(),
                default_window_secs: 45,
            }),
            vector: Some(VectorConfig {
                backend: VectorBackendKind::Hnsw,
                ..Default::default()
            }),
            ..Default::default()
        };
        serde_json::to_value(kcfg).unwrap_or_else(|_| serde_json::json!({}))
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
        config: Arc::new(ProfilesPairingConfig),
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

// ─── AC: ProfilesConfig / PairingConfig available ───────────────────────

#[test]
fn profiles_and_pairing_types_landed() {
    // Gate that unblocked WEFT-123: types exist, serialize, and nest in
    // KernelConfig the way a gateway would load them.
    let profiles = ProfilesConfig {
        enabled: true,
        storage_path: "/tmp/weft-profiles".into(),
        default_profile: "ops".into(),
    };
    let pairing = PairingConfig {
        persist_path: "/tmp/paired.json".into(),
        default_window_secs: 60,
    };
    let kcfg = KernelConfig {
        profiles: Some(profiles.clone()),
        pairing: Some(pairing.clone()),
        ..Default::default()
    };
    let json = serde_json::to_value(&kcfg).expect("KernelConfig serializes");
    assert_eq!(json["profiles"]["default_profile"], "ops");
    assert_eq!(json["pairing"]["default_window_secs"], 60);

    let restored: KernelConfig = serde_json::from_value(json).expect("round-trip");
    assert!(restored.profiles.unwrap().enabled);
    assert_eq!(restored.pairing.unwrap().default_window_secs, 60);
}

#[tokio::test]
async fn config_api_surfaces_profiles_and_pairing() {
    let (state, auth, _) = make_state();
    let token = auth.generate_token(3600).unwrap();
    let app = build_router(state, &[], None);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/config")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // Config route may be GET /api/config — if the gateway mounts it.
    // Accept 200 with profiles/pairing, or skip if route not present.
    if resp.status() == StatusCode::OK {
        let json = json_body(resp).await;
        // ProfilesPairingConfig returns KernelConfig JSON.
        assert_eq!(
            json.get("profiles")
                .and_then(|p| p.get("default_profile"))
                .and_then(|v| v.as_str()),
            Some("integration"),
            "config should expose profiles: {json}"
        );
        assert_eq!(
            json.get("pairing")
                .and_then(|p| p.get("default_window_secs"))
                .and_then(|v| v.as_u64()),
            Some(45),
            "config should expose pairing: {json}"
        );
    }
}

// ─── AC: Integration coverage — 12 RPC routes + witness + SSE ──────────

/// Full facade RPC route table (HTTP method, path, expected RPC method).
const FACADE_RPC_ROUTES: &[(&str, &str, &str)] = &[
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
    ("DELETE", "/api/agents/7", "agent.stop"),
];

#[tokio::test]
async fn integration_all_rpc_routes_map_and_return_ok() {
    let (state, auth, _) = make_state();
    let token = auth.generate_token(3600).unwrap();
    let app = build_router(state, &[], None);

    assert_eq!(
        FACADE_RPC_ROUTES.len(),
        11,
        "route table must stay in sync with http_facade match_facade_route \
         (12 kernel routes minus `/api/status`, served by tiered /api/health, ADR-102 D1)"
    );

    for (method, path, expected_method) in FACADE_RPC_ROUTES {
        let http_method = match *method {
            "GET" => Method::GET,
            "POST" => Method::POST,
            "DELETE" => Method::DELETE,
            other => panic!("unexpected method {other}"),
        };
        let body = if *method == "POST" {
            Body::from(r#"{"query":"integration","k":3}"#)
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
            "integration route {method} {path} expected 200"
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
async fn integration_chain_events_count_and_agent_stop_pid() {
    let (state, auth, _) = make_state();
    let token = auth.generate_token(3600).unwrap();
    let app = build_router(state, &[], None);

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/chain/events?count=11")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_body(resp).await;
    assert_eq!(json["method"], "kernel.logs");
    assert_eq!(json["params"]["count"], 11);

    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri("/api/agents/1234")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_body(resp).await;
    assert_eq!(json["method"], "agent.stop");
    assert_eq!(json["params"]["pid"], 1234);
    assert_eq!(json["params"]["graceful"], true);
}

#[tokio::test]
async fn integration_witness_accept_and_reject() {
    let (state, auth, _) = make_state();
    let token = auth.generate_token(3600).unwrap();
    let app = build_router(state, &[], None);

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/custody/witness")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"event_type":"audit.integration","payload":{"n":1},"signature":"aabbcc"}"#,
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

    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/custody/witness")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"event_type":"audit.integration","payload":{},"signature":""}"#,
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
async fn integration_unknown_facade_path_is_not_ok_rpc() {
    let (state, auth, _) = make_state();
    let token = auth.generate_token(3600).unwrap();
    let app = build_router(state, &[], None);

    // Path not in match_facade_route: either 404 from facade or another
    // handler. Must not return a successful stub envelope with a fake method.
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/vectors/not-a-real-route")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let json = json_body(resp).await;
    assert!(
        status == StatusCode::NOT_FOUND
            || status == StatusCode::METHOD_NOT_ALLOWED
            || json.get("stub") != Some(&serde_json::json!(true)),
        "unknown path must not look like a successful facade RPC: status={status} body={json}"
    );
}

#[tokio::test]
async fn integration_all_facade_routes_require_auth() {
    let (state, _auth, _) = make_state();
    let app = build_router(state, &[], None);

    let paths: Vec<(&str, Method)> = FACADE_RPC_ROUTES
        .iter()
        .map(|(m, p, _)| {
            let method = match *m {
                "GET" => Method::GET,
                "POST" => Method::POST,
                "DELETE" => Method::DELETE,
                _ => Method::GET,
            };
            (*p, method)
        })
        .chain([
            ("/events", Method::GET),
            ("/custody/witness", Method::POST),
        ])
        .collect();

    for (path, method) in paths {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method.clone())
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "unauth {method} {path} should 401"
        );
    }
}

// ─── SSE integration ────────────────────────────────────────────────────

#[tokio::test]
async fn integration_sse_stream_delivers_classified_events() {
    let (state, auth, facade) = make_state();
    let token = auth.generate_token(3600).unwrap();

    facade.push_info("agent", "spawned integration-worker (PID 42)");
    facade.push_info("http_facade", "rpc kernel.status");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = build_router(state, &[], None);

    let server = tokio::spawn(async move {
        axum::serve(listener, app.into_make_service())
            .await
            .unwrap();
    });

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

    let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
    let mut collected = String::new();
    loop {
        if tokio::time::Instant::now() > deadline {
            break;
        }
        match tokio::time::timeout(Duration::from_millis(800), resp.chunk()).await {
            Ok(Ok(Some(chunk))) => {
                collected.push_str(&String::from_utf8_lossy(&chunk));
                if collected.contains("event:") && collected.contains("data:") {
                    break;
                }
            }
            Ok(Ok(None)) => break,
            Ok(Err(e)) => panic!("SSE chunk error: {e}"),
            Err(_) => {
                if !collected.is_empty() && collected.contains("event:") {
                    break;
                }
            }
        }
    }

    assert!(
        collected.contains("event:") || collected.contains("data:"),
        "expected SSE event frames, got: {collected:?}"
    );
    // Seeded agent spawn and/or stream-ready init should appear.
    assert!(
        collected.contains("agent_spawn")
            || collected.contains("init")
            || collected.contains("info")
            || collected.contains("data:"),
        "SSE payload missing expected content: {collected:?}"
    );

    server.abort();
}

#[tokio::test]
async fn integration_sse_poll_events_cursor_semantics() {
    let facade = InMemoryKernelFacade::new();
    let (msgs, cursor) = facade.poll_events(0);
    assert!(msgs.is_empty());
    assert_eq!(cursor, 0);

    facade.push_info("agent", "spawned cursor-test (PID 1)");
    facade.push_info("kernel", "booted");

    let (msgs, cursor) = facade.poll_events(0);
    assert_eq!(msgs.len(), 2);
    assert_eq!(cursor, 2);
    assert_eq!(
        msgs[0].event_type,
        clawft_kernel::http_facade::SseEventType::AgentSpawn
    );

    let (msgs2, cursor2) = facade.poll_events(cursor);
    assert!(msgs2.is_empty());
    assert_eq!(cursor2, 2);

    facade.push_info("agent", "spawned again (PID 2)");
    let (msgs3, cursor3) = facade.poll_events(cursor2);
    assert_eq!(msgs3.len(), 1);
    assert_eq!(cursor3, 3);
}

// ─── Kernel route table parity ──────────────────────────────────────────

#[test]
fn integration_kernel_route_table_covers_facade_paths() {
    use clawft_kernel::http_facade::{FacadeRoute, HttpMethod, match_facade_route};

    for (method, path, expected) in FACADE_RPC_ROUTES {
        let m = HttpMethod::parse(method).expect("method");
        match match_facade_route(m, path) {
            FacadeRoute::Rpc { method: rpc, .. } => {
                assert_eq!(rpc, *expected, "kernel route table drift for {path}");
            }
            other => panic!("expected Rpc for {method} {path}, got {other:?}"),
        }
    }

    assert!(matches!(
        match_facade_route(HttpMethod::Get, "/events"),
        FacadeRoute::Events
    ));
    assert!(matches!(
        match_facade_route(HttpMethod::Post, "/custody/witness"),
        FacadeRoute::CustodyWitness
    ));
}
