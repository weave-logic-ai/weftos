//! User certificate: the machine key vouches for a user key (plan 1.4).
//!
//! Signed bytes are a fixed binary layout (no JSON canonicalisation):
//! `"weftos/user-cert/v1\0" || machine_pubkey || user_pubkey || serial(u64 BE)
//! || issued_at(u64 BE) || not_after(u64 BE)`.
//!
//! The local uid is deliberately not in the certificate; it is meaningless to
//! remote peers. The uid binding lives only in the machine journal.

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::hexser::{self, hex32, hex64};

/// Domain separator prepended to the signed bytes.
pub const CERT_DOMAIN: &[u8] = b"weftos/user-cert/v1\0";
/// Certificate format version.
pub const CERT_VERSION: u32 = 1;
/// Default lifetime (plan D-9): 24 hours.
pub const DEFAULT_TTL_S: u64 = 24 * 60 * 60;
/// Verifier leeway on both ends of the validity window (plan D-9): 5 minutes.
pub const LEEWAY_S: u64 = 5 * 60;

/// Derive a node/user id (32 lowercase hex) from an Ed25519 public key.
///
/// Same construction as `clawft-kernel`'s `node_id_from_pubkey` (ADR-103 D11):
/// the first 16 bytes of SHA-256 of the key. Duplicated here so this crate
/// does not depend on the kernel.
pub fn node_id_from_pubkey(pubkey: &[u8; 32]) -> String {
    hexser::encode(&Sha256::digest(pubkey)[..16])
}

/// Why a certificate was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CertError {
    #[error("unsupported certificate version {0}")]
    BadVersion(u32),
    #[error("certificate validity window is empty or inverted")]
    BadValidity,
    #[error("node_id does not match the machine public key")]
    NodeIdMismatch,
    #[error("user_id does not match the user public key")]
    UserIdMismatch,
    #[error("certificate was issued by a machine key that is not trusted")]
    UntrustedMachine,
    #[error("invalid public key")]
    BadKey,
    #[error("signature does not verify")]
    BadSignature,
    #[error("certificate is not valid yet (issued_at {issued_at}, now {now})")]
    NotYetValid { issued_at: u64, now: u64 },
    #[error("certificate expired (not_after {not_after}, now {now})")]
    Expired { not_after: u64, now: u64 },
}

/// A machine-signed binding of a user key to the machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserCert {
    pub v: u32,
    pub node_id: String,
    #[serde(with = "hex32")]
    pub machine_pubkey: [u8; 32],
    #[serde(with = "hex32")]
    pub user_pubkey: [u8; 32],
    pub user_id: String,
    pub serial: u64,
    pub issued_at: u64,
    pub not_after: u64,
    #[serde(with = "hex64")]
    pub sig: [u8; 64],
}

impl UserCert {
    /// The exact bytes the machine key signs.
    pub fn signed_bytes(
        machine_pubkey: &[u8; 32],
        user_pubkey: &[u8; 32],
        serial: u64,
        issued_at: u64,
        not_after: u64,
    ) -> Vec<u8> {
        let mut b = Vec::with_capacity(CERT_DOMAIN.len() + 32 + 32 + 24);
        b.extend_from_slice(CERT_DOMAIN);
        b.extend_from_slice(machine_pubkey);
        b.extend_from_slice(user_pubkey);
        b.extend_from_slice(&serial.to_be_bytes());
        b.extend_from_slice(&issued_at.to_be_bytes());
        b.extend_from_slice(&not_after.to_be_bytes());
        b
    }

    /// Issue a certificate for `user_pubkey` valid for `ttl_s` from `issued_at`.
    pub fn issue(
        machine_key: &SigningKey,
        user_pubkey: [u8; 32],
        serial: u64,
        issued_at: u64,
        ttl_s: u64,
    ) -> Self {
        let machine_pubkey = machine_key.verifying_key().to_bytes();
        let not_after = issued_at.saturating_add(ttl_s);
        let bytes =
            Self::signed_bytes(&machine_pubkey, &user_pubkey, serial, issued_at, not_after);
        Self {
            v: CERT_VERSION,
            node_id: node_id_from_pubkey(&machine_pubkey),
            machine_pubkey,
            user_pubkey,
            user_id: node_id_from_pubkey(&user_pubkey),
            serial,
            issued_at,
            not_after,
            sig: machine_key.sign(&bytes).to_bytes(),
        }
    }

    /// Verify against the machine key the caller already trusts (a pin, or a
    /// machine admitted by the mesh) at unix time `now`.
    pub fn verify(&self, trusted_machine: &[u8; 32], now: u64) -> Result<(), CertError> {
        if self.v != CERT_VERSION {
            return Err(CertError::BadVersion(self.v));
        }
        if self.not_after <= self.issued_at {
            return Err(CertError::BadValidity);
        }
        if self.node_id != node_id_from_pubkey(&self.machine_pubkey) {
            return Err(CertError::NodeIdMismatch);
        }
        if self.user_id != node_id_from_pubkey(&self.user_pubkey) {
            return Err(CertError::UserIdMismatch);
        }
        if &self.machine_pubkey != trusted_machine {
            return Err(CertError::UntrustedMachine);
        }
        let key = VerifyingKey::from_bytes(&self.machine_pubkey).map_err(|_| CertError::BadKey)?;
        let bytes = Self::signed_bytes(
            &self.machine_pubkey,
            &self.user_pubkey,
            self.serial,
            self.issued_at,
            self.not_after,
        );
        key.verify_strict(&bytes, &Signature::from_bytes(&self.sig))
            .map_err(|_| CertError::BadSignature)?;
        if now.saturating_add(LEEWAY_S) < self.issued_at {
            return Err(CertError::NotYetValid { issued_at: self.issued_at, now });
        }
        if now > self.not_after.saturating_add(LEEWAY_S) {
            return Err(CertError::Expired { not_after: self.not_after, now });
        }
        Ok(())
    }

    /// Unix time at which a client should renew: 50% of the lifetime (D-9).
    pub fn renew_due_at(&self) -> u64 {
        self.issued_at + (self.not_after - self.issued_at.min(self.not_after)) / 2
    }
}
