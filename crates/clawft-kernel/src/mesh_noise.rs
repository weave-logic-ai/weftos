//! Noise Protocol encryption for mesh transport (K6.1).
//!
//! Wraps raw [`MeshStream`] with Noise XX or IK handshake encryption.
//! The actual `snow` integration is planned for a future milestone;
//! this module provides the trait contract and a passthrough
//! implementation for testing.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::mesh::{MeshError, MeshStream};

/// Encrypted channel wrapping a raw mesh stream.
///
/// After a Noise handshake completes, all traffic flows through
/// this interface. Each `send_encrypted` / `recv_encrypted` call
/// handles framing, encryption, and authentication transparently.
#[async_trait]
pub trait EncryptedChannel: Send + Sync + 'static {
    /// Send encrypted data (plaintext in, ciphertext on the wire).
    async fn send_encrypted(&mut self, plaintext: &[u8]) -> Result<(), MeshError>;

    /// Receive and decrypt data (ciphertext from wire, plaintext out).
    async fn recv_encrypted(&mut self) -> Result<Vec<u8>, MeshError>;

    /// Get the remote peer's static public key (after handshake).
    fn remote_static_key(&self) -> Option<&[u8]>;

    /// Close the encrypted channel gracefully.
    async fn close(&mut self) -> Result<(), MeshError>;
}

/// Noise handshake pattern.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoisePattern {
    /// XX pattern: mutual authentication, first contact.
    XX,
    /// IK pattern: known responder key, 1-RTT.
    IK,
}

/// Configuration for a Noise handshake.
#[derive(Debug, Clone)]
pub struct NoiseConfig {
    /// Which handshake pattern to use.
    pub pattern: NoisePattern,

    /// Local Ed25519 private key bytes (32 bytes).
    pub local_private_key: [u8; 32],

    /// Remote static public key (required for IK, optional for XX).
    pub remote_static_key: Option<[u8; 32]>,
}

// ── Snow-backed Noise Protocol implementation ───────────────────

/// Real Noise Protocol encrypted channel using `snow`.
///
/// Performs a Noise XX handshake over the underlying `MeshStream`,
/// then encrypts/decrypts all subsequent messages using the established
/// session keys. Uses `Noise_XX_25519_ChaChaPoly_SHA256`.
pub struct NoiseChannel {
    stream: Box<dyn MeshStream>,
    transport: snow::TransportState,
    remote_static: Option<Vec<u8>>,
}

impl NoiseChannel {
    /// Perform a Noise XX handshake as the initiator and return an encrypted channel.
    pub async fn initiate(
        mut stream: Box<dyn MeshStream>,
        config: &NoiseConfig,
    ) -> Result<Self, MeshError> {
        let builder = snow::Builder::new(
            "Noise_XX_25519_ChaChaPoly_SHA256".parse()
                .map_err(|e| MeshError::Handshake(format!("bad noise params: {e}")))?,
        )
        .local_private_key(&config.local_private_key)
        .map_err(|e| MeshError::Handshake(format!("bad private key: {e}")))?;

        let mut handshake = builder
            .build_initiator()
            .map_err(|e| MeshError::Handshake(format!("build initiator: {e}")))?;

        let mut buf = vec![0u8; 65535];

        // XX pattern: initiator sends → responder sends → initiator sends

        // -> e
        let len = handshake.write_message(&[], &mut buf)
            .map_err(|e| MeshError::Handshake(format!("write msg 1: {e}")))?;
        stream.send(&buf[..len]).await?;

        // <- e, ee, s, es
        let msg = stream.recv().await?;
        handshake.read_message(&msg, &mut buf)
            .map_err(|e| MeshError::Handshake(format!("read msg 2: {e}")))?;

        // -> s, se
        let len = handshake.write_message(&[], &mut buf)
            .map_err(|e| MeshError::Handshake(format!("write msg 3: {e}")))?;
        stream.send(&buf[..len]).await?;

        let remote_static = handshake.get_remote_static().map(|k| k.to_vec());

        let transport = handshake.into_transport_mode()
            .map_err(|e| MeshError::Handshake(format!("transport mode: {e}")))?;

        tracing::info!("noise XX handshake complete (initiator)");

        Ok(Self { stream, transport, remote_static })
    }

