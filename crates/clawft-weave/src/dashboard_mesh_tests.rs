//! `dashboard.*` over the real signed `workload.ctl` mesh wire between two
//! in-process nodes: a controller (the operator's node) and a host (the node
//! with the reporter), with the in-memory transport the kernel's own placement
//! tests use. The dashboard is a local fake.

use std::sync::Arc;

use clawft_kernel::chain::ChainManager;
use clawft_kernel::node_facts_advert::sign_node_facts;
use clawft_kernel::workload_ctl::{
    CallFailure, MeshConnector, PlacementControlPlane, PlaneError, RefusalCode, WorkloadHostService,
};
use clawft_kernel::workload_governance::WorkloadGate;
use clawft_kernel::workload_pkg::TrustAnchors;
use clawft_kernel::{ArtifactExchange, ArtifactStore, ExchangeConfig, node_id_from_pubkey};
use clawft_types::placement::{NodeFacts, TrustTier};
use ed25519_dalek::SigningKey;
use serde_json::json;

use super::DashAdmin;
use crate::dashboard_test_support::*;

const ROT: &str = "/api/nodes/token/rotate";

struct Mesh {
    plane: PlacementControlPlane,
    host_id: String,
    host_chain: Arc<ChainManager>,
    ctl_chain: Arc<ChainManager>,
}

fn exchange(id: &str, chain: &Arc<ChainManager>) -> Arc<ArtifactExchange> {
    let mut ex = ArtifactExchange::new(id, Arc::new(ArtifactStore::new_memory()), ExchangeConfig::default()).unwrap();
    ex.set_chain_manager(chain.clone());
    Arc::new(ex)
}

fn gate(chain: &Arc<ChainManager>) -> Arc<WorkloadGate> {
    Arc::new(WorkloadGate::exempt(0.95, false, "test").with_chain(chain.clone()))
}

/// A host running `dash` behind the admin hook, trusting `ctl_seed`'s key, and
/// a controller that knows it at `tier`.
async fn mesh(dash: Option<Arc<crate::dashboard_report::Dashboard>>, tier: TrustTier) -> Mesh {
    let ctl_key = SigningKey::from_bytes(&[71; 32]);
    let host_key = SigningKey::from_bytes(&[72; 32]);
    let host_id = node_id_from_pubkey(&host_key.verifying_key().to_bytes());
    let host_chain = Arc::new(ChainManager::new(0, 1000));
    let svc = WorkloadHostService::new(host_key.clone(), exchange(&host_id, &host_chain), TrustAnchors::default(), gate(&host_chain))
        .with_controllers(vec![ctl_key.verifying_key().to_bytes()])
        .with_chain(host_chain.clone());
    let now = chrono::Utc::now().timestamp() as u64;
    svc.set_facts(sign_node_facts(&NodeFacts::new(host_id.clone(), now, 600, 1), &host_key).unwrap());
    if let Some(d) = dash {
        assert!(svc.set_node_admin(Arc::new(DashAdmin(d))));
    }
    let conn = Arc::new(MeshConnector::new(false));
    let addr = conn.register_local("host", Arc::new(svc));
    let ctl_chain = Arc::new(ChainManager::new(0, 1000));
    let ctl_id = node_id_from_pubkey(&ctl_key.verifying_key().to_bytes());
    let plane = PlacementControlPlane::new(ctl_key, gate(&ctl_chain), ctl_chain.clone(), exchange(&ctl_id, &ctl_chain), TrustAnchors::default(), conn);
    plane.add_target(&addr, tier).await.unwrap();
    Mesh { plane, host_id, host_chain, ctl_chain }
}

fn kinds(c: &ChainManager) -> Vec<String> {
    c.tail(c.len()).into_iter().map(|e| e.kind).collect()
}

#[tokio::test]
async fn an_operator_node_rotates_a_remote_nodes_dashboard_token_over_the_mesh() {
    let fake = FakeDash::start().await;
    let (old, new) = (token('a'), token('b'));
    fake.answer(ROT, &[(200, &json!({"token": new, "rotated_at": "2026-10-05T12:00:00Z"}).to_string())]);
    let (_d, path) = token_dir(&old);
    let m = mesh(Some(dashboard(config(&fake.url, &path))), TrustTier::Pinned).await;

    let out = m.plane.node_admin(&m.host_id, "dashboard.token.rotate", json!({})).await.unwrap();
    assert_eq!(out["rotated"], true);
    assert!(!out.to_string().contains(&new), "no token comes back over the mesh");

    // The HOST presented its own token to the dashboard and saved the new one.
    let reqs = fake.requests(ROT);
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].authorization.as_deref(), Some(format!("Bearer {old}").as_str()));
    assert_eq!(std::fs::read_to_string(&path).unwrap().trim(), new);

    // Both nodes chained it, and neither chain holds a token.
    assert!(kinds(&m.ctl_chain).contains(&"node_admin.sent".to_string()));
    assert!(kinds(&m.host_chain).contains(&"node_admin.request".to_string()));
    for c in [&m.ctl_chain, &m.host_chain] {
        let all = serde_json::to_string(&c.tail(c.len())).unwrap();
        assert!(!all.contains(&old) && !all.contains(&new));
    }
}

