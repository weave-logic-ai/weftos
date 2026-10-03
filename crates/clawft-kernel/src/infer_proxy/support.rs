//! Test fixtures: a raw fake upstream on a random loopback port, a raw
//! HTTP client, and a mesh dialer over in-memory streams. Nothing here
//! opens a default port or contacts a real server.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use super::mesh_forward::{InferPeer, ServeGate, serve_infer};
use super::table::PlacementTable;
use super::types::{MeshDialer, ProxyAudit, ProxyError, ProxyLimits};
use super::upstream::Upstream;
use crate::mesh::MeshStream;
use crate::mesh_admit::{Grant, PeerClass, PeerLimits};
use crate::mesh_test_support::connected_pair;

/// One request a fake upstream received.
#[derive(Debug, Clone)]
pub struct Seen {
    pub request_line: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Seen {
    pub fn header(&self, n: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(n))
            .map(|(_, v)| v.as_str())
    }
}

/// What the fake answers.
#[derive(Clone)]
pub struct Reply {
    pub status: u16,
    pub content_type: &'static str,
    pub chunks: Vec<Vec<u8>>,
    /// Pause before each chunk.
    pub gap: Duration,
    /// After the chunks, hold the connection open this long before closing.
    pub hold: Duration,
}

impl Reply {
    pub fn ok(body: &str) -> Self {
        Self {
            status: 200,
            content_type: "application/json",
            chunks: vec![body.as_bytes().to_vec()],
            gap: Duration::ZERO,
            hold: Duration::ZERO,
        }
    }
}

pub struct Fake {
    pub addr: SocketAddr,
    pub seen: Arc<Mutex<Vec<Seen>>>,
    task: JoinHandle<()>,
}

impl Fake {
    pub fn base(&self) -> String {
        format!("http://{}", self.addr)
    }
    pub fn count(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
    pub fn last(&self) -> Seen {
        self.seen.lock().unwrap().last().cloned().expect("no request seen")
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn read_req(s: &mut TcpStream) -> Option<Seen> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        let n = s.read(&mut tmp).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..end]).into_owned();
    let mut lines = head.split("\r\n");
    let request_line = lines.next()?.to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':').map(|(a, b)| (a.to_string(), b.trim().to_string())))
        .collect();
    let want: usize = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[end + 4..].to_vec();
    while body.len() < want {
        let n = s.read(&mut tmp).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    Some(Seen {
        request_line,
        headers,
        body,
    })
}

pub async fn fake(reply: Reply) -> Fake {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sn = seen.clone();
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else { return };
            let (sn, reply) = (sn.clone(), reply.clone());
            tokio::spawn(async move {
                let Some(r) = read_req(&mut s).await else { return };
                sn.lock().unwrap().push(r);
                let head = format!(
                    "HTTP/1.1 {} X\r\nContent-Type: {}\r\nConnection: close\r\n\r\n",
                    reply.status, reply.content_type
                );
                let _ = s.write_all(head.as_bytes()).await;
                for c in &reply.chunks {
                    tokio::time::sleep(reply.gap).await;
                    if s.write_all(c).await.is_err() {
                        return;
                    }
                    let _ = s.flush().await;
                }
                tokio::time::sleep(reply.hold).await;
                let _ = s.shutdown().await;
            });
        }
    });
    Fake { addr, seen, task }
}

/// Send raw bytes, return everything read until the peer closes.
pub async fn raw(addr: SocketAddr, req: &[u8]) -> String {
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(req).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut out)).await;
    String::from_utf8_lossy(&out).into_owned()
}

