//! Multi-node mesh test fixtures (WEFT-112).
//!
//! Provides in-process transport and peer mocks so K6 integration tests
//! do not need real TCP/QUIC sockets.
//!
//! # Fixtures
//!
//! | Type | Role |
//! |------|------|
//! | [`InMemoryTransport`] | `MeshTransport` over tokio channels (`mem://` URLs) |
//! | [`FaultyTransport`] | Wraps in-memory transport with drop/fail injection |
//! | [`MockPeer`] | Lightweight simulated mesh node with inbox |
//! | [`MockClock`] | Deterministic clock (re-export of [`crate::mesh_clock::MockClock`]) |
//!
//! # Downstream authors
//!
//! ```ignore
//! use clawft_kernel::mesh_test_support::{
//!     connected_pair, FaultyTransport, InMemoryTransport, MockClock, MockPeer,
//! };
//!
//! #[tokio::test]
//! async fn two_node_exchange() {
//!     let (mut a, mut b) = connected_pair().await.unwrap();
//!     a.send(b"hello").await.unwrap();
//!     assert_eq!(b.recv().await.unwrap(), b"hello");
//! }
//! ```
//!
//! Prefer `mem://node-a` style addresses. Share one
//! [`InMemoryTransport`] (or clone via [`InMemoryTransport::clone`])
//! across peers so `listen`/`connect` meet in the same registry.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::{mpsc, oneshot};

use crate::mesh::{
    MeshError, MeshPeer, MeshStream, MeshTransport, TransportListener, WeftHandshake,
};
use crate::mesh_framing::FrameType;

// Re-export for a single import path (WEFT-112 AC).
pub use crate::mesh_clock::{Clock, MockClock, RealClock};

// ── In-memory stream ─────────────────────────────────────────────

/// Bidirectional in-memory byte stream (tokio channels).
pub struct InMemoryStream {
    tx: Option<mpsc::Sender<Vec<u8>>>,
    rx: mpsc::Receiver<Vec<u8>>,
    remote: Option<SocketAddr>,
    closed: bool,
}

impl InMemoryStream {
    fn pair(buffer: usize) -> (Self, Self) {
        let (a_tx, b_rx) = mpsc::channel(buffer);
        let (b_tx, a_rx) = mpsc::channel(buffer);
        let left = Self {
            tx: Some(a_tx),
            rx: a_rx,
            remote: None,
            closed: false,
        };
        let right = Self {
            tx: Some(b_tx),
            rx: b_rx,
            remote: None,
            closed: false,
        };
        (left, right)
    }

    fn with_remote(mut self, addr: SocketAddr) -> Self {
        self.remote = Some(addr);
        self
    }
}

#[async_trait]
impl MeshStream for InMemoryStream {
    async fn send(&mut self, data: &[u8]) -> Result<(), MeshError> {
        if self.closed {
            return Err(MeshError::ConnectionClosed);
        }
        let tx = self.tx.as_ref().ok_or(MeshError::ConnectionClosed)?;
        tx.send(data.to_vec())
            .await
            .map_err(|_| MeshError::ConnectionClosed)
    }

    async fn recv(&mut self) -> Result<Vec<u8>, MeshError> {
        if self.closed {
            return Err(MeshError::ConnectionClosed);
        }
        self.rx.recv().await.ok_or(MeshError::ConnectionClosed)
    }

    async fn close(&mut self) -> Result<(), MeshError> {
        self.closed = true;
        self.tx.take();
        Ok(())
    }

    fn remote_addr(&self) -> Option<SocketAddr> {
        self.remote
    }
}

// ── In-memory listener ───────────────────────────────────────────

struct PendingConn {
    stream: InMemoryStream,
    remote: SocketAddr,
}

/// Listener that yields accepted in-memory streams.
pub struct InMemoryListener {
    rx: mpsc::Receiver<PendingConn>,
    local: SocketAddr,
}

#[async_trait]
impl TransportListener for InMemoryListener {
    async fn accept(&mut self) -> Result<(Box<dyn MeshStream>, SocketAddr), MeshError> {
        let pending = self
            .rx
            .recv()
            .await
            .ok_or_else(|| MeshError::Transport("listener closed".into()))?;
        Ok((Box::new(pending.stream), pending.remote))
    }

