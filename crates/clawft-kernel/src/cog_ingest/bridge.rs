//! The node-local HTTP bridge: `POST /api/v1/store/ingest`.
//!
//! A deliberately small HTTP/1.1 server: one request per connection,
//! `Content-Length` bodies only, strict limits, no keep-alive (the Seed
//! ingest endpoint stalls keep-alive connections, so cogs already cope).
//!
//! Order of checks per request: request line, headers (size-bounded),
//! token (401 unknown, 403 another instance's), per-instance request rate
//! (429), body length (411/413), body read, shape validation (400/413),
//! per-instance vector budget (429), then the store owner. Nothing is read
//! from the body of an unauthenticated request.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use super::registry::{InstanceBinding, RateBudget, StoreRouter, TokenRegistry};
use super::types::{INGEST_PATH, IngestError, MAX_BODY_BYTES, parse_batch};

/// Largest request head (request line and headers).
pub const MAX_HEAD_BYTES: usize = 8 * 1024;
/// Most concurrent connections one bridge listener serves.
pub const MAX_CONNECTIONS: usize = 64;

/// Which instances a listener accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeScope {
    /// Only this instance's token (one listener per instance; the address
    /// goes into [`HostContract::with_ingest_upstream`](crate::workload_runtime::HostContract)).
    Instance(String),
    /// Only the holder of one token, identified by its BLAKE3 hash. For a
    /// listener bound before the instance id exists (the token is issued
    /// with the host contract, before the adapter names the instance).
    Token([u8; 32]),
    /// Any registered instance (the node's shared `127.0.0.1:80`).
    Any,
}

/// Bridge settings.
#[derive(Debug, Clone)]
pub struct BridgeConfig {
    /// Time allowed to receive a request head and body.
    pub read_timeout: Duration,
    /// Allow binding a non-loopback address. Only for an instance-scoped
    /// listener whose address a container relay needs (the VM gateway);
    /// the token still applies.
    pub allow_non_loopback: bool,
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            read_timeout: Duration::from_secs(5),
            allow_non_loopback: false,
        }
    }
}

/// Counters, for evidence and tests.
#[derive(Debug, Default)]
pub struct BridgeStats {
    /// Batches forwarded and written.
    pub accepted: AtomicU64,
    /// Vectors written.
    pub vectors: AtomicU64,
    /// Requests refused before the owner (auth, shape, size, rate).
    pub rejected: AtomicU64,
    /// Requests the owner could not take.
    pub failed: AtomicU64,
}

/// The bridge.
pub struct IngestBridge {
    registry: Arc<TokenRegistry>,
    router: Arc<dyn StoreRouter>,
    budget: RateBudget,
    cfg: BridgeConfig,
    stats: BridgeStats,
}

/// A running listener.
pub struct BridgeHandle {
    addr: SocketAddr,
    task: JoinHandle<()>,
}

impl BridgeHandle {
    /// Address bound.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Drop for BridgeHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl IngestBridge {
    /// Bridge over `registry` and `router`.
    pub fn new(
        registry: Arc<TokenRegistry>,
        router: Arc<dyn StoreRouter>,
        budget: RateBudget,
        cfg: BridgeConfig,
    ) -> Arc<Self> {
        Arc::new(Self {
            registry,
            router,
            budget,
            cfg,
            stats: BridgeStats::default(),
        })
    }

    /// Drop an instance's rate counters (at unload).
    pub fn forget(&self, instance_id: &str) {
        self.budget.forget(instance_id);
    }

    /// Counters.
    pub fn stats(&self) -> &BridgeStats {
        &self.stats
    }

