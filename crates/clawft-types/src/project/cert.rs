//! Project key certificate, anchor statement and proof-of-possession bytes
//! (ADR-103 A6, Phase 2 package A).
//!
//! One project key (`<root>/.weftos/project.key`) is the node key, the chain
//! signing key, the anchor key and the proof-of-possession key. It therefore
//! never signs bare bytes: every statement starts with a domain tag.
//!
//! | Statement | Signer | Signed bytes |
//! |---|---|---|
//! | [`ProjectCert`] | user key | `"weftos-project-cert-v1\n"` + canonical JSON of the cert without `sig` |
//! | [`ProjectAnchorStmt`] | project key | `"weftos-project-anchor-v1\n"` + canonical JSON of the statement without `sig` |
//! | mesh-local PoP | project key | `"weftos-mesh-local-pop-v1\n"` + nonce + `"\n"` + project id (see [`pop_signed_bytes`]) |
//!
//! Canonical JSON is defined in [`canonical_json`]: sorted keys, no
//! whitespace. Hex fields are lowercase. A `key_id` is the first 16 bytes of
//! SHA-256 of the public key (32 hex characters); public keys and signatures
//! are 32 and 64 bytes (64 and 128 hex characters).
//!
//! A certificate carries no root path (projects move). `expires_at` is null
//! in Phase 2; the verifier honours it anyway.

use chrono::{DateTime, SecondsFormat, Utc};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::canon::{canonical_json, hex_decode, hex_encode};

/// Domain tag prepended to the signed bytes of a [`ProjectCert`].
pub const CERT_DOMAIN: &str = "weftos-project-cert-v1\n";
/// Domain tag prepended to the signed bytes of a [`ProjectAnchorStmt`].
pub const ANCHOR_DOMAIN: &str = "weftos-project-anchor-v1\n";
/// Domain tag prepended to the mesh-local proof-of-possession bytes.
pub const POP_DOMAIN: &str = "weftos-mesh-local-pop-v1\n";
/// `type` field of a project certificate.
pub const CERT_TYPE: &str = "project-cert";
/// Certificate format version.
pub const CERT_VERSION: u32 = 1;

/// Why a certificate or statement failed verification.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CertError {
    /// `v` is not the supported version.
    #[error("unsupported certificate version {0}")]
    BadVersion(u32),
    /// `type` is not `project-cert`.
    #[error("unexpected certificate type {0:?}")]
    BadType(String),
    /// A hex field is malformed or has the wrong length.
    #[error("malformed hex field `{0}`")]
    BadHex(&'static str),
    /// A timestamp field is not RFC 3339.
    #[error("malformed timestamp field `{0}`")]
    BadTimestamp(&'static str),
    /// A `*_key_id` does not equal the hash of its public key.
    #[error("`{0}` does not match its public key")]
    KeyIdMismatch(&'static str),
    /// The certificate was not issued by the trusted user key.
    #[error("certificate not issued by the trusted user key")]
    UntrustedUser,
    /// The signature does not verify.
    #[error("signature does not verify")]
    BadSignature,
    /// `expires_at` is at or before `now`.
    #[error("certificate expired")]
    Expired,
}

/// First 16 bytes of SHA-256 of `pubkey`, as 32 lowercase hex characters.
pub fn key_id(pubkey: &[u8]) -> String {
    hex_encode(&Sha256::digest(pubkey)[..16])
}

fn ts(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn parse_ts(s: &str, field: &'static str) -> Result<DateTime<Utc>, CertError> {
    DateTime::parse_from_rfc3339(s)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|_| CertError::BadTimestamp(field))
}

fn verifying_key(pk: &[u8; 32]) -> Result<VerifyingKey, CertError> {
    VerifyingKey::from_bytes(pk).map_err(|_| CertError::BadHex("pubkey"))
}

fn check_sig(
    pk: &[u8; 32],
    sig_hex: &str,
    domain: &str,
    body: &Value,
) -> Result<(), CertError> {
    let sig: [u8; 64] = hex_decode(sig_hex).ok_or(CertError::BadHex("sig"))?;
    let bytes = domain_bytes(domain, body);
    verifying_key(pk)?
        .verify_strict(&bytes, &Signature::from_bytes(&sig))
        .map_err(|_| CertError::BadSignature)
}

fn domain_bytes(domain: &str, body: &Value) -> Vec<u8> {
    let mut out = domain.as_bytes().to_vec();
    out.extend_from_slice(canonical_json(body).as_bytes());
    out
}

/// Inputs the user daemon chooses when it certifies a project key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertRequest {
    /// Project ULID.
    pub project_id: String,
    /// The project's public key.
    pub project_pubkey: [u8; 32],
    /// Monotonic serial (1 on first issue).
    pub serial: u64,
    /// Issue time (second precision is signed).
    pub issued_at: DateTime<Utc>,
    /// Expiry; `None` in Phase 2.
    pub expires_at: Option<DateTime<Utc>>,
}

/// The project key certificate, `~/.weftos/projects/<id>.cert.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectCert {
    /// Format version (1).
    pub v: u32,
    /// Always `project-cert`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Project ULID.
    pub project_id: String,
    /// Project public key, hex (64 chars).
    pub project_pubkey: String,
    /// `key_id(project_pubkey)`, hex (32 chars).
    pub project_key_id: String,
    /// `key_id(user_pubkey)`, hex (32 chars).
    pub user_key_id: String,
    /// Issuing user public key, hex (64 chars).
    pub user_pubkey: String,
    /// Monotonic serial.
    pub serial: u64,
    /// RFC 3339 UTC, second precision.
    pub issued_at: String,
    /// RFC 3339 UTC or null (never expires).
    pub expires_at: Option<String>,
    /// Ed25519 signature by the user key, hex (128 chars).
    #[serde(default)]
    pub sig: String,
}

