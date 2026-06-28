//! WebSocket transport backend for mesh networking (K6).
//!
//! Uses tokio-tungstenite for node-to-node communication.  This enables
//! browser-based nodes (via wasm-bindgen) and traversal of HTTP proxies
//! that block raw TCP.

use std::net::SocketAddr;

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::tungstenite::Message;

use crate::mesh::{MAX_MESSAGE_SIZE, MeshError, MeshStream, MeshTransport, TransportListener};

// ── Wire helpers ──────────────────────────────────────────────

/// Encode a payload into a length-prefixed binary WebSocket message.
fn encode_ws_message(data: &[u8]) -> Message {
    let len = (data.len() as u32).to_be_bytes();
    let mut payload = Vec::with_capacity(4 + data.len());
    payload.extend_from_slice(&len);
    payload.extend_from_slice(data);
    Message::Binary(payload)
}

/// Decode a length-prefixed binary WebSocket message payload.
fn decode_ws_payload(payload: &[u8]) -> Result<Vec<u8>, MeshError> {
    if payload.len() < 4 {
        return Err(MeshError::Io(
            "binary message too short for length prefix".into(),
        ));
    }
    let len = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;
    if len > MAX_MESSAGE_SIZE {
        return Err(MeshError::MessageTooLarge {
            size: len,
            max: MAX_MESSAGE_SIZE,
        });
    }
    if payload.len() < 4 + len {
        return Err(MeshError::Io("truncated binary message".into()));
    }
    Ok(payload[4..4 + len].to_vec())
}

// ── WsMeshStream (server-side, plain TcpStream) ──────────────

/// A [`MeshStream`] backed by a server-side WebSocket connection
/// (accepted from a [`TcpListener`]).
pub struct WsMeshStream {
    sink: futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
        Message,
    >,
    stream: futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    >,
    remote: SocketAddr,
}

#[async_trait]
impl MeshStream for WsMeshStream {
    async fn send(&mut self, data: &[u8]) -> Result<(), MeshError> {
        self.sink
            .send(encode_ws_message(data))
            .await
            .map_err(|e| MeshError::Io(e.to_string()))
    }

    async fn recv(&mut self) -> Result<Vec<u8>, MeshError> {
        loop {
            let msg = self
                .stream
                .next()
                .await
                .ok_or(MeshError::ConnectionClosed)?
                .map_err(|e| MeshError::Io(e.to_string()))?;
            match msg {
                Message::Binary(payload) => return decode_ws_payload(&payload),
                Message::Close(_) => return Err(MeshError::ConnectionClosed),
                _ => continue,
            }
        }
    }

    async fn close(&mut self) -> Result<(), MeshError> {
        self.sink
            .send(Message::Close(None))
            .await
            .map_err(|e| MeshError::Io(e.to_string()))
    }

    fn remote_addr(&self) -> Option<SocketAddr> {
        Some(self.remote)
    }
}

// ── WsClientStream (client-side, MaybeTlsStream) ─────────────

/// A [`MeshStream`] backed by a client-side WebSocket connection
/// (created via [`tokio_tungstenite::connect_async`]).
pub struct WsClientStream {
    sink: futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
        Message,
    >,
    stream: futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    >,
    remote: SocketAddr,
}

#[async_trait]
impl MeshStream for WsClientStream {
    async fn send(&mut self, data: &[u8]) -> Result<(), MeshError> {
        self.sink
            .send(encode_ws_message(data))
            .await
            .map_err(|e| MeshError::Io(e.to_string()))
    }

    async fn recv(&mut self) -> Result<Vec<u8>, MeshError> {
        loop {
            let msg = self
                .stream
                .next()
                .await
                .ok_or(MeshError::ConnectionClosed)?
                .map_err(|e| MeshError::Io(e.to_string()))?;
            match msg {
                Message::Binary(payload) => return decode_ws_payload(&payload),
                Message::Close(_) => return Err(MeshError::ConnectionClosed),
                _ => continue,
            }
        }
    }

    async fn close(&mut self) -> Result<(), MeshError> {
        self.sink
            .send(Message::Close(None))
            .await
            .map_err(|e| MeshError::Io(e.to_string()))
    }

    fn remote_addr(&self) -> Option<SocketAddr> {
        Some(self.remote)
    }
}

// ── WsTransportListener ──────────────────────────────────────

/// A [`TransportListener`] that accepts TCP connections and upgrades
/// them to WebSocket.
pub struct WsTransportListener {
    listener: TcpListener,
}

