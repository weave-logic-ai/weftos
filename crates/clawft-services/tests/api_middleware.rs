//! Integration tests for the gateway middleware stack
//! (auth + CORS deny-by-default + per-IP rate limit + CSP).
//!
//! These exercise the assembled router from
//! [`clawft_services::api::build_router`] using a fully stubbed
//! [`ApiState`]. Behavior under test is the WEFT-99/100/101/298 contract.

#![cfg(feature = "api")]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use clawft_services::api::{
    AgentAccess, AgentInfo, ApiState, BusAccess, ChannelAccess, ChannelStatusInfo, ConfigAccess,
    InMemoryKernelFacade, MemoryAccess, MemoryEntryInfo, SessionAccess, SessionDetail, SessionInfo,
    SkillAccess, SkillInfo, ToolInfo, ToolRegistryAccess, TtsProviderInfo, VoiceAccess,
    VoiceSettingsInfo, VoiceSettingsUpdate, VoiceStatusInfo, auth::MemoryTokenValidator,
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

fn make_state() -> (ApiState, Arc<MemoryTokenValidator>) {
    let auth = Arc::new(MemoryTokenValidator::new());
    let state = ApiState {
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
        kernel_facade: Arc::new(InMemoryKernelFacade::new()),
        routing_history: Arc::new(
            clawft_core::pipeline::decision_history::RoutingDecisionHistory::new(),
        ),
        rate_limiter: Arc::new(clawft_core::pipeline::rate_limiter::RateLimiter::new(60, 100)),
        health_cache: Default::default(),
        mcp: None,
    };
    (state, auth)
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn auth_middleware_rejects_no_bearer() {
    let (state, _auth) = make_state();
    let app = build_router(state, &[], None);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/agents")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        resp.headers().get(header::WWW_AUTHENTICATE).unwrap(),
        "Bearer"
    );
}

#[tokio::test]
async fn auth_middleware_accepts_valid_token() {
    let (state, auth) = make_state();
    let token = auth.generate_token(3600).unwrap();
    let app = build_router(state, &[], None);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/agents")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn auth_middleware_allows_health_without_token() {
    let (state, _auth) = make_state();
    let app = build_router(state, &[], None);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}

/// ADR-102 D5: the mint route is gone. It must not be public, and it must
/// not mint for an authenticated caller either (404 both ways).
#[tokio::test]
async fn token_mint_route_is_gone() {
    let (state, auth) = make_state();
    let token = auth.generate_token(3600).unwrap();
    let app = build_router(state, &[], None);

    for bearer in [None, Some(token)] {
        let mut req = Request::builder()
            .method(Method::POST)
            .uri("/api/auth/token");
        if let Some(t) = bearer {
            req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        let resp = app
            .clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}

/// WEFT-570: `POST /api/auth/revoke` must require a valid Bearer
/// (it is NOT in the public-paths allowlist) and, on success, the
/// token cannot be reused for any subsequent protected request.
#[tokio::test]
async fn auth_revoke_invalidates_bearer() {
    let (state, auth) = make_state();
    let token = auth.generate_token(3600).unwrap();
    let app = build_router(state, &[], None);

    // Sanity: token works against a protected route.
    let probe = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/agents")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(probe.status(), StatusCode::OK);

    // Revoke must succeed (204 No Content) and require the bearer.
    let revoke = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/auth/revoke")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revoke.status(), StatusCode::NO_CONTENT);

    // Subsequent request with the same bearer is now 401.
    let after = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/agents")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(after.status(), StatusCode::UNAUTHORIZED);
}

/// WEFT-570: revoke without a Bearer must be rejected by the auth
/// middleware itself (401), not silently 204'd. The endpoint is NOT
/// public — the caller must already prove they hold the token they're
/// asking us to revoke.
#[tokio::test]
async fn auth_revoke_rejects_anonymous_caller() {
    let (state, _auth) = make_state();
    let app = build_router(state, &[], None);

    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/auth/revoke")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn cors_denies_unconfigured_origin() {
    let (state, _auth) = make_state();
    let app = build_router(state, &[], None);

    // Preflight from a non-localhost origin.
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::OPTIONS)
                .uri("/api/health")
                .header(header::ORIGIN, "https://evil.example.com")
                .header("access-control-request-method", "GET")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // CORS layer should refuse to add Access-Control-Allow-Origin
    // for an unallowed origin.
    assert!(
        resp.headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none()
    );
}

#[tokio::test]
async fn cors_allows_localhost_default() {
    let (state, _auth) = make_state();
    let app = build_router(state, &[], None);

    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::OPTIONS)
                .uri("/api/health")
                .header(header::ORIGIN, "http://localhost:5173")
                .header("access-control-request-method", "GET")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let allow = resp
        .headers()
        .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
        .expect("Access-Control-Allow-Origin header should be present");
    assert_eq!(allow, "http://localhost:5173");
}