    fn local_addr(&self) -> Result<SocketAddr, MeshError> {
        Ok(self.local)
    }
}

// ── InMemoryTransport ────────────────────────────────────────────

struct ListenerEntry {
    tx: mpsc::Sender<PendingConn>,
}

/// Shared registry for in-memory `mem://` endpoints.
#[derive(Clone, Default)]
pub struct InMemoryTransport {
    inner: Arc<Mutex<HashMap<String, ListenerEntry>>>,
    next_port: Arc<AtomicU32>,
    buffer: usize,
}

impl InMemoryTransport {
    /// Create an empty in-memory transport fabric.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            next_port: Arc::new(AtomicU32::new(10_000)),
            buffer: 256,
        }
    }

    /// Channel buffer depth for new stream pairs.
    pub fn with_buffer(mut self, buffer: usize) -> Self {
        self.buffer = buffer.max(1);
        self
    }

    fn normalize(addr: &str) -> String {
        addr.trim().to_string()
    }

    fn alloc_local_addr(&self) -> SocketAddr {
        let port = self.next_port.fetch_add(1, Ordering::Relaxed) as u16;
        SocketAddr::from((Ipv4Addr::LOCALHOST, port))
    }

    fn synthetic_remote(&self) -> SocketAddr {
        self.alloc_local_addr()
    }
}

#[async_trait]
impl MeshTransport for InMemoryTransport {
    fn name(&self) -> &str {
        "in-memory"
    }

    async fn listen(&self, addr: &str) -> Result<Box<dyn TransportListener>, MeshError> {
        let key = Self::normalize(addr);
        if !self.supports(&key) {
            return Err(MeshError::Transport(format!(
                "in-memory transport requires mem:// address, got {addr}"
            )));
        }
        let (tx, rx) = mpsc::channel(self.buffer);
        let local = self.alloc_local_addr();
        {
            let mut map = self
                .inner
                .lock()
                .map_err(|_| MeshError::Transport("lock poisoned".into()))?;
            if map.contains_key(&key) {
                return Err(MeshError::Transport(format!("already listening on {key}")));
            }
            map.insert(key, ListenerEntry { tx });
        }
        Ok(Box::new(InMemoryListener { rx, local }))
    }

    async fn connect(&self, addr: &str) -> Result<Box<dyn MeshStream>, MeshError> {
        let key = Self::normalize(addr);
        if !self.supports(&key) {
            return Err(MeshError::Transport(format!(
                "in-memory transport requires mem:// address, got {addr}"
            )));
        }
        let (client, server) = InMemoryStream::pair(self.buffer);
        let remote = self.synthetic_remote();
        let server = server.with_remote(remote);
        let client = client.with_remote(remote);

        let tx = {
            let map = self
                .inner
                .lock()
                .map_err(|_| MeshError::Transport("lock poisoned".into()))?;
            map.get(&key)
                .map(|e| e.tx.clone())
                .ok_or_else(|| MeshError::Transport(format!("no listener for {key}")))?
        };

        tx.send(PendingConn {
            stream: server,
            remote,
        })
        .await
        .map_err(|_| MeshError::Transport("listener closed".into()))?;

        Ok(Box::new(client))
    }

    fn supports(&self, addr: &str) -> bool {
        addr.starts_with("mem://")
    }
}

// ── FaultyTransport ──────────────────────────────────────────────

/// Fault-injection wrapper around [`InMemoryTransport`].
///
/// Useful for SWIM / circuit-breaker / reconnect tests.
pub struct FaultyTransport {
    inner: InMemoryTransport,
    /// Drop every Nth *message* on streams created via connect (0 = never).
    drop_every_n: AtomicU32,
    /// Fail the next N `connect` attempts.
    fail_next_connects: AtomicU32,
    /// Artificial delay applied to connect (milliseconds). 0 = none.
    latency_ms: AtomicU64,
}

impl FaultyTransport {
    /// Wrap an in-memory transport with default (healthy) fault settings.
    pub fn new(inner: InMemoryTransport) -> Self {
        Self {
            inner,
            drop_every_n: AtomicU32::new(0),
            fail_next_connects: AtomicU32::new(0),
            latency_ms: AtomicU64::new(0),
        }
    }

