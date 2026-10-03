//! The loopback listener that keeps `127.0.0.1:<role port>` stable.
//!
//! Binding rules:
//! - Loopback addresses only. A non-loopback address is refused.
//! - A port something already answers on is never bound over. The caller
//!   chooses [`OccupiedPolicy::Refuse`] (an error) or
//!   [`OccupiedPolicy::Adopt`] (leave the existing server as the address;
//!   the card-18 adapter registers it as an `Adopted` instance).
//! - The socket is bound without `SO_REUSEADDR`, so a wildcard listener on
//!   the same port also makes the bind fail instead of being shadowed.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpSocket, TcpStream};
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;

use super::http::{read_request, write_error, write_head};
use super::mesh_forward::forward_remote;
use super::table::PlacementTable;
use super::types::{ProxyAudit, ProxyError, ProxyLimits, ResponseSink, Target};
use super::upstream::{PROBE_TIMEOUT, Upstream};

/// What to do when the port is already held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OccupiedPolicy {
    /// Fail with [`ProxyError::Occupied`].
    Refuse,
    /// Do not bind; the existing server stays the address.
    Adopt,
}

/// Result of [`InferProxy::start`].
pub enum Started {
    /// The proxy is listening.
    Running(InferProxy),
    /// Something else holds the port; nothing was bound.
    Adopted(SocketAddr),
}

/// Counters, readable while running.
#[derive(Debug, Default)]
pub struct ProxyStats {
    /// Requests accepted.
    pub requests: AtomicU64,
    /// Forwarded to a local instance.
    pub local: AtomicU64,
    /// Forwarded over the mesh.
    pub remote: AtomicU64,
    /// Refused or failed before any forwarding.
    pub rejected: AtomicU64,
}

/// A running listener for one role.
pub struct InferProxy {
    role: String,
    addr: SocketAddr,
    stats: Arc<ProxyStats>,
    task: JoinHandle<()>,
}

impl InferProxy {
    /// The bound address.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
    /// The role this listener fronts.
    pub fn role(&self) -> &str {
        &self.role
    }
    /// Counters.
    pub fn stats(&self) -> &ProxyStats {
        &self.stats
    }
    /// Stop listening (in-flight requests finish on their own tasks).
    pub fn stop(&self) {
        self.task.abort();
    }
}

impl Drop for InferProxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn held(addr: SocketAddr) -> bool {
    matches!(
        tokio::time::timeout(PROBE_TIMEOUT, TcpStream::connect(addr)).await,
        Ok(Ok(_))
    )
}

struct Shared {
    role: String,
    table: Arc<PlacementTable>,
    upstream: Upstream,
    limits: ProxyLimits,
    stats: Arc<ProxyStats>,
}

struct TcpSink<'a> {
    s: &'a mut TcpStream,
    sent_head: bool,
}

#[async_trait]
impl ResponseSink for TcpSink<'_> {
    async fn head(&mut self, status: u16, content_type: Option<&str>) -> Result<(), ProxyError> {
        write_head(self.s, status, content_type).await?;
        self.sent_head = true;
        Ok(())
    }
    async fn chunk(&mut self, data: &[u8]) -> Result<(), ProxyError> {
        self.s
            .write_all(data)
            .await
            .map_err(|e| ProxyError::Io(e.to_string()))
    }
}

async fn serve_conn(sh: Arc<Shared>, mut s: TcpStream) {
    sh.stats.requests.fetch_add(1, Ordering::Relaxed);
    let req = match read_request(&mut s, &sh.role, &sh.limits).await {
        Ok(r) => r,
        Err(e) => {
            sh.stats.rejected.fetch_add(1, Ordering::Relaxed);
            write_error(&mut s, &e).await;
            return;
        }
    };
    let target = sh.table.resolve(&req.role);
    let mut sink = TcpSink {
        s: &mut s,
        sent_head: false,
    };
    let fwd = async {
        match &target {
            Some(Target::Local { base }) => {
                sh.stats.local.fetch_add(1, Ordering::Relaxed);
                sh.upstream.forward(base, &req, true, &mut sink).await
            }
            Some(Target::Remote { node_id }) => {
                let Some(d) = sh.table.dialer() else {
                    return Err(ProxyError::NoInstance(req.role.clone()));
                };
                sh.stats.remote.fetch_add(1, Ordering::Relaxed);
                forward_remote(d.as_ref(), node_id, &req, &mut sink, &sh.limits).await
            }
            None => Err(ProxyError::NoInstance(req.role.clone())),
        }
    };
    let res = tokio::time::timeout(sh.limits.request_timeout, fwd)
        .await
        .unwrap_or_else(|_| Err(ProxyError::Timeout("request".into())));
    let sent_head = sink.sent_head;
    match res {
        Ok(()) => {
            let _ = s.shutdown().await;
        }
        Err(e) if !sent_head => {
            sh.stats.rejected.fetch_add(1, Ordering::Relaxed);
            write_error(&mut s, &e).await;
        }
        // Mid-body failure: the status is already out, so cut the stream.
        Err(_) => {
            let _ = s.shutdown().await;
        }
    }
}

