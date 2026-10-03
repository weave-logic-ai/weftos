//! Gateway auth and health behaviour (ADR-102 D1, D3, D5).
//!
//! Tokens come from the daemon (`auth.token.*` RPCs, faked here over a unix
//! socket); the gateway keeps no token store and serves no mint route.

#![cfg(feature = "api")]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use clawft_services::api::{
    AgentAccess, AgentInfo, ApiState, BusAccess, ChannelAccess, ChannelStatusInfo, ConfigAccess,
    DaemonKernelFacade, InMemoryKernelFacade, MemoryAccess, MemoryEntryInfo, SessionAccess, SessionDetail, SessionInfo,
    SkillAccess, SkillInfo, ToolInfo, ToolRegistryAccess, TtsProviderInfo, VoiceAccess,
    VoiceSettingsInfo, VoiceSettingsUpdate, VoiceStatusInfo, auth::{DaemonTokenValidator, MemoryTokenValidator, TokenValidator},
    broadcaster::TopicBroadcaster, build_router,
};
use tower::ServiceExt;

// ─── Stub access impls ──────────────────────────────────────────────────

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

/// Config that carries secrets and credentialed URLs, to prove the health
/// view never echoes them.
struct StubConfig;
impl ConfigAccess for StubConfig {
    fn get_config(&self) -> serde_json::Value {
        serde_json::json!({
            "providers": {
                "anthropic": {
                    "api_key_set": true,
                    "api_key": "sk-secret-do-not-leak",
                    "api_base": "https://user:hunter2@llm.example.test/v1",
                },
                "openai": { "api_key_set": false, "api_base": "" },
            },
        })
    }
    fn save_config(&self, _: serde_json::Value) -> Result<(), String> {
        Ok(())
    }
}