    /// Drop every Nth sent message (`0` disables).
    pub fn set_drop_every_n(&self, n: u32) {
        self.drop_every_n.store(n, Ordering::SeqCst);
    }

    /// Fail the next `n` connect attempts with a transport error.
    pub fn set_fail_next_connects(&self, n: u32) {
        self.fail_next_connects.store(n, Ordering::SeqCst);
    }

    /// Artificial connect latency in milliseconds.
    pub fn set_latency_ms(&self, ms: u64) {
        self.latency_ms.store(ms, Ordering::SeqCst);
    }

    /// Access the underlying fabric (e.g. to share listeners).
    pub fn inner(&self) -> &InMemoryTransport {
        &self.inner
    }
}

/// Stream that optionally drops outbound messages.
struct FaultyStream {
    inner: Box<dyn MeshStream>,
    drop_every_n: Arc<AtomicU32>,
    msg_counter: Arc<AtomicU64>,
}

#[async_trait]
impl MeshStream for FaultyStream {
    async fn send(&mut self, data: &[u8]) -> Result<(), MeshError> {
        let n = self.drop_every_n.load(Ordering::SeqCst);
        if n > 0 {
            let c = self.msg_counter.fetch_add(1, Ordering::SeqCst) + 1;
            if c.is_multiple_of(u64::from(n)) {
                // Silently drop.
                return Ok(());
            }
        }
        self.inner.send(data).await
    }

    async fn recv(&mut self) -> Result<Vec<u8>, MeshError> {
        self.inner.recv().await
    }

    async fn close(&mut self) -> Result<(), MeshError> {
        self.inner.close().await
    }

    fn remote_addr(&self) -> Option<SocketAddr> {
        self.inner.remote_addr()
    }
}

#[async_trait]
impl MeshTransport for FaultyTransport {
    fn name(&self) -> &str {
        "faulty-in-memory"
    }

    async fn listen(&self, addr: &str) -> Result<Box<dyn TransportListener>, MeshError> {
        self.inner.listen(addr).await
    }

    async fn connect(&self, addr: &str) -> Result<Box<dyn MeshStream>, MeshError> {
        let remaining = self.fail_next_connects.load(Ordering::SeqCst);
        if remaining > 0 {
            self.fail_next_connects
                .store(remaining.saturating_sub(1), Ordering::SeqCst);
            return Err(MeshError::Transport("injected connect failure".into()));
        }
        let latency = self.latency_ms.load(Ordering::SeqCst);
        if latency > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(latency)).await;
        }
        let stream = self.inner.connect(addr).await?;
        Ok(Box::new(FaultyStream {
            inner: stream,
            drop_every_n: Arc::new(AtomicU32::new(self.drop_every_n.load(Ordering::SeqCst))),
            // Share counter across streams created from this transport.
            msg_counter: Arc::new(AtomicU64::new(0)),
        }))
    }

    fn supports(&self, addr: &str) -> bool {
        self.inner.supports(addr)
    }
}

// ── MockPeer ─────────────────────────────────────────────────────

/// Lightweight simulated mesh node for multi-node tests.
///
/// Holds identity, handshake metadata, an optional shared transport, and
/// an inbox of received raw frames / payloads for assertions.
pub struct MockPeer {
    /// Node identity string.
    pub node_id: String,
    /// Listen / dial address (`mem://...`).
    pub address: String,
    /// Governance genesis for cluster isolation tests.
    pub governance_genesis: [u8; 32],
    /// Handshake payload this peer would present.
    pub handshake: WeftHandshake,
    /// Received payloads (raw bytes).
    pub inbox: Arc<Mutex<Vec<Vec<u8>>>>,
    transport: InMemoryTransport,
}

impl MockPeer {
    /// Create a peer with a fresh identity in the given cluster.
    pub fn new(node_id: impl Into<String>, genesis: [u8; 32]) -> Self {
        let node_id = node_id.into();
        let address = format!("mem://{node_id}");
        let handshake = WeftHandshake {
            node_id: node_id.clone(),
            governance_genesis_hash: genesis,
            governance_version: "1.0.0".into(),
            capabilities: 0,
            kem_supported: false,
            chain_seq: 0,
            supported_sync_streams: FrameType::ALL.iter().map(|t| *t as u8).collect(),
        };
        Self {
            node_id,
            address,
            governance_genesis: genesis,
            handshake,
            inbox: Arc::new(Mutex::new(Vec::new())),
            transport: InMemoryTransport::new(),
        }
    }

