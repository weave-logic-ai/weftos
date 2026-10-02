//! Project identity: the project key, its certificate and the revocation
//! view (ADR-103 A6/A7, Phase 2 package C).
//!
//! One project key (`<root>/.weftos/project.key`) is the node key, the
//! chain signing key, the anchor key and the proof-of-possession key. It
//! therefore never signs bare bytes: every signature it makes carries a
//! distinct domain tag defined next to the statement it signs
//! ([`clawft_types::project::cert`]): `weftos-project-anchor-v1` for anchors
//! and `weftos-mesh-local-pop-v1` for proof of possession (this module's
//! [`pop_sign`]). The user key signs certificates under
//! `weftos-project-cert-v1`. Chain events keep their own canonical form.
//!
//! # Trust on first use (TOFU), honestly
//!
//! The first key presented for a project id is certified; a different key
//! for the same id is refused until an explicit Admin `project.rekey`.
//! Nothing here can tell whether the first registrant really is the
//! project: the only guards for first registration are the one-shot spawn
//! nonce the user daemon handed to the child it started and the binding to
//! the project's manifest (the owner's registration act). A same-uid
//! process that learns the nonce first wins the id. Phase 3 peer
//! credentials narrow this; they do not remove it.
//!
//! # Revocation
//!
//! [`RevocationView`] is folded from `project.register|rekey|revoke` events
//! on the user chain (source [`SOURCE`]). The chain is only saved on clean
//! shutdown, so the RPC layer also keeps a revocations-only journal and
//! merges it with [`RevocationView::add_revoked`]; a journal can only add
//! revocations, never undo one (fail closed).

use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::path::Path;

use chrono::{DateTime, Utc};
use clawft_types::project::canon::{hex_decode, hex_encode};
use clawft_types::project::cert::{ProjectCert, key_id, pop_signed_bytes};
use clawft_types::project::{CertError, CertRequest, validate_id};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};

use crate::chain::ChainEvent;

/// Chain event source of the project identity events (user chain).
pub const SOURCE: &str = "user.projects";
/// A project key was certified.
pub const KIND_REGISTER: &str = "project.register";
/// A project key was replaced.
pub const KIND_REKEY: &str = "project.rekey";
/// A project key was revoked.
pub const KIND_REVOKE: &str = "project.revoke";

/// Why an identity operation was refused.
#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    /// Certificate or proof-of-possession check failed.
    #[error(transparent)]
    Cert(#[from] CertError),
    /// The key file is unsafe or malformed.
    #[error("project key file {path}: {why}")]
    KeyFile {
        /// The key path.
        path: String,
        /// What is wrong with it.
        why: String,
    },
    /// A different key is already certified for this project id.
    #[error("project {project_id} already has key {bound}; use project.rekey to replace it")]
    KeyConflict {
        /// Project id.
        project_id: String,
        /// The `key_id` currently bound.
        bound: String,
    },
    /// The key id was revoked and may not be certified again.
    #[error("key {key_id} of project {project_id} was revoked")]
    KeyRevoked {
        /// Project id.
        project_id: String,
        /// The revoked `key_id`.
        key_id: String,
    },
    /// The operation needs a certified key and there is none.
    #[error("project {0} has no certified key")]
    NotBound(String),
    /// `expires_at` is not after `issued_at`.
    #[error("expires_at must be after issued_at")]
    BadLifetime,
    /// Filesystem failure.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

fn key_file_err(path: &Path, why: impl Into<String>) -> IdentityError {
    IdentityError::KeyFile {
        path: path.display().to_string(),
        why: why.into(),
    }
}

/// Load the project key at `path`, or create it (mode 0600, raw 32-byte
/// seed: the format of `ChainManager::load_or_create_key`).
///
/// Unlike that function this one never "fixes" a loose file: a key that
/// was readable by anyone may already be copied, so a group/world-readable
/// file, a symlink, a non-regular file or a wrong-sized file is refused.
pub fn load_or_create_project_key(path: &Path) -> Result<SigningKey, IdentityError> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                return Err(key_file_err(path, "is a symlink"));
            }
            if !meta.is_file() {
                return Err(key_file_err(path, "is not a regular file"));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = meta.permissions().mode();
                if mode & 0o077 != 0 {
                    return Err(key_file_err(
                        path,
                        format!("mode {:04o} is group/world accessible (need 0600)", mode & 0o7777),
                    ));
                }
            }
            let bytes = std::fs::read(path)?;
            let seed: [u8; 32] = bytes
                .try_into()
                .map_err(|b: Vec<u8>| key_file_err(path, format!("is {} bytes, expected 32", b.len())))?;
            Ok(SigningKey::from_bytes(&seed))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let key = SigningKey::generate(&mut rand::rngs::OsRng);
            write_private_atomic(path, &key.to_bytes(), true)?;
            Ok(key)
        }
        Err(e) => Err(e.into()),
    }
}

