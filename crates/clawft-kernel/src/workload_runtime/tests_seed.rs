//! Seed API adapter against a local mock Seed (wiremock), over the real
//! reqwest transport.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::host_contract::HostContract;
use super::seed::{SEED_CONCURRENCY_CAP, SeedApiRuntime, SeedConfig, SeedPin};
use super::seed_http::{HttpSeedTransport, SeedCredentials, validate_base_url, validate_path};
use super::test_support::{MemoryCredentials, signed_workload};
use super::types::{
    InstanceState, RunMode, RuntimeError, VerifiedWorkload, WorkloadConfig, WorkloadRuntime,
};

const TOKEN: &str = "seed-token-0123456789abcdef";
const NODE: &str = "seed-01";

fn app(id: &str, version: &str, running: bool) -> Value {
    json!({"id": id, "version": version, "running": running, "has_binary": true})
}

async fn seed(installed: Vec<Value>) -> (MockServer, SeedApiRuntime, Arc<MemoryCredentials>) {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/apps"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"installed": installed, "count": 0})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/apps/available"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"version": "2.3.2", "cogs": [
            {"id": "fall-detect", "version": "1.0.0", "sha256": "038d0712e3b026b66bcbce8708c782a96aeb4c3f7c0b2838a9ce288df88bc77e"},
            {"id": "baby-cry", "version": "1.0.0"},
            {"id": "sleep-apnea", "version": "1.2.2"}
        ]})))
        .mount(&server)
        .await;
    let creds = Arc::new(MemoryCredentials::with(NODE, TOKEN));
    let rt = SeedApiRuntime::new(
        SeedConfig {
            node_id: NODE.into(),
            pins: vec![
                SeedPin::new("fall-detect", "1.0.0"),
                SeedPin::new("baby-cry", "1.0.0"),
                SeedPin::new("anomaly-detect", "1.2.0"),
            ],
            concurrency_cap: SEED_CONCURRENCY_CAP,
        },
        Arc::new(HttpSeedTransport::new(&server.uri(), false).unwrap()),
        creds.clone(),
    )
    .unwrap();
    (server, rt, creds)
}

fn post(p: &str, expect: u64) -> Mock {
    Mock::given(method("POST"))
        .and(path(p))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .expect(expect)
}

fn pin(id: &str, v: &str) -> VerifiedWorkload {
    VerifiedWorkload::store_pin("cognitum", id, v, None).unwrap()
}

fn cfg() -> WorkloadConfig {
    WorkloadConfig {
        mode: RunMode::Listener,
        args: vec![],
        host: HostContract::default_feed(),
        node_id: NODE.into(),
    }
}

#[test]
fn urls_and_paths_are_validated() {
    assert!(validate_base_url("https://seed.local:8443").is_ok());
    for bad in ["ftp://x", "https://u:p@x", "https://x/../y", "http://"] {
        assert!(validate_base_url(bad).is_err(), "{bad}");
    }
    assert!(validate_path("/api/v1/apps/x/logs?lines=20").is_ok());
    assert!(validate_path("/api/v1/../admin").is_err() && validate_path("/etc/passwd").is_err());
}

#[tokio::test]
async fn provides_is_adapter_attested_as_claimed() {
    let (_s, rt, _) = seed(vec![]).await;
    let caps = rt.provides();
    let ids: Vec<&str> = caps.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["runtime.remote.api", "node.class.cognitum-seed"]);
    assert!(
        caps.iter()
            .all(|c| c.provenance == clawft_types::placement::Provenance::Claimed)
    );
}

#[tokio::test]
async fn admission_takes_only_operator_pinned_store_cogs_the_store_lists() {
    let (_s, rt, _) = seed(vec![]).await;
    assert_eq!(
        rt.admit(&pin("fall-detect", "1.0.0")).await.unwrap().arch,
        "armv7"
    );
    let e = rt.admit(&pin("sleep-apnea", "1.2.2")).await.unwrap_err();
    assert!(e.to_string().contains("not pinned"), "{e}");
    let e = rt.admit(&pin("fall-detect", "0.9.0")).await.unwrap_err();
    assert!(e.to_string().contains("not pinned"), "{e}");
    // Pinned, but the Seed store registry lacks anomaly-detect.
    let e = rt.admit(&pin("anomaly-detect", "1.2.0")).await.unwrap_err();
    assert!(e.to_string().contains("does not list"), "{e}");
    let signed = signed_workload(
        "[cog]\nid = \"fall-detect\"\nversion = \"1.0.0\"\n",
        &[("armv7", b"\x7fELF")],
    );
    let e = rt.admit(&signed.workload).await.unwrap_err();
    assert!(
        matches!(e, RuntimeError::AdmissionRefused(ref m) if m.contains("signed packages")),
        "{e}"
    );
}

