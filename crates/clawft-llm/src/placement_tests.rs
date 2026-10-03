use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::router::ProviderRouter;
use crate::types::ChatMessage;

const KEY_ENV: &str = "PLACEMENT_TEST_KEY";

struct Fixed {
    answer: Mutex<Option<String>>,
    calls: AtomicUsize,
}

impl Fixed {
    fn new(a: Option<&str>) -> Arc<Self> {
        Arc::new(Self {
            answer: Mutex::new(a.map(String::from)),
            calls: AtomicUsize::new(0),
        })
    }
    fn set(&self, a: Option<&str>) {
        *self.answer.lock().unwrap() = a.map(String::from);
    }
}

impl PlacementResolver for Fixed {
    fn resolve_base_url(&self, _role: &str) -> Option<String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.answer.lock().unwrap().clone()
    }
}

fn cfg(base: &str) -> LlmProviderConfig {
    LlmProviderConfig {
        name: "local".into(),
        base_url: base.into(),
        api_key_env: KEY_ENV.into(),
        model_prefix: Some("local/".into()),
        default_model: None,
        headers: HashMap::new(),
        timeout_secs: Some(5),
    }
}

async fn server(tag: &str) -> MockServer {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": tag, "model": "m",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": tag}, "finish_reason": "stop"}]
        })))
        .mount(&s)
        .await;
    s
}

fn req() -> ChatRequest {
    ChatRequest::new("m", vec![ChatMessage::user("hi")])
}

fn router(fallback: &str, r: Arc<Fixed>, ttl: Duration, roles: &[(&str, &str)]) -> ProviderRouter {
    ProviderRouter::from_configs(vec![cfg(&format!("{fallback}/v1"))]).with_placement(r, ttl, roles)
}

async fn who(router: &ProviderRouter) -> Result<String> {
    let (p, m) = router.route("local/m").unwrap();
    let mut rq = req();
    rq.model = m;
    p.complete(&rq).await.map(|r| r.id)
}

fn run<F: std::future::Future<Output = ()>>(f: F) {
    temp_env::with_var(KEY_ENV, Some("k"), || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(f)
    });
}

#[test]
fn loopback_http_gate() {
    for ok in [
        "http://127.0.0.1:8090/v1",
        "http://localhost:8090/v1",
        "http://[::1]:8090/v1",
        "http://127.0.0.1/v1",
    ] {
        assert!(is_loopback_http(ok), "{ok}");
    }
    for bad in [
        "https://127.0.0.1:8090/v1",
        "http://10.0.0.5:8090/v1",
        "http://evil.example/v1",
        "http://127.0.0.1@evil.example/v1",
        "http://localhost.evil.example/v1",
        "http://0.0.0.0:8090/v1",
        "ftp://127.0.0.1/",
        "",
    ] {
        assert!(!is_loopback_http(bad), "{bad}");
    }
}

#[test]
fn cache_hits_within_ttl_and_invalidates() {
    let inner = Fixed::new(Some("http://127.0.0.1:1/v1"));
    let c = CachedResolver::new(inner.clone(), Duration::from_secs(60));
    assert_eq!(c.resolve_base_url("r").as_deref(), Some("http://127.0.0.1:1/v1"));
    inner.set(Some("http://127.0.0.1:2/v1"));
    assert_eq!(c.resolve_base_url("r").as_deref(), Some("http://127.0.0.1:1/v1"));
    assert_eq!(inner.calls.load(Ordering::SeqCst), 1);
    c.invalidate("r");
    assert_eq!(c.resolve_base_url("r").as_deref(), Some("http://127.0.0.1:2/v1"));
    c.invalidate_all();
    inner.set(None);
    assert_eq!(c.resolve_base_url("r"), None);
    // Negative answers are cached too.
    inner.set(Some("http://127.0.0.1:3/v1"));
    assert_eq!(c.resolve_base_url("r"), None);
}

#[test]
fn cache_expires_after_ttl() {
    let inner = Fixed::new(Some("http://127.0.0.1:1/v1"));
    let c = CachedResolver::new(inner.clone(), Duration::from_millis(20));
    c.resolve_base_url("r");
    std::thread::sleep(Duration::from_millis(40));
    inner.set(Some("http://127.0.0.1:2/v1"));
    assert_eq!(c.resolve_base_url("r").as_deref(), Some("http://127.0.0.1:2/v1"));
}

#[test]
fn placed_role_beats_configured_base_url() {
    run(async {
        let (fallback, placed) = (server("fallback").await, server("placed").await);
        let r = Fixed::new(Some(&format!("{}/v1", placed.uri())));
        let rt = router(&fallback.uri(), r, Duration::from_secs(60), &[("local", "hermes")]);
        assert_eq!(who(&rt).await.unwrap(), "placed");
    });
}

#[test]
fn no_answer_falls_back_to_configured_base_url() {
    run(async {
        let fallback = server("fallback").await;
        let rt = router(&fallback.uri(), Fixed::new(None), Duration::from_secs(60), &[("local", "hermes")]);
        assert_eq!(who(&rt).await.unwrap(), "fallback");
    });
}

