//! Delegation certificate lifecycle (WEFT-131 / K1).
//!
//! - [`DelegationCert`]: grant / revoke with Ed25519 signatures
//! - Certificate chain validation against a trust-anchor public key
//! - Time-bounded expiry (`expires_at`)

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::model::ResourceId;

/// Errors from delegation grant / verify / chain validation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DelegationError {
    /// Signature bytes / hex are malformed.
    #[error("bad signature encoding: {0}")]
    BadSignature(String),
    /// Public key bytes / hex are malformed.
    #[error("bad public key encoding: {0}")]
    BadPublicKey(String),
    /// Ed25519 verification failed.
    #[error("signature verification failed")]
    VerificationFailed,
    /// Certificate has been revoked.
    #[error("certificate revoked")]
    Revoked,
    /// Certificate past `expires_at`.
    #[error("certificate expired")]
    Expired,
    /// Chain link is broken (parent signature invalid or grantor mismatch).
    #[error("chain validation failed: {0}")]
    ChainBroken(String),
    /// Empty chain.
    #[error("empty certificate chain")]
    EmptyChain,
}

/// A signed delegation certificate granting temporary access.
///
/// The signable payload is the canonical UTF-8 bytes of
/// `grantor|grantee|resource|created_at|expires_at|parent_sig_hex`.
/// `signature_hex` is the Ed25519 signature over that payload by the
/// grantor's signing key. `grantor_pubkey_hex` is the grantor's
/// verifying key (32-byte hex).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegationCert {
    /// Who granted the delegation (principal id / DID string).
    pub grantor: String,
    /// Who receives the delegation.
    pub grantee: String,
    /// Resource scope of the delegation.
    pub resource: ResourceId,
    /// When the delegation was created (RFC3339 via chrono serde).
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// When the delegation expires (`None` = no expiry).
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Whether this certificate has been revoked.
    pub revoked: bool,
    /// Grantor Ed25519 public key (32 bytes, hex-encoded).
    pub grantor_pubkey_hex: String,
    /// Ed25519 signature over the canonical payload (64 bytes, hex).
    pub signature_hex: String,
    /// Optional parent certificate signature hex (links a chain).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_signature_hex: Option<String>,
}

impl DelegationCert {
    /// Canonical signable message for this certificate (without signature).
    pub fn canonical_message(&self) -> Vec<u8> {
        let exp = self
            .expires_at
            .map(|t| t.to_rfc3339())
            .unwrap_or_else(|| "none".into());
        let parent = self.parent_signature_hex.as_deref().unwrap_or("none");
        format!(
            "{}|{}|{}|{}|{}|{}",
            self.grantor,
            self.grantee,
            self.resource.0.as_str(),
            self.created_at.to_rfc3339(),
            exp,
            parent
        )
        .into_bytes()
    }

    /// Grant a new leaf certificate signed by `grantor_key`.
    pub fn grant(
        grantor: impl Into<String>,
        grantee: impl Into<String>,
        resource: ResourceId,
        grantor_key: &SigningKey,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
        parent: Option<&DelegationCert>,
    ) -> Self {
        let grantor = grantor.into();
        let grantee = grantee.into();
        let created_at = chrono::Utc::now();
        let parent_signature_hex = parent.map(|p| p.signature_hex.clone());
        let mut cert = Self {
            grantor,
            grantee,
            resource,
            created_at,
            expires_at,
            revoked: false,
            grantor_pubkey_hex: bytes_to_hex(&grantor_key.verifying_key().to_bytes()),
            signature_hex: String::new(),
            parent_signature_hex,
        };
        let msg = cert.canonical_message();
        let sig = grantor_key.sign(&msg);
        cert.signature_hex = bytes_to_hex(&sig.to_bytes());
        cert
    }

    /// Mark this certificate revoked (local state). Re-signing is not
    /// required; revocation is a status bit checked by [`Self::validate`].
    pub fn revoke(&mut self) {
        self.revoked = true;
    }

    /// Verify the Ed25519 signature against `grantor_pubkey_hex`.
    pub fn verify_signature(&self) -> Result<(), DelegationError> {
        let pk = decode_hex_32(&self.grantor_pubkey_hex)
            .map_err(DelegationError::BadPublicKey)?;
        let sig_bytes = decode_hex_64(&self.signature_hex).map_err(DelegationError::BadSignature)?;
        let vk = VerifyingKey::from_bytes(&pk)
            .map_err(|e| DelegationError::BadPublicKey(e.to_string()))?;
        let sig = Signature::from_bytes(&sig_bytes);
        vk.verify(&self.canonical_message(), &sig)
            .map_err(|_| DelegationError::VerificationFailed)
    }

    /// Validate signature + revoked + expiry at `now`.
    pub fn validate(&self, now: chrono::DateTime<chrono::Utc>) -> Result<(), DelegationError> {
        if self.revoked {
            return Err(DelegationError::Revoked);
        }
        if let Some(exp) = self.expires_at
            && now > exp {
                return Err(DelegationError::Expired);
            }
        self.verify_signature()
    }
}