/// Write `bytes` to `path` via a temp file in the same directory and an
/// atomic rename, mode 0600 from creation (never briefly wider).
///
/// With `exclusive` the final step is a hard link that fails when `path`
/// exists (two creators cannot both win); otherwise the rename replaces.
pub fn write_private_atomic(path: &Path, bytes: &[u8], exclusive: bool) -> Result<(), IdentityError> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let dir = path.parent().ok_or_else(|| key_file_err(path, "has no parent directory"))?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(
        ".{}.{}.{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let result = (|| -> std::io::Result<()> {
        let mut f = opts.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        if exclusive {
            std::fs::hard_link(&tmp, path)
        } else {
            std::fs::rename(&tmp, path)
        }
    })();
    if exclusive || result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            key_file_err(path, "already exists")
        } else {
            e.into()
        }
    })
}

/// Certify `req` with the user key. Deterministic. Refuses an invalid
/// project id and an expiry that is not after the issue time.
pub fn sign_cert(user_key: &SigningKey, req: &CertRequest) -> Result<ProjectCert, IdentityError> {
    validate_id(&req.project_id).map_err(|_| CertError::BadProjectId)?;
    if req.expires_at.is_some_and(|e| e <= req.issued_at) {
        return Err(IdentityError::BadLifetime);
    }
    Ok(ProjectCert::sign(user_key, req))
}

/// Verify `cert` against the user key the caller trusts, at the current
/// time: signature, both key ids recomputed from their public keys,
/// issuer, `expires_at`. Revocation is a separate check
/// ([`RevocationView::check_cert`]).
pub fn verify_cert(cert: &ProjectCert, trusted_user_pubkey: &[u8; 32]) -> Result<(), CertError> {
    verify_cert_at(cert, trusted_user_pubkey, Utc::now())
}

/// [`verify_cert`] against an explicit `now`.
pub fn verify_cert_at(
    cert: &ProjectCert,
    trusted_user_pubkey: &[u8; 32],
    now: DateTime<Utc>,
) -> Result<(), CertError> {
    cert.verify(trusted_user_pubkey, now)
}

/// Prove possession of the project key: sign
/// `weftos-mesh-local-pop-v1\n<nonce>\n<project_id>`. The nonce and id are
/// validated first; the caller owns nonce freshness (single use).
pub fn pop_sign(
    project_key: &SigningKey,
    nonce: &str,
    project_id: &str,
) -> Result<[u8; 64], CertError> {
    Ok(project_key.sign(&pop_signed_bytes(nonce, project_id)?).to_bytes())
}

/// Verify a proof of possession by `project_pubkey`.
pub fn pop_verify(
    project_pubkey: &[u8; 32],
    nonce: &str,
    project_id: &str,
    sig: &[u8; 64],
) -> Result<(), CertError> {
    let bytes = pop_signed_bytes(nonce, project_id)?;
    VerifyingKey::from_bytes(project_pubkey)
        .map_err(|_| CertError::BadHex("pubkey"))?
        .verify_strict(&bytes, &Signature::from_bytes(sig))
        .map_err(|_| CertError::BadSignature)
}

/// Outcome of [`RevocationView::plan_registration`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Registration {
    /// This key is already certified; reuse the certificate (no new event).
    Existing(Box<ProjectCert>),
    /// First key for the id (or the first after a revoke): certify with
    /// this serial.
    New {
        /// Serial to issue.
        serial: u64,
    },
}

#[derive(Debug, Default, Clone)]
struct ProjectIdentity {
    cert: Option<ProjectCert>,
    bound_key_id: Option<String>,
    serial: u64,
    revoked: HashSet<String>,
}

/// Which keys of which projects are certified or revoked, folded from the
/// user chain's `project.register|rekey|revoke` events.
#[derive(Debug, Default, Clone)]
pub struct RevocationView {
    projects: HashMap<String, ProjectIdentity>,
}

impl RevocationView {
    /// Fold `events` (only [`SOURCE`] events count, in order).
    pub fn from_events(events: &[ChainEvent]) -> Self {
        let mut v = Self::default();
        for e in events {
            v.apply(e);
        }
        v
    }

    /// Apply one event; unrelated or malformed events are ignored.
    pub fn apply(&mut self, ev: &ChainEvent) {
        if ev.source != SOURCE {
            return;
        }
        let Some(p) = ev.payload.as_ref() else { return };
        let cert_of = |field: &str| {
            p.get(field)
                .and_then(|c| serde_json::from_value::<ProjectCert>(c.clone()).ok())
        };
        match ev.kind.as_str() {
            KIND_REGISTER => {
                if let Some(c) = cert_of("cert") {
                    self.bind(c);
                }
            }
            KIND_REKEY => {
                let (Some(id), Some(old)) = (
                    p.get("project_id").and_then(|v| v.as_str()),
                    p.get("old_key_id").and_then(|v| v.as_str()),
                ) else {
                    return;
                };
                self.add_revoked(id, old);
                if let Some(c) = cert_of("new_cert") {
                    self.bind(c);
                }
            }
            KIND_REVOKE => {
                if let (Some(id), Some(old)) = (
                    p.get("project_id").and_then(|v| v.as_str()),
                    p.get("old_key_id").and_then(|v| v.as_str()),
                ) {
                    self.add_revoked(id, old);
                }
            }
            _ => {}
        }
    }

