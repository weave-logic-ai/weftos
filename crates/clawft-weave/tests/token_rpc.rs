//! Daemon-level tests for the token authority (ADR-102 cards 01/02):
//! capability resolution on the real dispatch path, including the TCP
//! relay and an untrusted unix peer.

use std::sync::Arc;
use std::time::Duration;

use clawft_kernel::boot::Kernel;
use clawft_platform::NativePlatform;
use clawft_types::config::{ChainConfig, Config, KernelConfig};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream, UnixListener, UnixStream};
use tokio::sync::{RwLock, watch};

type KernelRef = Arc<RwLock<Kernel<NativePlatform>>>;

struct Daemon {
    _tmp: tempfile::TempDir,
    sock: std::path::PathBuf,
    kernel: KernelRef,
    _shutdown: watch::Sender<bool>,
}

/// `untrusted`: serve every connection as a peer that is not our uid.
async fn spawn(untrusted: bool) -> Daemon {
    let tmp = tempfile::tempdir().unwrap();
    let sock = tmp.path().join("kernel.sock");
    let kcfg = KernelConfig {
        chain: Some(ChainConfig::isolated_in(
            &tempfile::tempdir().unwrap().keep(),
        )),
        ..KernelConfig::default()
    };
    let kernel = Kernel::boot(Config::default(), kcfg, Arc::new(NativePlatform::new()))
        .await
        .expect("boot");
    let kernel: KernelRef = Arc::new(RwLock::new(kernel));
    let listener = UnixListener::bind(&sock).unwrap();
    let (tx, _rx) = watch::channel(false);
    let (k, t) = (Arc::clone(&kernel), tx.clone());
    tokio::spawn(async move {
        while let Ok((s, _)) = listener.accept().await {
            tokio::spawn(clawft_weave::daemon::handle_connection_peer(
                s,
                Arc::clone(&k),
                t.clone(),
                untrusted,
            ));
        }
    });
    tokio::time::sleep(Duration::from_millis(30)).await;
    Daemon {
        _tmp: tmp,
        sock,
        kernel,
        _shutdown: tx,
    }
}

async fn exchange<R, W>(r: R, mut w: W, method: &str, params: Value, auth: Option<&str>) -> Value
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut req = json!({ "id": "t", "method": method, "params": params });
    if let Some(a) = auth {
        req["auth"] = json!(a);
    }
    w.write_all(format!("{req}\n").as_bytes()).await.unwrap();
    let mut line = String::new();
    BufReader::new(r).read_line(&mut line).await.unwrap();
    serde_json::from_str(line.trim()).unwrap()
}

async fn call(d: &Daemon, method: &str, params: Value, auth: Option<&str>) -> Value {
    let (r, w) = UnixStream::connect(&d.sock).await.unwrap().into_split();
    exchange(r, w, method, params, auth).await
}

/// Same call, but through the sanitising TCP relay.
async fn call_via_relay(d: &Daemon, method: &str, params: Value, auth: Option<&str>) -> Value {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let sock = d.sock.clone();
    tokio::spawn(async move {
        let (tcp, _) = l.accept().await.unwrap();
        let unix = UnixStream::connect(sock).await.unwrap();
        let _ = clawft_weave::relay_auth::relay(tcp, unix).await;
    });
    let (r, w) = TcpStream::connect(addr).await.unwrap().into_split();
    exchange(r, w, method, params, auth).await
}

fn denied(v: &Value) -> bool {
    v["ok"] == false
        && v["error"]
            .as_str()
            .unwrap_or("")
            .contains("permission denied")
}

#[tokio::test]
async fn local_owner_issues_validates_lists_revokes() {
    let d = spawn(false).await;
    let v = call(
        &d,
        "auth.token.issue",
        json!({"label": "pg", "ttl_secs": 60}),
        Some("admin"),
    )
    .await;
    assert_eq!(v["ok"], true, "{v}");
    let secret = v["result"]["secret"].as_str().unwrap().to_owned();
    let id = v["result"]["id"].as_str().unwrap().to_owned();

    let val = call(&d, "auth.token.validate", json!({"token": secret}), None).await;
    assert_eq!(val["result"]["valid"], true);
    let list = call(&d, "auth.token.list", Value::Null, Some("admin")).await;
    assert_eq!(list["result"]["tokens"][0]["id"], id);

    // the secret never reaches the chain
    let chain = serde_json::to_string(&d.kernel.read().await.chain_manager().unwrap().tail_from(0))
        .unwrap();
    assert!(chain.contains("auth.token.issued"));
    assert!(!chain.contains(&secret));

    let rv = call(&d, "auth.token.revoke", json!({"id": id}), Some("admin")).await;
    assert_eq!(rv["result"]["revoked"], true);
    let val = call(&d, "auth.token.validate", json!({"token": secret}), None).await;
    assert_eq!(val["result"]["valid"], false);
}