    /// Perform a Noise XX handshake as the responder and return an encrypted channel.
    pub async fn respond(
        mut stream: Box<dyn MeshStream>,
        config: &NoiseConfig,
    ) -> Result<Self, MeshError> {
        let builder = snow::Builder::new(
            "Noise_XX_25519_ChaChaPoly_SHA256".parse()
                .map_err(|e| MeshError::Handshake(format!("bad noise params: {e}")))?,
        )
        .local_private_key(&config.local_private_key)
        .map_err(|e| MeshError::Handshake(format!("bad private key: {e}")))?;

        let mut handshake = builder
            .build_responder()
            .map_err(|e| MeshError::Handshake(format!("build responder: {e}")))?;

        let mut buf = vec![0u8; 65535];

        // XX pattern: initiator sends → responder sends → initiator sends

        // <- e
        let msg = stream.recv().await?;
        handshake.read_message(&msg, &mut buf)
            .map_err(|e| MeshError::Handshake(format!("read msg 1: {e}")))?;

        // -> e, ee, s, es
        let len = handshake.write_message(&[], &mut buf)
            .map_err(|e| MeshError::Handshake(format!("write msg 2: {e}")))?;
        stream.send(&buf[..len]).await?;

        // <- s, se
        let msg = stream.recv().await?;
        handshake.read_message(&msg, &mut buf)
            .map_err(|e| MeshError::Handshake(format!("read msg 3: {e}")))?;

        let remote_static = handshake.get_remote_static().map(|k| k.to_vec());

        let transport = handshake.into_transport_mode()
            .map_err(|e| MeshError::Handshake(format!("transport mode: {e}")))?;

        tracing::info!("noise XX handshake complete (responder)");

        Ok(Self { stream, transport, remote_static })
    }
}

#[async_trait]
impl EncryptedChannel for NoiseChannel {
    async fn send_encrypted(&mut self, plaintext: &[u8]) -> Result<(), MeshError> {
        let mut buf = vec![0u8; plaintext.len() + 16 + 2]; // AEAD tag + length prefix room
        let len = self.transport.write_message(plaintext, &mut buf)
            .map_err(|e| MeshError::Io(format!("encrypt: {e}")))?;
        self.stream.send(&buf[..len]).await
    }

    async fn recv_encrypted(&mut self) -> Result<Vec<u8>, MeshError> {
        let ciphertext = self.stream.recv().await?;
        let mut buf = vec![0u8; ciphertext.len()];
        let len = self.transport.read_message(&ciphertext, &mut buf)
            .map_err(|e| MeshError::Io(format!("decrypt: {e}")))?;
        buf.truncate(len);
        Ok(buf)
    }

    fn remote_static_key(&self) -> Option<&[u8]> {
        self.remote_static.as_deref()
    }

    async fn close(&mut self) -> Result<(), MeshError> {
        self.stream.close().await
    }
}

// ── Passthrough (test/dev) implementation ────────────────────────

/// Passthrough "encryption" for testing and development.
///
/// No actual crypto — plaintext passes through unchanged.
/// Use `NoiseChannel` for production deployments.
pub struct PassthroughChannel {
    stream: Box<dyn MeshStream>,
}

impl PassthroughChannel {
    /// Wrap a raw stream with passthrough (no-op) encryption.
    pub fn new(stream: Box<dyn MeshStream>) -> Self {
        Self { stream }
    }
}

#[async_trait]
impl EncryptedChannel for PassthroughChannel {
    async fn send_encrypted(&mut self, plaintext: &[u8]) -> Result<(), MeshError> {
        self.stream.send(plaintext).await
    }

    async fn recv_encrypted(&mut self) -> Result<Vec<u8>, MeshError> {
        self.stream.recv().await
    }