#[test]
fn provider_without_a_role_is_never_placed() {
    // Env / [kernel.llm] precedence: the caller omits the provider.
    run(async {
        let (fallback, placed) = (server("fallback").await, server("placed").await);
        let r = Fixed::new(Some(&format!("{}/v1", placed.uri())));
        let rt = router(&fallback.uri(), r.clone(), Duration::from_secs(60), &[]);
        assert_eq!(who(&rt).await.unwrap(), "fallback");
        assert_eq!(r.calls.load(Ordering::SeqCst), 0);
    });
}

#[test]
fn moving_the_instance_follows_after_invalidation() {
    run(async {
        let (fallback, a, b) = (server("fallback").await, server("a").await, server("b").await);
        let r = Fixed::new(Some(&format!("{}/v1", a.uri())));
        let rt = router(&fallback.uri(), r.clone(), Duration::from_secs(60), &[("local", "hermes")]);
        assert_eq!(who(&rt).await.unwrap(), "a");
        r.set(Some(&format!("{}/v1", b.uri())));
        assert_eq!(who(&rt).await.unwrap(), "a", "cached within the TTL");
        rt.invalidate_placement("hermes");
        assert_eq!(who(&rt).await.unwrap(), "b");
    });
}

#[test]
fn non_loopback_resolution_is_ignored() {
    run(async {
        let fallback = server("fallback").await;
        let r = Fixed::new(Some("http://203.0.113.9:8090/v1"));
        let rt = router(&fallback.uri(), r, Duration::from_secs(60), &[("local", "hermes")]);
        assert_eq!(who(&rt).await.unwrap(), "fallback");
    });
}

#[test]
fn dead_placed_endpoint_falls_back_and_invalidates() {
    run(async {
        let fallback = server("fallback").await;
        // A closed loopback port: bind then drop.
        let dead = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let r = Fixed::new(Some(&format!("http://127.0.0.1:{dead}/v1")));
        let rt = router(&fallback.uri(), r.clone(), Duration::from_secs(60), &[("local", "hermes")]);
        assert_eq!(who(&rt).await.unwrap(), "fallback");
        r.set(None);
        assert_eq!(who(&rt).await.unwrap(), "fallback");
        assert_eq!(r.calls.load(Ordering::SeqCst), 2, "failure invalidated the cache");
    });
}

async fn status_server(code: u16) -> MockServer {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(code).set_body_string("nope"))
        .mount(&s)
        .await;
    s
}

#[test]
fn a_503_from_the_placed_endpoint_falls_back() {
    // The loopback proxy answers 503 when nothing serves the role.
    run(async {
        let (fallback, placed) = (server("fallback").await, status_server(503).await);
        let r = Fixed::new(Some(&format!("{}/v1", placed.uri())));
        let rt = router(&fallback.uri(), r.clone(), Duration::from_secs(60), &[("local", "hermes")]);
        assert_eq!(who(&rt).await.unwrap(), "fallback");
        assert_eq!(placed.received_requests().await.unwrap().len(), 1);
        r.set(None);
        assert_eq!(who(&rt).await.unwrap(), "fallback");
        assert_eq!(r.calls.load(Ordering::SeqCst), 2, "the failure dropped the cached answer");
    });
}

#[test]
fn only_gateway_class_failures_are_retried() {
    run(async {
        for code in [502u16, 504] {
            let (fallback, placed) = (server("fallback").await, status_server(code).await);
            let r = Fixed::new(Some(&format!("{}/v1", placed.uri())));
            let rt = router(&fallback.uri(), r, Duration::from_secs(60), &[("local", "hermes")]);
            assert_eq!(who(&rt).await.unwrap(), "fallback", "{code}");
        }
        // The server's real answer is returned, not replayed elsewhere.
        for code in [400u16, 401, 404, 429, 500] {
            let (fallback, placed) = (server("fallback").await, status_server(code).await);
            let r = Fixed::new(Some(&format!("{}/v1", placed.uri())));
            let rt = router(&fallback.uri(), r, Duration::from_secs(60), &[("local", "hermes")]);
            assert!(who(&rt).await.is_err(), "{code}");
            assert_eq!(fallback.received_requests().await.unwrap().len(), 0, "{code}: replayed on the fallback");
        }
    });
}

#[test]
fn a_placed_endpoint_that_redirects_is_not_followed() {
    run(async {
        let (fallback, other) = (server("fallback").await, server("elsewhere").await);
        let placed = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(307).insert_header("location", format!("{}/v1/chat/completions", other.uri())),
            )
            .mount(&placed)
            .await;
        let r = Fixed::new(Some(&format!("{}/v1", placed.uri())));
        let rt = router(&fallback.uri(), r, Duration::from_secs(60), &[("local", "hermes")]);
        assert!(who(&rt).await.is_err());
        assert_eq!(other.received_requests().await.unwrap().len(), 0, "the redirect was followed");
    });
}