struct StubChannels;
impl ChannelAccess for StubChannels {
    fn list_channels(&self) -> Vec<ChannelStatusInfo> {
        vec![ChannelStatusInfo {
            name: "web".into(),
            channel_type: "web".into(),
            status: "connected".into(),
            message_count: 0,
            last_activity: None,
            routes_to: None,
        }]
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


fn state_with(
    auth: Arc<dyn TokenValidator>,
    facade: Arc<dyn clawft_services::api::KernelFacadeBackend>,
) -> ApiState {
    ApiState {
        tools: Arc::new(StubTools),
        sessions: Arc::new(StubSessions),
        agents: Arc::new(StubAgents),
        bus: Arc::new(StubBus),
        auth,
        skills: Arc::new(StubSkills),
        memory: Arc::new(StubMemory),
        config: Arc::new(StubConfig),
        channels: Arc::new(StubChannels),
        voice: Arc::new(StubVoice),
        broadcaster: Arc::new(TopicBroadcaster::new()),
        kernel_facade: facade,
        routing_history: Arc::new(
            clawft_core::pipeline::decision_history::RoutingDecisionHistory::new(),
        ),
        rate_limiter: Arc::new(clawft_core::pipeline::rate_limiter::RateLimiter::new(60, 0)),
        health_cache: Default::default(),
        mcp: None,
    }
}

async fn get(app: &axum::Router, uri: &str, bearer: Option<&str>) -> (StatusCode, serde_json::Value) {
    let mut req = Request::builder().uri(uri);
    if let Some(t) = bearer {
        req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let resp = app.clone().oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
}

async fn post(app: &axum::Router, uri: &str, bearer: &str) -> StatusCode {
    app.clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

// ─── Tiered /api/health (in-memory kernel facade) ───────────────────────

fn memory_app() -> (axum::Router, Arc<MemoryTokenValidator>) {
    let auth = Arc::new(MemoryTokenValidator::new());
    let state = state_with(auth.clone(), Arc::new(InMemoryKernelFacade::new()));
    (build_router(state, &[], None), auth)
}

#[tokio::test]
async fn health_without_token_is_status_only() {
    let (app, _auth) = memory_app();
    let (code, body) = get(&app, "/api/health", None).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body, serde_json::json!({ "status": "ok" }));
}

#[tokio::test]
async fn health_with_invalid_token_is_still_status_only() {
    let (app, _auth) = memory_app();
    let (code, body) = get(&app, "/api/health", Some("not-a-token")).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body, serde_json::json!({ "status": "ok" }));
}

#[tokio::test]
async fn health_with_token_has_every_section() {
    let (app, auth) = memory_app();
    let token = auth.generate_token(3600).unwrap();
    let (code, body) = get(&app, "/api/health", Some(&token)).await;
    assert_eq!(code, StatusCode::OK);
    for k in [
        "status", "version", "uptime_secs", "build", "gateway", "daemon", "kernel", "chain",
        "mcp", "channels", "providers", "token",
    ] {
        assert!(body.get(k).is_some(), "missing section {k}: {body}");
    }
    assert_eq!(body["channels"][0]["name"], "web");
    assert!(body["token"]["id"].is_string());
    assert!(body["token"]["expires_at"].is_string());
    let providers = body["providers"].as_array().unwrap();
    let anthropic = providers.iter().find(|p| p["name"] == "anthropic").unwrap();
    assert_eq!(anthropic["configured"], true);
}

/// Leak test: nothing secret-shaped appears in the tokened document.
#[tokio::test]
async fn health_with_token_leaks_no_secrets() {
    let (app, auth) = memory_app();
    let token = auth.generate_token(3600).unwrap();
    let (_, body) = get(&app, "/api/health", Some(&token)).await;
    let text = body.to_string();
    for needle in ["sk-secret-do-not-leak", "hunter2", "llm.example.test", "api_key", "api_base", &token] {
        assert!(!text.contains(needle), "leaked {needle}: {text}");
    }
}

#[tokio::test]
async fn status_stub_route_is_removed() {
    let (app, auth) = memory_app();
    let token = auth.generate_token(3600).unwrap();
    let (code, _) = get(&app, "/api/status", Some(&token)).await;
    assert_eq!(code, StatusCode::NOT_FOUND);
}

/// Every route except health refuses an anonymous caller.
#[tokio::test]
async fn nothing_but_health_is_anonymous() {
    let (app, _auth) = memory_app();
    for path in [
        "/api/agents", "/api/sessions", "/api/tools", "/api/config", "/api/skills",
        "/api/memory", "/api/channels", "/api/processes", "/api/services", "/api/chain/status",
        "/api/monitoring/token-usage", "/events",
    ] {
        let (code, body) = get(&app, path, None).await;
        assert_eq!(code, StatusCode::UNAUTHORIZED, "{path}");
        assert_eq!(body, serde_json::Value::Null, "{path} leaked a body");
    }
}

/// `/ws` accepts the token as a query parameter (browsers cannot set the
/// header on an upgrade) but rejects a bad one with 401.
#[tokio::test]
async fn ws_query_token_is_validated() {
    let (app, auth) = memory_app();
    let token = auth.generate_token(3600).unwrap();
    let (bad, _) = get(&app, "/ws?token=nope", None).await;
    assert_eq!(bad, StatusCode::UNAUTHORIZED);
    let (none, _) = get(&app, "/ws", None).await;
    assert_eq!(none, StatusCode::UNAUTHORIZED);
    // A good token passes auth; the non-upgrade request then fails in the
    // handler, which is anything but 401.
    let (good, _) = get(&app, &format!("/ws?token={token}"), None).await;
    assert_ne!(good, StatusCode::UNAUTHORIZED);
}

// ─── Daemon-backed validation (fake daemon over a unix socket) ──────────

#[cfg(unix)]
mod daemon {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;

    /// A token the fake daemon knows: (secret, id, scope, project).
    type Known = (String, String, String, Option<String>);

    #[derive(Default)]
    struct Fake {
        tokens: Mutex<Vec<Known>>,
        revoked: Mutex<Vec<String>>,
        validate_calls: AtomicUsize,
        revoke_auth: Mutex<Vec<(String, Option<String>)>>,
        verify_calls: AtomicUsize,
        up: AtomicBool,
    }

    fn info(id: &str, scope: &str, project: &Option<String>) -> serde_json::Value {
        serde_json::json!({
            "id": id, "label": "playground",
            "issued_at": "2026-01-01T00:00:00+00:00",
            "expires_at": "2099-01-01T00:00:00+00:00",
            "scope": scope, "project": project,
        })
    }

    fn spawn(path: &std::path::Path) -> Arc<Fake> {
        let fake = Arc::new(Fake::default());
        fake.up.store(true, Ordering::SeqCst);
        let listener = UnixListener::bind(path).unwrap();
        let f = fake.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let f = f.clone();
                tokio::spawn(async move {
                    let (r, mut w) = stream.into_split();
                    let mut line = String::new();
                    BufReader::new(r).read_line(&mut line).await.unwrap();
                    let req: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
                    let params = &req["params"];
                    let resp = match req["method"].as_str().unwrap() {
                        "auth.token.validate" => {
                            f.validate_calls.fetch_add(1, Ordering::SeqCst);
                            let secret = params["token"].as_str().unwrap_or("");
                            let revoked = f.revoked.lock().unwrap().clone();
                            let hit = f
                                .tokens
                                .lock()
                                .unwrap()
                                .iter()
                                .find(|(s, id, _, _)| s == secret && !revoked.contains(id))
                                .cloned();
                            match hit {
                                Some((_, id, scope, project)) => clawft_rpc::Response::success(
                                    serde_json::json!({ "valid": true, "token": info(&id, &scope, &project) }),
                                ),
                                None => clawft_rpc::Response::success(serde_json::json!({ "valid": false })),
                            }
                        }
                        "auth.token.revoke" => {
                            let id = params["id"].as_str().unwrap().to_owned();
                            f.revoke_auth.lock().unwrap().push((id.clone(), req["auth"].as_str().map(str::to_owned)));
                            f.revoked.lock().unwrap().push(id.clone());
                            clawft_rpc::Response::success(serde_json::json!({ "revoked": true, "id": id }))
                        }
                        "kernel.status" => clawft_rpc::Response::success(serde_json::json!({
                            "state": "running", "uptime_secs": 12.5,
                            "build": { "sha": "abc1234", "timestamp": "t", "version": "0.0.0-test" },
                            "handshake": { "runtime_root": "/home/someone/.weftos", "user_id": "u-secret", "key_id": "k-secret" },
                        })),
                        "kernel.ps" => clawft_rpc::Response::success(serde_json::json!([
                            {"pid": 1, "agent_id": "kernel", "state": "running", "memory_bytes": 9, "cpu_time_ms": 9, "parent_pid": null}
                        ])),
                        "kernel.services" => clawft_rpc::Response::success(serde_json::json!([
                            {"name": "chain", "service_type": "core", "state": "running", "health": "ok", "pid": 2, "restarts": 0, "uptime_ms": 1, "detail": "/secret/path"}
                        ])),
                        "chain.status" => clawft_rpc::Response::success(serde_json::json!({
                            "chain_id": 0, "sequence": 41, "event_count": 42, "checkpoint_count": 1,
                            "events_since_checkpoint": 2, "last_hash": "deadbeef"
                        })),
                        "chain.verify" => {
                            f.verify_calls.fetch_add(1, Ordering::SeqCst);
                            clawft_rpc::Response::success(serde_json::json!({
                                "valid": true, "event_count": 42, "errors": ["/secret/path broke"], "signature_verified": true
                            }))
                        }
                        other => clawft_rpc::Response::error(format!("unknown {other}")),
                    };
                    let mut out = serde_json::to_string(&resp).unwrap();
                    out.push('\n');
                    w.write_all(out.as_bytes()).await.unwrap();
                });
            }
        });
        fake
    }

    fn app(sock: &std::path::Path, ttl: std::time::Duration) -> axum::Router {
        let facade = Arc::new(DaemonKernelFacade::with_socket(sock).with_timeout(std::time::Duration::from_secs(2)));
        let auth = Arc::new(DaemonTokenValidator::new(facade.clone()).with_cache_ttl(ttl));
        build_router(state_with(auth, facade), &[], None)
    }

    fn add(fake: &Fake, secret: &str, id: &str, scope: &str, project: Option<&str>) {
        fake.tokens.lock().unwrap().push((secret.into(), id.into(), scope.into(), project.map(str::to_owned)));
    }

    const TTL: std::time::Duration = std::time::Duration::from_secs(30);

    #[tokio::test]
    async fn issued_token_works_and_unknown_token_is_401() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("k.sock");
        let fake = spawn(&sock);
        add(&fake, "wft_good", "id-good", "owner", None);
        let app = app(&sock, TTL);

        assert_eq!(get(&app, "/api/agents", Some("wft_good")).await.0, StatusCode::OK);
        assert_eq!(get(&app, "/api/agents", Some("wft_nope")).await.0, StatusCode::UNAUTHORIZED);
        assert_eq!(get(&app, "/api/agents", None).await.0, StatusCode::UNAUTHORIZED);
    }

    /// A project token (ADR-103) is a child kernel's credential, not an
    /// operator credential: the gateway refuses it, as it does a token that
    /// merely carries a project claim.
    #[tokio::test]
    async fn project_scoped_tokens_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("k.sock");
        let fake = spawn(&sock);
        add(&fake, "wft_proj", "id-proj", "project", Some("01HZXAAAAAAAAAAAAAAAAAAAAA"));
        add(&fake, "wft_claim", "id-claim", "owner", Some("01HZXAAAAAAAAAAAAAAAAAAAAA"));
        let app = app(&sock, TTL);

        assert_eq!(get(&app, "/api/agents", Some("wft_proj")).await.0, StatusCode::UNAUTHORIZED);
        assert_eq!(get(&app, "/api/agents", Some("wft_claim")).await.0, StatusCode::UNAUTHORIZED);
        // And they do not unlock the detailed health view.
        let (_, body) = get(&app, "/api/health", Some("wft_proj")).await;
        assert_eq!(body, serde_json::json!({ "status": "ok" }));
    }

    #[tokio::test]
    async fn positive_results_are_cached_for_the_ttl() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("k.sock");
        let fake = spawn(&sock);
        add(&fake, "wft_good", "id-good", "owner", None);
        let app = app(&sock, TTL);

        for _ in 0..5 {
            assert_eq!(get(&app, "/api/agents", Some("wft_good")).await.0, StatusCode::OK);
        }
        assert_eq!(fake.validate_calls.load(Ordering::SeqCst), 1);
    }

    /// A token revoked at the daemon (e.g. `weft token revoke`) is refused
    /// once the positive cache lapses, so the worst case is the TTL.
    #[tokio::test]
    async fn daemon_side_revoke_is_seen_after_the_cache_lapses() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("k.sock");
        let fake = spawn(&sock);
        add(&fake, "wft_good", "id-good", "owner", None);
        let app = app(&sock, std::time::Duration::from_millis(150));

        assert_eq!(get(&app, "/api/agents", Some("wft_good")).await.0, StatusCode::OK);
        fake.revoked.lock().unwrap().push("id-good".into());
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        assert_eq!(get(&app, "/api/agents", Some("wft_good")).await.0, StatusCode::UNAUTHORIZED);
    }

    /// Revoking through the gateway bypasses the cache at once and forwards
    /// the caller's own token id to the daemon.
    #[tokio::test]
    async fn gateway_revoke_forwards_to_daemon_and_bypasses_cache() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("k.sock");
        let fake = spawn(&sock);
        add(&fake, "wft_good", "id-good", "owner", None);
        add(&fake, "wft_other", "id-other", "owner", None);
        let app = app(&sock, TTL);

        assert_eq!(get(&app, "/api/agents", Some("wft_good")).await.0, StatusCode::OK);
        assert_eq!(post(&app, "/api/auth/revoke", "wft_good").await, StatusCode::NO_CONTENT);

        let calls = fake.revoke_auth.lock().unwrap().clone();
        assert_eq!(calls, vec![("id-good".to_owned(), Some("admin".to_owned()))]);
        // Immediately 401, well inside the 30 s cache window.
        assert_eq!(get(&app, "/api/agents", Some("wft_good")).await.0, StatusCode::UNAUTHORIZED);
        // Another caller's token is untouched.
        assert_eq!(get(&app, "/api/agents", Some("wft_other")).await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn revoke_requires_a_token() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("k.sock");
        let fake = spawn(&sock);
        let app = app(&sock, TTL);
        assert_eq!(post(&app, "/api/auth/revoke", "wft_unknown").await, StatusCode::UNAUTHORIZED);
        assert!(fake.revoke_auth.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn daemon_down_is_503_with_remedy_not_401() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(&dir.path().join("absent.sock"), TTL);

        let (code, body) = get(&app, "/api/agents", Some("wft_any")).await;
        assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["remedy"], "start the daemon: weft kernel start");
        assert!(!body.to_string().contains("absent.sock"));
        // No credential at all is still 401: nothing to check.
        assert_eq!(get(&app, "/api/agents", None).await.0, StatusCode::UNAUTHORIZED);
        // The daemon being down never grants access to the revoke route.
        assert_eq!(post(&app, "/api/auth/revoke", "wft_any").await, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn anonymous_health_is_degraded_503_when_daemon_is_down() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(&dir.path().join("absent.sock"), TTL);
        let (code, body) = get(&app, "/api/health", None).await;
        assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body, serde_json::json!({ "status": "degraded" }));
    }

    #[tokio::test]
    async fn tokened_health_reports_live_daemon_data_and_allow_lists_fields() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("k.sock");
        let fake = spawn(&sock);
        add(&fake, "wft_good", "id-good", "owner", None);
        let app = app(&sock, TTL);

        let (code, body) = get(&app, "/api/health", Some("wft_good")).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(body["daemon"]["reachable"], true);
        assert_eq!(body["daemon"]["version"], "0.0.0-test");
        assert_eq!(body["daemon"]["version_skew"], true);
        assert_eq!(body["daemon"]["git_sha"], "abc1234");
        assert_eq!(body["kernel"]["processes"][0]["agent_id"], "kernel");
        assert_eq!(body["kernel"]["services"][0]["name"], "chain");
        assert_eq!(body["chain"]["sequence"], 41);
        assert_eq!(body["chain"]["head"], "deadbeef");
        assert_eq!(body["chain"]["verify"]["valid"], true);
        assert_eq!(body["chain"]["verify"]["error_count"], 1);
        assert_eq!(body["token"]["id"], "id-good");
        assert_eq!(body["token"]["label"], "playground");

        // Daemon fields outside the allow-list never reach the response.
        let text = body.to_string();
        for needle in ["/home/someone", "u-secret", "k-secret", "/secret/path", "memory_bytes", "wft_good"] {
            assert!(!text.contains(needle), "leaked {needle}: {text}");
        }
    }

    #[tokio::test]
    async fn chain_verify_is_cached_between_requests() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("k.sock");
        let fake = spawn(&sock);
        add(&fake, "wft_good", "id-good", "owner", None);
        let app = app(&sock, TTL);
        for _ in 0..3 {
            assert_eq!(get(&app, "/api/health", Some("wft_good")).await.0, StatusCode::OK);
        }
        assert_eq!(fake.verify_calls.load(Ordering::SeqCst), 1);
    }
}

