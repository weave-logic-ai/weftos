//! Project console routes, project-bound read tokens, `/console/` static
//! serving and the tailnet-identity token mint (dashboard project console).

#![cfg(feature = "api")]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use clawft_services::api::{
    AgentAccess, AgentInfo, ApiState, BusAccess, ChannelAccess, ChannelStatusInfo, ConfigAccess,
    MemoryAccess, MemoryEntryInfo, SessionAccess, SessionDetail, SessionInfo,
    SkillAccess, SkillInfo, ToolInfo, ToolRegistryAccess, TtsProviderInfo, VoiceAccess,
    VoiceSettingsInfo, VoiceSettingsUpdate, VoiceStatusInfo, auth::{MemoryTokenValidator, TokenValidator},
    broadcaster::TopicBroadcaster, build_router_with,
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


use std::net::SocketAddr;
use std::sync::Mutex;

use async_trait::async_trait;
use axum::extract::ConnectInfo;
use clawft_kernel::http_facade::{FacadeResponse, SseMessage, WitnessRequest, WitnessResponse};
use clawft_services::api::KernelFacadeBackend;
use clawft_services::api::auth::TokenScope;
use clawft_services::api::console::{
    ConsoleOptions, MintError, Minted, TailnetMint, TailnetWhois, TokenMinter, WhoisError,
};
use serde_json::{Value, json};

const A: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const B: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const C: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";

/// Daemon stand-in with two projects and a snapshot spanning both.
struct Projects {
    calls: Mutex<Vec<String>>,
}

impl Projects {
    fn new() -> Arc<Self> {
        Arc::new(Self { calls: Mutex::new(vec![]) })
    }
}

fn manifest(id: &str) -> Value {
    json!({ "id": id, "name": format!("proj-{id}"), "root": "/srv/p" })
}

#[async_trait]
impl KernelFacadeBackend for Projects {
    async fn call_rpc(&self, method: &str, params: Value) -> FacadeResponse {
        self.calls.lock().unwrap().push(method.to_owned());
        match method {
            "project.list" => FacadeResponse::ok(json!({
                "projects": [manifest(A), manifest(B)],
                "skipped": [{"path": "/home/u/.weftos/projects/x.toml", "reason": "bad"}],
            })),
            "project.show" => match params["id"].as_str() {
                Some(id) if id == A || id == B => FacadeResponse::ok(json!({ "project": manifest(id) })),
                _ => FacadeResponse { status: 404, body: json!({"error": "no such project"}) },
            },
            "project.status" => FacadeResponse::ok(json!({
                "project_id": params["id"], "state": "running", "pid": 4242,
                "socket": "/run/secret.sock", "restarts": 1, "failed_reason": "/secret/path",
                "kernel_version": "0.8.2", "stale_build": false,
            })),
            "fleet.snapshot" => FacadeResponse::ok(json!({
                "schema": 1,
                "nodes": [
                    {"node_id": "n1", "instances": {"value": [
                        {"id": "i1", "placement": {"project_id": A}},
                        {"id": "i2", "placement": {"project_id": B}},
                        {"id": "i3", "placement": {}},
                    ]}},
                    {"node_id": "n2", "instances": {"value": [
                        {"id": "i4", "placement": {"project_id": B}},
                    ]}},
                    {"node_id": "n3"},
                ],
            })),
            _ => FacadeResponse::ok(json!({"ok": true})),
        }
    }
    fn poll_events(&self, cursor: usize) -> (Vec<SseMessage>, usize) {
        (vec![], cursor)
    }
    fn inject_witness(&self, _: WitnessRequest) -> WitnessResponse {
        WitnessResponse::rejected("no")
    }
}

fn app_with(console: ConsoleOptions) -> (axum::Router, Arc<MemoryTokenValidator>, Arc<Projects>) {
    let auth = Arc::new(MemoryTokenValidator::new());
    let facade = Projects::new();
    let state = state_with(auth.clone(), facade.clone());
    (build_router_with(state, &[], None, console).unwrap(), auth, facade)
}

fn app() -> (axum::Router, Arc<MemoryTokenValidator>, Arc<Projects>) {
    app_with(ConsoleOptions::default())
}

