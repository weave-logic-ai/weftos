//! QUIC transport backend for mesh networking (WEFT-118 / ADR-026).
//!
//! Uses [`quinn`] for the QUIC wire protocol. Application-level encryption
//! and identity remain Noise (`mesh_noise` / snow) per ADR-024 — quinn's
//! TLS layer only satisfies the QUIC handshake. We mint an ephemeral
//! self-signed certificate on listen and skip certificate verification
//! on connect (the Noise XX/IK session authenticates peers).
//!
//! Addresses use the `quic://host:port` scheme (bare `host:port` is also
//! accepted when this transport is selected via `[kernel.mesh] transport`).
//!
//! # Cancel-safety (ADR-010 / WEFT-18)
//!
//! [`QuicMeshStream::recv`] keeps partial length/body progress on `self`
//! so it is safe to race inside `tokio::select!` (mesh peer loop).

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Once;

use async_trait::async_trait;
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use quinn::{ClientConfig, Connection, Endpoint, RecvStream, SendStream, ServerConfig};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};

use crate::mesh::{MAX_MESSAGE_SIZE, MeshError, MeshStream, MeshTransport, TransportListener};

/// ALPN protocol identifier for WeftOS mesh over QUIC.
const MESH_ALPN: &[u8] = b"weftos-mesh";

/// SNI name used on the client; certs are self-signed for this name.
const MESH_SNI: &str = "localhost";

static INSTALL_CRYPTO: Once = Once::new();

