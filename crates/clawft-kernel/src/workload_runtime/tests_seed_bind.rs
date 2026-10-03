//! Fleet identity binding: operator-signed records verified against the
//! Seed's own identity (a stub Seed), adapter-attested `claimed` facts,
//! chained refusals, and no bearer token anywhere in the chain.

use std::sync::Arc;

use clawft_types::placement::Provenance;
use ed25519_dalek::SigningKey;
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::seed::{SEED_CONCURRENCY_CAP, SeedApiRuntime, SeedConfig, SeedPin};
use super::seed_bind::{
    BindError, BindRecord, SeedBinder, SignedBind, attest_seed_facts, seed_node_id, sign_bind,
};
use super::seed_http::HttpSeedTransport;
use super::seed_tls::SeedTls;
use super::test_support::MemoryCredentials;
use crate::chain::{ChainManager, EVENT_KIND_WORKLOAD_NODE_BIND, EVENT_KIND_WORKLOAD_REFUSE};
use crate::node_facts_advert::verify_node_facts;

const TOKEN: &str = "seed-token-bind-0123456789abcdef";
const DEVICE: &str = "4b0c1b1e-7a52-4f0e-8d3a-0d3c8f9a1e11";
const DEVICE_KEY: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90";
const NOW: u64 = 1_790_000_000;

struct Rig {
    _server: MockServer,
    rt: SeedApiRuntime,
    adapter: SigningKey,
    operator: SigningKey,
    chain: Arc<ChainManager>,
    binder: SeedBinder,
}

async fn rig_with(identity: serde_json::Value) -> Rig {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/identity"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(identity))
        .mount(&server)
        .await;
    let adapter = SigningKey::from_bytes(&[61; 32]);
    let node = seed_node_id(&adapter);
    let rt = SeedApiRuntime::new(
        SeedConfig {
            node_id: node.clone(),
            pins: vec![SeedPin::new("fall-detect", "1.0.0")],
            concurrency_cap: SEED_CONCURRENCY_CAP,
        },
        Arc::new(
            HttpSeedTransport::new(&server.uri(), SeedTls::WebPki)
                .unwrap()
                .allow_unpinned_lab_link(),
        ),
        Arc::new(MemoryCredentials::with(&node, TOKEN)),
    )
    .unwrap();
    let operator = SigningKey::from_bytes(&[62; 32]);
    let chain = Arc::new(ChainManager::new(0, 1000));
    let binder = SeedBinder::new(vec![operator.verifying_key().to_bytes()], chain.clone());
    Rig {
        _server: server,
        rt,
        adapter,
        operator,
        chain,
        binder,
    }
}

async fn rig() -> Rig {
    rig_with(json!({"device_id": DEVICE, "public_key": DEVICE_KEY, "firmware_version": "0.24.2"}))
        .await
}

impl Rig {
    fn record(&self, bound_at: u64) -> BindRecord {
        BindRecord {
            device_id: DEVICE.into(),
            device_pubkey: DEVICE_KEY.into(),
            node_id: seed_node_id(&self.adapter),
            bound_at,
        }
    }

    fn signed(&self, bound_at: u64) -> SignedBind {
        sign_bind(&self.record(bound_at), &self.operator)
    }

    fn events(&self, kind: &str) -> Vec<serde_json::Value> {
        self.chain
            .tail(self.chain.len())
            .into_iter()
            .filter(|e| e.kind == kind)
            .map(|e| e.payload.unwrap_or_default())
            .collect()
    }

    fn refusal_codes(&self) -> Vec<String> {
        self.events(EVENT_KIND_WORKLOAD_REFUSE)
            .iter()
            .filter(|p| p["phase"] == "node.bind")
            .map(|p| p["code"].as_str().unwrap_or("").to_string())
            .collect()
    }
}

#[tokio::test]
async fn a_signed_record_that_matches_the_seed_binds_and_is_chained() {
    let r = rig().await;
    let b = r.binder.bind(&r.signed(NOW), &r.rt, NOW).await.unwrap();
    assert_eq!(b.record().device_id, DEVICE);
    let bound = r.events(EVENT_KIND_WORKLOAD_NODE_BIND);
    assert_eq!(bound.len(), 1);
    assert_eq!(bound[0]["device_id"], DEVICE);
    assert_eq!(bound[0]["device_pubkey"], DEVICE_KEY);
    assert_eq!(bound[0]["node_id"], seed_node_id(&r.adapter).as_str());
    assert_eq!(
        r.binder.device_of(&seed_node_id(&r.adapter)).as_deref(),
        Some(DEVICE)
    );
}

