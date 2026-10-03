//! A revocation takes down what is already running from it (forced unload),
//! and a revoked package cannot be placed, started, or kept.

use std::sync::Arc;
use std::time::Duration;

use clawft_types::placement::TrustTier;
use ed25519_dalek::SigningKey;

use super::host_service::CtlConfig;
use super::msg::method as ctl;
use super::plane::{PlacementControlPlane, PlacementRecord, PlaneError};
use super::plane_peers::OperatorPeer;
use super::plane_place::PlaceOrder;
use super::test_support::*;
use super::transport::MeshConnector;
use crate::chain::{ChainManager, EVENT_KIND_WORKLOAD_STOP, EVENT_KIND_WORKLOAD_UNLOAD};
use crate::revocation::{RevocationKind, RevocationList};
use crate::workload_governance::{NetworkPolicy, WorkloadGate, WorkloadPermitRule, revoke_and_record};
use crate::workload_pkg::codec::hex_encode;
use crate::workload_runtime::RunMode;

const SCRIPT: &str = "#!/bin/sh\necho revoke-ok\nexec sleep 30\n";

fn order(pkg: &std::path::Path) -> PlaceOrder {
    PlaceOrder {
        package_dir: pkg.to_path_buf(),
        config: CtlConfig {
            mode: RunMode::Listener,
            args: vec![],
            csi_port: 15031,
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

/// The gate every node and the controller use: permits `actions`, checks `list`.
fn revoking_gate(
    chain: &Arc<ChainManager>,
    list: &Arc<RevocationList>,
    actions: &[&str],
) -> Arc<WorkloadGate> {
    let mut permit = WorkloadPermitRule::new("revoke-test", actions.iter().copied(), ["cog"]);
    permit.max_network = NetworkPolicy::Egress;
    Arc::new(
        WorkloadGate::new(0.95, false)
            .with_permit(permit)
            .unwrap()
            .with_chain(chain.clone())
            .with_revocations(list.clone()),
    )
}

struct Rig {
    plane: PlacementControlPlane,
    chain: Arc<ChainManager>,
    pi: HostNode,
    list: Arc<RevocationList>,
    pkg: std::path::PathBuf,
    placed: PlacementRecord,
    _tmp: tempfile::TempDir,
}

async fn rig(actions: &'static [&'static str]) -> Rig {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = package(tmp.path(), "revoke-cog", SCRIPT, &[arch()]);
    let key = SigningKey::from_bytes(&[50; 32]);
    let list = Arc::new(RevocationList::new(tmp.path().join("revoked_hosts.json")));
    let l = list.clone();
    let pi = host_node_with(51, board_caps("pi5"), true, &key, move |c| {
        revoking_gate(c, &l, actions)
    });
    let conn = Arc::new(MeshConnector::new(false));
    let addr = conn.register_local("pi", pi.svc.clone());
    let chain = Arc::new(ChainManager::new(0, 1000));
    let id = crate::node_id_from_pubkey(&key.verifying_key().to_bytes());
    let plane = PlacementControlPlane::new(
        key,
        revoking_gate(&chain, &list, actions),
        chain.clone(),
        exchange(&id, &chain),
        anchors(),
        conn,
    );
    let failed = plane
        .apply_operator_peers(&[OperatorPeer::new(addr, TrustTier::Paired)], &[])
        .await;
    assert!(failed.is_empty());
    let placed = plane.place(&order(&pkg)).await.unwrap().placed.unwrap();
    assert_eq!(placed.node_id, pi.id);
    Rig {
        plane,
        chain,
        pi,
        list,
        pkg,
        placed,
        _tmp: tmp,
    }
}

const ALL: &[&str] = &["workload.*"];

#[tokio::test]
async fn a_revoked_package_signer_or_artifact_is_stopped_and_unloaded() {
    let signer_hex = hex_encode(&signer().verifying_key().to_bytes());
    for kind in [
        RevocationKind::Package,
        RevocationKind::SignerKey,
        RevocationKind::ArtifactHash,
    ] {
        let r = rig(ALL).await;
        let id = match kind {
            RevocationKind::Package => r.placed.package_id.clone().expect("package id recorded"),
            RevocationKind::SignerKey => signer_hex.clone(),
            RevocationKind::ArtifactHash => r.placed.artifact_hashes[0].clone(),
        };
        let iid = r.placed.instance_id.clone();
        // Nothing is revoked yet: a sweep leaves the instance alone.
        assert!(r.pi.svc.enforce_revocations(&r.list).await.is_empty());
        r.plane.instance(ctl::STATUS, &iid).await.unwrap();

        revoke_and_record(&r.list, Some(&r.chain), kind, &id, "test", "operator").unwrap();
        let forced = r.pi.svc.enforce_revocations(&r.list).await;
        assert_eq!(forced.len(), 1, "{kind}: {forced:?}");
        assert!(forced[0].unloaded, "{kind}: {:?}", forced[0].error);
        assert_eq!(forced[0].instance_id, iid);
        assert_eq!(forced[0].subject.kind, kind);

        // It is gone from the host and every step is chained on the host.
        assert!(r.plane.instance(ctl::STATUS, &iid).await.is_err(), "{kind}");
        for k in [EVENT_KIND_WORKLOAD_STOP, EVENT_KIND_WORKLOAD_UNLOAD] {
            let ev = events(&r.pi.chain, k);
            assert!(
                ev.iter().any(|(_, p)| p["forced_by_revocation"]["id"] == id.as_str()
                    && p["outcome"] == "ok"),
                "{kind}: {k} chained as forced: {ev:?}"
            );
        }
        // The controller's record goes too, durably; a second sweep is a no-op.
        assert_eq!(r.plane.forget_instances(&[iid.clone()]), 1);
        assert!(r.plane.placements().is_empty());
        assert!(r.pi.svc.enforce_revocations(&r.list).await.is_empty());
    }
}

#[tokio::test]
async fn an_unrelated_revocation_leaves_the_instance_running() {
    let r = rig(ALL).await;
    revoke_and_record(&r.list, Some(&r.chain), RevocationKind::Package, "cog.unrelated", "t", "op")
        .unwrap();
    revoke_and_record(&r.list, Some(&r.chain), RevocationKind::SignerKey, &"ab".repeat(32), "t", "op")
        .unwrap();
    assert!(r.pi.svc.enforce_revocations(&r.list).await.is_empty());
    r.plane
        .instance(ctl::STATUS, &r.placed.instance_id)
        .await
        .unwrap();
    assert_eq!(r.plane.placements().len(), 1);
}

#[tokio::test]
async fn forced_unload_needs_no_stop_or_unload_permit() {
    // The operator permitted placing and starting only: nothing lets this
    // node stop the cog through the gate, yet a revocation still takes it down.
    let r = rig(&["workload.place", "workload.load", "workload.start"]).await;
    let iid = r.placed.instance_id.clone();
    assert!(r.plane.instance(ctl::STOP, &iid).await.is_err(), "gate default-denies stop");
    let id = r.placed.package_id.clone().unwrap();
    revoke_and_record(&r.list, Some(&r.chain), RevocationKind::Package, &id, "t", "op").unwrap();
    let forced = r.pi.svc.enforce_revocations(&r.list).await;
    assert!(forced.len() == 1 && forced[0].unloaded, "{forced:?}");
}

#[tokio::test]
async fn a_revoked_package_cannot_be_placed_or_started_but_can_be_stopped() {
    let r = rig(ALL).await;
    let iid = r.placed.instance_id.clone();
    let id = r.placed.package_id.clone().unwrap();
    revoke_and_record(&r.list, Some(&r.chain), RevocationKind::Package, &id, "bad", "op").unwrap();

    // Placing it again finds no node: the gate refuses it on every one, naming
    // the revocation, and the gate's denial is chained.
    let report = r.plane.place(&order(&r.pkg)).await.unwrap();
    assert!(report.placed.is_none() && report.decision.placement.is_none());
    assert!(report.explain.contains("is revoked: bad"), "{}", report.explain);
    assert!(
        events(&r.chain, "workload.place")
            .iter()
            .any(|(_, p)| p["decision"] == "deny" && p["revoked"]["id"] == id.as_str()),
        "the gate's denial is chained with the revoked subject"
    );
    // Starting the running instance again names the package: denied.
    let err = r.plane.instance(ctl::START, &iid).await.unwrap_err();
    assert!(matches!(&err, PlaneError::Governance(w) if w.contains("revoked")), "{err}");
    // Taking it down by hand still works (a revoked package stays stoppable).
    tokio::time::sleep(Duration::from_millis(300)).await;
    r.plane.instance(ctl::STOP, &iid).await.unwrap();
    r.plane.instance(ctl::UNLOAD, &iid).await.unwrap();
    assert!(r.plane.placements().is_empty());
}