    /// Bind `addr` and serve `scope`. A non-loopback address is refused
    /// unless the config allows it and the scope is one instance.
    pub async fn bind(
        self: &Arc<Self>,
        addr: SocketAddr,
        scope: BridgeScope,
    ) -> Result<BridgeHandle, IngestError> {
        let one_instance = matches!(scope, BridgeScope::Instance(_) | BridgeScope::Token(_));
        if !(addr.ip().is_loopback() || self.cfg.allow_non_loopback && one_instance) {
            return Err(IngestError::Malformed(format!(
                "refusing to bind {addr}: the bridge listens on loopback, or on one \
                 instance-scoped address when allow_non_loopback is set"
            )));
        }
        let listener = TcpListener::bind(addr)
            .await
            .map_err(|e| IngestError::Unavailable(format!("bind {addr}: {e}")))?;
        let bound = listener
            .local_addr()
            .map_err(|e| IngestError::Unavailable(e.to_string()))?;
        let me = self.clone();
        let slots = Arc::new(tokio::sync::Semaphore::new(MAX_CONNECTIONS));
        let task = tokio::spawn(async move {
            loop {
                let Ok((sock, _)) = listener.accept().await else {
                    return;
                };
                let Ok(slot) = slots.clone().try_acquire_owned() else {
                    continue; // over the connection cap: dropped
                };
                let (me, scope) = (me.clone(), scope.clone());
                tokio::spawn(async move {
                    let _slot = slot;
                    me.serve(sock, &scope).await;
                });
            }
        });
        Ok(BridgeHandle { addr: bound, task })
    }

    async fn serve(&self, mut sock: TcpStream, scope: &BridgeScope) {
        let res = tokio::time::timeout(self.cfg.read_timeout, self.handle(&mut sock, scope)).await;
        let (status, body, extra) = match res {
            Ok(Ok(v)) => (200, v, None),
            Ok(Err(e)) => {
                match &e {
                    IngestError::Unavailable(_) => self.stats.failed.fetch_add(1, Ordering::Relaxed),
                    _ => self.stats.rejected.fetch_add(1, Ordering::Relaxed),
                };
                let extra = matches!(e, IngestError::RateLimited).then_some("Retry-After: 1\r\n");
                // Owner-side detail stays in the log, not on the wire.
                let msg = match &e {
                    IngestError::Unavailable(d) => {
                        tracing::warn!(detail = %d, "cog ingest forward failed");
                        "store unavailable".to_string()
                    }
                    other => other.to_string(),
                };
                (e.status(), serde_json::json!({"ok": false, "error": msg}), extra)
            }
            Err(_) => {
                self.stats.rejected.fetch_add(1, Ordering::Relaxed);
                (408, serde_json::json!({"ok": false, "error": "timeout"}), None)
            }
        };
        let body = body.to_string();
        let head = format!(
            "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{}Connection: close\r\n\r\n",
            reason(status),
            body.len(),
            extra.unwrap_or("")
        );
        let _ = sock.write_all(head.as_bytes()).await;
        let _ = sock.write_all(body.as_bytes()).await;
        let _ = sock.shutdown().await;
    }

    async fn handle(
        &self,
        sock: &mut TcpStream,
        scope: &BridgeScope,
    ) -> Result<serde_json::Value, IngestError> {
        let (head, mut body) = read_head(sock).await?;
        let req = parse_head(&head)?;
        if req.path != INGEST_PATH {
            return Err(IngestError::Malformed(format!("no such path `{}`", req.path)));
        }
        if req.method != "POST" {
            return Err(IngestError::Malformed("POST only".into()));
        }
        let binding = self.authenticate(req.token.as_deref(), scope)?;
        if !self.budget.charge_request(&binding.instance_id) {
            return Err(IngestError::RateLimited);
        }
        let len = req
            .content_length
            .ok_or_else(|| IngestError::Malformed("Content-Length required".into()))?;
        if len > MAX_BODY_BYTES {
            return Err(IngestError::TooLarge(format!(
                "body over {MAX_BODY_BYTES} bytes"
            )));
        }
        if body.len() > len {
            body.truncate(len); // pipelined bytes are not a second request
        }
        while body.len() < len {
            let mut chunk = vec![0u8; (len - body.len()).min(8192)];
            let n = sock
                .read(&mut chunk)
                .await
                .map_err(|e| IngestError::Malformed(format!("read: {e}")))?;
            if n == 0 {
                return Err(IngestError::Malformed("body shorter than Content-Length".into()));
            }
            body.extend_from_slice(&chunk[..n]);
        }
        let batch = parse_batch(&body)?;
        if !self.budget.charge_vectors(&binding.instance_id, batch.vectors.len()) {
            return Err(IngestError::RateLimited);
        }
        let fwd = self
            .router
            .route(&binding)
            .ok_or_else(|| IngestError::Unavailable("no store owner for this placement".into()))?;
        let out = fwd.forward(&binding, &batch).await?;
        self.stats.accepted.fetch_add(1, Ordering::Relaxed);
        self.stats
            .vectors
            .fetch_add(out.accepted as u64, Ordering::Relaxed);
        Ok(serde_json::json!({
            "ok": true,
            "accepted": out.accepted,
            "deduped": out.deduped,
        }))
    }

