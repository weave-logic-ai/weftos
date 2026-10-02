//! Loopback-only health endpoint (plan 2 S): `GET /health` (or `/`) returns a
//! small JSON status. It listens on a loopback address only (checked after
//! bind as well as in the configuration) and exposes counts and the node id,
//! never keys, uids or registrations.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use crate::state::{admission_str, ServiceState};

const REQUEST_DEADLINE: Duration = Duration::from_secs(2);
const MAX_REQUEST: usize = 2048;

/// The JSON body.
pub fn body(st: &ServiceState) -> String {
    let (read_only, degraded, seq) = {
        let c = st.core.lock().expect("core lock");
        (c.journal.read_only(), c.bindings.degraded().is_some(), c.journal.head().map(|h| h.seq))
    };
    serde_json::json!({
        "ok": !degraded,
        "node_id": st.node_id,
        "build_sha": st.cfg.build_sha,
        "started_at": st.started_at,
        "registered": st.registry.len(),
        "peers": st.router.runtime().map_or(0, |r| r.peer_count()),
        "admission": admission_str(st.policy.admission()),
        "journal": {"seq": seq, "read_only": read_only, "degraded": degraded},
    })
    .to_string()
}

async fn respond(st: Arc<ServiceState>, mut s: TcpStream) {
    let mut buf = vec![0u8; MAX_REQUEST];
    let n = match tokio::time::timeout(REQUEST_DEADLINE, s.read(&mut buf)).await {
        Ok(Ok(n)) => n,
        _ => return,
    };
    let line = String::from_utf8_lossy(&buf[..n]);
    let first = line.lines().next().unwrap_or_default();
    let ok = first.starts_with("GET /health ") || first.starts_with("GET / ");
    let (status, payload) = if ok { ("200 OK", body(&st)) } else { ("404 Not Found", "{}".to_string()) };
    let resp = format!(
        "HTTP/1.0 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    let _ = tokio::time::timeout(REQUEST_DEADLINE, s.write_all(resp.as_bytes())).await;
    let _ = s.shutdown().await;
}

/// Bind `addr` (must be loopback) and serve until the task is aborted.
pub async fn spawn(st: Arc<ServiceState>, addr: &str) -> std::io::Result<(SocketAddr, JoinHandle<()>)> {
    let listener = TcpListener::bind(addr).await?;
    let local = listener.local_addr()?;
    if !local.ip().is_loopback() {
        return Err(std::io::Error::other("health endpoint must bind a loopback address"));
    }
    let h = tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((s, _)) => {
                    tokio::spawn(respond(Arc::clone(&st), s));
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
    });
    Ok((local, h))
}
