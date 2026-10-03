//! Seed operations: backups bound to the Seed they came from, and the
//! persistent per-Seed credential store (pairing survives a restart).

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use clawft_types::secret::SecretString;
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::seed::{SeedApiRuntime, SeedConfig, SeedPin};
use super::seed_creds::FileCredentials;
use super::seed_http::{HttpSeedTransport, SeedCredentials};
use super::seed_tls::SeedTls;
use super::test_support::{MemoryCredentials, mode_of};

const TOKEN: &str = "seed-token-0123456789abcdef";

/// A mock Seed with identity `device` at firmware `fw`.
async fn seed_at(node: &str, device: &str, fw: &str) -> (MockServer, SeedApiRuntime) {
    let s = MockServer::start().await;
    let docs: [(&str, Value); 6] = [
        (
            "/api/v1/apps",
            json!({"installed": [{"id": "fall-detect", "version": "1.0.0"}]}),
        ),
        (
            "/api/v1/status",
            json!({"integrity": {"writes_gated": false}}),
        ),
        (
            "/api/v1/identity",
            json!({"device_id": device, "public_key": format!("pk-{device}"), "firmware_version": fw}),
        ),
        ("/api/v1/witness/chain", json!({"length": 3})),
        ("/api/v1/apps/fall-detect/config", json!({"interval": 1})),
        (
            "/api/v1/upgrade/check",
            json!({"current_version": fw, "pending_update": true, "target_version": "0.25.0"}),
        ),
    ];
    for (p, v) in docs {
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(ResponseTemplate::new(200).set_body_json(v))
            .mount(&s)
            .await;
    }
    let rt = SeedApiRuntime::new(
        SeedConfig {
            node_id: node.into(),
            pins: vec![SeedPin::new("fall-detect", "1.0.0")],
            concurrency_cap: 3,
        },
        Arc::new(HttpSeedTransport::new(&s.uri(), SeedTls::WebPki).unwrap()),
        Arc::new(MemoryCredentials::with(node, TOKEN)),
    )
    .unwrap();
    (s, rt)
}

async fn applies(s: &MockServer, n: u64) {
    Mock::given(method("POST"))
        .and(path("/api/v1/upgrade/apply"))
        .respond_with(ResponseTemplate::new(200))
        .expect(n)
        .mount(s)
        .await;
}

#[tokio::test]
async fn a_backup_only_authorizes_an_upgrade_of_the_seed_it_came_from() {
    let tmp = tempfile::tempdir().unwrap();
    let (_a, seed_a) = seed_at("seed-a", "dev-a", "0.24.2").await;
    let backup_a = seed_a.backup(&tmp.path().join("a")).await.unwrap();
    assert_eq!(backup_a.audit()["device_id"], "dev-a");

    // Another Seed under the same operator node id: device identity differs.
    let (b, seed_b) = seed_at("seed-a", "dev-b", "0.24.2").await;
    applies(&b, 0).await;
    let e = seed_b
        .upgrade_firmware(&backup_a, "0.25.0")
        .await
        .unwrap_err();
    assert!(e.to_string().contains("device identity"), "{e}");
    assert!(seed_b.recover_writes_gated(&backup_a).await.is_err());

    // Another operator node id.
    let (c, seed_c) = seed_at("seed-c", "dev-a", "0.24.2").await;
    applies(&c, 0).await;
    let e = seed_c
        .upgrade_firmware(&backup_a, "0.25.0")
        .await
        .unwrap_err();
    assert!(e.to_string().contains("different Seed node"), "{e}");

    // Same Seed after its firmware moved on: the backup is stale.
    let (d, seed_d) = seed_at("seed-a", "dev-a", "0.25.0").await;
    applies(&d, 0).await;
    let e = seed_d
        .upgrade_firmware(&backup_a, "0.25.0")
        .await
        .unwrap_err();
    assert!(e.to_string().contains("predates"), "{e}");

    // The Seed it came from, unchanged: allowed.
    let (_a2, seed_a2) = seed_at("seed-a", "dev-a", "0.24.2").await;
    applies(&_a2, 1).await;
    seed_a2.upgrade_firmware(&backup_a, "0.25.0").await.unwrap();
}