#[async_trait]
impl TransportListener for WsTransportListener {
    async fn accept(&mut self) -> Result<(Box<dyn MeshStream>, SocketAddr), MeshError> {
        let (tcp_stream, addr) = self
            .listener
            .accept()
            .await
            .map_err(|e| MeshError::Io(e.to_string()))?;

        let ws_stream = tokio_tungstenite::accept_async(tcp_stream)
            .await
            .map_err(|e| MeshError::Handshake(e.to_string()))?;

        let (sink, stream) = ws_stream.split();
        Ok((
            Box::new(WsMeshStream {
                sink,
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

// ── WsTransport ──────────────────────────────────────────────

/// WebSocket mesh transport.
///
/// Implements [`MeshTransport`] using tokio-tungstenite.  Addresses
/// use the `ws://` or `wss://` scheme.
pub struct WsTransport;

#[async_trait]
impl MeshTransport for WsTransport {
    fn name(&self) -> &str {
        "websocket"
    }

    async fn listen(&self, addr: &str) -> Result<Box<dyn TransportListener>, MeshError> {
        let bind_addr = addr
            .strip_prefix("ws://")
            .or_else(|| addr.strip_prefix("wss://"))
            .unwrap_or(addr);
        let listener = TcpListener::bind(bind_addr)
            .await
            .map_err(|e| MeshError::Io(e.to_string()))?;
        Ok(Box::new(WsTransportListener { listener }))
    }

    async fn connect(&self, addr: &str) -> Result<Box<dyn MeshStream>, MeshError> {
        // Ensure the address has a ws:// scheme for tungstenite.
        let url = if addr.starts_with("ws://") || addr.starts_with("wss://") {
            addr.to_string()
        } else {
            format!("ws://{addr}")
        };

        let (ws_stream, _response) = tokio_tungstenite::connect_async(&url)
            .await
            .map_err(|e| MeshError::Io(e.to_string()))?;

        // Extract remote address from the URL for bookkeeping.
        let remote: SocketAddr = url
            .strip_prefix("ws://")
            .or_else(|| url.strip_prefix("wss://"))
            .unwrap_or(&url)
            .parse()
            .map_err(|e: std::net::AddrParseError| MeshError::Io(e.to_string()))?;

        let (sink, stream) = ws_stream.split();
        Ok(Box::new(WsClientStream {
            sink,
            stream,
            remote,
        }))
    }

    fn supports(&self, addr: &str) -> bool {
        addr.starts_with("ws://") || addr.starts_with("wss://")
    }
}

// ── Tests ────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ws_transport_connect_send_recv() {
        let transport = WsTransport;

        // Bind to an OS-assigned port.
        let mut listener = transport.listen("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // "Client" node sends, then reads response.
        let ws_url = format!("ws://{addr}");
        let send_task = tokio::spawn(async move {
            let mut stream = WsTransport.connect(&ws_url).await.unwrap();
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

    #[test]
    fn ws_transport_supports() {
        let t = WsTransport;
        assert!(t.supports("ws://127.0.0.1:9470"));
        assert!(t.supports("wss://example.com:443"));
        assert!(!t.supports("tcp://127.0.0.1:9470"));
        assert!(!t.supports("127.0.0.1:9470"));
    }

    #[tokio::test]
    async fn browser_node_connects_via_websocket() {
        // Simulates a browser node joining the mesh over WebSocket.
        let transport = WsTransport;
        let mut listener = transport.listen("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let ws_url = format!("ws://{addr}");
        let browser_task = tokio::spawn(async move {
            let mut stream = WsTransport.connect(&ws_url).await.unwrap();

            // Browser sends a join request payload.
            let join_payload = br#"{"node_id":"browser-1","type":"join"}"#;
            stream.send(join_payload).await.unwrap();

            // Receive acknowledgement.
            let ack = stream.recv().await.unwrap();
            assert_eq!(ack, b"welcome");

            stream.close().await.unwrap();
        });

        // Server side accepts the browser node.
        let (mut peer, _) = listener.accept().await.unwrap();
        let data = peer.recv().await.unwrap();
        assert!(data.starts_with(b"{"));
        let parsed: serde_json::Value = serde_json::from_slice(&data).unwrap();
        assert_eq!(parsed["node_id"], "browser-1");

        peer.send(b"welcome").await.unwrap();

        browser_task.await.unwrap();
    }
}
