//! A Seed addressed on its operator-assigned node id through card 09's
//! `remote.api` adapter (mock Seed over the real HTTP transport).

use std::sync::Arc;

use clawft_types::placement::TrustTier;
use clawft_types::secret::SecretString;
use serde_json::json;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::host_service::CtlConfig;
use super::msg::method as ctl;
use super::plane::{PlacementControlPlane, PlaneError};
use super::plane_seed::{SEED_ROUTE, StorePinOrder};
use super::test_support::{anchors, events, exchange};
use super::transport::MeshConnector;
use crate::chain::{
    ChainManager, EVENT_KIND_WORKLOAD_PLACE, EVENT_KIND_WORKLOAD_REFUSE, EVENT_KIND_WORKLOAD_START,
    EVENT_KIND_WORKLOAD_UNLOAD,
};
use crate::workload_governance::{
    NetworkPolicy, NodeTrustTier, PackageTrust, WorkloadGate, WorkloadPermitRule,
};
use crate::workload_runtime::seed_tls::SeedTls;
use crate::workload_runtime::{
    HttpSeedTransport, RunMode, RuntimeError, SeedApiRuntime, SeedConfig, SeedCredentials, SeedPin,
    WorkloadHost,
};

pub(super) const NODE: &str = "seed-kitchen";

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
    mock_seed_start(200, 1).await
}

pub(super) async fn mock_seed_start(start_status: u16, starts: u64) -> MockServer {
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
        .respond_with(ResponseTemplate::new(start_status).set_body_json(json!({"ok": true})))
        .expect(starts)
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
    plane.add_seed(NODE, host, TrustTier::Paired).unwrap();
    assert!(
        plane
            .add_seed("bad id!", plane_host_placeholder(), TrustTier::Paired)
            .is_err()
    );

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

/// A plane with the Seed adapter registered at `tier`; permits need Paired.
async fn seed_plane(
    server: &MockServer,
    tier: TrustTier,
) -> (PlacementControlPlane, Arc<ChainManager>, Arc<ChainManager>) {
    seed_plane_at(server, tier, None)
}

/// [`seed_plane`] with the controller's state file at `state`: a second
/// call with the same path is a controller restart (fresh plane, fresh
/// adapter).
pub(super) fn seed_plane_at(
    server: &MockServer,
    tier: TrustTier,
    state: Option<&std::path::Path>,
) -> (PlacementControlPlane, Arc<ChainManager>, Arc<ChainManager>) {
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
    let host = Arc::new(
        WorkloadHost::new(
            Arc::new(rt),
            Arc::new(
                WorkloadGate::new(0.95, false)
                    .with_chain(seed_chain.clone())
                    .with_permit(permit.clone())
                    .unwrap(),
            ),
            "operator",
            NodeTrustTier::Paired,
        )
        .with_chain(seed_chain.clone()),
    );
    let chain = Arc::new(ChainManager::new(0, 1000));
    let plane = PlacementControlPlane::new(
        ed25519_dalek::SigningKey::from_bytes(&[10; 32]),
        Arc::new(
            WorkloadGate::new(0.95, false)
                .with_chain(chain.clone())
                .with_permit(permit)
                .unwrap(),
        ),
        chain.clone(),
        exchange("ctl", &chain),
        anchors(),
        Arc::new(MeshConnector::new(false)),
    );
    let plane = match state {
        Some(p) => plane.with_state_file(p).unwrap(),
        None => plane,
    };
    plane.add_seed(NODE, host, tier).unwrap();
    (plane, chain, seed_chain)
}

pub(super) fn pin_order(start: bool) -> StorePinOrder {
    StorePinOrder {
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
        start,
    }
}

#[tokio::test]
async fn governance_sees_the_operator_assigned_seed_tier() {
    let server = mock_seed_start(200, 0).await;
    let (plane, chain, _) = seed_plane(&server, TrustTier::Discovered).await;
    let err = plane.place_store_pin(&pin_order(false)).await.unwrap_err();
    assert!(matches!(err, PlaneError::Governance(_)), "{err}");
    let gate = events(&chain, EVENT_KIND_WORKLOAD_PLACE);
    assert!(
        gate.iter().any(|(s, p)| s == "workload"
            && p["decision"] == "deny"
            && p.to_string().contains("\"discovered\"")),
        "the gate saw the discovered tier: {gate:?}"
    );
    assert!(plane.placements().is_empty());
}

#[tokio::test]
async fn a_seed_start_failure_is_not_a_placement_and_is_rolled_back() {
    let server = mock_seed_start(500, 1).await;
    let (plane, chain, seed_chain) = seed_plane(&server, TrustTier::Paired).await;
    let err = plane.place_store_pin(&pin_order(true)).await.unwrap_err();
    assert!(err.to_string().contains("start failed"), "{err}");
    assert!(err.to_string().contains("unloaded"), "{err}");
    assert!(plane.placements().is_empty(), "nothing recorded as placed");
    assert!(
        events(&chain, EVENT_KIND_WORKLOAD_PLACE)
            .iter()
            .all(|(_, p)| p["phase"] != "placed")
    );
    assert!(
        events(&chain, EVENT_KIND_WORKLOAD_REFUSE)
            .iter()
            .any(|(_, p)| p["phase"] == "start")
    );
    assert!(!events(&seed_chain, EVENT_KIND_WORKLOAD_UNLOAD).is_empty());
}