/// Validate a certificate chain from trust-anchor outward.
///
/// `chain[0]` must be signed by `trust_anchor` (the root grantor's
/// verifying key). Each subsequent cert must:
/// 1. Pass [`DelegationCert::validate`],
/// 2. List the previous cert's grantee as its grantor,
/// 3. Carry `parent_signature_hex == previous.signature_hex`.
pub fn validate_chain(
    chain: &[DelegationCert],
    trust_anchor: &VerifyingKey,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), DelegationError> {
    if chain.is_empty() {
        return Err(DelegationError::EmptyChain);
    }

    // Root must match trust anchor pubkey.
    let root = &chain[0];
    let root_pk = decode_hex_32(&root.grantor_pubkey_hex).map_err(DelegationError::BadPublicKey)?;
    if root_pk != trust_anchor.to_bytes() {
        return Err(DelegationError::ChainBroken(
            "root grantor pubkey does not match trust anchor".into(),
        ));
    }
    root.validate(now)?;

    for i in 1..chain.len() {
        let prev = &chain[i - 1];
        let cert = &chain[i];
        cert.validate(now)?;
        if cert.grantor != prev.grantee {
            return Err(DelegationError::ChainBroken(format!(
                "link {i}: grantor '{}' != parent grantee '{}'",
                cert.grantor, prev.grantee
            )));
        }
        match &cert.parent_signature_hex {
            Some(ps) if ps == &prev.signature_hex => {}
            Some(_) => {
                return Err(DelegationError::ChainBroken(format!(
                    "link {i}: parent_signature_hex mismatch"
                )));
            }
            None => {
                return Err(DelegationError::ChainBroken(format!(
                    "link {i}: missing parent_signature_hex"
                )));
            }
        }
    }
    Ok(())
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

fn decode_hex_32(s: &str) -> Result<[u8; 32], String> {
    let v = decode_hex(s)?;
    if v.len() != 32 {
        return Err(format!("expected 32 bytes, got {}", v.len()));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&v);
    Ok(out)
}

fn decode_hex_64(s: &str) -> Result<[u8; 64], String> {
    let v = decode_hex(s)?;
    if v.len() != 64 {
        return Err(format!("expected 64 bytes, got {}", v.len()));
    }
    let mut out = [0u8; 64];
    out.copy_from_slice(&v);
    Ok(out)
}

fn decode_hex(s: &str) -> Result<Vec<u8>, String> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) {
        return Err("odd hex length".into());
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hi = hex_val(bytes[i]).ok_or_else(|| format!("bad hex at {i}"))?;
        let lo = hex_val(bytes[i + 1]).ok_or_else(|| format!("bad hex at {}", i + 1))?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Ok(out)
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use ed25519_dalek::SigningKey;

    fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    #[test]
    fn grant_and_validate() {
        let root = key(1);
        let cert = DelegationCert::grant(
            "admin",
            "agent-007",
            ResourceId::new("/apps/secret"),
            &root,
            Some(Utc::now() + Duration::hours(1)),
            None,
        );
        assert!(!cert.revoked);
        assert!(!cert.signature_hex.is_empty());
        cert.validate(Utc::now()).unwrap();
    }

    #[test]
    fn revoke_denies() {
        let root = key(2);
        let mut cert = DelegationCert::grant(
            "admin",
            "agent",
            ResourceId::new("/x"),
            &root,
            None,
            None,
        );
        cert.revoke();
        assert_eq!(cert.validate(Utc::now()), Err(DelegationError::Revoked));
    }

    #[test]
    fn expiry_honored() {
        let root = key(3);
        let cert = DelegationCert::grant(
            "admin",
            "agent",
            ResourceId::new("/x"),
            &root,
            Some(Utc::now() - Duration::seconds(1)),
            None,
        );
        assert_eq!(cert.validate(Utc::now()), Err(DelegationError::Expired));
    }

    #[test]
    fn multi_cert_chain() {
        let root_key = key(4);
        let mid_key = key(5);
        let leaf_key = key(6);

        let root = DelegationCert::grant(
            "root",
            "mid",
            ResourceId::new("/apps"),
            &root_key,
            Some(Utc::now() + Duration::hours(2)),
            None,
        );
        // Mid is both grantee of root and grantor of leaf; mid signs with mid_key.
        // For chain link grantor-name matching we use principal names, not keys.
        let mid = DelegationCert::grant(
            "mid",
            "leaf-agent",
            ResourceId::new("/apps/secret"),
            &mid_key,
            Some(Utc::now() + Duration::hours(1)),
            Some(&root),
        );
        // Optional third hop: leaf re-delegates (name = leaf-agent).
        let leaf = DelegationCert::grant(
            "leaf-agent",
            "worker",
            ResourceId::new("/apps/secret/read"),
            &leaf_key,
            Some(Utc::now() + Duration::minutes(30)),
            Some(&mid),
        );

        let chain = vec![root, mid, leaf];
        validate_chain(&chain, &root_key.verifying_key(), Utc::now()).unwrap();
    }

    #[test]
    fn chain_rejects_wrong_anchor() {
        let root_key = key(7);
        let wrong = key(8);
        let root = DelegationCert::grant(
            "root",
            "mid",
            ResourceId::new("/"),
            &root_key,
            None,
            None,
        );
        let err = validate_chain(&[root], &wrong.verifying_key(), Utc::now()).unwrap_err();
        assert!(matches!(err, DelegationError::ChainBroken(_)));
    }

    #[test]
    fn serde_roundtrip() {
        let root = key(9);
        let cert = DelegationCert::grant(
            "admin",
            "agent-007",
            ResourceId::new("/apps/secret"),
            &root,
            None,
            None,
        );
        let json = serde_json::to_string(&cert).unwrap();
        let back: DelegationCert = serde_json::from_str(&json).unwrap();
        assert_eq!(back.grantor, "admin");
        assert_eq!(back.grantee, "agent-007");
        back.validate(Utc::now()).unwrap();
    }
}