    fn authenticate(
        &self,
        token: Option<&str>,
        scope: &BridgeScope,
    ) -> Result<InstanceBinding, IngestError> {
        let b = token
            .and_then(|t| self.registry.lookup(t))
            .ok_or(IngestError::Unauthorized)?;
        match scope {
            BridgeScope::Instance(id) if *id != b.instance_id => Err(IngestError::Forbidden),
            BridgeScope::Token(h)
                if token.map(|t| blake3::hash(t.as_bytes())) != Some(blake3::Hash::from(*h)) =>
            {
                Err(IngestError::Forbidden)
            }
            _ => Ok(b),
        }
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        408 => "Request Timeout",
        411 => "Length Required",
        413 => "Payload Too Large",
        429 => "Too Many Requests",
        502 => "Bad Gateway",
        _ => "Error",
    }
}

struct Head {
    method: String,
    path: String,
    content_length: Option<usize>,
    token: Option<String>,
}

/// Read until the blank line; returns the head text and any body bytes
/// already read.
async fn read_head(sock: &mut TcpStream) -> Result<(String, Vec<u8>), IngestError> {
    let mut buf = Vec::with_capacity(1024);
    loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let rest = buf.split_off(i + 4);
            buf.truncate(i);
            let head = String::from_utf8(buf)
                .map_err(|_| IngestError::Malformed("head is not UTF-8".into()))?;
            return Ok((head, rest));
        }
        if buf.len() > MAX_HEAD_BYTES {
            return Err(IngestError::TooLarge("request head".into()));
        }
        let mut chunk = [0u8; 1024];
        let n = sock
            .read(&mut chunk)
            .await
            .map_err(|e| IngestError::Malformed(format!("read: {e}")))?;
        if n == 0 {
            return Err(IngestError::Malformed("connection closed in head".into()));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

fn parse_head(head: &str) -> Result<Head, IngestError> {
    let bad = |m: &str| IngestError::Malformed(m.to_string());
    let mut lines = head.split("\r\n");
    let line = lines.next().ok_or_else(|| bad("empty request"))?;
    let mut parts = line.split(' ');
    let (method, target, version) = (parts.next(), parts.next(), parts.next());
    let (Some(method), Some(target), Some(version), None) = (method, target, version, parts.next())
    else {
        return Err(bad("bad request line"));
    };
    if version != "HTTP/1.1" && version != "HTTP/1.0" {
        return Err(bad("unsupported HTTP version"));
    }
    let path = target.split('?').next().unwrap_or("").to_string();
    let (mut content_length, mut token) = (None, None);
    for l in lines {
        let (k, v) = l.split_once(':').ok_or_else(|| bad("bad header line"))?;
        let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
        match k.as_str() {
            "content-length" => {
                if content_length.is_some() {
                    return Err(bad("duplicate Content-Length"));
                }
                content_length = Some(v.parse::<usize>().map_err(|_| bad("bad Content-Length"))?);
            }
            "transfer-encoding" => return Err(bad("Transfer-Encoding is not supported")),
            "authorization" => {
                let t = v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer "));
                token = token.or(t.map(|t| t.trim().to_string()));
            }
            "x-api-key" | "x-cog-token" => token = token.or(Some(v.to_string())),
            _ => {}
        }
    }
    Ok(Head {
        method: method.to_string(),
        path,
        content_length,
        token,
    })
}
