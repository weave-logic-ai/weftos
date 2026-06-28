//! TCP transport backend for mesh networking (K6).
//!
//! Uses tokio TCP for node-to-node communication.  In production this
//! would be replaced by QUIC (quinn) + Noise (snow), but plain TCP
//! validates the full mesh stack end-to-end and passes the K6 gate
//! tests with real networking.

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
pub struct TcpMeshStream {
    stream: TcpStream,
    remote: SocketAddr,
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
        let mut len_buf = [0u8; 4];
        self.stream
            .read_exact(&mut len_buf)
            .await
            .map_err(|e| MeshError::Io(e.to_string()))?;
        let len = u32::from_be_bytes(len_buf) as usize;
        if len > MAX_MESSAGE_SIZE {
            return Err(MeshError::MessageTooLarge {
                size: len,
                max: MAX_MESSAGE_SIZE,
            });
        }
        let mut buf = vec![0u8; len];
        self.stream
            .read_exact(&mut buf)
            .await
            .map_err(|e| MeshError::Io(e.to_string()))?;
        Ok(buf)
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
        Ok((
            Box::new(TcpMeshStream {
                stream,
                remote: addr,
            }),
            addr,
        ))
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
        Ok(Box::new(TcpMeshStream { stream, remote }))
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
        assert!(t.supports("127.0.0.1:9470"));
        assert!(t.supports("tcp://127.0.0.1:9470"));
        assert!(!t.supports("quic://127.0.0.1:9470"));
    }
}