    fn remote_static_key(&self) -> Option<&[u8]> {
        None
    }

    async fn close(&mut self) -> Result<(), MeshError> {
        self.stream.close().await
    }
}

/// Create an encrypted channel for a mesh connection.
///
/// When `noise_config` is `Some`, performs a real Noise XX handshake.
/// When `None`, returns a passthrough (plaintext) channel.
pub async fn create_encrypted_channel(
    stream: Box<dyn MeshStream>,
    noise_config: Option<&NoiseConfig>,
    is_initiator: bool,
) -> Result<Box<dyn EncryptedChannel>, MeshError> {
    match noise_config {
        Some(config) => {
            let channel = if is_initiator {
                NoiseChannel::initiate(stream, config).await?
            } else {
                NoiseChannel::respond(stream, config).await?
            };
            Ok(Box::new(channel))
        }
        None => Ok(Box::new(PassthroughChannel::new(stream))),
    }
}

// ── Key rotation protocol ───────────────────────────────────────

/// Key rotation state for a mesh connection.
#[derive(Debug, Clone)]
pub struct KeyRotationState {
    /// Current key generation number.
    pub generation: u64,
    /// When the current key was established.
    pub established_at: std::time::Instant,
    /// Maximum key lifetime before rotation.
    pub max_lifetime: std::time::Duration,
    /// Grace period: old key accepted during transition.
    pub grace_period: std::time::Duration,
    /// Whether rotation is in progress.
    pub rotating: bool,
}

impl KeyRotationState {
    /// Create a new key rotation state at generation 0.
    pub fn new(
        max_lifetime: std::time::Duration,
        grace_period: std::time::Duration,
    ) -> Self {
        Self {
            generation: 0,
            established_at: std::time::Instant::now(),
            max_lifetime,
            grace_period,
            rotating: false,
        }
    }

    /// Check if key rotation is needed based on elapsed lifetime.
    pub fn needs_rotation(&self) -> bool {
        self.established_at.elapsed() > self.max_lifetime
    }

    /// Start key rotation (marks as in-progress).
    pub fn begin_rotation(&mut self) {
        self.rotating = true;
    }

    /// Complete key rotation, advancing to the next generation.
    pub fn complete_rotation(&mut self) {
        self.generation += 1;
        self.established_at = std::time::Instant::now();
        self.rotating = false;
    }

    /// Check if the old key (previous generation) is still valid.
    /// True during active rotation or within the grace period.
    pub fn old_key_valid(&self) -> bool {
        self.rotating || self.established_at.elapsed() < self.grace_period
    }
}

// ── Post-quantum KEM types and hybrid handshake protocol ──────────

/// Post-quantum Key Encapsulation Mechanism (KEM) configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KemConfig {
    /// Whether this node supports ML-KEM-768.
    pub kem_supported: bool,
    /// KEM public key (1184 bytes for ML-KEM-768).
    pub kem_public_key: Option<Vec<u8>>,
}

/// Result of KEM capability negotiation between two peers.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KemNegotiationResult {
    /// Both sides support KEM -- full PQ protection.
    BothSupported,
    /// One side lacks KEM -- graceful fallback to classical.
    GracefulDegradation,
    /// Neither supports KEM -- classical only.
    ClassicalOnly,
}

/// Negotiate KEM capability between two peers.
pub fn negotiate_kem(local: &KemConfig, remote: &KemConfig) -> KemNegotiationResult {
    match (local.kem_supported, remote.kem_supported) {
        (true, true) => KemNegotiationResult::BothSupported,
        (true, false) | (false, true) => KemNegotiationResult::GracefulDegradation,
        (false, false) => KemNegotiationResult::ClassicalOnly,
    }
}

/// Result of a hybrid key exchange (classical Noise XX + optional ML-KEM-768).
#[derive(Debug, Clone)]
pub struct HybridKeyExchange {
    /// Classical shared secret (from Noise XX X25519 DH).
    pub classical_secret: Vec<u8>,
    /// Post-quantum shared secret (from ML-KEM-768, if both sides support).
    pub pq_secret: Option<Vec<u8>>,
    /// Final combined key: SHA-256(classical || pq || "weftos-hybrid-kem-v1").
    pub final_key: Vec<u8>,
    /// Whether PQ was used.
    pub pq_active: bool,
}