#[tokio::test]
async fn facts_of_a_bound_seed_are_adapter_signed_and_only_claimed() {
    let r = rig().await;
    let b = r.binder.bind(&r.signed(NOW), &r.rt, NOW).await.unwrap();
    let signed = attest_seed_facts(&b, &r.rt, &r.adapter, NOW, 1).unwrap();
    let facts = verify_node_facts(&signed, NOW).unwrap();
    assert_eq!(facts.node_id, seed_node_id(&r.adapter));
    assert!(!facts.capabilities.is_empty());
    assert!(
        facts
            .capabilities
            .iter()
            .all(|c| c.provenance == Provenance::Claimed),
        "{:?}",
        facts.capabilities
    );
    // Another key cannot attest for this binding.
    let other = SigningKey::from_bytes(&[63; 32]);
    assert!(attest_seed_facts(&b, &r.rt, &other, NOW, 2).is_err());
}

#[tokio::test]
async fn a_tampered_record_is_refused_and_chained() {
    let r = rig().await;
    let mut s = r.signed(NOW);
    s.record = s
        .record
        .replace(DEVICE, "ffffffff-7a52-4f0e-8d3a-0d3c8f9a1e11");
    let e = r.binder.bind(&s, &r.rt, NOW).await.unwrap_err();
    assert_eq!(e, BindError::BadSignature);
    assert_eq!(r.refusal_codes(), ["bad_signature"]);
    assert!(r.events(EVENT_KIND_WORKLOAD_NODE_BIND).is_empty());
}

#[tokio::test]
async fn a_record_signed_by_an_unpinned_key_is_refused() {
    let r = rig().await;
    let stranger = SigningKey::from_bytes(&[64; 32]);
    let s = sign_bind(&r.record(NOW), &stranger);
    assert_eq!(
        r.binder.bind(&s, &r.rt, NOW).await.unwrap_err(),
        BindError::UntrustedOperator
    );
    assert_eq!(r.refusal_codes(), ["untrusted_operator"]);
}

#[tokio::test]
async fn a_key_that_is_not_the_devices_is_refused() {
    let r = rig().await;
    let mut rec = r.record(NOW);
    rec.device_pubkey = "00".repeat(32);
    let e = r
        .binder
        .bind(&sign_bind(&rec, &r.operator), &r.rt, NOW)
        .await
        .unwrap_err();
    assert_eq!(e, BindError::IdentityMismatch("device key"));
    let mut rec = r.record(NOW);
    rec.device_id = "some-other-seed".into();
    let e = r
        .binder
        .bind(&sign_bind(&rec, &r.operator), &r.rt, NOW)
        .await
        .unwrap_err();
    assert_eq!(e, BindError::IdentityMismatch("device id"));
    assert_eq!(
        r.refusal_codes(),
        ["identity_mismatch", "identity_mismatch"]
    );
    assert!(r.events(EVENT_KIND_WORKLOAD_NODE_BIND).is_empty());
}

#[tokio::test]
async fn a_record_for_another_node_is_refused() {
    let r = rig().await;
    let mut rec = r.record(NOW);
    rec.node_id = "some-other-node".into();
    let e = r
        .binder
        .bind(&sign_bind(&rec, &r.operator), &r.rt, NOW)
        .await
        .unwrap_err();
    assert!(matches!(e, BindError::WrongNode { .. }), "{e}");
    assert_eq!(r.refusal_codes(), ["wrong_node"]);
}

#[tokio::test]
async fn a_replayed_or_older_record_is_refused() {
    let r = rig().await;
    let first = r.signed(NOW);
    r.binder.bind(&first, &r.rt, NOW).await.unwrap();
    // The very same signed record again.
    assert_eq!(
        r.binder.bind(&first, &r.rt, NOW + 1).await.unwrap_err(),
        BindError::Replayed
    );
    // A newer binding for the device, then the first one resurfaces.
    r.binder
        .bind(&r.signed(NOW + 10), &r.rt, NOW + 10)
        .await
        .unwrap();
    assert_eq!(
        r.binder
            .bind(&r.signed(NOW + 5), &r.rt, NOW + 11)
            .await
            .unwrap_err(),
        BindError::Replayed
    );
    assert_eq!(r.refusal_codes(), ["replayed", "replayed"]);
    assert_eq!(r.events(EVENT_KIND_WORKLOAD_NODE_BIND).len(), 2);
}

#[tokio::test]
async fn an_expired_or_future_record_is_refused() {
    let r = rig().await;
    let old = r.signed(NOW - 3600);
    assert_eq!(
        r.binder.bind(&old, &r.rt, NOW).await.unwrap_err(),
        BindError::Expired
    );
    let future = r.signed(NOW + 3600);
    assert_eq!(
        r.binder.bind(&future, &r.rt, NOW).await.unwrap_err(),
        BindError::Expired
    );
    assert_eq!(r.refusal_codes(), ["expired", "expired"]);
}