// ─── /mcp mounted in the gateway (ADR-102 D2) ───────────────────────────

mod mcp {
    use super::*;
    use async_trait::async_trait;
    use clawft_services::api::mcp_mount::McpMount;
    use clawft_services::mcp::ToolDefinition;
    use clawft_services::mcp::composite::CompositeToolProvider;
    use clawft_services::mcp::middleware::AuditLog;
    use clawft_services::mcp::provider::{CallToolResult, ToolError, ToolProvider};
    use clawft_services::mcp::server::McpServerShell;
    use serde_json::{Value, json};

    struct Echo;

    #[async_trait]
    impl ToolProvider for Echo {
        fn namespace(&self) -> &str {
            ""
        }
        fn list_tools(&self) -> Vec<ToolDefinition> {
            vec![ToolDefinition {
                name: "echo".into(),
                description: "e".into(),
                input_schema: json!({"type": "object"}),
            }]
        }
        async fn call_tool(&self, name: &str, _args: Value) -> Result<CallToolResult, ToolError> {
            Ok(CallToolResult::text(format!("ok:{name}")))
        }
    }

    fn mount() -> Arc<McpMount> {
        let mut composite = CompositeToolProvider::new();
        composite.register(Box::new(Echo));
        let mut shell = McpServerShell::new(composite);
        let audit = AuditLog::new();
        let label = audit.label_handle();
        shell.add_middleware(Box::new(audit));
        Arc::new(McpMount::new(shell, label, "full", 1))
    }

