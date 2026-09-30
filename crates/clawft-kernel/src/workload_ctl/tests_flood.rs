//! A served `workload-host` bounds what unauthenticated peers can cost it
//! (review round 2): refusals of unverified requests are chained within a
//! budget, a connection is dropped after its first unauthenticated
//! request, and concurrent sessions are capped. Real TCP listener.

use std::sync::Arc;
use std::time::Duration;

use ed25519_dalek::SigningKey;
use serde_json::json;

use super::msg::{CtlOutcome, CtlRequest, RefusalCode, method, verify_response};
use super::refusal_budget::DEFAULT_PER_WINDOW;
use super::session::CtlConnection;
use super::test_support::*;
use super::transport::{
    CtlConnector, MeshConnector, listen_tcp, serve_listener, serve_listener_with,
};
use crate::chain::EVENT_KIND_WORKLOAD_REFUSE;

fn now() -> u64 {
    chrono::Utc::now().timestamp_millis() as u64
}

/// One signed call on `c`; `Ok(refusal code)` or `Ok(None)` when served,
/// `Err` when the connection gave no answer.
async fn call(
    c: &mut CtlConnection,
    key: &SigningKey,
    target: &str,
    m: &str,
    decision: Option<String>,
) -> Result<Option<RefusalCode>, String> {
    let req = CtlRequest::new(key, m, target, now(), 60_000, decision, json!({}));
    let signed = req.sign(key).unwrap();
    let resp = c
        .call(target, m, &signed, None, Duration::from_secs(5))
        .await
        .map_err(|e| e.to_string())?;
    Ok(
        match verify_response(&resp, &req, None).unwrap().0.outcome {
            CtlOutcome::Ok { .. } => None,
            CtlOutcome::Refused { refusal } => Some(refusal.code),
        },
    )
}

#[tokio::test]
async fn unauthenticated_requests_cannot_grow_the_chain_without_limit() {
    let ctl = SigningKey::from_bytes(&[90; 32]);
    let stranger = SigningKey::from_bytes(&[91; 32]);
    let host = host_node(92, board_caps("pi5"), true, &ctl);
    let listener = listen_tcp("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let server = tokio::spawn(serve_listener(listener, host.svc.clone(), false));
    let conn = MeshConnector::new(false);
    let refusals = || events(&host.chain, EVENT_KIND_WORKLOAD_REFUSE).len();

    // The first unauthenticated request on a connection is answered (a
    // signed refusal), then the connection is dropped.
    let mut c = CtlConnection::new(conn.connect(&addr).await.unwrap(), "x");
    let first = call(&mut c, &stranger, &host.id, method::STATUS, None).await;
    assert_eq!(first, Ok(Some(RefusalCode::Unauthorized)));
    assert!(
        call(&mut c, &stranger, &host.id, method::STATUS, None)
            .await
            .is_err(),
        "a second frame on the same connection is not served"
    );

    // Many more, each on a fresh connection: the chain grows by the budget only.
    for _ in 0..(3 * DEFAULT_PER_WINDOW) {
        let mut c = CtlConnection::new(conn.connect(&addr).await.unwrap(), "x");
        let r = call(&mut c, &stranger, &host.id, method::STATUS, None).await;
        assert_eq!(r, Ok(Some(RefusalCode::Unauthorized)));
    }
    assert_eq!(refusals(), DEFAULT_PER_WINDOW as usize);

    // Authenticated work is unaffected, and its refusals are always chained.
    let mut c = CtlConnection::new(conn.connect(&addr).await.unwrap(), "ctl");
    assert_eq!(
        call(&mut c, &ctl, &host.id, method::STATUS, None).await,
        Ok(None)
    );
    let bad = Some("not-a-chain-hash".to_string());
    assert_eq!(
        call(&mut c, &ctl, &host.id, method::STOP, bad).await,
        Ok(Some(RefusalCode::InvalidRequest))
    );
    assert_eq!(refusals(), DEFAULT_PER_WINDOW as usize + 1);
    server.abort();
}

#[tokio::test]
async fn the_listener_caps_concurrent_sessions() {
    let ctl = SigningKey::from_bytes(&[93; 32]);
    let host = host_node(94, board_caps("pi5"), true, &ctl);
    let listener = listen_tcp("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let server = tokio::spawn(serve_listener_with(listener, host.svc.clone(), false, 1));
    let conn = Arc::new(MeshConnector::new(false));

    let mut held = CtlConnection::new(conn.connect(&addr).await.unwrap(), "ctl");
    assert_eq!(
        call(&mut held, &ctl, &host.id, method::STATUS, None).await,
        Ok(None)
    );
    // The slot is taken: a second session is closed without an answer.
    let mut extra = CtlConnection::new(conn.connect(&addr).await.unwrap(), "ctl");
    assert!(
        call(&mut extra, &ctl, &host.id, method::STATUS, None)
            .await
            .is_err()
    );
    // Releasing the slot lets the next session in.
    held.close().await;
    let mut served = false;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(20)).await;
        let mut c = CtlConnection::new(conn.connect(&addr).await.unwrap(), "ctl");
        if call(&mut c, &ctl, &host.id, method::STATUS, None).await == Ok(None) {
            served = true;
            break;
        }
    }
    assert!(served, "a freed slot is reused");
    server.abort();
}