pub fn post(addr: SocketAddr, path: &str, body: &str) -> Vec<u8> {
    format!(
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

pub fn get(addr: SocketAddr, path: &str) -> Vec<u8> {
    format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\n\r\n").into_bytes()
}

pub fn status(resp: &str) -> u16 {
    resp.split(' ').nth(1).and_then(|s| s.parse().ok()).unwrap_or(0)
}

pub fn body(resp: &str) -> &str {
    resp.split_once("\r\n\r\n").map_or("", |(_, b)| b)
}

pub fn small_limits() -> ProxyLimits {
    ProxyLimits {
        stall_timeout: Duration::from_millis(800),
        request_timeout: Duration::from_secs(10),
        head_timeout: Duration::from_secs(2),
        body_timeout: Duration::from_secs(2),
        connect_timeout: Duration::from_secs(1),
        ..ProxyLimits::default()
    }
}

/// Records audit events.
#[derive(Default)]
pub struct Audit(pub Mutex<Vec<(String, serde_json::Value)>>);

impl ProxyAudit for Audit {
    fn record(&self, kind: &str, payload: serde_json::Value) {
        self.0.lock().unwrap().push((kind.to_string(), payload));
    }
}

impl Audit {
    pub fn kinds(&self) -> Vec<String> {
        self.0.lock().unwrap().iter().map(|(k, _)| k.clone()).collect()
    }
}

/// A "mesh" of in-memory streams: dialing node X serves the stream with X's
/// table, as X's daemon would after admitting the caller.
pub struct FakeMesh {
    pub admitted: Mutex<HashSet<String>>,
    /// Per-node class and enforcement overrides (default: enforced Node).
    pub overrides: Mutex<std::collections::HashMap<String, Grant>>,
    pub gate: Arc<ServeGate>,
    pub nodes: Mutex<std::collections::HashMap<String, Arc<PlacementTable>>>,
    /// The grant the serving side holds for the dialing peer.
    pub serve_grant: Mutex<Option<Grant>>,
    pub caller: String,
    pub dials: Mutex<Vec<String>>,
    pub audit: Option<Arc<dyn ProxyAudit>>,
}

impl FakeMesh {
    pub fn new(caller: &str) -> Arc<Self> {
        Arc::new(Self {
            admitted: Mutex::new(HashSet::new()),
            overrides: Mutex::new(Default::default()),
            gate: Arc::new(ServeGate::new(4, 16)),
            nodes: Mutex::new(Default::default()),
            serve_grant: Mutex::new(Some(Grant {
                limits: PeerLimits::None,
                class: PeerClass::Node,
                admitted: true,
                trust_scope: true,
                observed: None,
            })),
            caller: caller.to_string(),
            dials: Mutex::new(Vec::new()),
            audit: None,
        })
    }
    pub fn add_node(&self, id: &str, t: Arc<PlacementTable>, admitted: bool) {
        self.nodes.lock().unwrap().insert(id.to_string(), t);
        if admitted {
            self.admitted.lock().unwrap().insert(id.to_string());
        }
    }
}

#[async_trait]
impl MeshDialer for FakeMesh {
    fn standing(&self, node_id: &str) -> Option<Grant> {
        if let Some(g) = self.overrides.lock().unwrap().get(node_id) {
            return Some(g.clone());
        }
        if !self.admitted.lock().unwrap().contains(node_id) {
            return None;
        }
        Some(Grant {
            limits: PeerLimits::None,
            class: PeerClass::Node,
            admitted: true,
            trust_scope: true,
            observed: None,
        })
    }
    async fn dial(&self, node_id: &str) -> Result<Box<dyn MeshStream>, ProxyError> {
        self.dials.lock().unwrap().push(node_id.to_string());
        let table = self
            .nodes
            .lock()
            .unwrap()
            .get(node_id)
            .cloned()
            .ok_or_else(|| ProxyError::Mesh("no route".into()))?;
        let (client, mut server) = connected_pair().await.map_err(|e| ProxyError::Mesh(e.to_string()))?;
        let peer = InferPeer {
            node_id: self.caller.clone(),
            grant: self.serve_grant.lock().unwrap().clone(),
        };
        let audit = self.audit.clone();
        let gate = self.gate.clone();
        tokio::spawn(async move {
            let up = Upstream::new(small_limits()).unwrap();
            let t = table.clone();
            let f = move |role: &str, peer: &str| t.local_for_peer(role, peer);
            let _ = serve_infer(&mut server, &peer, &f, &up, &gate, audit.as_deref()).await;
        });
        Ok(Box::new(client))
    }
}

/// A peer holding an enforced-admission full-node grant.
pub fn enforced_node() -> Grant {
    Grant {
        limits: PeerLimits::None,
        class: PeerClass::Node,
        admitted: true,
        trust_scope: true,
        observed: None,
    }
}

pub fn trusted_peer(id: &str) -> InferPeer {
    InferPeer {
        node_id: id.into(),
        grant: Some(enforced_node()),
    }
}