    fn app(mounted: bool) -> (axum::Router, Arc<MemoryTokenValidator>) {
        let auth = Arc::new(MemoryTokenValidator::new());
        let mut state = state_with(auth.clone(), Arc::new(InMemoryKernelFacade::new()));
        if mounted {
            state.mcp = Some(mount());
        }
        (build_router(state, &[], None), auth)
    }

    async fn rpc(app: &axum::Router, bearer: Option<&str>, body: Value) -> (StatusCode, Value) {
        let mut req = Request::builder()
            .method(Method::POST)
            .uri("/mcp")
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(t) = bearer {
            req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        let resp = app
            .clone()
            .oneshot(req.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let code = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (code, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    fn init() -> Value {
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-06-18","capabilities":{},
            "clientInfo":{"name":"t","version":"1"}}})
    }

    #[tokio::test]
    async fn mcp_requires_a_token() {
        let (app, _auth) = app(true);
        for bearer in [None, Some("not-a-token")] {
            let (code, body) = rpc(&app, bearer, init()).await;
            assert_eq!(code, StatusCode::UNAUTHORIZED);
            assert_eq!(body, Value::Null, "no JSON-RPC body for an unauthenticated caller");
        }
        // Other methods on the path are not an anonymous way in either.
        let resp = app
            .clone()
            .oneshot(Request::builder().uri("/mcp").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn initialize_list_and_call_pass_through_with_a_token() {
        let (app, auth) = app(true);
        let token = auth.generate_token(3600).unwrap();

        let (code, v) = rpc(&app, Some(&token), init()).await;
        assert_eq!(code, StatusCode::OK);
        assert!(v["result"]["serverInfo"]["name"].is_string());

        let (_, v) = rpc(&app, Some(&token), json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).await;
        assert_eq!(v["result"]["tools"][0]["name"], "echo");

        let (_, v) = rpc(
            &app,
            Some(&token),
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"echo","arguments":{}}}),
        )
        .await;
        assert!(v.to_string().contains("ok:echo"), "{v}");

        let (code, _) = rpc(&app, Some(&token), json!({"jsonrpc":"2.0","method":"notifications/initialized"})).await;
        assert_eq!(code, StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn malformed_json_is_400_for_an_authenticated_caller() {
        let (app, auth) = app(true);
        let token = auth.generate_token(3600).unwrap();
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/mcp")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from("{not json"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn revoked_token_is_refused_on_mcp() {
        let (app, auth) = app(true);
        let token = auth.generate_token(3600).unwrap();
        assert_eq!(rpc(&app, Some(&token), init()).await.0, StatusCode::OK);
        assert_eq!(post(&app, "/api/auth/revoke", &token).await, StatusCode::NO_CONTENT);
        assert_eq!(rpc(&app, Some(&token), init()).await.0, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn unmounted_gateway_has_no_mcp_route() {
        let (app, auth) = app(false);
        let token = auth.generate_token(3600).unwrap();
        assert_eq!(rpc(&app, Some(&token), init()).await.0, StatusCode::NOT_FOUND);
        assert_eq!(rpc(&app, None, init()).await.0, StatusCode::NOT_FOUND);
        let (_, body) = get(&app, "/api/health", Some(&token)).await;
        assert_eq!(body["mcp"], json!({ "mounted": false }));
    }

    #[tokio::test]
    async fn health_reports_the_mounted_surface_to_token_holders_only() {
        let (app, auth) = app(true);
        let token = auth.generate_token(3600).unwrap();
        let (_, body) = get(&app, "/api/health", Some(&token)).await;
        assert_eq!(
            body["mcp"],
            json!({ "mounted": true, "path": "/mcp", "profile": "full", "tool_count": 1 })
        );
        let (_, anon) = get(&app, "/api/health", None).await;
        assert_eq!(anon, json!({ "status": "ok" }));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn daemon_down_is_503_on_mcp() {
        let dir = tempfile::tempdir().unwrap();
        let facade = Arc::new(
            DaemonKernelFacade::with_socket(dir.path().join("absent.sock"))
                .with_timeout(std::time::Duration::from_secs(1)),
        );
        let auth = Arc::new(DaemonTokenValidator::new(facade.clone()));
        let mut state = state_with(auth, facade);
        state.mcp = Some(mount());
        let app = build_router(state, &[], None);
        let (code, body) = rpc(&app, Some("wft_any"), init()).await;
        assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["remedy"], "start the daemon: weft kernel start");
    }
}