impl InferProxy {
    /// Start fronting `role` on `addr` (must be loopback).
    pub async fn start(
        role: &str,
        addr: SocketAddr,
        policy: OccupiedPolicy,
        table: Arc<PlacementTable>,
        limits: ProxyLimits,
        audit: Option<Arc<dyn ProxyAudit>>,
    ) -> Result<Started, ProxyError> {
        let note = |kind: &str, extra: serde_json::Value| {
            if let Some(a) = &audit {
                a.record(
                    kind,
                    serde_json::json!({"role": role, "addr": addr.to_string(), "detail": extra}),
                );
            }
        };
        if !addr.ip().is_loopback() {
            note("infer.proxy.refused", serde_json::json!("not loopback"));
            return Err(ProxyError::NotLoopback(addr.to_string()));
        }
        // Probe the same port on every loopback name a client might use, v4
        // and v6, so a server bound to the wildcard or to the other family
        // is seen as well.
        let probes = [
            addr,
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), addr.port()),
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), addr.port()),
        ];
        for p in probes {
            if addr.port() != 0 && held(p).await {
                note("infer.proxy.occupied", serde_json::json!(format!("{policy:?}")));
                return match policy {
                    OccupiedPolicy::Refuse => Err(ProxyError::Occupied(addr.port())),
                    OccupiedPolicy::Adopt => Ok(Started::Adopted(addr)),
                };
            }
        }
        let socket = match addr {
            SocketAddr::V4(_) => TcpSocket::new_v4(),
            SocketAddr::V6(_) => TcpSocket::new_v6(),
        }
        .map_err(|e| ProxyError::Io(e.to_string()))?;
        socket
            .set_reuseaddr(false)
            .map_err(|e| ProxyError::Io(e.to_string()))?;
        if socket.bind(addr).is_err() {
            note("infer.proxy.occupied", serde_json::json!("bind failed"));
            return match policy {
                OccupiedPolicy::Refuse => Err(ProxyError::Occupied(addr.port())),
                OccupiedPolicy::Adopt => Ok(Started::Adopted(addr)),
            };
        }
        let listener: TcpListener = socket
            .listen(128)
            .map_err(|e| ProxyError::Io(e.to_string()))?;
        let bound = listener
            .local_addr()
            .map_err(|e| ProxyError::Io(e.to_string()))?;
        table.set_proxy_port(role, bound.port());
        note("infer.proxy.bind", serde_json::json!("listening"));

        let stats = Arc::new(ProxyStats::default());
        let sh = Arc::new(Shared {
            role: role.to_string(),
            table,
            upstream: Upstream::new(limits.clone())?,
            limits: limits.clone(),
            stats: stats.clone(),
        });
        let permits = Arc::new(Semaphore::new(limits.max_connections));
        let task = tokio::spawn(async move {
            let mut backoff = Duration::from_millis(10);
            loop {
                let (s, peer) = match listener.accept().await {
                    Ok(x) => {
                        backoff = Duration::from_millis(10);
                        x
                    }
                    Err(_) => {
                        // EMFILE and friends: do not spin.
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(Duration::from_secs(1));
                        continue;
                    }
                };
                // Loopback listener: a non-loopback peer cannot arrive, but
                // the check costs nothing.
                if !peer.ip().is_loopback() {
                    continue;
                }
                let Ok(permit) = permits.clone().try_acquire_owned() else {
                    // Over the connection cap: shed load.
                    drop(s);
                    continue;
                };
                let sh = sh.clone();
                tokio::spawn(async move {
                    serve_conn(sh, s).await;
                    drop(permit);
                });
            }
        });
        Ok(Started::Running(Self {
            role: role.to_string(),
            addr: bound,
            stats,
            task,
        }))
    }
}