impl ProjectCert {
    /// The signed JSON object: every field except `sig`.
    fn body(&self) -> Value {
        json!({
            "v": self.v,
            "type": self.kind,
            "project_id": self.project_id,
            "project_pubkey": self.project_pubkey,
            "project_key_id": self.project_key_id,
            "user_key_id": self.user_key_id,
            "user_pubkey": self.user_pubkey,
            "serial": self.serial,
            "issued_at": self.issued_at,
            "expires_at": self.expires_at,
        })
    }

    /// The bytes the user key signs: domain tag plus canonical JSON.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        domain_bytes(CERT_DOMAIN, &self.body())
    }

    /// Certify `req` with the user key. Deterministic: the same inputs give
    /// byte-identical signed bytes and signature.
    pub fn sign(user_key: &SigningKey, req: &CertRequest) -> Self {
        let user_pubkey = user_key.verifying_key().to_bytes();
        let mut cert = Self {
            v: CERT_VERSION,
            kind: CERT_TYPE.to_owned(),
            project_id: req.project_id.clone(),
            project_pubkey: hex_encode(&req.project_pubkey),
            project_key_id: key_id(&req.project_pubkey),
            user_key_id: key_id(&user_pubkey),
            user_pubkey: hex_encode(&user_pubkey),
            serial: req.serial,
            issued_at: ts(req.issued_at),
            expires_at: req.expires_at.map(ts),
            sig: String::new(),
        };
        cert.sig = hex_encode(&user_key.sign(&cert.canonical_bytes()).to_bytes());
        cert
    }

    /// The project key id (`key_id` of `project_pubkey`), recomputed.
    pub fn computed_key_id(&self) -> Result<String, CertError> {
        let pk: [u8; 32] =
            hex_decode(&self.project_pubkey).ok_or(CertError::BadHex("project_pubkey"))?;
        Ok(key_id(&pk))
    }

    /// Verify against the user key the caller trusts: version, type, hex
    /// shapes, both key ids recomputed from their public keys, issuer, the
    /// signature, then expiry against `now`.
    pub fn verify(
        &self,
        trusted_user_pubkey: &[u8; 32],
        now: DateTime<Utc>,
    ) -> Result<(), CertError> {
        if self.v != CERT_VERSION {
            return Err(CertError::BadVersion(self.v));
        }
        if self.kind != CERT_TYPE {
            return Err(CertError::BadType(self.kind.clone()));
        }
        let project_pk: [u8; 32] =
            hex_decode(&self.project_pubkey).ok_or(CertError::BadHex("project_pubkey"))?;
        let user_pk: [u8; 32] =
            hex_decode(&self.user_pubkey).ok_or(CertError::BadHex("user_pubkey"))?;
        if self.project_key_id != key_id(&project_pk) {
            return Err(CertError::KeyIdMismatch("project_key_id"));
        }
        if self.user_key_id != key_id(&user_pk) {
            return Err(CertError::KeyIdMismatch("user_key_id"));
        }
        if &user_pk != trusted_user_pubkey {
            return Err(CertError::UntrustedUser);
        }
        parse_ts(&self.issued_at, "issued_at")?;
        let expiry = self
            .expires_at
            .as_deref()
            .map(|s| parse_ts(s, "expires_at"))
            .transpose()?;
        check_sig(&user_pk, &self.sig, CERT_DOMAIN, &self.body())?;
        match expiry {
            Some(t) if t <= now => Err(CertError::Expired),
            _ => Ok(()),
        }
    }
}