impl HybridKeyExchange {
    /// Combine classical and PQ secrets into a final hybrid key.
    ///
    /// The combination is: `SHA-256(classical || pq || "weftos-hybrid-kem-v1")`.
    /// When `pq` is `None`, the domain tag still differentiates the output
    /// from a raw classical key.
    pub fn combine(classical: Vec<u8>, pq: Option<Vec<u8>>) -> Self {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(&classical);
        if let Some(ref pq_s) = pq {
            hasher.update(pq_s);
        }
        hasher.update(b"weftos-hybrid-kem-v1");
        let final_key = hasher.finalize().to_vec();

        Self {
            pq_active: pq.is_some(),
            classical_secret: classical,
            pq_secret: pq,
            final_key,
        }
    }
}

/// KEM upgrade protocol: runs after Noise XX handshake completes,
/// before sync streams are opened.
///
/// When both peers support ML-KEM-768, the upgrade must complete
/// before any sync stream traffic begins, protecting against
/// store-now-decrypt-later attacks.
pub struct KemUpgradeProtocol {
    /// Local KEM config.
    pub local: KemConfig,
    /// Remote KEM config (from WeftHandshake).
    pub remote: KemConfig,
    /// Negotiation result.
    pub result: KemNegotiationResult,
}

impl KemUpgradeProtocol {
    /// Create a new KEM upgrade protocol from local and remote configs.
    pub fn new(local: KemConfig, remote: KemConfig) -> Self {
        let result = negotiate_kem(&local, &remote);
        Self { local, remote, result }
    }

    /// Whether sync streams should wait for KEM to complete.
    ///
    /// Returns `true` when both sides support KEM, meaning the
    /// PQ key exchange MUST finish before any sync traffic flows.
    pub fn requires_upgrade(&self) -> bool {
        self.result == KemNegotiationResult::BothSupported
    }

    /// Execute the KEM exchange, combining the classical Noise secret
    /// with the post-quantum shared secret (when both sides support it).
    ///
    /// In the current placeholder implementation, the PQ secret is
    /// derived deterministically from the remote KEM public key.
    /// A real implementation would use ML-KEM-768 encapsulate/decapsulate.
    pub fn execute(&self, classical_secret: Vec<u8>) -> HybridKeyExchange {
        match self.result {
            KemNegotiationResult::BothSupported => {
                // In real impl: ML-KEM-768 encapsulate/decapsulate.
                // Placeholder: derive PQ secret from remote public key.
                let pq_secret = {
                    use sha2::{Digest, Sha256};
                    let mut h = Sha256::new();
                    h.update(b"ml-kem-768-simulated");
                    if let Some(ref pk) = self.remote.kem_public_key {
                        h.update(pk);
                    }
                    h.finalize().to_vec()
                };
                HybridKeyExchange::combine(classical_secret, Some(pq_secret))
            }
            _ => HybridKeyExchange::combine(classical_secret, None),
        }
    }
}

// ── In-memory mock stream for testing ───────────────────────────

#[cfg(test)]
mod mock {
    use std::collections::VecDeque;
    use std::net::SocketAddr;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;

    use crate::mesh::{MeshError, MeshStream};

    /// Shared buffer backing a pair of mock streams.
    #[derive(Debug, Default)]
    pub struct MockBuffer {
        pub data: VecDeque<Vec<u8>>,
    }

    /// An in-memory mock [`MeshStream`] for unit tests.
    pub struct MockStream {
        /// Outbound buffer (what this side sends).
        pub tx: Arc<Mutex<MockBuffer>>,
        /// Inbound buffer (what this side receives).
        pub rx: Arc<Mutex<MockBuffer>>,
        pub closed: bool,
    }