    /// Attach a shared transport fabric (required for multi-peer meshes).
    pub fn with_transport(mut self, transport: InMemoryTransport) -> Self {
        self.transport = transport;
        self
    }

    /// Peer that rejects foreign clusters (different genesis).
    pub fn foreign_cluster(node_id: impl Into<String>) -> Self {
        Self::new(node_id, [0xFF; 32])
    }

    /// Build a [`MeshPeer`] view of this mock.
    pub fn as_mesh_peer(&self) -> MeshPeer {
        MeshPeer {
            node_id: self.node_id.clone(),
            handshake: self.handshake.clone(),
            address: "127.0.0.1:0"
                .parse()
                .expect("static socket addr"),
        }
    }

    /// Shared transport handle.
    pub fn transport(&self) -> &InMemoryTransport {
        &self.transport
    }

    /// Push a payload into the inbox (for tests that inject delivery).
    pub fn push_inbox(&self, data: Vec<u8>) {
        self.inbox
            .lock()
            .expect("inbox lock")
            .push(data);
    }

    /// Snapshot of inbox contents.
    pub fn inbox_snapshot(&self) -> Vec<Vec<u8>> {
        self.inbox.lock().expect("inbox lock").clone()
    }

    /// Start accepting one connection and reading frames into the inbox.
    ///
    /// Returns a oneshot that fires when the accept loop has bound the
    /// listener (so dialers can race-free connect).
    pub async fn listen_once(
        &self,
    ) -> Result<(tokio::task::JoinHandle<()>, oneshot::Receiver<()>), MeshError> {
        let mut listener = self.transport.listen(&self.address).await?;
        let (ready_tx, ready_rx) = oneshot::channel();
        let inbox = Arc::clone(&self.inbox);
        let handle = tokio::spawn(async move {
            let _ = ready_tx.send(());
            if let Ok((mut stream, _)) = listener.accept().await {
                while let Ok(data) = stream.recv().await {
                    if let Ok(mut guard) = inbox.lock() {
                        guard.push(data);
                    }
                }
            }
        });
        Ok((handle, ready_rx))
    }
}

// ── Helpers ──────────────────────────────────────────────────────

/// Create a connected pair of in-memory streams (no peers).
pub async fn connected_pair() -> Result<(InMemoryStream, InMemoryStream), MeshError> {
    // Direct pair without registry — fastest unit path.
    Ok(InMemoryStream::pair(256))
}

/// Two mock peers sharing a fabric, with an established stream pair.
///
/// Returns `(peer_a, peer_b, stream_from_a, stream_from_b)` after A
/// listens and B connects.
pub async fn two_node_streams(
    genesis: [u8; 32],
) -> Result<
    (
        MockPeer,
        MockPeer,
        Box<dyn MeshStream>,
        Box<dyn MeshStream>,
    ),
    MeshError,