/// The project's signed statement of its chain head, appended by the user
/// daemon as a `project.anchor` event.
///
/// Honest limit: the user daemon attests "this key claimed head X at time
/// T"; it cannot verify X without subscribing to the project chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectAnchorStmt {
    /// Project ULID.
    pub project_id: String,
    /// `key_id` of the signing project key.
    pub project_key_id: String,
    /// Serial of the certificate the project holds.
    pub cert_serial: u64,
    /// Accepted-statement counter; the user daemon requires last + 1.
    pub seq: u64,
    /// Chain id within the project (0 for the main chain).
    pub chain_id: u64,
    /// Chain head hash, hex (64 chars).
    pub head_hash: String,
    /// Chain head sequence number.
    pub head_seq: u64,
    /// Effective rule hash at that head, hex (64 chars).
    pub rule_hash: String,
    /// RFC 3339 UTC; the user daemon refuses more than 5 minutes ahead.
    pub at: String,
    /// [`ProjectAnchorStmt::hash`] of the previous accepted statement.
    pub prev_anchor: Option<String>,
    /// Ed25519 signature by the project key, hex (128 chars).
    #[serde(default)]
    pub sig: String,
}

impl ProjectAnchorStmt {
    fn body(&self) -> Value {
        json!({
            "project_id": self.project_id,
            "project_key_id": self.project_key_id,
            "cert_serial": self.cert_serial,
            "seq": self.seq,
            "chain_id": self.chain_id,
            "head_hash": self.head_hash,
            "head_seq": self.head_seq,
            "rule_hash": self.rule_hash,
            "at": self.at,
            "prev_anchor": self.prev_anchor,
        })
    }

    /// The bytes the project key signs: domain tag plus canonical JSON.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        domain_bytes(ANCHOR_DOMAIN, &self.body())
    }

    /// Fill `sig` (and `project_key_id`) with `project_key`.
    pub fn sign(mut self, project_key: &SigningKey) -> Self {
        self.project_key_id = key_id(&project_key.verifying_key().to_bytes());
        self.sig = hex_encode(&project_key.sign(&self.canonical_bytes()).to_bytes());
        self
    }

    /// Verify the signature and that `project_key_id` names `project_pubkey`.
    /// Sequence, time and chain-link checks belong to the user daemon.
    pub fn verify(&self, project_pubkey: &[u8; 32]) -> Result<(), CertError> {
        if self.project_key_id != key_id(project_pubkey) {
            return Err(CertError::KeyIdMismatch("project_key_id"));
        }
        parse_ts(&self.at, "at")?;
        check_sig(project_pubkey, &self.sig, ANCHOR_DOMAIN, &self.body())
    }

    /// Statement hash used as the next statement's `prev_anchor`: SHA-256 of
    /// the anchor domain tag plus canonical JSON including `sig`, as hex.
    pub fn hash(&self) -> String {
        let mut body = self.body();
        body["sig"] = Value::String(self.sig.clone());
        hex_encode(&Sha256::digest(domain_bytes(ANCHOR_DOMAIN, &body)))
    }
}

/// Bytes the project key signs to prove possession during `mesh.register`:
/// `"weftos-mesh-local-pop-v1\n<nonce>\n<project_id>"`.
pub fn pop_signed_bytes(nonce: &str, project_id: &str) -> Vec<u8> {
    format!("{POP_DOMAIN}{nonce}\n{project_id}").into_bytes()
}
