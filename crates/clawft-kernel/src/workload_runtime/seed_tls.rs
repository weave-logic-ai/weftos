//! TLS trust for the Cognitum Seed transport.
//!
//! A Seed serves HTTPS with a self-signed certificate, so public-CA
//! verification cannot succeed. Rather than disabling verification (which
//! would hand the per-Seed bearer token to anyone answering the handshake),
//! the operator pins the SHA-256 of the Seed's leaf certificate, recorded
//! once over a trusted path (USB link or first pairing). Any other
//! certificate fails the handshake before a request, and so before the
//! `Authorization` header, is sent.

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};

use super::types::RuntimeError;

/// How an `https://` Seed is authenticated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeedTls {
    /// Public-CA verification (a Seed behind a CA-issued certificate).
    WebPki,
    /// Accept exactly the leaf certificate with this SHA-256 (DER).
    PinnedSha256([u8; 32]),
}

impl SeedTls {
    /// Parse `sha256:<64 hex>` (colons between byte pairs allowed, as
    /// `openssl x509 -fingerprint -sha256` prints them).
    pub fn pinned(fingerprint: &str) -> Result<Self, RuntimeError> {
        let bad = || {
            RuntimeError::InvalidConfig(
                "seed certificate pin must be sha256:<64 hex digits>".into(),
            )
        };
        let hex: String = fingerprint
            .strip_prefix("sha256:")
            .ok_or_else(bad)?
            .chars()
            .filter(|c| *c != ':')
            .collect();
        if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(bad());
        }
        let mut out = [0u8; 32];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).map_err(|_| bad())?;
        }
        Ok(Self::PinnedSha256(out))
    }

    /// `sha256:<hex>` of a DER certificate (for recording a pin).
    pub fn fingerprint(der: &[u8]) -> String {
        let d = Sha256::digest(der);
        let hex: String = d.iter().map(|b| format!("{b:02x}")).collect();
        format!("sha256:{hex}")
    }

    /// The rustls client config for a pinned Seed (`None` for WebPki,
    /// which uses reqwest's default roots).
    pub(crate) fn client_config(&self) -> Option<rustls::ClientConfig> {
        let Self::PinnedSha256(pin) = self else {
            return None;
        };
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let verifier = Arc::new(PinnedCert {
            pin: *pin,
            provider: provider.clone(),
        });
        let mut cfg = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .ok()?
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_no_client_auth();
        cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
        Some(cfg)
    }
}

/// Accepts only the pinned leaf; handshake signatures are still verified,
/// so the peer must hold the pinned certificate's private key.
#[derive(Debug)]
struct PinnedCert {
    pin: [u8; 32],
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl ServerCertVerifier for PinnedCert {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let got: [u8; 32] = Sha256::digest(end_entity.as_ref()).into();
        if got == self.pin {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(
                "seed certificate does not match the pinned fingerprint".into(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}