> {
    let fabric = InMemoryTransport::new();
    let peer_a = MockPeer::new("node-a", genesis).with_transport(fabric.clone());
    let peer_b = MockPeer::new("node-b", genesis).with_transport(fabric.clone());

    let mut listener = fabric.listen(&peer_a.address).await?;
    let connect_fut = fabric.connect(&peer_a.address);
    let accept_fut = listener.accept();
    let (client, (server, _)) = tokio::try_join!(connect_fut, accept_fut)?;
    Ok((peer_a, peer_b, client, server))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh_clock::MockClock;
    use crate::mesh_framing::{FrameType, MeshFrame};
    use crate::mesh_heartbeat::{HeartbeatConfig, HeartbeatState, HeartbeatTracker};
    use std::time::Duration;

    #[tokio::test]
    async fn in_memory_two_node_exchange() {
        let fabric = InMemoryTransport::new();
        let mut listener = fabric.listen("mem://alpha").await.unwrap();
        let connect = fabric.connect("mem://alpha");
        let accept = listener.accept();
        let (mut client, (mut server, addr)) = tokio::try_join!(connect, accept).unwrap();
        assert_eq!(addr.ip(), Ipv4Addr::LOCALHOST);

        client.send(b"ping").await.unwrap();
        assert_eq!(server.recv().await.unwrap(), b"ping");
        server.send(b"pong").await.unwrap();
        assert_eq!(client.recv().await.unwrap(), b"pong");
    }

    #[tokio::test]
    async fn mock_peer_inbox_receives_frames() {
        let genesis = [0x11; 32];
        let fabric = InMemoryTransport::new();
        let peer = MockPeer::new("peer-a", genesis).with_transport(fabric.clone());
        let (handle, ready) = peer.listen_once().await.unwrap();
        ready.await.unwrap();

        let mut stream = fabric.connect(&peer.address).await.unwrap();
        let frame = MeshFrame {
            frame_type: FrameType::Heartbeat,
            payload: b"tick".to_vec(),
        };
        stream.send(&frame.encode().unwrap()).await.unwrap();
        stream.close().await.unwrap();
        let _ = handle.await;

        let inbox = peer.inbox_snapshot();
        assert_eq!(inbox.len(), 1);
        let decoded = MeshFrame::decode(&inbox[0][4..]).unwrap();
        assert_eq!(decoded.frame_type, FrameType::Heartbeat);
        assert_eq!(decoded.payload, b"tick");
    }

    #[tokio::test]
    async fn faulty_transport_fails_connect() {
        let fabric = InMemoryTransport::new();
        let _listener = fabric.listen("mem://fault").await.unwrap();
        let faulty = FaultyTransport::new(fabric);
        faulty.set_fail_next_connects(1);
        match faulty.connect("mem://fault").await {
            Ok(_) => panic!("expected injected connect failure"),
            Err(err) => assert!(err.to_string().contains("injected connect failure")),
        }
        // Second connect succeeds.
        let _ok = faulty.connect("mem://fault").await.expect("connect after fail budget");
    }

    #[tokio::test]
    async fn faulty_transport_drops_messages() {
        let fabric = InMemoryTransport::new();
        let mut listener = fabric.listen("mem://drop").await.unwrap();
        let faulty = FaultyTransport::new(fabric);
        // Drop every message (N=1).
        faulty.set_drop_every_n(1);
        let connect = faulty.connect("mem://drop");
        let accept = listener.accept();
        let (mut client, (mut server, _)) = tokio::try_join!(connect, accept).unwrap();

        client.send(b"lost").await.unwrap();
        // Server should not receive; use timeout.
        let result = tokio::time::timeout(Duration::from_millis(50), server.recv()).await;
        assert!(result.is_err(), "dropped message must not arrive");
    }

    #[test]
    fn mock_clock_drives_heartbeat() {
        let clock = MockClock::new();
        let config = HeartbeatConfig {
            suspect_timeout: Duration::from_millis(100),
            ..HeartbeatConfig::default()
        };
        let mut tracker = HeartbeatTracker::with_clock(config, Arc::new(clock.clone()));
        tracker.add_peer("n1".into());
        tracker.record_miss("n1");
        assert_eq!(tracker.peer_state("n1"), Some(HeartbeatState::Suspect));
        clock.advance(Duration::from_millis(150));
        tracker.record_miss("n1");
        assert_eq!(tracker.peer_state("n1"), Some(HeartbeatState::Dead));
    }

    #[tokio::test]
    async fn connected_pair_helper() {
        let (mut a, mut b) = connected_pair().await.unwrap();
        a.send(b"x").await.unwrap();
        assert_eq!(b.recv().await.unwrap(), b"x");
    }

    #[tokio::test]
    async fn two_node_streams_helper() {
        let (_a, _b, mut sa, mut sb) = two_node_streams([7; 32]).await.unwrap();
        sa.send(b"hello").await.unwrap();
        assert_eq!(sb.recv().await.unwrap(), b"hello");
    }

    #[test]
    fn mock_peer_foreign_cluster() {
        let p = MockPeer::foreign_cluster("x");
        assert_eq!(p.governance_genesis, [0xFF; 32]);
    }
}
