//! Operator peer policy is authoritative on every sync (review round 2):
//! a peer dropped from the list, or lowered to `discovered`, loses its
//! placement eligibility without a restart, and a request-named
//! (`discovered`) peer later listed as `paired` is raised.

use std::sync::Arc;

use clawft_types::placement::TrustTier;
use ed25519_dalek::SigningKey;

use super::host_service::CtlConfig;
use super::plane::PlacementControlPlane;
use super::plane_peers::OperatorPeer;
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
        project_id: None,
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
        .apply_operator_peers(&[OperatorPeer::new(pi_addr.clone(), TrustTier::Paired)], &keep)
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
        .apply_operator_peers(&[OperatorPeer::new(pi_addr.clone(), TrustTier::Paired)], &keep)
        .await;
    assert_eq!(tier_of(&plane, &pi.id), TrustTier::Paired);
    assert_eq!(chosen(&plane, &pkg).await.as_deref(), Some(pi.id.as_str()));

    // Listed but lowered to discovered: demoted in place.
    plane
        .apply_operator_peers(&[OperatorPeer::new(pi_addr, TrustTier::Discovered)], &keep)
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
                OperatorPeer::new(pi_addr, TrustTier::Paired),
                OperatorPeer::new("mem://gone", TrustTier::Paired),
            ],
            &[],
        )
        .await;
    assert_eq!(tier_of(&plane, &pi.id), TrustTier::Paired);
    assert_eq!(chosen(&plane, &pkg).await.as_deref(), Some(pi.id.as_str()));
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].0, "mem://gone");
}

fn known(plane: &PlacementControlPlane, id: &str) -> Option<TrustTier> {
    plane
        .targets()
        .into_iter()
        .find(|t| t.node_id == id)
        .map(|t| t.tier)
}

/// Review round 3 (high): trust was bound to the address. A different key
/// answering at a listed address must never inherit the listed tier.
#[tokio::test]
async fn another_key_at_a_listed_address_never_inherits_its_tier() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = package(tmp.path(), "peers-cog", SCRIPT, &[arch()]);
    let key = SigningKey::from_bytes(&[25; 32]);
    let pi = host_node(26, board_caps("pi5"), true, &key);
    let imposter = host_node(27, board_caps("pi5"), true, &key);
    let conn = Arc::new(MeshConnector::new(false));
    let addr = conn.register_local("pi5", pi.svc.clone());
    let (plane, _chain) = controller(&key, conn.clone());
    let peers = [OperatorPeer::new(addr.clone(), TrustTier::Pinned)];
    assert!(plane.apply_operator_peers(&peers, &[]).await.is_empty());
    assert_eq!(known(&plane, &pi.id), Some(TrustTier::Pinned));

    // Another host with a fresh key takes the address (DHCP reuse, mDNS
    // spoof, reflashed board).
    conn.register_local("pi5", imposter.svc.clone());
    for _ in 0..2 {
        plane.refresh().await;
        plane.apply_operator_peers(&peers, &[]).await;
    }
    assert_eq!(known(&plane, &imposter.id), None, "never learned at all");
    assert!(
        plane
            .view()
            .iter()
            .all(|v| clawft_types::placement::engine::PlacementFacts::node_id(v) != imposter.id)
    );
    assert_ne!(
        chosen(&plane, &pkg).await.as_deref(),
        Some(imposter.id.as_str())
    );
    // A request naming the address directly gets it only as discovered.
    plane
        .add_target(&addr, TrustTier::Discovered)
        .await
        .unwrap();
    plane.apply_operator_peers(&peers, &[]).await;
    assert_eq!(known(&plane, &imposter.id), Some(TrustTier::Discovered));
    assert_eq!(chosen(&plane, &pkg).await, None);
}

/// A key pinned in the peer list gets the tier only for that key; the
/// node previously known at the address is demoted.
#[tokio::test]
async fn a_pinned_key_gets_the_tier_and_the_old_key_at_the_address_is_demoted() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = package(tmp.path(), "peers-cog", SCRIPT, &[arch()]);
    let key = SigningKey::from_bytes(&[28; 32]);
    let old = host_node(29, board_caps("pi5"), true, &key);
    let new = host_node(30, board_caps("pi5"), true, &key);
    let new_pk = SigningKey::from_bytes(&[30; 32]).verifying_key().to_bytes();
    let conn = Arc::new(MeshConnector::new(false));
    let addr = conn.register_local("pi5", old.svc.clone());
    let (plane, _chain) = controller(&key, conn.clone());
    plane
        .apply_operator_peers(&[OperatorPeer::new(addr.clone(), TrustTier::Paired)], &[])
        .await;
    assert_eq!(known(&plane, &old.id), Some(TrustTier::Paired));

    // Pinned to the new key while the old one still answers: refused.
    let pinned = [OperatorPeer::new(addr.clone(), TrustTier::Paired).with_key(new_pk)];
    let failed = plane.apply_operator_peers(&pinned, &[]).await;
    assert_eq!(failed.len(), 1, "the old key's answer is refused");
    assert_eq!(known(&plane, &old.id), Some(TrustTier::Discovered));
    assert_eq!(known(&plane, &new.id), None);

    // The reflashed board answers with the pinned key: learned and chosen.
    conn.register_local("pi5", new.svc.clone());
    assert!(plane.apply_operator_peers(&pinned, &[]).await.is_empty());
    assert_eq!(known(&plane, &new.id), Some(TrustTier::Paired));
    assert_eq!(known(&plane, &old.id), Some(TrustTier::Discovered));
    assert_eq!(chosen(&plane, &pkg).await.as_deref(), Some(new.id.as_str()));
}
