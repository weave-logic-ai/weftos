//! The operator's version pin holds on every path that could run a cog:
//! before the install call, right after it (the Seed auto-starts what it
//! installs), and on the console path.

use std::sync::Arc;

use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::host_contract::HostContract;
use super::seed::{SEED_CONCURRENCY_CAP, SeedApiRuntime, SeedConfig, SeedPin};
use super::seed_http::HttpSeedTransport;
use super::seed_tls::SeedTls;
use super::test_support::MemoryCredentials;
use super::types::{RunMode, RuntimeError, VerifiedWorkload, WorkloadConfig, WorkloadRuntime};

const NODE: &str = "seed-03";

fn app(version: &str, running: bool) -> Value {
    json!({"id": "fall-detect", "version": version, "running": running})
}

fn json_ok(v: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(v)
}

async fn mock(store_version: &str) -> MockServer {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/apps/available"))
        .respond_with(json_ok(
            json!({"cogs": [{"id": "fall-detect", "version": store_version}]}),
        ))
        .mount(&s)
        .await;
    s
}

fn runtime(s: &MockServer) -> SeedApiRuntime {
    SeedApiRuntime::new(
        SeedConfig {
            node_id: NODE.into(),
            pins: vec![SeedPin::new("fall-detect", "1.0.0")],
            concurrency_cap: SEED_CONCURRENCY_CAP,
        },
        Arc::new(HttpSeedTransport::new(&s.uri(), SeedTls::WebPki).unwrap()),
        Arc::new(MemoryCredentials::with(NODE, "seed-token-pin-tests")),
    )
    .unwrap()
}

fn fall_detect() -> VerifiedWorkload {
    VerifiedWorkload::store_pin("cognitum", "fall-detect", "1.0.0", None).unwrap()
}

fn cfg() -> WorkloadConfig {
    WorkloadConfig {
        mode: RunMode::Listener,
        args: vec![],
        host: HostContract::default_feed(),
        node_id: NODE.into(),
    }
}

async fn paths(s: &MockServer) -> Vec<String> {
    s.received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| format!("{} {}", r.method, r.url.path()))
        .collect()
}

#[tokio::test]
async fn a_store_version_other_than_the_pin_is_refused_before_any_install_call() {
    let s = mock("1.1.0").await;
    Mock::given(method("GET"))
        .and(path("/api/v1/apps"))
        .respond_with(json_ok(json!({"installed": []})))
        .mount(&s)
        .await;
    let err = runtime(&s).load(&fall_detect(), &cfg()).await.unwrap_err();
    assert!(matches!(err, RuntimeError::AdmissionRefused(_)), "{err}");
    let seen = paths(&s).await;
    assert!(
        !seen.iter().any(|p| p.starts_with("POST")),
        "nothing was installed or started: {seen:?}"
    );
}

#[tokio::test]
async fn a_version_that_slips_in_at_install_time_is_stopped_and_removed_before_anything_else() {
    // The listing says 1.0.0, but the install lands 1.1.0 and starts it.
    let s = mock("1.0.0").await;
    Mock::given(method("GET"))
        .and(path("/api/v1/apps"))
        .respond_with(json_ok(json!({"installed": []})))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&s)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/apps"))
        .respond_with(json_ok(json!({"installed": [app("1.1.0", true)]})))
        .with_priority(2)
        .mount(&s)
        .await;
    for (m, p) in [
        ("POST", "/api/v1/apps/install"),
        ("POST", "/api/v1/apps/fall-detect/stop"),
        ("DELETE", "/api/v1/apps/fall-detect"),
    ] {
        Mock::given(method(m))
            .and(path(p))
            .respond_with(json_ok(json!({"ok": true})))
            .expect(1)
            .mount(&s)
            .await;
    }
    let err = runtime(&s).load(&fall_detect(), &cfg()).await.unwrap_err();
    let RuntimeError::StrandedInstall { rolled_back, .. } = err else {
        panic!("expected a stranded install, got {err:?}");
    };
    assert!(rolled_back, "the unpinned version was uninstalled");
    let seen = paths(&s).await;
    let at = |needle: &str| seen.iter().position(|p| p == needle).unwrap();
    assert!(at("POST /api/v1/apps/install") < at("POST /api/v1/apps/fall-detect/stop"));
    assert!(at("POST /api/v1/apps/fall-detect/stop") < at("DELETE /api/v1/apps/fall-detect"));
    assert!(!seen.iter().any(|p| p.ends_with("/start")), "never started");
}

#[tokio::test]
async fn console_refuses_a_cog_whose_installed_version_is_not_the_pin() {
    let s = mock("1.0.0").await;
    // Loaded at the pin ...
    Mock::given(method("GET"))
        .and(path("/api/v1/apps"))
        .respond_with(json_ok(json!({"installed": [app("1.0.0", false)]})))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&s)
        .await;
    // ... then the Seed upgrades it behind our back.
    Mock::given(method("GET"))
        .and(path("/api/v1/apps"))
        .respond_with(json_ok(json!({"installed": [app("1.0.1", false)]})))
        .with_priority(2)
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/apps/fall-detect/console"))
        .respond_with(json_ok(json!({"output": "ran"})))
        .expect(0)
        .mount(&s)
        .await;
    let rt = runtime(&s);
    let h = rt.load(&fall_detect(), &cfg()).await.unwrap();
    let err = rt.console(&h, "--once").await.unwrap_err();
    assert!(matches!(err, RuntimeError::AdmissionRefused(_)), "{err}");
    assert!(err.to_string().contains("pinned 1.0.0"), "{err}");
}