    impl MockStream {
        /// Create a connected pair of mock streams.
        pub fn pair() -> (Self, Self) {
            let a_to_b = Arc::new(Mutex::new(MockBuffer::default()));
            let b_to_a = Arc::new(Mutex::new(MockBuffer::default()));

            let a = MockStream {
                tx: Arc::clone(&a_to_b),
                rx: Arc::clone(&b_to_a),
                closed: false,
            };
            let b = MockStream {
                tx: b_to_a,
                rx: a_to_b,
                closed: false,
            };
            (a, b)
        }
    }

    #[async_trait]
    impl MeshStream for MockStream {
        async fn send(&mut self, data: &[u8]) -> Result<(), MeshError> {
            if self.closed {
                return Err(MeshError::ConnectionClosed);
            }
            self.tx.lock().unwrap().data.push_back(data.to_vec());
            Ok(())
        }

        async fn recv(&mut self) -> Result<Vec<u8>, MeshError> {
            if self.closed {
                return Err(MeshError::ConnectionClosed);
            }
            self.rx
                .lock()
                .unwrap()
                .data
                .pop_front()
                .ok_or(MeshError::ConnectionClosed)
        }

        async fn close(&mut self) -> Result<(), MeshError> {
            self.closed = true;
            Ok(())
        }

        fn remote_addr(&self) -> Option<SocketAddr> {
            Some("127.0.0.1:0".parse().unwrap())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::mock::MockStream;

    #[test]
    fn noise_pattern_variants() {
        assert_eq!(NoisePattern::XX, NoisePattern::XX);
        assert_ne!(NoisePattern::XX, NoisePattern::IK);
    }

    #[test]
    fn noise_pattern_serde_roundtrip() {
        for pattern in [NoisePattern::XX, NoisePattern::IK] {
            let json = serde_json::to_string(&pattern).unwrap();
            let restored: NoisePattern = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, pattern);
        }
    }

    #[test]
    fn noise_config_fields() {
        let cfg = NoiseConfig {
            pattern: NoisePattern::XX,
            local_private_key: [0x42; 32],
            remote_static_key: None,
        };
        assert_eq!(cfg.pattern, NoisePattern::XX);
        assert_eq!(cfg.local_private_key, [0x42; 32]);
        assert!(cfg.remote_static_key.is_none());

        let cfg_ik = NoiseConfig {
            pattern: NoisePattern::IK,
            local_private_key: [1; 32],
            remote_static_key: Some([2; 32]),
        };
        assert_eq!(cfg_ik.pattern, NoisePattern::IK);
        assert_eq!(cfg_ik.remote_static_key, Some([2; 32]));
    }

    #[tokio::test]
    async fn passthrough_send_recv() {
        let (a, b) = MockStream::pair();
        let mut chan_a = PassthroughChannel::new(Box::new(a));
        let mut chan_b = PassthroughChannel::new(Box::new(b));

        chan_a.send_encrypted(b"hello mesh").await.unwrap();
        let received = chan_b.recv_encrypted().await.unwrap();
        assert_eq!(received, b"hello mesh");
    }

    #[tokio::test]
    async fn passthrough_bidirectional() {
        let (a, b) = MockStream::pair();
        let mut chan_a = PassthroughChannel::new(Box::new(a));
        let mut chan_b = PassthroughChannel::new(Box::new(b));

        chan_a.send_encrypted(b"ping").await.unwrap();
        let msg = chan_b.recv_encrypted().await.unwrap();
        assert_eq!(msg, b"ping");

        chan_b.send_encrypted(b"pong").await.unwrap();
        let msg = chan_a.recv_encrypted().await.unwrap();
        assert_eq!(msg, b"pong");
    }

    #[test]
    fn passthrough_no_remote_key() {
        let (a, _b) = MockStream::pair();
        let chan = PassthroughChannel::new(Box::new(a));
        assert!(chan.remote_static_key().is_none());
    }

    // ── Key rotation tests ────────────────────────────────────────

