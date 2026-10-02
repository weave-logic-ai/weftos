//! A stub cog placed through the real placement path (controller, signed
//! `workload.ctl`, fetch-before-load, native adapter, governance) posts to
//! the ingest bridge and its vectors appear in the placing project's store;
//! stop and unload revoke its token.

use std::sync::Arc;
use std::time::Duration;

use clawft_types::placement::TrustTier;
use ed25519_dalek::SigningKey;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::host_service::CtlConfig;
use super::msg::method;
use super::plane_place::PlaceOrder;
use super::test_support::*;
use super::transport::MeshConnector;
use crate::cog_ingest::{
    BridgeConfig, BridgeScope, IngestBridge, IngestHooks, IngestStore, LocalForwarder,
    MemoryIngestStore, RateBudget, StaticDirectory, StaticRouter, StoreDirectory, TokenRegistry,
};
use crate::workload_runtime::RunMode;

const PROJECT: &str = "01J9ZXW0PROJECTAAAAAAAAAAA";

/// Posts one vector with the token from the environment, then idles. It
/// also prints a lying stdout line: stdout is evidence only.
const COG: &str = r#"#!/bin/sh
echo "$COGNITUM_COG_TOKEN" > "$COGNITUM_COG_DATA_DIR/token"
echo '{"ingested":999,"vectors":[[999,[9,9,9,9,9,9,9,9]]]}'
curl -s -m 5 -X POST -H "Authorization: Bearer $COGNITUM_COG_TOKEN" -H 'Content-Type: application/json' \
  -d '{"vectors":[[1,[0.1,0.2,0.3,0.4,0.5,0.6,0.7,0.8]]],"dedup":true}' \
  "$COGNITUM_INGEST_URL" > "$COGNITUM_COG_DATA_DIR/resp.json"
exec sleep 30
"#;

async fn post(addr: std::net::SocketAddr, token: &str, body: &str) -> u16 {
    let raw = format!(
        "POST /api/v1/store/ingest HTTP/1.1\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(raw.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out).await;
    String::from_utf8_lossy(&out)
        .split(' ')
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0)
}

async fn until(what: &str, mut f: impl FnMut() -> bool) {
    for _ in 0..100 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for {what}");
}

#[tokio::test]
async fn placed_cog_vectors_reach_the_project_store_and_stop_and_unload_revoke_its_token() {
    if std::process::Command::new("curl").arg("--version").output().is_err() {
        eprintln!("curl not found: skipping");
        return;
    }
    let key = SigningKey::from_bytes(&[10; 32]);
    // The node's bridge and the project's store (on this node).
    let (proj_store, ctl_store) = (
        Arc::new(MemoryIngestStore::new(1000)),
        Arc::new(MemoryIngestStore::new(1000)),
    );
    let dir: Arc<dyn StoreDirectory> = Arc::new(
        StaticDirectory::new()
            .with_project(PROJECT, proj_store.clone())
            .with_fallback(ctl_store.clone()),
    );
    let fwd = Arc::new(LocalForwarder::new("host", dir));
    let registry = Arc::new(TokenRegistry::new());
    let bridge = IngestBridge::new(
        registry.clone(),
        Arc::new(StaticRouter::new().with_project(PROJECT, fwd)),
        RateBudget::default(),
        BridgeConfig::default(),
    );
    let listener = bridge
        .bind("127.0.0.1:0".parse().unwrap(), BridgeScope::Any)
        .await
        .unwrap();
    let addr = listener.addr();
    let hooks = IngestHooks::new(registry.clone(), bridge.clone(), addr, None);

    let host = host_node_ingest(11, board_caps("pi5"), &key, hooks);
    let tmp = tempfile::tempdir().unwrap();
    let pkg = package(tmp.path(), "ingest-cog", COG, &[arch()]);
    let conn = Arc::new(MeshConnector::new(false));
    let host_addr = conn.register_local("host", host.svc.clone());
    let (plane, _chain) = controller(&key, conn);
    plane.add_target(&host_addr, TrustTier::Paired).await.unwrap();

    let order = PlaceOrder {
        package_dir: pkg,
        config: CtlConfig {
            mode: RunMode::Listener,
            args: vec![],
            csi_port: 15007,
        },
        pin: None,
        prefer: vec![],
        avoid: vec![],
        allow_emulated: false,
        start: true,
        dry_run: false,
        project_id: Some(PROJECT.to_string()),
    };
    let r = plane.place(&order).await.unwrap();
    let rec = r.placed.clone().expect("placed");
    assert_eq!(rec.project_id.as_deref(), Some(PROJECT), "{}", r.explain);
    assert!(registry.contains(&rec.instance_id), "token issued at place time");

    // The cog posts with its injected token and URL; vectors land in the
    // project's store (not the controller's), and the lying stdout does not.
    until("vectors in the project store", || proj_store.len() == 1).await;
    assert_eq!(proj_store.query(&[0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8], 1)[0].0, 1);
    assert_eq!(ctl_store.len(), 0);
    let p = proj_store.provenance(1).unwrap();
    assert_eq!(p.instance_id, rec.instance_id);

    let token_file = host
        ._tmp
        .path()
        .join("instances")
        .join(&rec.instance_id)
        .join("data/token");
    let token = std::fs::read_to_string(token_file).unwrap().trim().to_string();
    let more = r#"{"vectors":[[2,[1,1,1,1,1,1,1,1]]]}"#;
    assert_eq!(post(addr, &token, more).await, 200);
    assert_eq!(proj_store.len(), 2);

    // stdout is evidence only: the cog's claim of 999 vectors never landed.
    plane.instance(method::STOP, &rec.instance_id).await.unwrap();
    let logs = plane.instance(method::LOGS, &rec.instance_id).await.unwrap();
    assert!(logs["stdout"].as_str().unwrap().contains("999"));
    assert_eq!(proj_store.len(), 2);

    // Stop revokes the token; start issues it again; unload revokes for good.
    assert!(!registry.contains(&rec.instance_id));
    assert_eq!(post(addr, &token, more).await, 401, "stopped: revoked");
    plane.instance(method::START, &rec.instance_id).await.unwrap();
    assert!(registry.contains(&rec.instance_id));
    assert_eq!(post(addr, &token, more).await, 200, "restarted: accepted again");
    plane.instance(method::STOP, &rec.instance_id).await.unwrap();
    plane.instance(method::UNLOAD, &rec.instance_id).await.unwrap();
    assert!(registry.is_empty());
    assert_eq!(post(addr, &token, more).await, 401, "unloaded: revoked");
    assert_eq!(proj_store.len(), 2);
}

