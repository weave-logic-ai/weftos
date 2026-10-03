//! ADR-102 card api-playground-03: gateway routes over `DaemonKernelFacade`
//! against a fake daemon on a temp UDS speaking the clawft-rpc line protocol.

#![cfg(all(feature = "api", unix))]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use clawft_services::api::{
    AgentAccess, AgentInfo, ApiState, BusAccess, ChannelAccess, ChannelStatusInfo, ConfigAccess,
    DaemonKernelFacade, MemoryAccess, MemoryEntryInfo, SessionAccess, SessionDetail, SessionInfo,
    SkillAccess, SkillInfo, ToolInfo, ToolRegistryAccess, TtsProviderInfo, VoiceAccess,
    VoiceSettingsInfo, VoiceSettingsUpdate, VoiceStatusInfo, auth::MemoryTokenValidator,
    broadcaster::TopicBroadcaster, build_router,
};
use http_body_util::BodyExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tower::ServiceExt;

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

fn make_state(socket: &std::path::Path) -> (ApiState, Arc<MemoryTokenValidator>) {
    let auth = Arc::new(MemoryTokenValidator::new());
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
        kernel_facade: Arc::new(DaemonKernelFacade::with_socket(socket)),
    };
    (state, auth)
}

/// Fake daemon: one JSON request line in, one JSON response line out.
fn spawn_fake_daemon(path: &std::path::Path) {
    let listener = UnixListener::bind(path).unwrap();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (r, mut w) = stream.into_split();
                let mut line = String::new();
                BufReader::new(r).read_line(&mut line).await.unwrap();
                let req: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
                let result = match req["method"].as_str().unwrap() {
                    "kernel.ps" => serde_json::json!([{"pid": 1, "agent_id": "kernel"}]),
                    "chain.status" => serde_json::json!({"height": 99, "healthy": true}),
                    other => serde_json::json!({ "echo": other }),
                };
                let resp = serde_json::json!({"ok": true, "result": result});
                w.write_all(format!("{resp}\n").as_bytes()).await.unwrap();
            });
        }
    });
}

async fn get(app: axum::Router, token: &str, path: &str) -> (StatusCode, serde_json::Value) {
    let resp = app
        .oneshot(
            Request::builder()
                .uri(path)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or_default())
}

#[tokio::test]
async fn processes_and_chain_status_return_daemon_data() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("kernel.sock");
    spawn_fake_daemon(&sock);
    let (state, auth) = make_state(&sock);
    let token = auth.generate_token(3600).unwrap();
    let app = build_router(state, &[], None);

    let (status, body) = get(app.clone(), &token, "/api/processes").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body[0]["agent_id"], "kernel");

    let (status, body) = get(app, &token, "/api/chain/status").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["height"], 99);
}

#[tokio::test]
async fn daemon_down_returns_503_without_socket_path() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("absent.sock");
    let (state, auth) = make_state(&sock);
    let token = auth.generate_token(3600).unwrap();
    let app = build_router(state, &[], None);

    let (status, body) = get(app, &token, "/api/processes").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"], "daemon unavailable");
    assert!(!body.to_string().contains("absent.sock"));
    assert_eq!(body["remedy"], "weaver kernel start");
}

#[tokio::test]
async fn coherence_route_is_501() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("kernel.sock");
    spawn_fake_daemon(&sock);
    let (state, auth) = make_state(&sock);
    let token = auth.generate_token(3600).unwrap();
    let (status, _) = get(build_router(state, &[], None), &token, "/api/ecc/coherence").await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
}

/// Host guard is wired through `serve()` on loopback listeners.
#[tokio::test]
async fn serve_pins_host_header_on_loopback() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _auth) = make_state(&dir.path().join("absent.sock"));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        clawft_services::api::serve(listener, state, &[], None, async {
            let _ = stop_rx.await;
        })
        .await
    });

    async fn status_with_host(addr: std::net::SocketAddr, host: &str) -> String {
        use tokio::io::AsyncReadExt;
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        let req = format!("GET /api/nothing HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
        s.write_all(req.as_bytes()).await.unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        out.lines().next().unwrap_or_default().to_string()
    }

    assert!(status_with_host(addr, "evil.example").await.contains("421"));
    // Allowed host passes the guard (unauthenticated, so 401 rather than 421).
    let ok = status_with_host(addr, "localhost").await;
    assert!(!ok.contains("421"), "{ok}");

    let _ = stop_tx.send(());
    let _ = server.await;
}