async fn get(app: &axum::Router, uri: &str, bearer: &str) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn read_token(auth: &MemoryTokenValidator, project: Option<&str>) -> String {
    auth.generate_token_for_project(3600, TokenScope::Read, project).unwrap()
}

fn instance_ids(snapshot: &Value) -> Vec<String> {
    snapshot["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|n| n["instances"]["value"].as_array().cloned().unwrap_or_default())
        .map(|r| r["id"].as_str().unwrap().to_owned())
        .collect()
}

// ─── Project read routes ────────────────────────────────────────────────

#[tokio::test]
async fn projects_list_and_show_for_an_unbound_read_token() {
    let (app, auth, _) = app();
    let t = read_token(&auth, None);
    let (code, body) = get(&app, "/api/projects", &t).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body["projects"].as_array().unwrap().len(), 2);

    let (code, body) = get(&app, &format!("/api/projects/{A}"), &t).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body["project"]["id"], A);
    assert_eq!(body["status"]["state"], "running");
    let text = body.to_string();
    for leak in ["4242", "secret.sock", "/secret/path"] {
        assert!(!text.contains(leak), "status leaked {leak}: {text}");
    }
}

#[tokio::test]
async fn project_show_rejects_a_bad_id_and_maps_unknown_to_404() {
    let (app, auth, facade) = app();
    let t = read_token(&auth, None);
    assert_eq!(get(&app, "/api/projects/not-a-ulid", &t).await.0, StatusCode::BAD_REQUEST);
    assert!(facade.calls.lock().unwrap().is_empty(), "a bad id never reaches the daemon");
    assert_eq!(get(&app, &format!("/api/projects/{C}"), &t).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn project_routes_need_a_token_and_owner_tokens_work_too() {
    let (app, auth, _) = app();
    let r = app
        .clone()
        .oneshot(Request::builder().uri("/api/projects").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    let owner = auth.generate_token(3600).unwrap();
    assert_eq!(get(&app, "/api/projects", &owner).await.0, StatusCode::OK);
}

#[tokio::test]
async fn snapshot_project_filter_keeps_only_that_projects_instances() {
    let (app, auth, _) = app();
    let t = read_token(&auth, None);
    let (_, all) = get(&app, "/api/fleet/snapshot", &t).await;
    assert_eq!(instance_ids(&all), ["i1", "i2", "i3", "i4"]);
    let (code, a) = get(&app, &format!("/api/fleet/snapshot?project={A}"), &t).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(instance_ids(&a), ["i1"]);
    let (_, b) = get(&app, &format!("/api/fleet/snapshot?project={B}"), &t).await;
    assert_eq!(instance_ids(&b), ["i2", "i4"]);
    assert_eq!(get(&app, "/api/fleet/snapshot?project=zzz", &t).await.0, StatusCode::BAD_REQUEST);
}

// ─── Project-bound read tokens ──────────────────────────────────────────

#[tokio::test]
async fn a_project_bound_read_token_is_confined_to_its_project() {
    let (app, auth, _) = app();
    let t = read_token(&auth, Some(A));

    let (code, body) = get(&app, "/api/projects", &t).await;
    assert_eq!(code, StatusCode::OK);
    let ids: Vec<_> = body["projects"].as_array().unwrap().iter().map(|m| m["id"].clone()).collect();
    assert_eq!(ids, [json!(A)]);
    assert!(body.get("skipped").is_none(), "no store diagnostics for a confined token");

    assert_eq!(get(&app, &format!("/api/projects/{A}"), &t).await.0, StatusCode::OK);
    assert_eq!(get(&app, &format!("/api/projects/{B}"), &t).await.0, StatusCode::FORBIDDEN);

    // The snapshot is forced to its project, and cannot be pointed elsewhere.
    let (_, snap) = get(&app, "/api/fleet/snapshot", &t).await;
    assert_eq!(instance_ids(&snap), ["i1"]);
    let (_, snap) = get(&app, &format!("/api/fleet/snapshot?project={A}"), &t).await;
    assert_eq!(instance_ids(&snap), ["i1"]);
    assert_eq!(get(&app, &format!("/api/fleet/snapshot?project={B}"), &t).await.0, StatusCode::FORBIDDEN);

    // Machine-wide reads are not part of a project's view.
    for p in ["/api/processes", "/api/services", "/api/chain/status", "/api/chain/events", "/api/vectors/status", "/api/config"] {
        assert_eq!(get(&app, p, &t).await.0, StatusCode::FORBIDDEN, "{p}");
    }
}

#[tokio::test]
async fn a_project_bound_read_token_gets_the_minimal_health_view() {
    let (app, auth, _) = app();
    let t = read_token(&auth, Some(A));
    let (code, body) = get(&app, "/api/health", &t).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body, json!({"status": "ok"}));
}

#[tokio::test]
async fn read_tokens_cannot_write_project_routes() {
    let (app, auth, _) = app();
    for t in [read_token(&auth, None), read_token(&auth, Some(A))] {
        let r = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/projects")
                    .header(header::AUTHORIZATION, format!("Bearer {t}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
    }
}

// ─── /console/ static ───────────────────────────────────────────────────

fn console_dir() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("index.html"), "<html>console</html>").unwrap();
    std::fs::write(d.path().join("app.js"), "console.log(1)").unwrap();
    d
}

async fn raw_get(app: &axum::Router, uri: &str) -> (StatusCode, String, Option<String>) {
    let resp = app
        .clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let csp = resp
        .headers()
        .get(header::CONTENT_SECURITY_POLICY)
        .map(|v| v.to_str().unwrap().to_owned());
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned(), csp)
}

#[tokio::test]
async fn console_is_served_under_console_with_its_own_csp() {
    let dir = console_dir();
    let (app, _, _) = app_with(ConsoleOptions {
        static_dir: Some(dir.path().to_str().unwrap().into()),
        connect_src: vec!["http://192.168.1.50:8081".into(), "ws://seed.local".into()],
        tailnet: None,
    });
    let (code, body, csp) = raw_get(&app, "/console/").await;
    assert_eq!(code, StatusCode::OK);
    assert!(body.contains("console"));
    let csp = csp.unwrap();
    assert!(csp.contains("connect-src 'self' http://192.168.1.50:8081 ws://seed.local;"), "{csp}");
    assert!(csp.contains("frame-ancestors 'none'"));
    assert!(!csp.contains("ws: wss:"), "no blanket ws allowance: {csp}");

    let (code, body, _) = raw_get(&app, "/console/app.js").await;
    assert_eq!((code, body.as_str()), (StatusCode::OK, "console.log(1)"));
    // SPA fallback for client-side routes.
    let (code, body, _) = raw_get(&app, "/console/some/route").await;
    assert_eq!(code, StatusCode::OK);
    assert!(body.contains("console"));

    // Not at the root, and the API keeps the gateway-wide CSP.
    assert_eq!(raw_get(&app, "/").await.0, StatusCode::NOT_FOUND);
    assert_eq!(raw_get(&app, "/app.js").await.0, StatusCode::NOT_FOUND);
    let (_, _, api_csp) = raw_get(&app, "/api/health").await;
    assert_eq!(api_csp.as_deref(), Some(clawft_services::api::middleware::CSP_HEADER_VALUE));
}

#[tokio::test]
async fn console_is_not_served_without_a_static_dir() {
    let (app, _, _) = app();
    assert_eq!(raw_get(&app, "/console/").await.0, StatusCode::NOT_FOUND);
}

#[test]
fn a_bad_console_connect_src_origin_is_refused_at_startup() {
    let dir = console_dir();
    for bad in ["*", "http://a.test; script-src *", "http://a.test/path", "a.test", "http://a b", "'self'"] {
        let state = state_with(Arc::new(MemoryTokenValidator::new()), Projects::new());
        let r = build_router_with(
            state,
            &[],
            None,
            ConsoleOptions {
                static_dir: Some(dir.path().to_str().unwrap().into()),
                connect_src: vec![bad.into()],
                tailnet: None,
            },
        );
        assert!(r.is_err(), "{bad} must be refused");
    }
}

// ─── POST /api/console/token ────────────────────────────────────────────

struct FakeWhois(Result<Option<String>, String>);

#[async_trait]
impl TailnetWhois for FakeWhois {
    async fn login_of(&self, _: std::net::IpAddr) -> Result<Option<String>, WhoisError> {
        self.0.clone().map_err(WhoisError)
    }
}

struct FakeMinter {
    minted: Mutex<Vec<(String, String, u64)>>,
    result: Result<(), MintError>,
}

#[async_trait]
impl TokenMinter for FakeMinter {
    async fn mint_read_for_project(&self, project: &str, label: &str, ttl: u64) -> Result<Minted, MintError> {
        self.minted.lock().unwrap().push((project.into(), label.into(), ttl));
        match &self.result {
            Ok(()) => Ok(Minted { token: "wft_fake".into(), expires_at: "2099-01-01T00:00:00+00:00".into() }),
            Err(MintError::Unavailable) => Err(MintError::Unavailable),
            Err(MintError::Refused) => Err(MintError::Refused),
        }
    }
}

fn minting_app(
    who: Result<Option<String>, String>,
    minted: Result<(), MintError>,
    ttl: u64,
) -> (axum::Router, Arc<FakeMinter>) {
    let minter = Arc::new(FakeMinter { minted: Mutex::new(vec![]), result: minted });
    let mint = TailnetMint::new(
        &["Alice@Example.com".into(), "bob@example.com".into()],
        ttl,
        &["https://console.example".into()],
        Arc::new(FakeWhois(who)),
        minter.clone(),
    );
    let (app, _, _) = app_with(ConsoleOptions { tailnet: Some(Arc::new(mint)), ..Default::default() });
    (app, minter)
}

fn alice() -> Result<Option<String>, String> {
    Ok(Some("alice@example.com".into()))
}

async fn mint(
    app: &axum::Router,
    peer: Option<&str>,
    body: &str,
    extra: &[(&str, &str)],
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(Method::POST)
        .uri("/api/console/token")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::HOST, "gw.tailnet:18789");
    for (k, v) in extra {
        req = req.header(*k, *v);
    }
    let mut req = req.body(Body::from(body.to_owned())).unwrap();
    if let Some(p) = peer {
        let addr: SocketAddr = format!("{p}:5555").parse().unwrap();
        req.extensions_mut().insert(ConnectInfo(addr));
    }
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn project_body(p: &str) -> String {
    json!({ "project": p }).to_string()
}

#[tokio::test]
async fn an_allowlisted_tailnet_login_mints_a_short_read_token() {
    let (app, minter) = minting_app(alice(), Ok(()), 600);
    let (code, body) = mint(&app, Some("100.101.102.103"), &project_body(A), &[]).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body["token"], "wft_fake");
    assert_eq!(body["project"], A);
    assert!(body["expires_at"].is_string());
    // Login matched case-insensitively; label names the login, ttl as configured.
    assert_eq!(
        *minter.minted.lock().unwrap(),
        [(A.to_owned(), "console:alice@example.com".to_owned(), 600)]
    );
}

