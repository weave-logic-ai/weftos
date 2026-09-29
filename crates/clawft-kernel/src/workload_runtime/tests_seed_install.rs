//! Seed installs whose outcome is uncertain, and the concurrency cap an
//! auto-starting install must respect (mock Seed over the real transport).

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

const TOKEN: &str = "seed-token-install-tests";
const NODE: &str = "seed-02";

fn app(id: &str, running: bool) -> Value {
    json!({"id": id, "version": "1.0.0", "running": running, "has_binary": true})
}

fn apps(list: Vec<Value>) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"installed": list, "count": 0}))
}

/// A mock Seed whose installed list is `before` for the first `reads`
/// reads and `after` from then on; the install POST answers `install`.
async fn seed(
    before: Vec<Value>,
    reads: u64,
    after: Vec<Value>,
    install: ResponseTemplate,
    installs: u64,
) -> (MockServer, SeedApiRuntime) {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/apps"))
        .respond_with(apps(before))
        .up_to_n_times(reads)
        .with_priority(1)
        .mount(&s)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/apps"))
        .respond_with(apps(after))
        .with_priority(2)
        .mount(&s)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/apps/available"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"cogs": [{"id": "fall-detect", "version": "1.0.0"}]})),
        )
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/apps/install"))
        .respond_with(install)
        .expect(installs)
        .mount(&s)
        .await;
    let rt = SeedApiRuntime::new(
        SeedConfig {
            node_id: NODE.into(),
            pins: vec![SeedPin::new("fall-detect", "1.0.0")],
            concurrency_cap: SEED_CONCURRENCY_CAP,
        },
        Arc::new(HttpSeedTransport::new(&s.uri(), SeedTls::WebPki).unwrap()),
        Arc::new(MemoryCredentials::with(NODE, TOKEN)),
    )
    .unwrap();
    (s, rt)
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

fn stop_mock(expect: u64) -> Mock {
    Mock::given(method("POST"))
        .and(path("/api/v1/apps/fall-detect/stop"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .expect(expect)
}

#[tokio::test]
async fn a_failed_install_call_that_did_install_is_tracked_stopped_and_unloadable() {
    // The install call errors, but the Seed finished it: fall-detect is
    // listed (auto-started) on the next read.
    let (s, rt) = seed(
        vec![],
        1,
        vec![app("fall-detect", true)],
        ResponseTemplate::new(504),
        1,
    )
    .await;
    stop_mock(1).mount(&s).await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/apps/fall-detect"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .expect(1)
        .mount(&s)
        .await;
    let h = rt
        .load(&fall_detect(), &cfg())
        .await
        .expect("reconciled install");
    assert!(h.store_installed, "the install is owned by this load");
    rt.unload(h).await.expect("a governed unload removes it");
}

#[tokio::test]
async fn a_failed_install_call_that_did_not_install_keeps_its_error() {
    let (s, rt) = seed(vec![], 1, vec![], ResponseTemplate::new(500), 1).await;
    stop_mock(0).mount(&s).await;
    let e = rt.load(&fall_detect(), &cfg()).await.unwrap_err();
    assert!(!matches!(e, RuntimeError::StrandedInstall { .. }), "{e:?}");
    assert!(e.to_string().contains("500"), "{e}");
}

#[tokio::test]
async fn an_install_is_refused_when_its_auto_start_would_pass_the_cap() {
    let full: Vec<Value> = ["a", "b", "c"].iter().map(|id| app(id, true)).collect();
    assert_eq!(full.len(), SEED_CONCURRENCY_CAP);
    let (s, rt) = seed(full.clone(), 99, full, ResponseTemplate::new(200), 0).await;
    stop_mock(0).mount(&s).await;
    let e = rt.load(&fall_detect(), &cfg()).await.unwrap_err();
    assert!(matches!(e, RuntimeError::AdmissionRefused(_)), "{e:?}");
    assert!(e.to_string().contains("concurrency cap"), "{e}");
}

fn app_at(id: &str, version: &str, running: bool) -> Value {
    json!({"id": id, "version": version, "running": running, "has_binary": true})
}

fn delete_mock(status: u16, expect: u64) -> Mock {
    Mock::given(method("DELETE"))
        .and(path("/api/v1/apps/fall-detect"))
        .respond_with(ResponseTemplate::new(status).set_body_json(json!({"ok": status == 200})))
        .expect(expect)
}

fn start_mock(expect: u64) -> Mock {
    Mock::given(method("POST"))
        .and(path("/api/v1/apps/fall-detect/start"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .expect(expect)
}

#[tokio::test]
async fn an_install_that_lands_an_unpinned_version_is_stopped_and_rolled_back() {
    // The store listed 1.0.0 (the pin) at admit, but the Seed installed
    // 1.1.0: the install names only the id.
    let (s, rt) = seed(
        vec![],
        1,
        vec![app_at("fall-detect", "1.1.0", true)],
        ResponseTemplate::new(200).set_body_json(json!({"ok": true})),
        1,
    )
    .await;
    stop_mock(1).mount(&s).await;
    delete_mock(200, 1).mount(&s).await;
    start_mock(0).mount(&s).await;
    let e = rt.load(&fall_detect(), &cfg()).await.unwrap_err();
    match &e {
        RuntimeError::StrandedInstall {
            rolled_back,
            reason,
            ..
        } => {
            assert!(*rolled_back, "the unpinned install is uninstalled");
            assert!(reason.contains("1.1.0") && reason.contains("1.0.0"), "{reason}");
        }
        other => panic!("expected a rolled-back install, got {other:?}"),
    }
}

#[tokio::test]
async fn an_unpinned_install_that_cannot_be_removed_is_tracked_but_never_started() {
    let (s, rt) = seed(
        vec![],
        1,
        vec![app_at("fall-detect", "1.1.0", false)],
        ResponseTemplate::new(200).set_body_json(json!({"ok": true})),
        1,
    )
    .await;
    stop_mock(1).mount(&s).await;
    delete_mock(500, 1).mount(&s).await;
    start_mock(0).mount(&s).await;
    let e = rt.load(&fall_detect(), &cfg()).await.unwrap_err();
    let RuntimeError::StrandedInstall {
        handle,
        rolled_back,
        ..
    } = e
    else {
        panic!("expected a stranded install, got {e:?}");
    };
    assert!(!rolled_back);
    let e = rt.start(&handle).await.unwrap_err();
    assert!(matches!(e, RuntimeError::AdmissionRefused(_)), "{e:?}");
    assert!(e.to_string().contains("pinned 1.0.0"), "{e}");
}

#[tokio::test]
async fn a_start_is_refused_once_the_seed_runs_a_different_version_than_the_pin() {
    // fall-detect was installed at the pin when loaded; the Seed then
    // upgraded it behind the adapter's back.
    let (s, rt) = seed(
        vec![app("fall-detect", false)],
        1,
        vec![app_at("fall-detect", "1.1.0", false)],
        ResponseTemplate::new(200),
        0,
    )
    .await;
    start_mock(0).mount(&s).await;
    let h = rt.load(&fall_detect(), &cfg()).await.expect("pinned load");
    assert!(!h.store_installed);
    let e = rt.start(&h).await.unwrap_err();
    assert!(e.to_string().contains("1.1.0"), "{e}");
}
