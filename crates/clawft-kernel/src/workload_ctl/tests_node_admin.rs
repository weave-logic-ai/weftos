//! Node-admin methods over the real `workload.ctl` wire (in-process
//! transport): the controller-key check, the pinned-peer rule on the
//! controller, replay refusal, and the chain on both nodes.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use clawft_types::placement::TrustTier;
use ed25519_dalek::SigningKey;
use serde_json::{Value, json};

use super::host_node_admin::{EVENT_NODE_ADMIN_REQUEST, EVENT_NODE_ADMIN_RESULT, NodeAdmin};
use super::msg::{RefusalCode, method};
use super::plane::{CallFailure, PlaneError};
use super::plane_node_admin::{EVENT_NODE_ADMIN_OUTCOME, EVENT_NODE_ADMIN_SENT};
use super::test_support::*;
use super::transport::MeshConnector;

#[derive(Default)]
struct Recorder {
    calls: Mutex<Vec<(String, String, Value)>>,
    fail: bool,
}

#[async_trait]
impl NodeAdmin for Recorder {
    async fn call(&self, m: &str, requester: &str, body: &Value) -> Result<Value, String> {
        self.calls.lock().unwrap().push((m.into(), requester.into(), body.clone()));
        if self.fail {
            return Err("token file is not writable".into());
        }
        Ok(json!({ "method": m, "done": true }))
    }
}

fn key() -> SigningKey {
    SigningKey::from_bytes(&[10; 32])
}

#[tokio::test]
async fn rotate_reaches_a_pinned_peers_hook_and_is_chained_on_both_nodes() {
    let ctl = key();
    let host = host_node(21, board_caps("pi5"), true, &ctl);
    let hook = Arc::new(Recorder::default());
    assert!(host.svc.set_node_admin(hook.clone()));
    let conn = Arc::new(MeshConnector::new(false));
    let addr = conn.register_local("h", host.svc.clone());
    let (plane, cchain) = controller(&ctl, conn);
    plane.add_target(&addr, TrustTier::Pinned).await.unwrap();

    let out = plane
        .node_admin(&host.id, method::DASHBOARD_ROTATE, json!({}))
        .await
        .unwrap();
    assert_eq!(out["done"], true);
    let calls = hook.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, method::DASHBOARD_ROTATE);
    assert_eq!(calls[0].1, plane.node_id(), "the hook sees the verified requester");

    let sent = events(&cchain, EVENT_NODE_ADMIN_SENT);
    assert_eq!(sent.len(), 1);
    let outcome = events(&cchain, EVENT_NODE_ADMIN_OUTCOME);
    assert_eq!(outcome[0].1["ok"], true);
    let req = events(&host.chain, EVENT_NODE_ADMIN_REQUEST);
    assert_eq!(req[0].1["method"], method::DASHBOARD_ROTATE);
    let dec = req[0].1["decision_id"].as_str().expect("a mutating call names its decision");
    assert_eq!(dec.len(), 64);
    assert_eq!(events(&host.chain, EVENT_NODE_ADMIN_RESULT)[0].1["ok"], true);
}

#[tokio::test]
async fn rotate_is_refused_before_sending_to_a_peer_that_is_not_pinned_but_status_is_served() {
    let ctl = key();
    let host = host_node(22, board_caps("pi5"), true, &ctl);
    let hook = Arc::new(Recorder::default());
    host.svc.set_node_admin(hook.clone());
    let conn = Arc::new(MeshConnector::new(false));
    let addr = conn.register_local("h", host.svc.clone());
    let (plane, _) = controller(&ctl, conn);
    plane.add_target(&addr, TrustTier::Paired).await.unwrap();

    let e = plane
        .node_admin(&host.id, method::DASHBOARD_ROTATE, json!({}))
        .await
        .unwrap_err();
    assert!(matches!(e, PlaneError::Governance(_)), "{e}");
    assert!(hook.calls.lock().unwrap().is_empty(), "nothing was sent");

    plane.node_admin(&host.id, method::DASHBOARD_STATUS, json!({})).await.unwrap();
    assert_eq!(hook.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_host_without_the_hook_refuses_and_a_hook_error_is_a_refusal_with_its_reason() {
    let ctl = key();
    let bare = host_node(23, board_caps("pi5"), true, &ctl);
    let failing = host_node(24, board_caps("pi5"), true, &ctl);
    failing.svc.set_node_admin(Arc::new(Recorder { fail: true, ..Default::default() }));
    let conn = Arc::new(MeshConnector::new(false));
    let a = conn.register_local("bare", bare.svc.clone());
    let b = conn.register_local("fail", failing.svc.clone());
    let (plane, _) = controller(&ctl, conn);
    plane.add_target(&a, TrustTier::Pinned).await.unwrap();
    plane.add_target(&b, TrustTier::Pinned).await.unwrap();

    let e = plane.node_admin(&bare.id, method::DASHBOARD_STATUS, json!({})).await.unwrap_err();
    match e {
        PlaneError::Call(CallFailure::Refused(r)) => assert_eq!(r.code, RefusalCode::UnknownMethod),
        other => panic!("{other:?}"),
    }
    let e = plane.node_admin(&failing.id, method::DASHBOARD_ROTATE, json!({})).await.unwrap_err();
    match e {
        PlaneError::Call(CallFailure::Refused(r)) => {
            assert_eq!(r.code, RefusalCode::Runtime);
            assert!(r.reason.contains("not writable"), "{}", r.reason);
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn an_unlisted_controller_key_never_reaches_the_hook() {
    let ctl = key();
    let host = host_node(25, board_caps("pi5"), true, &ctl);
    let hook = Arc::new(Recorder::default());
    host.svc.set_node_admin(hook.clone());
    let conn = Arc::new(MeshConnector::new(false));
    let addr = conn.register_local("h", host.svc.clone());
    // Learn the target with the listed key, then call with a stranger.
    let (known, _) = controller(&ctl, conn.clone());
    known.add_target(&addr, TrustTier::Pinned).await.unwrap();
    let (stranger, _) = controller(&SigningKey::from_bytes(&[99; 32]), conn);
    let e = stranger.add_target(&addr, TrustTier::Pinned).await.unwrap_err();
    assert!(e.to_string().contains("unauthorized"), "{e}");
    assert!(hook.calls.lock().unwrap().is_empty());
}
