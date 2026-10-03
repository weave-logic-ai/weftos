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
        WorkloadGate::exempt(0.95, false, "test")
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

// ── a revocation racing a place ──

use crate::gate::{GateBackend, GateDecision};

/// Revokes `signer` in `list` at the moment `on` is asked about, `before`
/// or after the real gate decides: the race a slow place can lose.
struct RaceGate {
    inner: Arc<WorkloadGate>,
    list: Arc<RevocationList>,
    on: &'static str,
    before: bool,
    fired: std::sync::atomic::AtomicBool,
}

impl RaceGate {
    fn fire(&self, action: &str) {
        if action == self.on && !self.fired.swap(true, std::sync::atomic::Ordering::SeqCst) {
            let key = hex_encode(&signer().verifying_key().to_bytes());
            self.list
                .revoke_subject_by(RevocationKind::SignerKey, &key, "raced", "test")
                .unwrap();
        }
    }
}

impl GateBackend for RaceGate {
    fn check(&self, a: &str, action: &str, ctx: &serde_json::Value) -> GateDecision {
        if self.before {
            self.fire(action);
        }
        let d = self.inner.check(a, action, ctx);
        if !self.before {
            self.fire(action);
        }
        d
    }
    fn check_teardown(&self, a: &str, action: &str, ctx: &serde_json::Value) -> GateDecision {
        self.inner.check_teardown(a, action, ctx)
    }
}

/// A node whose host gate races `on`, permitting only `actions`.
fn race_node(
    list: &Arc<RevocationList>,
    controller: &SigningKey,
    on: &'static str,
    before: bool,
    actions: &[&str],
) -> HostNode {
    use crate::node_facts_advert::sign_node_facts;
    use crate::workload_runtime::{NativeConfig, NativeRuntime, WorkloadHost};
    use clawft_types::placement::NodeFacts;
    let key = SigningKey::from_bytes(&[61; 32]);
    let id = crate::node_id_from_pubkey(&key.verifying_key().to_bytes());
    let chain = Arc::new(ChainManager::new(0, 1000));
    let tmp = tempfile::tempdir().unwrap();
    let gate: Arc<dyn GateBackend> = Arc::new(RaceGate {
        inner: revoking_gate(&chain, list, actions),
        list: list.clone(),
        on,
        before,
        fired: Default::default(),
    });
    let rt = NativeRuntime::new(NativeConfig {
        root: tmp.path().join("instances"),
        run_as: None,
        allow_interpreted: true,
    });
    let host = Arc::new(
        WorkloadHost::new(
            Arc::new(rt),
            gate.clone(),
            id.clone(),
            crate::workload_governance::NodeTrustTier::Paired,
        )
        .with_chain(chain.clone()),
    );
    let mut facts = NodeFacts::new(id.clone(), chrono::Utc::now().timestamp() as u64, 600, 1);
    facts.capabilities = board_caps("pi5");
    let svc = WorkloadHostService::new(key.clone(), exchange(&id, &chain), anchors(), gate)
        .with_route("native", host)
        .with_controllers(vec![controller.verifying_key().to_bytes()])
        .with_chain(chain.clone());
    svc.set_facts(sign_node_facts(&facts, &key).unwrap());
    assert!(svc.set_revocations(list.clone()));
    HostNode { svc: Arc::new(svc), chain, id, _tmp: tmp }
}

use super::host_service::WorkloadHostService;

async fn raced_place(
    on: &'static str,
    before: bool,
    actions: &[&str],
) -> (Arc<RevocationList>, HostNode, PlacementControlPlane, super::plane_place::PlaceReport) {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = package(tmp.path(), "race-cog", SCRIPT, &[arch()]);
    let key = SigningKey::from_bytes(&[60; 32]);
    let list = Arc::new(RevocationList::new(tmp.path().join("revoked_hosts.json")));
    let node = race_node(&list, &key, on, before, actions);
    let conn = Arc::new(MeshConnector::new(false));
    let addr = conn.register_local("pi", node.svc.clone());
    let chain = Arc::new(ChainManager::new(0, 1000));
    let id = crate::node_id_from_pubkey(&key.verifying_key().to_bytes());
    // The controller's own gate does not race (it has already decided).
    let plane = PlacementControlPlane::new(
        key,
        gate(&chain),
        chain.clone(),
        exchange(&id, &chain),
        anchors(),
        conn,
    );
    plane
        .apply_operator_peers(&[OperatorPeer::new(addr, TrustTier::Paired)], &[])
        .await;
    let report = plane.place(&order(&pkg)).await.unwrap();
    std::mem::forget(tmp);
    (list, node, plane, report)
}

async fn instances_on(plane: &PlacementControlPlane, node: &str) -> usize {
    plane
        .call(node, ctl::STATUS, None, serde_json::json!({}))
        .await
        .unwrap()
        .as_array()
        .map_or(0, |a| a.len())
}

#[tokio::test]
async fn a_revocation_landing_between_the_load_check_and_the_listing_is_swept() {
    // The gate permits the load, then the signer is revoked: without the
    // check after listing, the sweep that follows the revocation would have
    // run before the instance existed and nothing would ever stop it.
    let (_l, node, plane, report) = raced_place("workload.load", false, ALL).await;
    assert!(report.placed.is_none(), "{}", report.explain);
    assert!(
        report.attempts.iter().any(|a| a.reason.as_deref().is_some_and(|r| r.contains("revoked while"))),
        "{:?}",
        report.attempts
    );
    assert_eq!(instances_on(&plane, &node.id).await, 0, "nothing left running");
    assert!(
        events(&node.chain, EVENT_KIND_WORKLOAD_UNLOAD)
            .iter()
            .any(|(_, p)| p["forced_by_revocation"]["kind"] == "signer_key"),
        "torn down by the revocation"
    );
}

#[tokio::test]
async fn a_start_refused_for_a_revocation_is_rolled_back_without_an_unload_permit() {
    // Revoked after the placement check but before the start check; the
    // operator never permitted unload, yet the instance must not stay listed.
    let (_l, node, plane, report) = raced_place(
        "workload.start",
        true,
        &["workload.place", "workload.load", "workload.start"],
    )
    .await;
    assert!(report.placed.is_none(), "{}", report.explain);
    assert_eq!(instances_on(&plane, &node.id).await, 0, "rolled back, not left listed");
    let why = report.attempts.iter().filter_map(|a| a.reason.clone()).collect::<String>();
    assert!(why.contains("unloaded"), "{why}");
}