#[tokio::test]
async fn remote_status_is_served_to_a_paired_peer_without_token_material() {
    let fake = FakeDash::start().await;
    fake.answer("/api/nodes/heartbeat", &[(200, "{}")]);
    let tok = token('a');
    let (_d, path) = token_dir(&tok);
    let dash = dashboard(config(&fake.url, &path));
    dash.heartbeat_once().await;
    let m = mesh(Some(dash), TrustTier::Paired).await;

    let st = m.plane.node_admin(&m.host_id, "dashboard.status", json!({})).await.unwrap();
    assert_eq!(st["enabled"], true);
    assert_eq!(st["last_heartbeat"], "ok");
    assert!(!st.to_string().contains(&tok));
    // ...but rotating needs a pinned peer, and nothing was sent.
    let e = m.plane.node_admin(&m.host_id, "dashboard.token.rotate", json!({})).await.unwrap_err();
    assert!(matches!(e, PlaneError::Governance(_)), "{e}");
    assert_eq!(fake.requests(ROT).len(), 0);
}

#[tokio::test]
async fn a_node_that_switched_off_remote_rotation_refuses_and_keeps_its_token() {
    let fake = FakeDash::start().await;
    fake.answer(ROT, &[(200, &json!({"token": token('b')}).to_string())]);
    let old = token('a');
    let (_d, path) = token_dir(&old);
    let mut cfg = config(&fake.url, &path);
    cfg.allow_remote_rotate = false;
    let dash = dashboard(cfg);
    let m = mesh(Some(dash.clone()), TrustTier::Pinned).await;

    let e = m.plane.node_admin(&m.host_id, "dashboard.token.rotate", json!({})).await.unwrap_err();
    match e {
        PlaneError::Call(CallFailure::Refused(r)) => {
            assert_eq!(r.code, RefusalCode::Runtime);
            assert!(r.reason.contains("allow_remote_rotate"), "{}", r.reason);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(fake.total(), 0);
    assert_eq!(std::fs::read_to_string(&path).unwrap().trim(), old);
    // The node's own operator can still rotate locally.
    fake.answer(ROT, &[(200, &json!({"token": token('c')}).to_string())]);
    crate::dashboard_rpc::run_local(&dash, "dashboard.token.rotate", false).await.unwrap();
}

#[tokio::test]
async fn a_node_without_the_reporter_refuses_the_mesh_request() {
    let m = mesh(None, TrustTier::Pinned).await;
    let e = m.plane.node_admin(&m.host_id, "dashboard.token.rotate", json!({})).await.unwrap_err();
    match e {
        PlaneError::Call(CallFailure::Refused(r)) => assert_eq!(r.code, RefusalCode::UnknownMethod),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_dashboard_failure_on_the_remote_node_comes_back_as_the_reason() {
    let fake = FakeDash::start().await;
    fake.answer(ROT, &[(401, "{}")]);
    let (_d, path) = token_dir(&token('a'));
    let m = mesh(Some(dashboard(config(&fake.url, &path))), TrustTier::Pinned).await;
    let e = m.plane.node_admin(&m.host_id, "dashboard.token.rotate", json!({})).await.unwrap_err();
    assert!(e.to_string().contains("dashboard token rejected"), "{e}");
}

#[tokio::test]
async fn the_lazy_mesh_hook_refuses_until_the_reporter_is_running() {
    use clawft_kernel::workload_ctl::NodeAdmin;
    // No test installs the process-wide reporter, so this is the "not enabled" path.
    let e = super::MeshAdmin.call("dashboard.status", "n", &json!({})).await.unwrap_err();
    assert!(e.contains("not enabled"), "{e}");
}

#[test]
fn capabilities_status_is_read_and_everything_that_changes_state_is_admin() {
    use crate::capability::{Capability, required_capability};
    assert_eq!(required_capability("dashboard.status"), Capability::Read);
    assert_eq!(required_capability("dashboard.token.rotate"), Capability::Admin);
    // An unclassified dashboard verb is never anonymous Read.
    assert_eq!(required_capability("dashboard.token.reveal"), Capability::Admin);
}