#[tokio::test]
async fn anonymous_cannot_issue_but_can_validate() {
    let d = spawn(false).await;
    assert!(denied(&call(&d, "auth.token.issue", json!({}), None).await));
    assert!(denied(
        &call(&d, "auth.token.list", Value::Null, None).await
    ));
    let v = call(&d, "auth.token.validate", json!({"token": "wft_x"}), None).await;
    assert_eq!(v["result"]["valid"], false);
}

#[tokio::test]
async fn token_bearer_gets_owner_caps_but_cannot_mint() {
    let d = spawn(false).await;
    let v = call(&d, "auth.token.issue", json!({}), Some("admin")).await;
    let secret = v["result"]["secret"].as_str().unwrap().to_owned();
    // Admin-gated method the bearer may use: passes the capability check
    // and is refused by the token-cannot-mint rule, not "permission denied".
    let r = call(&d, "auth.token.issue", json!({}), Some(&secret)).await;
    assert_eq!(r["error_kind"], "token_cannot_manage_tokens", "{r}");
    // A Write verb passes the capability check with the token.
    let w = call(&d, "memory.delete", json!({}), Some(&secret)).await;
    assert!(!denied(&w), "{w}");
}

#[tokio::test]
async fn revoked_or_unknown_token_is_denied_not_anonymous() {
    let d = spawn(false).await;
    let v = call(&d, "auth.token.issue", json!({}), Some("admin")).await;
    let (secret, id) = (
        v["result"]["secret"].as_str().unwrap().to_owned(),
        v["result"]["id"].as_str().unwrap().to_owned(),
    );
    call(&d, "auth.token.revoke", json!({"id": id}), Some("admin")).await;
    assert!(denied(
        &call(&d, "kernel.status", Value::Null, Some(&secret)).await
    ));
    assert!(denied(
        &call(&d, "kernel.status", Value::Null, Some("wft_deadbeef")).await
    ));
}

#[tokio::test]
async fn literal_admin_over_tcp_relay_no_longer_grants() {
    let d = spawn(false).await;
    // Direct unix socket, same uid: the ADR-070 owner shortcut still works.
    let ok = call(&d, "auth.token.issue", json!({}), Some("admin")).await;
    assert_eq!(ok["ok"], true);
    // Through the relay the literal is stripped: anonymous, refused.
    let r = call_via_relay(&d, "auth.token.issue", json!({}), Some("admin")).await;
    assert!(denied(&r), "{r}");
    let r = call_via_relay(&d, "kernel.shutdown", Value::Null, Some("admin")).await;
    assert!(denied(&r), "{r}");
    // A token still works through the relay (for a non-admin-minting verb).
    let secret = ok["result"]["secret"].as_str().unwrap().to_owned();
    let r = call_via_relay(
        &d,
        "auth.token.validate",
        json!({"token": secret}),
        Some(&secret),
    )
    .await;
    assert_eq!(r["result"]["valid"], true, "{r}");
}

#[tokio::test]
async fn literal_scope_from_untrusted_unix_peer_is_denied() {
    let d = spawn(true).await;
    assert!(denied(
        &call(&d, "auth.token.issue", json!({}), Some("admin")).await
    ));
    // anonymous reads still work for that peer
    let v = call(&d, "auth.token.validate", json!({"token": "x"}), None).await;
    assert_eq!(v["ok"], true);
}

#[tokio::test]
async fn voice_principal_scope_is_unaffected() {
    let d = spawn(false).await;
    // The voice principal's literal (read,chat,write) still resolves on a
    // trusted peer, and still cannot reach Admin verbs.
    let v = call(&d, "auth.token.issue", json!({}), Some("read,chat,write")).await;
    assert!(denied(&v), "{v}");
    let w = call(&d, "memory.delete", json!({}), Some("read,chat,write")).await;
    assert!(!denied(&w), "{w}");
}