    #[test]
    fn key_rotation_initial_state() {
        let state = KeyRotationState::new(
            std::time::Duration::from_secs(3600),
            std::time::Duration::from_secs(60),
        );
        assert_eq!(state.generation, 0);
        assert!(!state.rotating);
        assert!(!state.needs_rotation());
    }

    #[test]
    fn key_rotation_needs_rotation_after_lifetime() {
        let mut state = KeyRotationState::new(
            std::time::Duration::from_secs(0), // immediate
            std::time::Duration::from_secs(60),
        );
        // With zero lifetime, needs rotation immediately
        assert!(state.needs_rotation());
        // But old key is still valid within grace period
        assert!(state.old_key_valid());

        state.begin_rotation();
        assert!(state.rotating);
        assert!(state.old_key_valid()); // always valid during rotation
    }

    #[test]
    fn key_rotation_complete_lifecycle() {
        let mut state = KeyRotationState::new(
            std::time::Duration::from_secs(0),
            std::time::Duration::from_secs(3600), // long grace
        );
        assert_eq!(state.generation, 0);

        state.begin_rotation();
        assert!(state.rotating);

        state.complete_rotation();
        assert_eq!(state.generation, 1);
        assert!(!state.rotating);
        // After completion, old key valid within grace period
        assert!(state.old_key_valid());
    }

    #[test]
    fn key_rotation_multiple_generations() {
        let mut state = KeyRotationState::new(
            std::time::Duration::from_secs(0),
            std::time::Duration::from_secs(3600),
        );
        for i in 0..5 {
            assert_eq!(state.generation, i);
            state.begin_rotation();
            state.complete_rotation();
        }
        assert_eq!(state.generation, 5);
    }

    #[tokio::test]
    async fn passthrough_close() {
        let (a, _b) = MockStream::pair();
        let mut chan = PassthroughChannel::new(Box::new(a));
        chan.close().await.unwrap();
        // After close, send should fail
        let result = chan.send_encrypted(b"after close").await;
        assert!(result.is_err());
    }

    // ── KEM negotiation and hybrid handshake tests ───────────────

    #[test]
    fn kem_negotiation_both_supported() {
        let local = KemConfig {
            kem_supported: true,
            kem_public_key: Some(vec![0xAA; 1184]),
        };
        let remote = KemConfig {
            kem_supported: true,
            kem_public_key: Some(vec![0xBB; 1184]),
        };
        assert_eq!(
            negotiate_kem(&local, &remote),
            KemNegotiationResult::BothSupported,
        );
    }

    #[test]
    fn kem_negotiation_graceful_degradation() {
        // Local supports, remote does not.
        let local = KemConfig {
            kem_supported: true,
            kem_public_key: Some(vec![0xAA; 1184]),
        };
        let remote = KemConfig {
            kem_supported: false,
            kem_public_key: None,
        };
        assert_eq!(
            negotiate_kem(&local, &remote),
            KemNegotiationResult::GracefulDegradation,
        );

        // Remote supports, local does not.
        assert_eq!(
            negotiate_kem(&remote, &local),
            KemNegotiationResult::GracefulDegradation,
        );
    }

    #[test]
    fn kem_negotiation_classical_only() {
        let local = KemConfig {
            kem_supported: false,
            kem_public_key: None,
        };
        let remote = KemConfig {
            kem_supported: false,
            kem_public_key: None,
        };
        assert_eq!(
            negotiate_kem(&local, &remote),
            KemNegotiationResult::ClassicalOnly,
        );
    }

    #[test]
    fn hybrid_key_exchange_with_pq() {
        let classical = vec![0x01; 32];
        let pq = vec![0x02; 32];
        let hke = HybridKeyExchange::combine(classical.clone(), Some(pq.clone()));

        assert!(hke.pq_active, "PQ should be active");
        assert_eq!(hke.classical_secret, classical);
        assert_eq!(hke.pq_secret.as_ref().unwrap(), &pq);
        assert_eq!(hke.final_key.len(), 32, "SHA-256 output is 32 bytes");

        // Final key should differ from classical-only derivation.
        let classical_only = HybridKeyExchange::combine(classical.clone(), None);
        assert_ne!(
            hke.final_key, classical_only.final_key,
            "hybrid key must differ from classical-only key"
        );
    }