fn ensure_crypto_provider() {
    INSTALL_CRYPTO.call_once(|| {
        // Best-effort: another crate may have installed already.
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

// ── Skip-verify TLS (Noise owns peer auth) ─────────────────────────

#[derive(Debug)]
struct SkipServerVerification(Arc<rustls::crypto::CryptoProvider>);

impl SkipServerVerification {
    fn new() -> Arc<Self> {
        Arc::new(Self(Arc::new(rustls::crypto::ring::default_provider())))
    }
}

impl rustls::client::danger::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

// ── TLS / endpoint helpers ─────────────────────────────────────────

fn make_server_config() -> Result<ServerConfig, MeshError> {
    ensure_crypto_provider();

    let certified = rcgen::generate_simple_self_signed(vec![MESH_SNI.into()]).map_err(|e| {
        MeshError::Transport(format!("quic self-signed cert: {e}"))
    })?;
    let cert_der = CertificateDer::from(certified.cert);
    let key_der = PrivatePkcs8KeyDer::from(certified.key_pair.serialize_der());

    let mut tls = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], PrivateKeyDer::Pkcs8(key_der))
        .map_err(|e| MeshError::Transport(format!("quic server tls: {e}")))?;
    tls.alpn_protocols = vec![MESH_ALPN.to_vec()];

    let quic_tls = QuicServerConfig::try_from(tls)
        .map_err(|e| MeshError::Transport(format!("quic server crypto: {e}")))?;
    Ok(ServerConfig::with_crypto(Arc::new(quic_tls)))
}

fn make_client_config() -> Result<ClientConfig, MeshError> {
    ensure_crypto_provider();

    let mut tls = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(SkipServerVerification::new())
        .with_no_client_auth();
    tls.alpn_protocols = vec![MESH_ALPN.to_vec()];

    let quic_tls = QuicClientConfig::try_from(tls)
        .map_err(|e| MeshError::Transport(format!("quic client crypto: {e}")))?;
    Ok(ClientConfig::new(Arc::new(quic_tls)))
}

fn strip_quic_scheme(addr: &str) -> &str {
    addr.strip_prefix("quic://").unwrap_or(addr)
}

fn parse_socket_addr(addr: &str) -> Result<SocketAddr, MeshError> {
    strip_quic_scheme(addr)
        .parse()
        .map_err(|e: std::net::AddrParseError| MeshError::Io(format!("bad quic addr '{addr}': {e}")))
}

// ── QuicMeshStream ─────────────────────────────────────────────────

/// A [`MeshStream`] backed by a single bidirectional QUIC stream.
///
/// Messages are length-prefixed on the stream (4-byte big-endian length
/// followed by that many bytes of payload), matching TCP/WS framing so
/// higher layers (`NoiseChannel`, mesh peer loop) stay transport-neutral.
///
/// `recv` is **cancel-safe** (WEFT-18): partial prefix/body progress is
/// retained on `self` across cancellations.
pub struct QuicMeshStream {
    send: SendStream,
    recv: RecvStream,
    remote: SocketAddr,
    /// Keeps the QUIC connection open for the stream lifetime.
    _conn: Connection,
    /// Client-side endpoint must outlive the connection.
    _endpoint: Option<Endpoint>,
    /// Staging for the 4-byte length prefix.
    len_buf: [u8; 4],
    len_filled: usize,
    body: Option<Vec<u8>>,
    body_filled: usize,
}

impl QuicMeshStream {
    fn new(
        send: SendStream,
        recv: RecvStream,
        remote: SocketAddr,
        conn: Connection,
        endpoint: Option<Endpoint>,
    ) -> Self {
        Self {
            send,
            recv,
            remote,
            _conn: conn,
            _endpoint: endpoint,
            len_buf: [0u8; 4],
            len_filled: 0,
            body: None,
            body_filled: 0,
        }
    }

    fn reset_frame(&mut self) {
        self.len_filled = 0;
        self.body = None;
        self.body_filled = 0;
    }
}

#[async_trait]
impl MeshStream for QuicMeshStream {
    async fn send(&mut self, data: &[u8]) -> Result<(), MeshError> {
        if data.len() > MAX_MESSAGE_SIZE {
            return Err(MeshError::MessageTooLarge {
                size: data.len(),
                max: MAX_MESSAGE_SIZE,
            });
        }
        let len = (data.len() as u32).to_be_bytes();
        self.send
            .write_all(&len)
            .await
            .map_err(|e| MeshError::Io(e.to_string()))?;
        self.send
            .write_all(data)
            .await
            .map_err(|e| MeshError::Io(e.to_string()))?;
        // quinn SendStream has no explicit flush; writes are packetized by the
        // connection. finish() is only for closing the write half.
        Ok(())
    }

    async fn recv(&mut self) -> Result<Vec<u8>, MeshError> {
        // Cancel-safe framed read (ADR-010 / WEFT-18). Progress lives on
        // `self` so dropping this future mid-frame does not desync the
        // length-prefix stream.

        while self.len_filled < 4 {
            let Self {
                recv,
                len_buf,
                len_filled,
                ..
            } = self;
            let n = recv
                .read(&mut len_buf[*len_filled..])
                .await
                .map_err(|e| MeshError::Io(e.to_string()))?
                .ok_or(MeshError::ConnectionClosed)?;
            *len_filled += n;
        }

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

        let expected = self.body.as_ref().map(|b| b.len()).unwrap_or(0);
        while self.body_filled < expected {
            let Self {
                recv,
                body,
                body_filled,
                ..
            } = self;
            let buf = body
                .as_mut()
                .expect("body allocated before body-fill loop");
            if expected == 0 {
                break;
            }
            let n = recv
                .read(&mut buf[*body_filled..])
                .await
                .map_err(|e| MeshError::Io(e.to_string()))?
                .ok_or(MeshError::ConnectionClosed)?;
            *body_filled += n;
        }

        let out = self.body.take().expect("body complete");
        self.reset_frame();
        Ok(out)
    }

    async fn close(&mut self) -> Result<(), MeshError> {
        // Finish the send half; connection drop closes the rest.
        self.send
            .finish()
            .map_err(|e| MeshError::Io(e.to_string()))?;
        Ok(())
    }

    fn remote_addr(&self) -> Option<SocketAddr> {
        Some(self.remote)
    }
}

// ── QuicTransportListener ──────────────────────────────────────────

/// A [`TransportListener`] backed by a quinn [`Endpoint`] server.
pub struct QuicTransportListener {
    endpoint: Endpoint,
}

#[async_trait]
impl TransportListener for QuicTransportListener {
    async fn accept(&mut self) -> Result<(Box<dyn MeshStream>, SocketAddr), MeshError> {
        let incoming = self
            .endpoint
            .accept()
            .await
            .ok_or_else(|| MeshError::Io("quic endpoint closed".into()))?;
        let conn = incoming
            .await
            .map_err(|e| MeshError::Io(format!("quic accept handshake: {e}")))?;
        let remote = conn.remote_address();
        let (send, recv) = conn
            .accept_bi()
            .await
            .map_err(|e| MeshError::Io(format!("quic accept_bi: {e}")))?;
        Ok((
            Box::new(QuicMeshStream::new(send, recv, remote, conn, None)),
            remote,
        ))
    }

    fn local_addr(&self) -> Result<SocketAddr, MeshError> {
        self.endpoint
            .local_addr()
            .map_err(|e| MeshError::Io(e.to_string()))
    }
}

// ── QuicTransport ──────────────────────────────────────────────────

/// QUIC mesh transport (ADR-026 primary for Cloud/Edge).
///
/// Implements [`MeshTransport`] with quinn. Wire addresses use
/// `quic://host:port`. Real peer authentication is Noise on top of the
/// bidirectional stream (see `mesh_noise` / `[kernel.mesh] noise`).
pub struct QuicTransport;

#[async_trait]
impl MeshTransport for QuicTransport {
    fn name(&self) -> &str {
        "quic"
    }

    async fn listen(&self, addr: &str) -> Result<Box<dyn TransportListener>, MeshError> {
        let bind_addr = parse_socket_addr(addr)?;
        let server_config = make_server_config()?;
        let endpoint = Endpoint::server(server_config, bind_addr)
            .map_err(|e| MeshError::Io(format!("quic bind {bind_addr}: {e}")))?;
        Ok(Box::new(QuicTransportListener { endpoint }))
    }

    async fn connect(&self, addr: &str) -> Result<Box<dyn MeshStream>, MeshError> {
        let remote = parse_socket_addr(addr)?;
        let client_config = make_client_config()?;

        // Bind ephemeral local UDP port for the client endpoint.
        let bind: SocketAddr = if remote.is_ipv6() {
            "[::]:0".parse().unwrap()
        } else {
            "0.0.0.0:0".parse().unwrap()
        };
        let mut endpoint =
            Endpoint::client(bind).map_err(|e| MeshError::Io(format!("quic client endpoint: {e}")))?;
        endpoint.set_default_client_config(client_config);

        let conn = endpoint
            .connect(remote, MESH_SNI)
            .map_err(|e| MeshError::Io(format!("quic connect setup: {e}")))?
            .await
            .map_err(|e| MeshError::Io(format!("quic connect: {e}")))?;

        let (send, recv) = conn
            .open_bi()
            .await
            .map_err(|e| MeshError::Io(format!("quic open_bi: {e}")))?;

        Ok(Box::new(QuicMeshStream::new(
            send,
            recv,
            remote,
            conn,
            Some(endpoint),
        )))
    }

    fn supports(&self, addr: &str) -> bool {
        addr.starts_with("quic://")
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn quic_transport_connect_send_recv() {
        let transport = QuicTransport;

        let mut listener = transport.listen("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let quic_url = format!("quic://{addr}");

        let send_task = tokio::spawn(async move {
            let mut stream = QuicTransport.connect(&quic_url).await.unwrap();
            stream.send(b"hello from node A").await.unwrap();
            let response = stream.recv().await.unwrap();
            assert_eq!(response, b"hello from node B");
            stream.close().await.unwrap();
        });

        let (mut server_stream, _client_addr) = listener.accept().await.unwrap();
        let received = server_stream.recv().await.unwrap();
        assert_eq!(received, b"hello from node A");
        server_stream.send(b"hello from node B").await.unwrap();

        send_task.await.unwrap();
    }

    /// Two-node integration: WeftHandshake frames over QUIC (AC for WEFT-118).
    #[tokio::test]
    async fn two_node_cluster_forms_over_quic() {
        use crate::mesh::WeftHandshake;
        use crate::mesh_framing::{FrameType, MeshFrame};

        let transport = QuicTransport;
        let mut listener = transport.listen("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let quic_url = format!("quic://{addr}");

        let node_a = tokio::spawn(async move {
            let mut stream = QuicTransport.connect(&quic_url).await.unwrap();

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
            stream.send(&frame.encode().unwrap()).await.unwrap();

            let data = stream.recv().await.unwrap();
            let len = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
            let response = MeshFrame::decode(&data[4..4 + len]).unwrap();
            assert_eq!(response.frame_type, FrameType::Handshake);
            let remote: WeftHandshake = serde_json::from_slice(&response.payload).unwrap();
            assert_eq!(remote.node_id, "node-b");

            stream.close().await.unwrap();
        });

        let (mut stream, _) = listener.accept().await.unwrap();
        let data = stream.recv().await.unwrap();
        let len = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let frame = MeshFrame::decode(&data[4..4 + len]).unwrap();
        assert_eq!(frame.frame_type, FrameType::Handshake);
        let remote: WeftHandshake = serde_json::from_slice(&frame.payload).unwrap();
        assert_eq!(remote.node_id, "node-a");

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

    /// QUIC + Noise XX: application encryption on top of quinn TLS (AC: quinn + snow).
    #[tokio::test]
    async fn two_node_quic_with_noise() {
        use crate::mesh_noise::{EncryptedChannel, NoiseChannel, NoiseConfig, NoisePattern};
        use rand::RngCore;

        let transport = QuicTransport;
        let mut listener = transport.listen("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let quic_url = format!("quic://{addr}");

        let mut key_a = [0u8; 32];
        let mut key_b = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut key_a);
        rand::thread_rng().fill_bytes(&mut key_b);

        let cfg_a = NoiseConfig {
            pattern: NoisePattern::XX,
            local_private_key: key_a,
            remote_static_key: None,
        };
        let cfg_b = NoiseConfig {
            pattern: NoisePattern::XX,
            local_private_key: key_b,
            remote_static_key: None,
        };

        let node_a = tokio::spawn(async move {
            let stream = QuicTransport.connect(&quic_url).await.unwrap();
            let mut ch = NoiseChannel::initiate(stream, &cfg_a).await.unwrap();
            ch.send_encrypted(b"secret-from-a").await.unwrap();
            let reply = ch.recv_encrypted().await.unwrap();
            assert_eq!(reply, b"secret-from-b");
            ch.close().await.unwrap();
        });

        let (stream, _) = listener.accept().await.unwrap();
        let mut ch = NoiseChannel::respond(stream, &cfg_b).await.unwrap();
        let msg = ch.recv_encrypted().await.unwrap();
        assert_eq!(msg, b"secret-from-a");
        ch.send_encrypted(b"secret-from-b").await.unwrap();

        node_a.await.unwrap();
    }

    #[test]
    fn quic_transport_supports() {
        let t = QuicTransport;
        assert!(t.supports("quic://127.0.0.1:9489"));
        assert!(!t.supports("tcp://127.0.0.1:9489"));
        assert!(!t.supports("ws://127.0.0.1:9489"));
        assert!(!t.supports("127.0.0.1:9489"));
    }

    #[tokio::test]
    async fn quic_recv_survives_select_cancellation() {
        use std::time::Duration;

        let transport = QuicTransport;
        let mut listener = transport.listen("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let quic_url = format!("quic://{addr}");

        let payload_a = vec![0xAAu8; 64];
        let payload_b = vec![0xBBu8; 128];
        let payload_a2 = payload_a.clone();
        let payload_b2 = payload_b.clone();

        let writer = tokio::spawn(async move {
            let mut stream = QuicTransport.connect(&quic_url).await.unwrap();
            stream.send(&payload_a2).await.unwrap();
            tokio::time::sleep(Duration::from_millis(5)).await;
            stream.send(&payload_b2).await.unwrap();
            // Keep the QUIC connection alive until the server has drained
            // both frames. Dropping the client Connection early can abort
            // in-flight stream data (unlike TCP FIN buffering).
            tokio::time::sleep(Duration::from_millis(500)).await;
            let _ = stream.close().await;
        });

        let (mut server, _) = listener.accept().await.unwrap();

        let mut got_first = None;
        for _ in 0..200 {
            tokio::select! {
                biased;
                result = server.recv() => {
                    got_first = Some(result.expect("recv after cancel must succeed"));
                    break;
                }
                _ = tokio::time::sleep(Duration::from_micros(50)) => {}
            }
        }
        let first = got_first.expect("should eventually complete first frame");
        assert_eq!(first, payload_a);

        let second = server.recv().await.expect("second frame");
        assert_eq!(second, payload_b);

        writer.await.unwrap();
    }
}
