//! TCP transport backend for mesh networking (K6).
//!
//! Uses tokio TCP for node-to-node communication. Production Cloud/Edge
//! deployments prefer QUIC (`mesh_quic` / ADR-026) with optional Noise
//! (`mesh_noise` / snow). Plain TCP remains the default and is useful
//! when UDP is blocked.
//!
//! # Cancel-safety (ADR-010 / WEFT-18)
//!
//! [`TcpMeshStream::recv`] keeps partial length/body progress on `self` so
//! it is safe to race inside `tokio::select!` (mesh peer loop in `boot.rs`).
//! A naive `read_exact(len)` then `read_exact(body)` pair is **not**
//! cancel-safe: dropping mid-body desyncs the next length prefix.

use std::net::SocketAddr;

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::mesh::{MAX_MESSAGE_SIZE, MeshError, MeshStream, MeshTransport, TransportListener};

// ── TcpMeshStream ───────────────────────────────────────────────

/// A [`MeshStream`] backed by a single TCP connection.
///
/// Messages are length-prefixed on the wire (4-byte big-endian length
/// followed by that many bytes of payload).
///
/// `recv` is **cancel-safe** (WEFT-18): partial prefix/body progress is
/// retained on `self` across cancellations so `tokio::select!` peer loops
/// do not desync the frame stream.
pub struct TcpMeshStream {
    stream: TcpStream,
    remote: SocketAddr,
    /// Staging for the 4-byte length prefix (always partially or fully filled).
    len_buf: [u8; 4],
    /// How many of `len_buf` bytes are valid.
    len_filled: usize,
    /// In-progress body once the length prefix is complete (`None` = still
    /// reading the length).
    body: Option<Vec<u8>>,
    /// Bytes of `body` filled so far.
    body_filled: usize,
}

impl TcpMeshStream {
    fn new(stream: TcpStream, remote: SocketAddr) -> Self {
        Self {
            stream,
            remote,
            len_buf: [0u8; 4],
            len_filled: 0,
            body: None,
            body_filled: 0,
        }
    }

    /// Reset framing state after a complete message (or hard error).
    fn reset_frame(&mut self) {
        self.len_filled = 0;
        self.body = None;
        self.body_filled = 0;
    }
}

#[async_trait]
impl MeshStream for TcpMeshStream {
    async fn send(&mut self, data: &[u8]) -> Result<(), MeshError> {
        let len = (data.len() as u32).to_be_bytes();
        self.stream
            .write_all(&len)
            .await
            .map_err(|e| MeshError::Io(e.to_string()))?;
        self.stream
            .write_all(data)
            .await
            .map_err(|e| MeshError::Io(e.to_string()))?;
        self.stream
            .flush()
            .await
            .map_err(|e| MeshError::Io(e.to_string()))?;
        Ok(())
    }

    async fn recv(&mut self) -> Result<Vec<u8>, MeshError> {
        // Cancel-safe framed read (ADR-010 / WEFT-18).
        //
        // Progress counters and buffers live on `self`. Each successful
        // `read` updates those fields before the next `.await`. Dropping
        // this future while Pending does not discard already-consumed
        // bytes, so a later `recv` resumes the same frame.
        //
        // Field destructuring lets us mutably borrow the socket and the
        // staging buffers together without fighting the borrow checker
        // across await points.

        // ── length prefix ──────────────────────────────────────────
        while self.len_filled < 4 {
            let Self {
                stream,
                len_buf,
                len_filled,
                ..
            } = self;
            let n = stream
                .read(&mut len_buf[*len_filled..])
                .await
                .map_err(|e| MeshError::Io(e.to_string()))?;
            if n == 0 {
                return Err(MeshError::ConnectionClosed);
            }
            *len_filled += n;
        }

        // ── allocate body once length is known ─────────────────────
        if self.body.is_none() {
            let len = u32::from_be_bytes(self.len_buf) as usize;
            if len > MAX_MESSAGE_SIZE {
                self.reset_frame();
                return Err(MeshError::MessageTooLarge {
                    size: len,
                    max: MAX_MESSAGE_SIZE,
                });
            }
            self.body = Some(vec![0u8; len]);
            self.body_filled = 0;
        }

        // ── body ───────────────────────────────────────────────────
        let expected = self.body.as_ref().map(|b| b.len()).unwrap_or(0);
        while self.body_filled < expected {
            let Self {
                stream,
                body,
                body_filled,
                ..
            } = self;
            let buf = body
                .as_mut()
                .expect("body allocated before body-fill loop");
            let n = stream
                .read(&mut buf[*body_filled..])
                .await
                .map_err(|e| MeshError::Io(e.to_string()))?;
            if n == 0 {
                return Err(MeshError::ConnectionClosed);
            }
            *body_filled += n;
        }

        let out = self.body.take().expect("body complete");
        self.reset_frame();
        Ok(out)
    }