    #[test]
    fn hybrid_key_exchange_classical_only() {
        let classical = vec![0x01; 32];
        let hke = HybridKeyExchange::combine(classical.clone(), None);

        assert!(!hke.pq_active, "PQ should not be active");
        assert!(hke.pq_secret.is_none());
        assert_eq!(hke.final_key.len(), 32);
    }

    #[test]
    fn kem_upgrade_requires_upgrade_when_both_support() {
        let local = KemConfig {
            kem_supported: true,
            kem_public_key: Some(vec![0xAA; 1184]),
        };
        let remote = KemConfig {
            kem_supported: true,
            kem_public_key: Some(vec![0xBB; 1184]),
        };
        let protocol = KemUpgradeProtocol::new(local, remote);
        assert!(
            protocol.requires_upgrade(),
            "must require upgrade when both sides support KEM"
        );
        assert_eq!(protocol.result, KemNegotiationResult::BothSupported);
    }

    #[test]
    fn kem_upgrade_no_upgrade_when_degraded() {
        let local = KemConfig {
            kem_supported: true,
            kem_public_key: Some(vec![0xAA; 1184]),
        };
        let remote = KemConfig {
            kem_supported: false,
            kem_public_key: None,
        };
        let protocol = KemUpgradeProtocol::new(local, remote);
        assert!(
            !protocol.requires_upgrade(),
            "should not require upgrade with graceful degradation"
        );
    }

    #[test]
    fn kem_upgrade_before_sync_streams() {
        // Both sides support KEM: requires_upgrade is true and execute
        // produces a hybrid key with PQ active.
        let local = KemConfig {
            kem_supported: true,
            kem_public_key: Some(vec![0xAA; 1184]),
        };
        let remote = KemConfig {
            kem_supported: true,
            kem_public_key: Some(vec![0xBB; 1184]),
        };
        let protocol = KemUpgradeProtocol::new(local, remote);
        assert!(protocol.requires_upgrade());

        let classical_secret = vec![0xCC; 32];
        let hke = protocol.execute(classical_secret.clone());

        assert!(hke.pq_active, "PQ must be active after KEM upgrade");
        assert!(hke.pq_secret.is_some(), "PQ secret must be present");
        assert_eq!(hke.final_key.len(), 32);

        // Classical-only fallback produces different key.
        let classical_only = HybridKeyExchange::combine(classical_secret, None);
        assert_ne!(
            hke.final_key, classical_only.final_key,
            "hybrid final key must differ from classical-only"
        );
    }

    #[test]
    fn kem_upgrade_classical_fallback() {
        // When degraded, execute produces classical-only key.
        let local = KemConfig {
            kem_supported: false,
            kem_public_key: None,
        };
        let remote = KemConfig {
            kem_supported: false,
            kem_public_key: None,
        };
        let protocol = KemUpgradeProtocol::new(local, remote);
        assert!(!protocol.requires_upgrade());

        let classical_secret = vec![0xCC; 32];
        let hke = protocol.execute(classical_secret);

        assert!(!hke.pq_active);
        assert!(hke.pq_secret.is_none());
    }

    #[test]
    fn kem_config_serde_roundtrip() {
        let cfg = KemConfig {
            kem_supported: true,
            kem_public_key: Some(vec![0x42; 1184]),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let restored: KemConfig = serde_json::from_str(&json).unwrap();
        assert!(restored.kem_supported);
        assert_eq!(restored.kem_public_key.unwrap().len(), 1184);
    }

    #[test]
    fn kem_negotiation_result_serde_roundtrip() {
        for result in [
            KemNegotiationResult::BothSupported,
            KemNegotiationResult::GracefulDegradation,
            KemNegotiationResult::ClassicalOnly,
        ] {
            let json = serde_json::to_string(&result).unwrap();
            let restored: KemNegotiationResult = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, result);
        }
    }
}
