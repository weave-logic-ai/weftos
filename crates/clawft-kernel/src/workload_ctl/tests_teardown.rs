//! An operator can always take down what they placed: stop and unload work
//! on a peer the operator has since demoted, and Seed placements survive a
//! controller restart and stay controllable.

use std::sync::Arc;
use std::time::Duration;

use clawft_types::placement::TrustTier;
use ed25519_dalek::SigningKey;
use serde_json::json;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::host_service::CtlConfig;
use super::msg::method as ctl;
use super::plane::PlaneError;
use super::plane_peers::OperatorPeer;
use super::plane_place::PlaceOrder;
use super::test_support::*;
use super::tests_seed::{NODE, mock_seed_start, pin_order, seed_plane_at};
use super::transport::MeshConnector;
use crate::chain::{EVENT_KIND_WORKLOAD_LOAD, EVENT_KIND_WORKLOAD_STOP};
use crate::workload_runtime::RunMode;

const SCRIPT: &str = "#!/bin/sh\necho teardown-ok\nexec sleep 30\n";

fn order(pkg: &std::path::Path) -> PlaceOrder {
    PlaceOrder {
        package_dir: pkg.to_path_buf(),
        config: CtlConfig {
            mode: RunMode::Listener,
            args: vec![],
            csi_port: 15017,
        },
        pin: None,
        prefer: vec![],
        avoid: vec![],
        allow_emulated: false,
        start: true,
        dry_run: false,
    }
}

#[tokio::test]
async fn stop_and_unload_work_on_a_demoted_peer_but_start_does_not() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = package(tmp.path(), "teardown-cog", SCRIPT, &[arch()]);
    let key = SigningKey::from_bytes(&[40; 32]);
    let pi = host_node(41, board_caps("pi5"), true, &key);
    let conn = Arc::new(MeshConnector::new(false));
    let pi_addr = conn.register_local("pi", pi.svc.clone());
    let (plane, chain) = controller(&key, conn);
    let failed = plane
        .apply_operator_peers(&[OperatorPeer::new(pi_addr, TrustTier::Paired)], &[])
        .await;
    assert!(failed.is_empty());
    let placed = plane.place(&order(&pkg)).await.unwrap().placed.unwrap();
    assert_eq!(placed.node_id, pi.id);

    // The operator drops the peer from the list: demoted to discovered.
    plane.apply_operator_peers(&[], &[]).await;
    let tier = plane
        .targets()
        .into_iter()
        .find(|t| t.node_id == pi.id)
        .unwrap()
        .tier;
    assert_eq!(tier, TrustTier::Discovered);

    // Growing what runs on it is refused ...
    let err = plane
        .instance(ctl::START, &placed.instance_id)
        .await
        .unwrap_err();
    assert!(matches!(err, PlaneError::Governance(_)), "{err}");

    // ... taking it down is not.
    tokio::time::sleep(Duration::from_millis(300)).await;
    plane
        .instance(ctl::STOP, &placed.instance_id)
        .await
        .unwrap();
    plane
        .instance(ctl::UNLOAD, &placed.instance_id)
        .await
        .unwrap();
    assert!(plane.placements().is_empty());

    // The audit shows the override, not a silent bypass.
    let stops = events(&chain, crate::chain::EVENT_KIND_WORKLOAD_STOP);
    assert!(
        stops.iter().any(|(_, p)| p["phase"] == "request"
            && p["gate_denial_overridden_for_teardown"].is_string()),
        "override chained: {stops:?}"
    );
}

async fn teardown_mocks(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path_regex(r"^/api/v1/apps/fall-detect/stop$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .expect(1..)
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/apps/fall-detect/logs"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"output": ["l1"], "errors": []})),
        )
        .mount(server)
        .await;
}

#[tokio::test]
async fn a_seed_placement_survives_a_controller_restart_and_stays_controllable() {
    let tmp = tempfile::tempdir().unwrap();
    let state = tmp.path().join("workload-placements.json");
    let server = mock_seed_start(200, 1).await;
    teardown_mocks(&server).await;

    // First controller: place, then go away.
    let rec = {
        let (plane, _, _) = seed_plane_at(&server, TrustTier::Paired, Some(&state));
        plane.place_store_pin(&pin_order(true)).await.unwrap()
    };
    let text = std::fs::read_to_string(&state).unwrap();
    assert!(text.contains(&rec.instance_id), "seed placement persisted");
    assert!(
        !text.contains("seed-token"),
        "no credential in the state file"
    );

    // Restarted controller: fresh plane, fresh Seed adapter (empty table).
    let (plane, _, seed_chain) = seed_plane_at(&server, TrustTier::Paired, Some(&state));
    let listed = plane.placements();
    assert_eq!(listed.len(), 1, "the placement is listed after restart");
    assert_eq!(listed[0].instance_id, rec.instance_id);

    let st = plane.instance(ctl::STATUS, &rec.instance_id).await.unwrap();
    assert_eq!(st["instance_id"], rec.instance_id.as_str());
    plane.instance(ctl::STOP, &rec.instance_id).await.unwrap();
    assert!(!events(&seed_chain, EVENT_KIND_WORKLOAD_STOP).is_empty());
    assert!(
        events(&seed_chain, EVENT_KIND_WORKLOAD_LOAD)
            .iter()
            .any(|(_, p)| p["phase"] == "readopted"),
        "re-adoption is chained"
    );
    plane.instance(ctl::UNLOAD, &rec.instance_id).await.unwrap();
    assert!(plane.placements().is_empty());

    // And the unload is durable too.
    let (again, _, _) = seed_plane_at(&server, TrustTier::Paired, Some(&state));
    assert!(again.placements().is_empty());
}

#[tokio::test]
async fn a_restarted_controller_refuses_a_seed_that_no_longer_has_the_pinned_cog() {
    let tmp = tempfile::tempdir().unwrap();
    let state = tmp.path().join("workload-placements.json");
    let server = mock_seed_start(200, 1).await;
    let rec = {
        let (plane, _, _) = seed_plane_at(&server, TrustTier::Paired, Some(&state));
        plane.place_store_pin(&pin_order(true)).await.unwrap()
    };
    // The same Seed, but the cog was upgraded behind our back.
    let other = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/apps"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"installed": [
            {"id": "fall-detect", "version": "9.9.9", "running": false}]})),
        )
        .mount(&other)
        .await;
    let (plane, _, _) = seed_plane_at(&other, TrustTier::Paired, Some(&state));
    let err = plane
        .instance(ctl::STOP, &rec.instance_id)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("pinned"), "{err}");
}

#[tokio::test]
async fn stop_and_unload_work_on_a_demoted_seed_but_a_new_place_does_not() {
    let tmp = tempfile::tempdir().unwrap();
    let server = mock_seed_start(200, 1).await;
    teardown_mocks(&server).await;
    let (plane, _, _) = seed_plane_at(&server, TrustTier::Paired, None);
    let rec = plane.place_store_pin(&pin_order(true)).await.unwrap();
    drop(tmp);

    assert!(plane.set_tier(NODE, TrustTier::Discovered), "seed is known");
    let err = plane.place_store_pin(&pin_order(false)).await.unwrap_err();
    assert!(matches!(err, PlaneError::Governance(_)), "{err}");

    plane.instance(ctl::STOP, &rec.instance_id).await.unwrap();
    plane.instance(ctl::UNLOAD, &rec.instance_id).await.unwrap();
    assert!(plane.placements().is_empty());
}