#[tokio::test]
async fn load_installs_a_missing_pinned_cog_then_stops_its_auto_start() {
    let (s, rt, _) = seed(vec![]).await;
    Mock::given(method("POST"))
        .and(path("/api/v1/apps/install"))
        .and(body_json(json!({"id": "fall-detect"})))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .expect(1)
        .mount(&s)
        .await;
    post("/api/v1/apps/fall-detect/stop", 1).mount(&s).await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/apps/fall-detect"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&s)
        .await;
    let h = rt.load(&pin("fall-detect", "1.0.0"), &cfg()).await.unwrap();
    assert_eq!(h.instance_id, "fall-detect-seed-seed-01");
    rt.unload(h).await.unwrap();
}

#[tokio::test]
async fn load_of_an_installed_cog_installs_nothing_and_unload_keeps_it() {
    let (s, rt, _) = seed(vec![app("fall-detect", "1.0.0", false)]).await;
    post("/api/v1/apps/install", 0).mount(&s).await;
    Mock::given(method("DELETE"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&s)
        .await;
    let h = rt.load(&pin("fall-detect", "1.0.0"), &cfg()).await.unwrap();
    assert_eq!(rt.status(&h).await.state, InstanceState::Loaded);
    let mut c = cfg();
    c.args = vec!["--interval".into(), "5".into()];
    assert!(
        rt.load(&pin("baby-cry", "1.0.0"), &c).await.is_err(),
        "argv is not the Seed config surface"
    );
    rt.unload(h).await.unwrap();
}

#[tokio::test]
async fn start_honors_the_concurrency_cap() {
    let (s, rt, _) = seed(vec![
        app("fall-detect", "1.0.0", false),
        app("a", "1", true),
        app("b", "1", true),
        app("c", "1", true),
    ])
    .await;
    post("/api/v1/apps/fall-detect/start", 0).mount(&s).await;
    let h = rt.load(&pin("fall-detect", "1.0.0"), &cfg()).await.unwrap();
    let e = rt.start(&h).await.unwrap_err();
    assert!(e.to_string().contains("concurrency cap 3"), "{e}");
}

#[tokio::test]
async fn console_refuses_while_any_cog_runs_and_never_stops_one_itself() {
    let (s, rt, _) = seed(vec![
        app("fall-detect", "1.0.0", false),
        app("baby-cry", "1.0.0", true),
    ])
    .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&s)
        .await;
    let h = rt.load(&pin("fall-detect", "1.0.0"), &cfg()).await.unwrap();
    assert!(matches!(
        rt.console(&h, "--interval 1").await,
        Err(RuntimeError::InvalidConfig(_))
    ));
    let e = rt.console(&h, "--once").await.unwrap_err();
    assert!(e.to_string().contains("baby-cry"), "{e}");
    let pre = rt.console_preemptions(&h).await.unwrap();
    assert_eq!(pre.len(), 1);
    assert_eq!(
        (pre[0].workload_id.as_str(), pre[0].version.as_str()),
        ("baby-cry", "1.0.0")
    );
    assert_eq!(
        rt.network_exposure(),
        crate::workload_governance::NetworkPolicy::Egress
    );
}

#[tokio::test]
async fn stop_returns_log_evidence() {
    let (s, rt, _) = seed(vec![app("fall-detect", "1.0.0", true)]).await;
    post("/api/v1/apps/fall-detect/stop", 1).mount(&s).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/apps/fall-detect/logs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "output": ["{\"status\":\"quiet\"}"], "errors": ["[cog-fall-detect] start"]})))
        .mount(&s)
        .await;
    let h = rt.load(&pin("fall-detect", "1.0.0"), &cfg()).await.unwrap();
    let ev = rt.stop(&h, Duration::from_secs(1)).await.unwrap();
    assert_eq!(ev.json_lines().len(), 1);
    assert!(ev.stderr.contains("start"));
}