#[tokio::test]
async fn rate_limit_429_after_general_quota() {
    let (state, _auth) = make_state();
    let app = build_router(state, &[], None);

    // /api/health is exempt from rate limiting (k8s probes, etc.) so use
    // /api/agents which is rate-limited but cheap. We need a valid token
    // to pass the auth gate first.
    let token = _auth.generate_token(3600).unwrap();

    // 60 requests should pass…
    for i in 0..60 {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/agents")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "request {i} unexpectedly throttled"
        );
    }
    // …and the 61st should be throttled.
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/agents")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn csp_header_present_on_health() {
    let (state, _auth) = make_state();
    let app = build_router(state, &[], None);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let csp = resp
        .headers()
        .get("content-security-policy")
        .expect("CSP header missing on /api/health");
    let v = csp.to_str().unwrap();
    assert!(v.contains("default-src 'self'"));
    assert!(v.contains("frame-ancestors 'none'"));
}

#[tokio::test]
async fn csp_header_present_on_unauthorized() {
    let (state, _auth) = make_state();
    let app = build_router(state, &[], None);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/agents")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(
        resp.headers().get("content-security-policy").is_some(),
        "CSP header must accompany 401 responses too"
    );
}

// ── WEFT-40: admin routing decision history ─────────────────────────────