#[tokio::test]
async fn a_backup_whose_manifest_was_rewritten_or_emptied_does_not_verify() {
    let tmp = tempfile::tempdir().unwrap();
    let (_s, seed) = seed_at("seed-a", "dev-a", "0.24.2").await;
    let b = seed.backup(&tmp.path().join("b")).await.unwrap();
    b.verify().unwrap();
    let m = tmp.path().join("b/manifest.json");
    let mut doc: Value = serde_json::from_slice(&std::fs::read(&m).unwrap()).unwrap();
    doc["files"] = json!([]);
    std::fs::set_permissions(&m, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::write(&m, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();
    assert!(b.verify().is_err(), "emptied manifest");
    std::fs::remove_file(tmp.path().join("b/apps.json")).unwrap();
    assert!(b.verify().is_err());
}

#[test]
fn file_credentials_persist_privately_across_instances() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("secrets/workload.seed");
    FileCredentials::new(&dir)
        .put("seed-01", SecretString::new(TOKEN))
        .unwrap();
    assert_eq!(mode_of(&dir), 0o700);
    assert_eq!(mode_of(&dir.join("seed-01.token")), 0o600);
    // A fresh store over the same directory, as after a daemon restart.
    let again = FileCredentials::new(&dir);
    assert_eq!(again.get("seed-01").unwrap().expose(), TOKEN);
    again
        .put("seed-01", SecretString::new("rotated-token-0123456789"))
        .unwrap();
    assert_eq!(
        again.get("seed-01").unwrap().expose(),
        "rotated-token-0123456789"
    );
    assert!(again.get("seed-02").is_err());
    for bad in ["../x", ".hidden", "a b", ""] {
        assert!(again.put(bad, SecretString::new(TOKEN)).is_err(), "{bad}");
    }
    assert!(
        again
            .put("seed-03", SecretString::new("bad token"))
            .is_err()
    );
}

#[test]
fn file_credentials_refuse_exposed_or_linked_tokens() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("store");
    let creds = FileCredentials::new(&dir);
    creds.put("seed-01", SecretString::new(TOKEN)).unwrap();
    let f = dir.join("seed-01.token");
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
    let e = creds.get("seed-01").unwrap_err();
    assert!(e.to_string().contains("other users"), "{e}");
    let elsewhere = tmp.path().join("planted");
    std::fs::write(&elsewhere, TOKEN).unwrap();
    std::fs::set_permissions(&elsewhere, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::os::unix::fs::symlink(&elsewhere, dir.join("seed-02.token")).unwrap();
    assert!(creds.get("seed-02").is_err(), "symlinked token");
}

#[tokio::test]
async fn pairing_persists_the_token_for_a_restarted_adapter() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("secrets");
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/pair/window"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/pair"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"token": "paired-token-abcdef0123456789"})),
        )
        .mount(&s)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/apps"))
        .and(wiremock::matchers::header(
            "authorization",
            "Bearer paired-token-abcdef0123456789",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"installed": []})))
        .expect(1)
        .mount(&s)
        .await;
    let make = || {
        SeedApiRuntime::new(
            SeedConfig {
                node_id: "seed-01".into(),
                pins: vec![],
                concurrency_cap: 3,
            },
            Arc::new(HttpSeedTransport::new(&s.uri(), SeedTls::WebPki).unwrap()),
            Arc::new(FileCredentials::new(&dir)),
        )
        .unwrap()
    };
    make()
        .pair("weftos-mac", &SecretString::new(""))
        .await
        .unwrap();
    // A new adapter (new process) reads the stored token.
    assert!(make().installed().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_firmware_upgrade_is_gated_by_version_and_applies_only_the_pinned_one() {
    use super::host::{RUNTIME_CHAIN_SOURCE, WorkloadHost};
    use crate::chain::ChainManager;
    use crate::workload_governance::{
        NetworkPolicy, NodeTrustTier, PackageTrust, WorkloadGate, WorkloadPermitRule,
    };
    let tmp = tempfile::tempdir().unwrap();
    let (s, seed) = seed_at("seed-a", "dev-a", "0.24.2").await;
    applies(&s, 1).await;
    let seed = Arc::new(seed);
    let backup = seed.backup(&tmp.path().join("b")).await.unwrap();
    let chain = Arc::new(ChainManager::new(0, 1000));
    let mut rule = WorkloadPermitRule::new("fw", ["workload.install"], ["seed-firmware"]);
    rule.min_package_trust = PackageTrust::OperatorAttested;
    rule.max_network = NetworkPolicy::Egress;
    let gate = WorkloadGate::exempt(0.8, false, "test")
        .with_chain(chain.clone())
        .with_permit(rule)
        .unwrap();
    let host = WorkloadHost::new(
        seed.clone(),
        Arc::new(gate),
        "operator",
        NodeTrustTier::Paired,
    )
    .with_chain(chain.clone());

    // The Seed has 0.25.0 pending; a pin of 0.26.0 must not apply it.
    let e = host
        .upgrade_seed_firmware(&seed, &backup, "0.26.0")
        .await
        .unwrap_err();
    assert!(e.to_string().contains("operator pinned 0.26.0"), "{e}");
    assert!(
        host.upgrade_seed_firmware(&seed, &backup, "--latest")
            .await
            .is_err(),
        "a malformed pin is refused before anything is applied"
    );
    host.upgrade_seed_firmware(&seed, &backup, "0.25.0")
        .await
        .unwrap();

    // Each attempt is chained by the host with the version it targeted.
    let attempts: Vec<(String, String)> = chain
        .tail(0)
        .into_iter()
        .filter(|e| e.source == RUNTIME_CHAIN_SOURCE)
        .map(|e| {
            let p = e.payload.unwrap();
            (
                e.kind,
                p["target_version"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    let a = |k: &str, v: &str| (k.to_string(), v.to_string());
    assert_eq!(
        attempts,
        [
            a("workload.refuse", "0.26.0"),
            a("workload.install", "0.25.0")
        ]
    );
    let applied = chain
        .tail(0)
        .into_iter()
        .rfind(|e| e.source == RUNTIME_CHAIN_SOURCE)
        .unwrap();
    assert_eq!(applied.payload.unwrap()["from_version"], "0.24.2");
    // `applies(&s, 1)` verifies on drop that exactly one apply was sent.
}
