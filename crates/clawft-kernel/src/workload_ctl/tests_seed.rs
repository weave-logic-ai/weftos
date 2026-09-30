//! A Seed addressed on its operator-assigned node id through card 09's
//! `remote.api` adapter (mock Seed over the real HTTP transport).

use std::sync::Arc;

use clawft_types::secret::SecretString;
use serde_json::json;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::host_service::CtlConfig;
use super::msg::method as ctl;
use super::plane::PlacementControlPlane;
use super::plane_seed::{SEED_ROUTE, StorePinOrder};
use super::test_support::{anchors, events, exchange};
use super::transport::MeshConnector;
use crate::chain::{ChainManager, EVENT_KIND_WORKLOAD_PLACE, EVENT_KIND_WORKLOAD_START};
use crate::workload_governance::{
    NetworkPolicy, NodeTrustTier, PackageTrust, WorkloadGate, WorkloadPermitRule,
};
use crate::workload_runtime::seed_tls::SeedTls;
use crate::workload_runtime::{
    HttpSeedTransport, RunMode, RuntimeError, SeedApiRuntime, SeedConfig, SeedCredentials, SeedPin,
    WorkloadHost,
};

const NODE: &str = "seed-kitchen";

struct Creds;
impl SeedCredentials for Creds {
    fn get(&self, _: &str) -> Result<SecretString, RuntimeError> {
        Ok(SecretString::new("seed-token-0123456789abcdef".to_string()))
    }
    fn put(&self, _: &str, _: SecretString) -> Result<(), RuntimeError> {
        Ok(())
    }
}

async fn mock_seed() -> MockServer {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/apps"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"installed": [
            {"id": "fall-detect", "version": "1.0.0", "running": false}]})),
        )
        .mount(&s)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/apps/available"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"cogs": [
            {"id": "fall-detect", "version": "1.0.0"}]})))
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r"^/api/v1/apps/fall-detect/start$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .expect(1)
        .mount(&s)
        .await;
    s
}

#[tokio::test]
async fn store_pin_goes_to_the_seed_adapter_on_its_operator_assigned_id() {
    let server = mock_seed().await;
    let rt = SeedApiRuntime::new(
        SeedConfig {
            node_id: NODE.into(),
            pins: vec![SeedPin::new("fall-detect", "1.0.0")],
            concurrency_cap: 3,
        },
        Arc::new(HttpSeedTransport::new(&server.uri(), SeedTls::WebPki).unwrap()),
        Arc::new(Creds),
    )
    .unwrap();
    let seed_chain = Arc::new(ChainManager::new(0, 1000));
    let mut permit = WorkloadPermitRule::new("seed", ["workload.*"], ["cog"]);
    permit.min_package_trust = PackageTrust::OperatorAttested;
    permit.max_network = NetworkPolicy::Egress;
    let gate = WorkloadGate::new(0.95, false)
        .with_chain(seed_chain.clone())
        .with_permit(permit)
        .unwrap();
    let host = Arc::new(
        WorkloadHost::new(
            Arc::new(rt),
            Arc::new(gate),
            "operator",
            NodeTrustTier::Paired,
        )
        .with_chain(seed_chain.clone()),
    );
    let key = ed25519_dalek::SigningKey::from_bytes(&[10; 32]);
    let chain = Arc::new(ChainManager::new(0, 1000));
    let mut place = WorkloadPermitRule::new("seed-place", ["workload.place"], ["cog"]);
    place.min_package_trust = PackageTrust::OperatorAttested;
    place.max_network = NetworkPolicy::Egress;
    let ctl_gate = WorkloadGate::new(0.95, false)
        .with_chain(chain.clone())
        .with_permit(place)
        .unwrap();
    let plane = PlacementControlPlane::new(
        key.clone(),
        Arc::new(ctl_gate),
        chain.clone(),
        exchange("ctl", &chain),
        anchors(),
        Arc::new(MeshConnector::new(false)),
    );
    plane.add_seed(NODE, host).unwrap();
    assert!(plane.add_seed("bad id!", plane_host_placeholder()).is_err());

    let rec = plane
        .place_store_pin(&StorePinOrder {
            node_id: NODE.into(),
            registry: "cognitum".into(),
            id: "fall-detect".into(),
            version: "1.0.0".into(),
            sha256: None,
            config: CtlConfig {
                mode: RunMode::Listener,
                args: vec![],
                csi_port: 5006,
            },
            start: true,
        })
        .await
        .unwrap();
    assert_eq!(rec.node_id, NODE);
    assert_eq!(rec.variant, SEED_ROUTE);
    let placed = events(&chain, EVENT_KIND_WORKLOAD_PLACE);
    assert!(
        placed
            .iter()
            .any(|(_, p)| p["phase"] == "decision" && p["pin"] == NODE)
    );
    assert!(
        placed
            .iter()
            .any(|(_, p)| p["phase"] == "placed" && p["instance_id"] == rec.instance_id.as_str())
    );
    assert!(
        !events(&seed_chain, EVENT_KIND_WORKLOAD_START).is_empty(),
        "adapter start chained"
    );
    let st = plane.instance(ctl::STATUS, &rec.instance_id).await.unwrap();
    assert_eq!(st["instance_id"], rec.instance_id.as_str());

    // A pin the operator did not make is refused by the adapter.
    let err = plane
        .place_store_pin(&StorePinOrder {
            node_id: NODE.into(),
            registry: "cognitum".into(),
            id: "baby-cry".into(),
            version: "1.0.0".into(),
            sha256: None,
            config: CtlConfig {
                mode: RunMode::Listener,
                args: vec![],
                csi_port: 5006,
            },
            start: false,
        })
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not pinned"), "{err}");
}

fn plane_host_placeholder() -> Arc<WorkloadHost> {
    let chain = Arc::new(ChainManager::new(0, 1000));
    let rt = crate::workload_runtime::NativeRuntime::new(crate::workload_runtime::NativeConfig {
        root: std::env::temp_dir(),
        run_as: None,
        allow_interpreted: false,
    });
    Arc::new(
        WorkloadHost::new(
            Arc::new(rt),
            Arc::new(WorkloadGate::new(0.8, false)),
            "x",
            NodeTrustTier::Paired,
        )
        .with_chain(chain),
    )
}