#[tokio::test]
async fn no_bearer_token_reaches_the_chain_or_an_error() {
    let r = rig().await;
    let ok = r.signed(NOW);
    r.binder.bind(&ok, &r.rt, NOW).await.unwrap();
    let mut errors = Vec::new();
    errors.push(
        r.binder
            .bind(&ok, &r.rt, NOW)
            .await
            .unwrap_err()
            .to_string(),
    );
    let mut tampered = r.signed(NOW + 1);
    tampered.record.push(' ');
    errors.push(
        r.binder
            .bind(&tampered, &r.rt, NOW + 1)
            .await
            .unwrap_err()
            .to_string(),
    );
    let export = serde_json::to_string(&r.chain.tail(r.chain.len())).unwrap();
    assert!(!export.contains(TOKEN), "token in the chain export");
    assert!(export.contains(DEVICE), "the export is not empty");
    for e in errors {
        assert!(!e.contains(TOKEN), "token in an error: {e}");
    }
    let facts = attest_seed_facts(
        &r.binder
            .bind(&r.signed(NOW + 2), &r.rt, NOW + 2)
            .await
            .unwrap(),
        &r.rt,
        &r.adapter,
        NOW + 2,
        1,
    )
    .unwrap();
    assert!(!serde_json::to_string(&facts).unwrap().contains(TOKEN));
}

#[tokio::test]
async fn a_bind_over_an_unpinned_link_is_refused_before_the_seed_is_asked() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let adapter = SigningKey::from_bytes(&[61; 32]);
    let node = seed_node_id(&adapter);
    // No pin and no lab opt-in.
    let rt = SeedApiRuntime::new(
        SeedConfig {
            node_id: node.clone(),
            pins: vec![SeedPin::new("fall-detect", "1.0.0")],
            concurrency_cap: SEED_CONCURRENCY_CAP,
        },
        Arc::new(HttpSeedTransport::new(&server.uri(), SeedTls::WebPki).unwrap()),
        Arc::new(MemoryCredentials::with(&node, TOKEN)),
    )
    .unwrap();
    let operator = SigningKey::from_bytes(&[62; 32]);
    let chain = Arc::new(ChainManager::new(0, 1000));
    let binder = SeedBinder::new(vec![operator.verifying_key().to_bytes()], chain.clone());
    let rec = BindRecord {
        device_id: DEVICE.into(),
        device_pubkey: DEVICE_KEY.into(),
        node_id: node,
        bound_at: NOW,
    };
    let e = binder
        .bind(&sign_bind(&rec, &operator), &rt, NOW)
        .await
        .unwrap_err();
    assert!(matches!(e, BindError::UnpinnedTransport(_)), "{e}");
    let codes: Vec<_> = chain
        .tail(chain.len())
        .into_iter()
        .filter(|e| e.kind == EVENT_KIND_WORKLOAD_REFUSE)
        .map(|e| e.payload.unwrap()["code"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(codes, ["unpinned_transport"]);
}

#[tokio::test]
async fn a_replayed_record_is_refused_after_a_restart() {
    let r = rig().await;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join(super::seed_bind::BIND_STATE_FILE);
    let key = r.operator.verifying_key().to_bytes();
    let first = r.signed(NOW);
    {
        let b = SeedBinder::new(vec![key], r.chain.clone())
            .with_state_file(&file)
            .unwrap();
        b.bind(&first, &r.rt, NOW).await.unwrap();
    }
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    // "Restart": a new binder, same file.
    let b = SeedBinder::new(vec![key], r.chain.clone())
        .with_state_file(&file)
        .unwrap();
    assert_eq!(
        b.bind(&first, &r.rt, NOW + 1).await.unwrap_err(),
        BindError::Replayed
    );
    assert_eq!(
        b.bind(&r.signed(NOW - 5), &r.rt, NOW + 1)
            .await
            .unwrap_err(),
        BindError::Replayed
    );
    // Strictly newer still binds, and stays monotonic.
    b.bind(&r.signed(NOW + 5), &r.rt, NOW + 6).await.unwrap();
    // A malformed state file is refused and left alone.
    std::fs::write(&file, "{not json").unwrap();
    assert!(
        SeedBinder::new(vec![key], r.chain.clone())
            .with_state_file(&file)
            .is_err()
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "{not json");
}

#[tokio::test]
async fn a_binding_that_cannot_be_saved_is_not_made() {
    let r = rig().await;
    let key = r.operator.verifying_key().to_bytes();
    let b = SeedBinder::new(vec![key], r.chain.clone())
        .with_state_file("/nonexistent-dir-for-bind-state/binds.json")
        .unwrap();
    let e = b.bind(&r.signed(NOW), &r.rt, NOW).await.unwrap_err();
    assert!(matches!(e, BindError::State(_)), "{e}");
    assert!(b.device_of(&seed_node_id(&r.adapter)).is_none());
}