#[tokio::test]
async fn a_project_less_placement_delivers_to_the_controllers_store() {
    let key = SigningKey::from_bytes(&[12; 32]);
    let ctl_node = crate::node_registry::node_id_from_pubkey(&key.verifying_key().to_bytes());
    let ctl_store = Arc::new(MemoryIngestStore::new(1000));
    let dir: Arc<dyn StoreDirectory> =
        Arc::new(StaticDirectory::new().with_fallback(ctl_store.clone()));
    let registry = Arc::new(TokenRegistry::new());
    let bridge = IngestBridge::new(
        registry.clone(),
        Arc::new(StaticRouter::new().with_controller(&ctl_node, Arc::new(LocalForwarder::new("h", dir)))),
        RateBudget::default(),
        BridgeConfig::default(),
    );
    let l = bridge
        .bind("127.0.0.1:0".parse().unwrap(), BridgeScope::Any)
        .await
        .unwrap();
    let hooks = IngestHooks::new(registry.clone(), bridge, l.addr(), None);
    let host = host_node_ingest(13, board_caps("pi5"), &key, hooks);
    let tmp = tempfile::tempdir().unwrap();
    let pkg = package(tmp.path(), "ingest-cog", COG, &[arch()]);
    let conn = Arc::new(MeshConnector::new(false));
    let a = conn.register_local("host", host.svc.clone());
    let (plane, _c) = controller(&key, conn);
    plane.add_target(&a, TrustTier::Paired).await.unwrap();
    let order = PlaceOrder {
        package_dir: pkg,
        config: CtlConfig { mode: RunMode::Listener, args: vec![], csi_port: 15008 },
        pin: None,
        prefer: vec![],
        avoid: vec![],
        allow_emulated: false,
        start: true,
        dry_run: false,
        project_id: None,
    };
    let r = plane.place(&order).await.unwrap();
    let id = r.placed.unwrap().instance_id;
    if std::process::Command::new("curl").arg("--version").output().is_ok() {
        until("vectors in the controller's store", || ctl_store.len() == 1).await;
    }
    plane.instance(method::UNLOAD, &id).await.unwrap();
    assert!(registry.is_empty());
}

#[tokio::test]
async fn a_malformed_project_id_is_refused_by_the_host() {
    let key = SigningKey::from_bytes(&[14; 32]);
    let registry = Arc::new(TokenRegistry::new());
    let bridge = IngestBridge::new(
        registry.clone(),
        Arc::new(StaticRouter::new()),
        RateBudget::default(),
        BridgeConfig::default(),
    );
    let hooks = IngestHooks::new(registry.clone(), bridge, "127.0.0.1:1".parse().unwrap(), None);
    let host = host_node_ingest(15, board_caps("pi5"), &key, hooks);
    let tmp = tempfile::tempdir().unwrap();
    let pkg = package(tmp.path(), "ingest-cog", COG, &[arch()]);
    let conn = Arc::new(MeshConnector::new(false));
    let a = conn.register_local("host", host.svc.clone());
    let (plane, _c) = controller(&key, conn);
    plane.add_target(&a, TrustTier::Paired).await.unwrap();
    let order = PlaceOrder {
        package_dir: pkg,
        config: CtlConfig { mode: RunMode::Listener, args: vec![], csi_port: 15009 },
        pin: None,
        prefer: vec![],
        avoid: vec![],
        allow_emulated: false,
        start: true,
        dry_run: false,
        project_id: Some("not-a-project".into()),
    };
    let r = plane.place(&order).await.unwrap();
    assert!(r.placed.is_none(), "{}", r.explain);
    assert!(registry.is_empty());
}
