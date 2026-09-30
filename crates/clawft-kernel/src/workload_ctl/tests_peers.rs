//! Operator peer policy is authoritative on every sync (review round 2):
//! a peer dropped from the list, or lowered to `discovered`, loses its
//! placement eligibility without a restart, and a request-named
//! (`discovered`) peer later listed as `paired` is raised.

use std::sync::Arc;

use clawft_types::placement::TrustTier;
use ed25519_dalek::SigningKey;

use super::host_service::CtlConfig;
use super::plane::PlacementControlPlane;
use super::plane_place::PlaceOrder;
use super::test_support::*;
use super::transport::MeshConnector;
use crate::workload_runtime::RunMode;

const SCRIPT: &str = "#!/bin/sh\necho peers-ok\nexec sleep 30\n";

fn explain(pkg: &std::path::Path) -> PlaceOrder {
    PlaceOrder {
        package_dir: pkg.to_path_buf(),
        config: CtlConfig {
            mode: RunMode::Listener,
            args: vec![],
            csi_port: 15016,
        },
        pin: None,
        prefer: vec![],
        avoid: vec![],
        allow_emulated: false,
        start: true,
        dry_run: true,
    }
}

async fn chosen(plane: &PlacementControlPlane, pkg: &std::path::Path) -> Option<String> {
    let r = plane.place(&explain(pkg)).await.unwrap();
    r.decision.placement.map(|p| p.node_id)
}

fn tier_of(plane: &PlacementControlPlane, id: &str) -> TrustTier {
    plane
        .targets()
        .into_iter()
        .find(|t| t.node_id == id)
        .unwrap()
        .tier
}

#[tokio::test]
async fn operator_peer_policy_is_reapplied_on_every_sync() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = package(tmp.path(), "peers-cog", SCRIPT, &[arch()]);
    let key = SigningKey::from_bytes(&[20; 32]);
    let mac = host_node(21, mac_caps(), false, &key);
    let pi = host_node(22, board_caps("pi5"), true, &key);
    let conn = Arc::new(MeshConnector::new(false));
    let local = conn.register_local("local", mac.svc.clone());
    let pi_addr = conn.register_local("pi", pi.svc.clone());
    let (plane, _chain) = controller(&key, conn);
    plane.add_target(&local, TrustTier::Pinned).await.unwrap();
    let keep = [local.as_str()];

    // Listed as paired: added and chosen.
    let failed = plane
        .apply_operator_peers(&[(pi_addr.clone(), TrustTier::Paired)], &keep)
        .await;
    assert!(failed.is_empty());
    assert_eq!(tier_of(&plane, &pi.id), TrustTier::Paired);
    assert_eq!(chosen(&plane, &pkg).await.as_deref(), Some(pi.id.as_str()));

    // Dropped from the list: demoted, no longer chosen, still known (its
    // instances stay manageable). The local host is untouched.
    plane.apply_operator_peers(&[], &keep).await;
    assert_eq!(tier_of(&plane, &pi.id), TrustTier::Discovered);
    assert_eq!(tier_of(&plane, &mac.id), TrustTier::Pinned);
    assert_ne!(chosen(&plane, &pkg).await.as_deref(), Some(pi.id.as_str()));

    // A refresh re-describes with the stored (demoted) tier, not the old one.
    plane.refresh().await;
    assert_eq!(tier_of(&plane, &pi.id), TrustTier::Discovered);
    assert_ne!(chosen(&plane, &pkg).await.as_deref(), Some(pi.id.as_str()));

    // Listed again as paired: raised and chosen again.
    plane
        .apply_operator_peers(&[(pi_addr.clone(), TrustTier::Paired)], &keep)
        .await;
    assert_eq!(tier_of(&plane, &pi.id), TrustTier::Paired);
    assert_eq!(chosen(&plane, &pkg).await.as_deref(), Some(pi.id.as_str()));

    // Listed but lowered to discovered: demoted in place.
    plane
        .apply_operator_peers(&[(pi_addr, TrustTier::Discovered)], &keep)
        .await;
    assert_eq!(tier_of(&plane, &pi.id), TrustTier::Discovered);
    assert_ne!(chosen(&plane, &pkg).await.as_deref(), Some(pi.id.as_str()));
}

#[tokio::test]
async fn a_discovered_peer_later_listed_as_paired_is_raised_and_unreachable_ones_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = package(tmp.path(), "peers-cog", SCRIPT, &[arch()]);
    let key = SigningKey::from_bytes(&[23; 32]);
    let pi = host_node(24, board_caps("pi5"), true, &key);
    let conn = Arc::new(MeshConnector::new(false));
    let pi_addr = conn.register_local("pi", pi.svc.clone());
    let (plane, _chain) = controller(&key, conn);
    // First named in a request: discovered only, not placeable.
    plane
        .add_target(&pi_addr, TrustTier::Discovered)
        .await
        .unwrap();
    assert_eq!(chosen(&plane, &pkg).await, None);

    let failed = plane
        .apply_operator_peers(
            &[
                (pi_addr, TrustTier::Paired),
                ("mem://gone".into(), TrustTier::Paired),
            ],
            &[],
        )
        .await;
    assert_eq!(tier_of(&plane, &pi.id), TrustTier::Paired);
    assert_eq!(chosen(&plane, &pkg).await.as_deref(), Some(pi.id.as_str()));
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].0, "mem://gone");
}