    async fn close(&mut self) -> Result<(), MeshError> {
        self.stream
            .shutdown()
            .await
            .map_err(|e| MeshError::Io(e.to_string()))
    }

    fn remote_addr(&self) -> Option<SocketAddr> {
        Some(self.remote)
    }
}

// ── TcpTransportListener ────────────────────────────────────────

/// A [`TransportListener`] backed by a tokio [`TcpListener`].
pub struct TcpTransportListener {
    listener: TcpListener,
}

#[async_trait]
impl TransportListener for TcpTransportListener {
    async fn accept(&mut self) -> Result<(Box<dyn MeshStream>, SocketAddr), MeshError> {
        let (stream, addr) = self
            .listener
            .accept()
            .await
            .map_err(|e| MeshError::Io(e.to_string()))?;
        Ok((Box::new(TcpMeshStream::new(stream, addr)), addr))
    }

    fn local_addr(&self) -> Result<SocketAddr, MeshError> {
        self.listener
            .local_addr()
            .map_err(|e| MeshError::Io(e.to_string()))
    }
}

// ── TcpTransport ────────────────────────────────────────────────

/// TCP mesh transport.
///
/// Implements [`MeshTransport`] using plain TCP.  Addresses may be
/// bare `ip:port` or prefixed with `tcp://`.
pub struct TcpTransport;

#[async_trait]
impl MeshTransport for TcpTransport {
    fn name(&self) -> &str {
        "tcp"
    }

    async fn listen(&self, addr: &str) -> Result<Box<dyn TransportListener>, MeshError> {
        let bind_addr = addr.strip_prefix("tcp://").unwrap_or(addr);
        let listener = TcpListener::bind(bind_addr)
            .await
            .map_err(|e| MeshError::Io(e.to_string()))?;
        Ok(Box::new(TcpTransportListener { listener }))
    }

    async fn connect(&self, addr: &str) -> Result<Box<dyn MeshStream>, MeshError> {
        let connect_addr = addr.strip_prefix("tcp://").unwrap_or(addr);
        let stream = TcpStream::connect(connect_addr)
            .await
            .map_err(|e| MeshError::Io(e.to_string()))?;
        let remote = stream
            .peer_addr()
            .map_err(|e| MeshError::Io(e.to_string()))?;
        Ok(Box::new(TcpMeshStream::new(stream, remote)))
    }

    fn supports(&self, addr: &str) -> bool {
        // Supports bare IP:port or tcp:// scheme.
        !addr.contains("://") || addr.starts_with("tcp://")
    }
}

// ── Tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tcp_transport_connect_send_recv() {
        let transport = TcpTransport;

        // Bind to an OS-assigned port.
        let mut listener = transport.listen("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // "Client" node sends, then reads response.
        let send_task = tokio::spawn(async move {
            let mut stream = TcpTransport.connect(&addr.to_string()).await.unwrap();
            stream.send(b"hello from node A").await.unwrap();
            let response = stream.recv().await.unwrap();
            assert_eq!(response, b"hello from node B");
            stream.close().await.unwrap();
        });

        // "Server" node accepts, reads, responds.
        let (mut server_stream, _client_addr) = listener.accept().await.unwrap();
        let received = server_stream.recv().await.unwrap();
        assert_eq!(received, b"hello from node A");
        server_stream.send(b"hello from node B").await.unwrap();

        send_task.await.unwrap();
    }

    #[tokio::test]
    async fn two_node_cluster_forms() {
        use crate::mesh::WeftHandshake;
        use crate::mesh_framing::{FrameType, MeshFrame};

        let transport = TcpTransport;
        let mut listener = transport.listen("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // Node A connects to Node B and sends a handshake.
        let node_a = tokio::spawn(async move {
            let mut stream = TcpTransport.connect(&addr.to_string()).await.unwrap();

            let handshake = WeftHandshake {
                node_id: "node-a".into(),
                governance_genesis_hash: [0u8; 32],
                governance_version: "1.0".into(),
                capabilities: 0xFF,
                kem_supported: false,
                chain_seq: 0,
                supported_sync_streams: vec![1, 2, 3],
            };
            let payload = serde_json::to_vec(&handshake).unwrap();
            let frame = MeshFrame {
                frame_type: FrameType::Handshake,
                payload,
            };
            let encoded = frame.encode().unwrap();
            // Send the wire bytes *after* the 4-byte length prefix through
            // the transport layer (which adds its own length prefix).
            stream.send(&encoded).await.unwrap();

            // Receive handshake response.
            let data = stream.recv().await.unwrap();
            // The transport-level length prefix is consumed by recv();
            // the returned bytes are a full encoded MeshFrame (length + type + payload).
            let len = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
            let response = MeshFrame::decode(&data[4..4 + len]).unwrap();
            assert_eq!(response.frame_type, FrameType::Handshake);
            let remote: WeftHandshake = serde_json::from_slice(&response.payload).unwrap();
            assert_eq!(remote.node_id, "node-b");

            stream.close().await.unwrap();
        });

        // Node B accepts and exchanges handshake.
        let (mut stream, _) = listener.accept().await.unwrap();
        let data = stream.recv().await.unwrap();
        let len = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let frame = MeshFrame::decode(&data[4..4 + len]).unwrap();
        assert_eq!(frame.frame_type, FrameType::Handshake);
        let remote: WeftHandshake = serde_json::from_slice(&frame.payload).unwrap();
        assert_eq!(remote.node_id, "node-a");

        // Send our handshake back.
        let handshake = WeftHandshake {
            node_id: "node-b".into(),
            governance_genesis_hash: [0u8; 32],
            governance_version: "1.0".into(),
            capabilities: 0xFF,
            kem_supported: false,
            chain_seq: 0,
            supported_sync_streams: vec![1, 2, 3],
        };
        let payload = serde_json::to_vec(&handshake).unwrap();
        let response = MeshFrame {
            frame_type: FrameType::Handshake,
            payload,
        };
        stream.send(&response.encode().unwrap()).await.unwrap();

        node_a.await.unwrap();
    }

    #[tokio::test]
    async fn cross_node_ipc_delivers() {
        use crate::ipc::{KernelMessage, MessageTarget};
        use crate::mesh_ipc::MeshIpcEnvelope;

        let transport = TcpTransport;
        let mut listener = transport.listen("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // Node A sends an IPC message to Node B.
        let node_a = tokio::spawn(async move {
            let mut stream = TcpTransport.connect(&addr.to_string()).await.unwrap();

            let msg = KernelMessage::text(1, MessageTarget::Service("health".into()), "ping");
            let envelope = MeshIpcEnvelope::new("node-a".into(), "node-b".into(), msg);
            let bytes = envelope.to_bytes().unwrap();
            stream.send(&bytes).await.unwrap();

            // Receive response.
            let response_bytes = stream.recv().await.unwrap();
            let response = MeshIpcEnvelope::from_bytes(&response_bytes).unwrap();
            assert_eq!(response.source_node, "node-b");
            assert_eq!(response.dest_node, "node-a");

            stream.close().await.unwrap();
        });

        // Node B receives and responds.
        let (mut stream, _) = listener.accept().await.unwrap();
        let data = stream.recv().await.unwrap();
        let envelope = MeshIpcEnvelope::from_bytes(&data).unwrap();
        assert_eq!(envelope.source_node, "node-a");
        assert_eq!(envelope.dest_node, "node-b");

        // Send response.
        let response_msg = KernelMessage::text(0, MessageTarget::Kernel, "pong");
        let response = MeshIpcEnvelope::new("node-b".into(), "node-a".into(), response_msg);
        stream.send(&response.to_bytes().unwrap()).await.unwrap();

        node_a.await.unwrap();
    }

    #[test]
    fn tcp_transport_supports() {
        let t = TcpTransport;
        assert!(t.supports("127.0.0.1:9489"));
        assert!(t.supports("tcp://127.0.0.1:9489"));
        assert!(!t.supports("quic://127.0.0.1:9489"));
    }

    /// WEFT-18: race `recv` against another arm; the peer must still decode
    /// subsequent frames after many mid-frame cancellations (simulates the
    /// mesh bidirectional `select!` in `boot.rs`).
    #[tokio::test]
    async fn recv_survives_select_cancellation() {
        use std::time::Duration;

        let transport = TcpTransport;
        let mut listener = transport.listen("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let payload_a = vec![0xAAu8; 64];
        let payload_b = vec![0xBBu8; 128];
        let payload_a2 = payload_a.clone();
        let payload_b2 = payload_b.clone();

        let writer = tokio::spawn(async move {
            let mut stream = TcpTransport.connect(&addr.to_string()).await.unwrap();
            // Pace writes so the reader often parks mid-frame.
            stream.send(&payload_a2).await.unwrap();
            tokio::time::sleep(Duration::from_millis(5)).await;
            stream.send(&payload_b2).await.unwrap();
            stream.close().await.unwrap();
        });

        let (mut server, _) = listener.accept().await.unwrap();

        // Hammer select! cancellations while the first frame arrives.
        let mut got_first = None;
        for _ in 0..200 {
            tokio::select! {
                biased;
                result = server.recv() => {
                    got_first = Some(result.expect("recv after cancel must succeed"));
                    break;
                }
                _ = tokio::time::sleep(Duration::from_micros(50)) => {
                    // Cancel recv mid-frame; framing state must survive.
                }
            }
        }
        let first = got_first.expect("should eventually complete first frame");
        assert_eq!(first, payload_a);

        // Second frame must still align (no length-prefix desync).
        let second = server.recv().await.expect("second frame");
        assert_eq!(second, payload_b);

        writer.await.unwrap();
    }

    /// WEFT-18: length-prefix progress stored on self after partial fill.
    #[tokio::test]
    async fn recv_resumes_after_manual_partial_len_state() {
        let transport = TcpTransport;
        let mut listener = transport.listen("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let writer = tokio::spawn(async move {
            let mut stream = TcpTransport.connect(&addr.to_string()).await.unwrap();
            stream.send(b"resume-ok").await.unwrap();
            stream.close().await.unwrap();
        });

        let (mut server, _) = listener.accept().await.unwrap();

        // Force a cancel race a few times, then a clean recv.
        for _ in 0..20 {
            tokio::select! {
                biased;
                r = server.recv() => {
                    assert_eq!(r.unwrap(), b"resume-ok");
                    writer.await.unwrap();
                    return;
                }
                _ = std::future::ready(()) => {}
            }
        }
        let msg = server.recv().await.unwrap();
        assert_eq!(msg, b"resume-ok");
        writer.await.unwrap();
    }
}
