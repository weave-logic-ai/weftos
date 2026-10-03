//! Review round 3 (medium): placement records lived only in memory, so a
//! controller restart orphaned instances on other nodes. They are now
//! persisted and survive a restart.

use std::sync::Arc;
use std::time::Duration;

use clawft_types::placement::TrustTier;
use ed25519_dalek::SigningKey;

use super::host_service::CtlConfig;
use super::msg::method;
use super::plane_place::PlaceOrder;
use super::test_support::*;
use super::transport::MeshConnector;
use crate::workload_runtime::RunMode;

const SCRIPT: &str = "#!/bin/sh\necho state-ok\nexec sleep 30\n";

fn order(pkg: &std::path::Path) -> PlaceOrder {
    PlaceOrder {
        package_dir: pkg.to_path_buf(),
        config: CtlConfig {
            mode: RunMode::Listener,
            args: vec![],
            csi_port: 15026,
        },
        pin: None,
        prefer: vec![],
        avoid: vec![],
        allow_emulated: false,
        start: true,
        dry_run: false,
        project_id: None,
    }
}

#[tokio::test]
async fn placed_instances_stay_manageable_after_a_controller_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let state = tmp.path().join("workload-placements.json");
    let pkg = package(tmp.path(), "state-cog", SCRIPT, &[arch()]);
    let key = SigningKey::from_bytes(&[33; 32]);
    let pi = host_node(34, board_caps("pi5"), true, &key);
    let conn = Arc::new(MeshConnector::new(false));
    let addr = conn.register_local("pi", pi.svc.clone());

    let iid = {
        let (plane, _chain) = controller(&key, conn.clone());
        let plane = plane.with_state_file(&state).unwrap();
        plane.add_target(&addr, TrustTier::Paired).await.unwrap();
        let r = plane.place(&order(&pkg)).await.unwrap();
        r.placed.expect("placed on the board").instance_id
    }; // the controller process goes away

    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(&state).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);

    let (plane, _chain) = controller(&key, conn.clone());
    let plane = plane.with_state_file(&state).unwrap();
    assert_eq!(plane.placements().len(), 1);
    assert!(
        plane.targets().iter().all(|t| !t.reachable),
        "not yet re-described"
    );
    let st = plane.instance(method::STATUS, &iid).await.unwrap();
    assert_eq!(st["status"]["state"], "running");
    tokio::time::sleep(Duration::from_millis(300)).await;
    plane.instance(method::STOP, &iid).await.unwrap();
    plane.instance(method::UNLOAD, &iid).await.unwrap();
    assert!(plane.placements().is_empty());

    // The unload is durable too.
    let (again, _chain) = controller(&key, conn);
    let again = again.with_state_file(&state).unwrap();
    assert!(again.placements().is_empty());
    assert_eq!(again.targets().len(), 1);
}

#[tokio::test]
async fn a_restored_target_is_re_described_only_with_its_stored_key() {
    let tmp = tempfile::tempdir().unwrap();
    let state = tmp.path().join("workload-placements.json");
    let key = SigningKey::from_bytes(&[35; 32]);
    let pi = host_node(36, board_caps("pi5"), true, &key);
    let other = host_node(37, board_caps("pi5"), true, &key);
    let conn = Arc::new(MeshConnector::new(false));
    let addr = conn.register_local("pi", pi.svc.clone());
    {
        let (plane, _c) = controller(&key, conn.clone());
        let plane = plane.with_state_file(&state).unwrap();
        plane.add_target(&addr, TrustTier::Paired).await.unwrap();
    }
    conn.register_local("pi", other.svc.clone());
    let (plane, _c) = controller(&key, conn);
    let plane = plane.with_state_file(&state).unwrap();
    plane.refresh().await;
    assert!(plane.targets().iter().all(|t| t.node_id != other.id));
    assert!(plane.view().is_empty(), "the stored key no longer answers");
}

#[test]
fn a_malformed_state_file_is_refused_and_left_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let state = tmp.path().join("workload-placements.json");
    std::fs::write(&state, "{\"version\": 1, \"targets\": 3}").unwrap();
    let key = SigningKey::from_bytes(&[38; 32]);
    let (plane, _c) = controller(&key, Arc::new(MeshConnector::new(false)));
    assert!(plane.with_state_file(&state).is_err());
    assert_eq!(
        std::fs::read_to_string(&state).unwrap(),
        "{\"version\": 1, \"targets\": 3}"
    );
}