#[tokio::test]
async fn an_ipv6_tailnet_peer_is_accepted() {
    let (app, _) = minting_app(alice(), Ok(()), 600);
    let mut req = Request::builder()
        .method(Method::POST)
        .uri("/api/console/token")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(project_body(A)))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo("[fd7a:115c:a1e0:ab12::7]:4000".parse::<SocketAddr>().unwrap()));
    assert_eq!(app.clone().oneshot(req).await.unwrap().status(), StatusCode::OK);
}

#[tokio::test]
async fn the_ttl_is_capped_at_the_authority_maximum() {
    let (app, minter) = minting_app(alice(), Ok(()), 10_000_000);
    assert_eq!(mint(&app, Some("100.64.0.1"), &project_body(A), &[]).await.0, StatusCode::OK);
    assert_eq!(minter.minted.lock().unwrap()[0].2, 24 * 3600);
}

#[tokio::test]
async fn a_login_off_the_allowlist_is_refused_and_nothing_is_minted() {
    let (app, minter) = minting_app(Ok(Some("mallory@example.com".into())), Ok(()), 900);
    let (code, body) = mint(&app, Some("100.100.1.1"), &project_body(A), &[]).await;
    assert_eq!(code, StatusCode::FORBIDDEN);
    assert!(!body.to_string().contains("mallory"), "the reply does not echo the login");
    assert!(minter.minted.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_non_tailnet_peer_is_refused_before_whois_and_xff_is_ignored() {
    let (app, minter) = minting_app(alice(), Ok(()), 900);
    for peer in ["127.0.0.1", "192.168.1.5", "100.63.255.255", "100.128.0.0", "8.8.8.8"] {
        let (code, _) = mint(&app, Some(peer), &project_body(A), &[("x-forwarded-for", "100.101.102.103")]).await;
        assert_eq!(code, StatusCode::FORBIDDEN, "{peer}");
    }
    // No connection info at all (not behind `serve`) is refused as well.
    assert_eq!(mint(&app, None, &project_body(A), &[]).await.0, StatusCode::FORBIDDEN);
    assert!(minter.minted.lock().unwrap().is_empty());
}

#[tokio::test]
async fn whois_failures_are_503_and_unknown_peers_403() {
    let (app, minter) = minting_app(Err("tailscaled down".into()), Ok(()), 900);
    let (code, body) = mint(&app, Some("100.100.1.1"), &project_body(A), &[]).await;
    assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
    assert!(!body.to_string().contains("tailscaled"));
    let (app2, _) = minting_app(Ok(None), Ok(()), 900);
    assert_eq!(mint(&app2, Some("100.100.1.1"), &project_body(A), &[]).await.0, StatusCode::FORBIDDEN);
    assert!(minter.minted.lock().unwrap().is_empty());
}

#[tokio::test]
async fn bad_requests_are_refused() {
    let (app, minter) = minting_app(alice(), Ok(()), 900);
    let p = Some("100.100.1.1");
    assert_eq!(mint(&app, p, "{}", &[]).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(mint(&app, p, &project_body("nope"), &[]).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(mint(&app, p, "not json", &[]).await.0, StatusCode::BAD_REQUEST);
    // A project the daemon does not know.
    assert_eq!(mint(&app, p, &project_body(C), &[]).await.0, StatusCode::NOT_FOUND);
    assert!(minter.minted.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_form_content_type_is_refused() {
    let (app, _) = minting_app(alice(), Ok(()), 900);
    let mut req = Request::builder()
        .method(Method::POST)
        .uri("/api/console/token")
        .header(header::CONTENT_TYPE, "text/plain")
        .body(Body::from(project_body(A)))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo("100.100.1.1:1".parse::<SocketAddr>().unwrap()));
    assert_eq!(app.clone().oneshot(req).await.unwrap().status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn cross_origin_browsers_are_refused_but_the_console_origin_is_not() {
    let (app, minter) = minting_app(alice(), Ok(()), 900);
    let p = Some("100.100.1.1");
    let evil = [("origin", "https://evil.example")];
    assert_eq!(mint(&app, p, &project_body(A), &evil).await.0, StatusCode::FORBIDDEN);
    let cross = [("origin", "https://evil.example"), ("sec-fetch-site", "cross-site")];
    assert_eq!(mint(&app, p, &project_body(A), &cross).await.0, StatusCode::FORBIDDEN);
    assert!(minter.minted.lock().unwrap().is_empty());
    // Same origin as the Host header.
    let same = [("origin", "http://gw.tailnet:18789"), ("sec-fetch-site", "same-origin")];
    assert_eq!(mint(&app, p, &project_body(A), &same).await.0, StatusCode::OK);
    // A configured console origin.
    let listed = [("origin", "https://console.example")];
    assert_eq!(mint(&app, p, &project_body(A), &listed).await.0, StatusCode::OK);
}

#[tokio::test]
async fn mint_failures_map_to_503_and_502() {
    let (app, _) = minting_app(alice(), Err(MintError::Unavailable), 900);
    assert_eq!(mint(&app, Some("100.100.1.1"), &project_body(A), &[]).await.0, StatusCode::SERVICE_UNAVAILABLE);
    let (app, _) = minting_app(alice(), Err(MintError::Refused), 900);
    assert_eq!(mint(&app, Some("100.100.1.1"), &project_body(A), &[]).await.0, StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn the_mint_route_is_absent_unless_tailnet_identity_is_enabled() {
    let (app, _, _) = app();
    let (code, _) = mint(&app, Some("100.100.1.1"), &project_body(A), &[]).await;
    assert_eq!(code, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_mint_route_is_rate_limited() {
    let (app, _) = minting_app(alice(), Ok(()), 900);
    let mut last = StatusCode::OK;
    for _ in 0..12 {
        last = mint(&app, Some("100.100.1.1"), &project_body(A), &[]).await.0;
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
}