    fn bind(&mut self, c: ProjectCert) {
        let st = self.projects.entry(c.project_id.clone()).or_default();
        st.serial = st.serial.max(c.serial);
        st.bound_key_id = Some(c.project_key_id.clone());
        st.cert = Some(c);
    }

    /// Record `key_id` of `project_id` as revoked (also unbinds it when it
    /// is the current key). Idempotent.
    pub fn add_revoked(&mut self, project_id: &str, key_id: &str) {
        let st = self.projects.entry(project_id.to_owned()).or_default();
        st.revoked.insert(key_id.to_owned());
        if st.bound_key_id.as_deref() == Some(key_id) {
            st.bound_key_id = None;
            st.cert = None;
        }
    }

    /// Is this key revoked for this project?
    pub fn is_revoked(&self, project_id: &str, key_id: &str) -> bool {
        self.projects
            .get(project_id)
            .is_some_and(|s| s.revoked.contains(key_id))
    }

    /// The `key_id` currently certified for `project_id`.
    pub fn bound_key_id(&self, project_id: &str) -> Option<&str> {
        self.projects.get(project_id)?.bound_key_id.as_deref()
    }

    /// The certificate currently in force for `project_id`.
    pub fn current_cert(&self, project_id: &str) -> Option<&ProjectCert> {
        self.projects.get(project_id)?.cert.as_ref()
    }

    /// Highest serial ever issued for `project_id` (0 when none).
    pub fn last_serial(&self, project_id: &str) -> u64 {
        self.projects.get(project_id).map_or(0, |s| s.serial)
    }

    /// Refuse a certificate whose key was revoked or replaced.
    pub fn check_cert(&self, cert: &ProjectCert) -> Result<(), IdentityError> {
        let id = &cert.project_id;
        if self.is_revoked(id, &cert.project_key_id) {
            return Err(IdentityError::KeyRevoked {
                project_id: id.clone(),
                key_id: cert.project_key_id.clone(),
            });
        }
        match self.bound_key_id(id) {
            Some(b) if b != cert.project_key_id => Err(IdentityError::KeyConflict {
                project_id: id.clone(),
                bound: b.to_owned(),
            }),
            _ => Ok(()),
        }
    }

    /// TOFU decision for `project.register` of `project_pubkey`.
    pub fn plan_registration(
        &self,
        project_id: &str,
        project_pubkey: &[u8; 32],
    ) -> Result<Registration, IdentityError> {
        let kid = key_id(project_pubkey);
        if self.is_revoked(project_id, &kid) {
            return Err(IdentityError::KeyRevoked {
                project_id: project_id.to_owned(),
                key_id: kid,
            });
        }
        match (self.bound_key_id(project_id), self.current_cert(project_id)) {
            (Some(b), Some(c)) if b == kid => Ok(Registration::Existing(Box::new(c.clone()))),
            (Some(b), _) => Err(IdentityError::KeyConflict {
                project_id: project_id.to_owned(),
                bound: b.to_owned(),
            }),
            (None, _) => Ok(Registration::New {
                serial: self.last_serial(project_id) + 1,
            }),
        }
    }

    /// Rekey decision: the project must have a certified key, and the new
    /// one must differ from it and never have been revoked. Returns the
    /// old `key_id` and the next serial.
    pub fn plan_rekey(
        &self,
        project_id: &str,
        new_pubkey: &[u8; 32],
    ) -> Result<(String, u64), IdentityError> {
        let old = self
            .bound_key_id(project_id)
            .ok_or_else(|| IdentityError::NotBound(project_id.to_owned()))?
            .to_owned();
        let new = key_id(new_pubkey);
        if new == old {
            return Err(IdentityError::KeyConflict {
                project_id: project_id.to_owned(),
                bound: old,
            });
        }
        if self.is_revoked(project_id, &new) {
            return Err(IdentityError::KeyRevoked {
                project_id: project_id.to_owned(),
                key_id: new,
            });
        }
        Ok((old, self.last_serial(project_id) + 1))
    }
}

/// Decode a 32-byte lowercase-hex public key.
pub fn parse_pubkey(hex: &str) -> Option<[u8; 32]> {
    hex_decode::<32>(hex)
}

/// Lowercase hex of `bytes` (re-export for callers building payloads).
pub fn hex(bytes: &[u8]) -> String {
    hex_encode(bytes)
}

#[cfg(test)]
#[path = "project_identity_tests.rs"]
mod tests;