#[tokio::test]
async fn admin_routing_decisions_requires_auth() {
    let (state, _auth) = make_state();
    let app = build_router(state, &[], None);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/admin/routing/decisions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn admin_routing_decisions_returns_seeded_history() {
    use clawft_core::pipeline::traits::RoutingDecision;
    use http_body_util::BodyExt;

    let (state, auth) = make_state();
    // Seed the shared ring buffer.
    for i in 0..5 {
        let d = RoutingDecision {
            provider: "openai".into(),
            model: format!("m-{i}"),
            reason: format!("tiered routing: complexity=0.5, tier=free, level=1, user=alice"),
            tier: Some("free".into()),
            cost_estimate_usd: Some(0.01),
            escalated: false,
            budget_constrained: i == 2,
            sender_id: Some("alice".into()),
        };
        state.routing_history.record(&d);
    }
    // One bob/premium decision for filter tests.
    state.routing_history.record(&RoutingDecision {
        provider: "anthropic".into(),
        model: "claude".into(),
        reason: "fallback: primary unavailable".into(),
        tier: Some("premium".into()),
        cost_estimate_usd: Some(0.05),
        escalated: true,
        budget_constrained: false,
        sender_id: Some("bob".into()),
    });

    let token = auth.generate_token(3600).unwrap();
    let app = build_router(state, &[], None);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/admin/routing/decisions?limit=10")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["returned"], 6);
    assert_eq!(body["stats"]["count"], 6);
    assert!(body["stats"]["fallback"].as_u64().unwrap() >= 1);
    // Newest first; free-text reason must not appear.
    let decisions = body["decisions"].as_array().unwrap();
    assert_eq!(decisions.len(), 6);
    let first = &decisions[0];
    assert_eq!(first["principal"], "bob");
    assert_eq!(first["reason_category"], "fallback_chosen");
    assert!(first.get("reason").is_none());

}

#[tokio::test]
async fn admin_routing_decisions_filter_by_user_and_tier() {
    use clawft_core::pipeline::traits::RoutingDecision;
    use http_body_util::BodyExt;

    let (state, auth) = make_state();
    state.routing_history.record(&RoutingDecision {
        provider: "openai".into(),
        model: "m".into(),
        reason: "tiered routing: complexity=0.1, tier=free, level=0, user=alice".into(),
        tier: Some("free".into()),
        sender_id: Some("alice".into()),
        ..Default::default()
    });
    state.routing_history.record(&RoutingDecision {
        provider: "openai".into(),
        model: "m2".into(),
        reason: "tiered routing: complexity=0.9, tier=premium, level=2, user=bob".into(),
        tier: Some("premium".into()),
        sender_id: Some("bob".into()),
        ..Default::default()
    });
    let token = auth.generate_token(3600).unwrap();
    let app = build_router(state, &[], None);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/admin/routing/decisions?user=alice&tier=free")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["returned"], 1);
    assert_eq!(body["decisions"][0]["principal"], "alice");
    assert_eq!(body["decisions"][0]["tier"], "free");
    // reason category is redacted, not free-text with user=
    let cat = body["decisions"][0]["reason_category"].as_str().unwrap();
    assert!(!cat.contains("alice"));
    assert_eq!(cat, "tiered_routing");
}

#[tokio::test]
async fn admin_routing_stats_endpoint() {
    use clawft_core::pipeline::traits::RoutingDecision;
    use http_body_util::BodyExt;

    let (state, auth) = make_state();
    state.routing_history.record(&RoutingDecision {
        provider: "openai".into(),
        model: "m".into(),
        reason: "rate limited: using free tier".into(),
        tier: Some("free".into()),
        sender_id: Some("u".into()),
        ..Default::default()
    });
    let token = auth.generate_token(3600).unwrap();
    let app = build_router(state, &[], None);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/admin/routing/stats")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["count"], 1);
    assert_eq!(body["by_reason"]["rate_limited"], 1);
}

// ── WEFT-48/49: admin rate-limiter metrics + LRU flush ──────────────────

#[tokio::test]
async fn admin_rate_limiter_requires_auth() {
    let (state, _auth) = make_state();
    let app = build_router(state, &[], None);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/admin/rate-limiter")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let (state, _auth) = make_state();
    let app = build_router(state, &[], None);
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/admin/rate-limiter/flush")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn admin_rate_limiter_metrics_endpoint() {
    use http_body_util::BodyExt;

    let (state, auth) = make_state();
    // Seed traffic so metrics are non-zero.
    assert!(state.rate_limiter.check("alice", 10));
    assert!(state.rate_limiter.check("bob", 10));
    assert!(state.rate_limiter.check("alice", 10));

    let token = auth.generate_token(3600).unwrap();
    let app = build_router(state, &[], None);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/admin/rate-limiter")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["window_seconds"], 60);
    assert_eq!(body["global_rate_limit"], 100);
    assert_eq!(body["tracked_senders"], 2);
    assert_eq!(body["global_request_count"], 3);
    assert_eq!(body["max_tracked_users"], 10_000);
    assert!(body["global_utilization_pct"].as_f64().unwrap() > 0.0);
}

#[tokio::test]
async fn admin_rate_limiter_flush_endpoint() {
    use http_body_util::BodyExt;

    let (state, auth) = make_state();
    state.rate_limiter.check("alice", 10);
    state.rate_limiter.check("bob", 10);
    assert_eq!(state.rate_limiter.tracked_senders(), 2);

    let token = auth.generate_token(3600).unwrap();
    let app = build_router(state.clone(), &[], None);
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/admin/rate-limiter/flush")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["flushed"], true);
    assert_eq!(body["metrics"]["tracked_senders"], 0);
    assert_eq!(body["metrics"]["global_request_count"], 0);
    // Live Arc also cleared.
    assert_eq!(state.rate_limiter.tracked_senders(), 0);
    assert_eq!(state.rate_limiter.global_request_count(), 0);
}
