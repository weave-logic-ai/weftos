//! TLS trust for the Cognitum Seed transport.
//!
//! A Seed serves HTTPS with a self-signed certificate, so public-CA
//! verification cannot succeed. Rather than disabling verification (which
//! would hand the per-Seed bearer token to anyone answering the handshake),
//! the operator pins either the SHA-256 of the Seed's leaf public key
//! ([`SeedTls::PinnedSpki`], recommended) or of the whole leaf certificate
//! ([`SeedTls::PinnedSha256`]), recorded once over a trusted path (USB link
//! or first pairing). Any other certificate fails the handshake before a
//! request, and so before the `Authorization` header, is sent.
//!
//! Firmware 0.22.20 and later cap leaf certificates at 825 days and renew
//! them on the 6-hourly update loop, so a certificate pin can break with
//! no operator action. The SPKI pin survives a renewal that keeps the
//! device key; a renewal that changes the key is refused and needs the
//! operator to re-pin over a trusted path (a new pin, never a silent
//! re-trust).

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
    /// Accept any leaf certificate whose SubjectPublicKeyInfo (DER) has
    /// this SHA-256. Survives certificate renewal under the same key.
    PinnedSpki([u8; 32]),
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

    /// Parse `spki-sha256:<64 hex>` (colons between byte pairs allowed).
    pub fn pinned_spki(fingerprint: &str) -> Result<Self, RuntimeError> {
        let rest = fingerprint.strip_prefix("spki-sha256:").ok_or_else(|| {
            RuntimeError::InvalidConfig("seed key pin must be spki-sha256:<64 hex digits>".into())
        })?;
        match Self::pinned(&format!("sha256:{rest}"))? {
            Self::PinnedSha256(h) => Ok(Self::PinnedSpki(h)),
            other => Ok(other),
        }
    }

    /// `spki-sha256:<hex>` of a DER certificate's public key (for
    /// recording a pin). `None` if the certificate does not parse.
    pub fn spki_fingerprint(der: &[u8]) -> Option<String> {
        let d = Sha256::digest(spki_der(der)?);
        let hex: String = d.iter().map(|b| format!("{b:02x}")).collect();
        Some(format!("spki-sha256:{hex}"))
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
        let pin = match self {
            Self::PinnedSha256(p) => Pin::Cert(*p),
            Self::PinnedSpki(p) => Pin::Spki(*p),
            Self::WebPki => return None,
        };
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let verifier = Arc::new(PinnedCert {
            pin,
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

#[derive(Debug, Clone, Copy)]
enum Pin {
    Cert([u8; 32]),
    Spki([u8; 32]),
}

/// One DER element: returns `(content, rest)` for a `tag` at the start of
/// `b`. Lengths beyond 4 bytes are rejected.
fn der_take(b: &[u8], tag: u8) -> Option<(&[u8], &[u8])> {
    let (&t, b) = b.split_first()?;
    if t != tag {
        return None;
    }
    let (&l0, b) = b.split_first()?;
    let (len, b) = if l0 < 0x80 {
        (l0 as usize, b)
    } else {
        let n = (l0 & 0x7f) as usize;
        if n == 0 || n > 4 || b.len() < n {
            return None;
        }
        let len = b[..n].iter().fold(0usize, |a, x| (a << 8) | *x as usize);
        (len, &b[n..])
    };
    (b.len() >= len).then(|| b.split_at(len))
}

/// The DER `SubjectPublicKeyInfo` element (header included) of an X.509
/// certificate.
fn spki_der(cert: &[u8]) -> Option<&[u8]> {
    let (cert, _) = der_take(cert, 0x30)?;
    let (tbs, _) = der_take(cert, 0x30)?;
    let mut rest = tbs;
    if rest.first() == Some(&0xa0) {
        rest = der_take(rest, 0xa0)?.1; // version
    }
    // serial, signature, issuer, validity, subject
    let (_, r) = der_take(rest, 0x02)?;
    rest = r;
    for _ in 0..4 {
        rest = der_take(rest, 0x30)?.1;
    }
    let (inner, _) = der_take(rest, 0x30)?;
    // Header length = element length minus content length.
    let head = rest.len() - der_take(rest, 0x30)?.1.len() - inner.len();
    Some(&rest[..head + inner.len()])
}

/// Accepts only the pinned leaf; handshake signatures are still verified,
/// so the peer must hold the pinned certificate's private key.
#[derive(Debug)]
struct PinnedCert {
    pin: Pin,
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
        let ok = match self.pin {
            Pin::Cert(p) => <[u8; 32]>::from(Sha256::digest(end_entity.as_ref())) == p,
            Pin::Spki(p) => spki_der(end_entity.as_ref())
                .is_some_and(|k| <[u8; 32]>::from(Sha256::digest(k)) == p),
        };
        if ok {
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