#[tokio::test]
async fn http_errors_never_carry_the_token() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({"error": "unauthorized"})))
        .mount(&server)
        .await;
    let rt = SeedApiRuntime::new(
        SeedConfig {
            node_id: NODE.into(),
            pins: vec![SeedPin::new("fall-detect", "1.0.0")],
            concurrency_cap: 3,
        },
        Arc::new(HttpSeedTransport::new(&server.uri(), false).unwrap()),
        Arc::new(MemoryCredentials::with(NODE, TOKEN)),
    )
    .unwrap();
    let e = rt.installed().await.unwrap_err().to_string();
    assert!(e.contains("401") && !e.contains(TOKEN), "{e}");
    let dead = SeedApiRuntime::new(
        SeedConfig {
            node_id: NODE.into(),
            pins: vec![],
            concurrency_cap: 3,
        },
        Arc::new(HttpSeedTransport::new("http://127.0.0.1:1", false).unwrap()),
        Arc::new(MemoryCredentials::with(NODE, TOKEN)),
    )
    .unwrap();
    let e = dead.installed().await.unwrap_err().to_string();
    assert!(!e.contains(TOKEN), "{e}");
}

#[tokio::test]
async fn pairing_stores_the_token_in_the_secret_store() {
    let (s, rt, creds) = seed(vec![]).await;
    post("/api/v1/pair/window", 1).mount(&s).await;
    Mock::given(method("POST"))
        .and(path("/api/v1/pair"))
        .and(body_json(json!({"client_name": "weftos-mac"})))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"token": "fresh-token-abcdef0123456789"})),
        )
        .expect(1)
        .mount(&s)
        .await;
    rt.pair("weftos-mac", &clawft_types::secret::SecretString::new(""))
        .await
        .unwrap();
    assert_eq!(
        creds.get(NODE).unwrap().expose(),
        "fresh-token-abcdef0123456789"
    );
    assert!(
        rt.pair("bad name", &clawft_types::secret::SecretString::new(""))
            .await
            .is_err()
    );
}

fn identity(device: &str, fw: &str) -> Value {
    json!({"device_id": device, "public_key": format!("pk-{device}"), "firmware_version": fw})
}

fn status(gated: bool) -> Value {
    json!({"integrity": {"writes_gated": gated}, "paired": true})
}

async fn firmware_seed(gated: bool, pending: bool) -> (MockServer, SeedApiRuntime) {
    let (s, rt, _) = seed(vec![app("fall-detect", "1.0.0", false)]).await;
    for (p, v) in [
        ("/api/v1/status", status(gated)),
        (
            "/api/v1/identity",
            identity("dev-a", "0.24.2"),
        ),
        ("/api/v1/witness/chain", json!({"length": 3})),
        ("/api/v1/apps/fall-detect/config", json!({"interval": 1})),
        (
            "/api/v1/upgrade/check",
            json!({"current_version": "0.24.2", "pending_update": pending}),
        ),
    ] {
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(ResponseTemplate::new(200).set_body_json(v))
            .mount(&s)
            .await;
    }
    (s, rt)
}

#[tokio::test]
async fn firmware_upgrade_needs_a_verified_backup_and_ungated_writes() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, rt) = firmware_seed(false, true).await;
    post("/api/v1/upgrade/apply", 1).mount(&s).await;
    post("/api/v1/store/truncate-confirm", 0).mount(&s).await;
    let b = rt.backup(&tmp.path().join("b1")).await.unwrap();
    assert_eq!(b.firmware(), "0.24.2");
    assert_eq!((b.device_id(), b.node_id()), ("dev-a", NODE));
    assert_eq!(b.files().len(), 5);
    assert_eq!(
        super::test_support::mode_of(&tmp.path().join("b1/identity.json")),
        0o600
    );
    assert!(
        rt.recover_writes_gated(&b).await.is_err(),
        "nothing to recover"
    );
    let out = rt.upgrade_firmware(&b).await.unwrap();
    assert_eq!(
        out,
        super::seed_ops::UpgradeOutcome::Applied {
            from: "0.24.2".into()
        }
    );

    std::fs::write(tmp.path().join("b1/status.json"), b"{}").unwrap();
    assert!(rt.upgrade_firmware(&b).await.is_err(), "tampered backup");
}

#[tokio::test]
async fn gated_writes_block_upgrade_and_allow_recovery() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, rt) = firmware_seed(true, true).await;
    post("/api/v1/upgrade/apply", 0).mount(&s).await;
    post("/api/v1/store/truncate-confirm", 1).mount(&s).await;
    let b = rt.backup(&tmp.path().join("b")).await.unwrap();
    let e = rt.upgrade_firmware(&b).await.unwrap_err();
    assert!(e.to_string().contains("gated"), "{e}");
    rt.recover_writes_gated(&b).await.unwrap();
}
