//! Project identity: the project key, its certificate and the revocation
//! view (ADR-103 A6/A7, Phase 2 package C).
//!
//! One project key (`<root>/.weftos/project.key`) is the node key, the
//! chain signing key, the anchor key and the proof-of-possession key. It
//! therefore never signs bare bytes: every signature it makes carries a
//! distinct domain tag defined next to the statement it signs
//! ([`clawft_types::project::cert`]): `weftos-project-anchor-v1` for anchors
//! and `weftos-mesh-local-pop-v2` for proof of possession (this module's
//! [`pop_sign`], which also signs the operation and the user key id). The user key signs certificates under
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
//! # Revocation and crash safety
//!
//! [`RevocationView::build`] merges three sources: the user chain's
//! `project.register|rekey|revoke` events (source [`SOURCE`], reserved:
//! see [`is_reserved_source`]), the append-only identity journal
//! ([`IdentityJournal`]), and the certificate files. The chain is saved only
//! on clean shutdown, so the journal (fsynced before every chain append) is
//! what keeps bindings and revocations across a crash. Every certificate is
//! re-verified against the user key while folding. The merge is a union:
//! the bound key of a project is its highest-serial certificate whose key is
//! not revoked, so source order cannot resurrect a revoked key.

use std::io::{Read as _, Write as _};
use std::path::Path;

use chrono::{DateTime, Utc};
use clawft_types::project::canon::{hex_decode, hex_encode};
use clawft_types::project::cert::{PopOp, ProjectCert, pop_signed_bytes};
use clawft_types::project::{CertError, CertRequest, validate_id};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};

#[path = "project_identity_journal.rs"]
mod journal;
pub use journal::{IdentityJournal, JOURNAL_FILE, JournalLock, JournalRecord};
#[path = "project_identity_view.rs"]
mod view;
pub use view::{Registration, RevocationView};

/// Chain event source of the project identity events (user chain).
pub const SOURCE: &str = "user.projects";
/// A project key was certified.
pub const KIND_REGISTER: &str = "project.register";
/// A project key was replaced.
pub const KIND_REKEY: &str = "project.rekey";
/// A project key was revoked.
pub const KIND_REVOKE: &str = "project.revoke";

/// Chain sources only the daemon's own code may append under. A caller
/// that chooses its own source (the `chain.append` RPC) must be refused
/// these: a forged `user.projects` event would otherwise feed the view.
pub const RESERVED_SOURCES: &[&str] = &[SOURCE];

/// Is `source` reserved for the daemon's own identity events?
pub fn is_reserved_source(source: &str) -> bool {
    RESERVED_SOURCES.contains(&source.trim())
}

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
    /// The key id is already used by another project (bound or revoked).
    #[error("key {key_id} belongs to project {other_project}; a key cannot certify two projects")]
    KeyReuse {
        /// The reused key id.
        key_id: String,
        /// The project that owns it.
        other_project: String,
    },
    /// The identity journal exists but cannot be trusted.
    #[error("identity journal {path}: {why}; refusing to issue or verify until it is repaired")]
    JournalCorrupt {
        /// Journal path.
        path: String,
        /// What is wrong.
        why: String,
    },
    /// An exclusive create found the file already there.
    #[error("{0} already exists")]
    Exists(String),
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
/// was readable by anyone may already be copied. Refused: a symlink
/// (opened `O_NOFOLLOW`), a non-regular file, a file not owned by this
/// user, group/world access to the file, a group/world-writable parent
/// directory, a wrong size. The checks run on the open descriptor, not on
/// the path, and a lost creation race re-loads the winner's key.
pub fn load_or_create_project_key(path: &Path) -> Result<SigningKey, IdentityError> {
    for _ in 0..3 {
        if let Some(k) = read_key_file(path)? {
            return Ok(k);
        }
        let key = SigningKey::generate(&mut rand::rngs::OsRng);
        match write_private_atomic(path, &key.to_bytes(), true) {
            Ok(()) => {
                check_parent(path)?;
                return Ok(key);
            }
            Err(IdentityError::Exists(_)) => {}
            Err(e) => return Err(e),
        }
    }
    Err(key_file_err(path, "kept appearing and disappearing during load"))
}

fn check_parent(path: &Path) -> Result<(), IdentityError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let dir = match path.parent() {
            Some(d) if !d.as_os_str().is_empty() => d,
            _ => Path::new("."),
        };
        let mode = std::fs::metadata(dir)?.permissions().mode();
        if mode & 0o022 != 0 {
            return Err(key_file_err(
                path,
                format!("parent directory mode {:04o} is group/world writable", mode & 0o7777),
            ));
        }
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn read_key_file(path: &Path) -> Result<Option<SigningKey>, IdentityError> {
    if path.parent().is_some_and(|d| d.exists()) {
        check_parent(path)?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    let mut f = match opts.open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        #[cfg(unix)]
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => {
            return Err(key_file_err(path, "is a symlink"));
        }
        Err(e) => return Err(e.into()),
    };
    let meta = f.metadata()?;
    if !meta.is_file() {
        return Err(key_file_err(path, "is not a regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let mode = meta.permissions().mode();
        if mode & 0o077 != 0 {
            return Err(key_file_err(
                path,
                format!("mode {:04o} is group/world accessible (need 0600)", mode & 0o7777),
            ));
        }
        // SAFETY: geteuid has no preconditions.
        if meta.uid() != unsafe { libc::geteuid() } {
            return Err(key_file_err(path, "is not owned by this user"));
        }
    }
    if meta.len() != 32 {
        return Err(key_file_err(path, format!("is {} bytes, expected 32", meta.len())));
    }
    let mut seed = [0u8; 32];
    f.read_exact(&mut seed)?;
    Ok(Some(SigningKey::from_bytes(&seed)))
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
            IdentityError::Exists(path.display().to_string())
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

/// Check only that `cert` is well-formed and signed by `trusted_user_pubkey`,
/// ignoring expiry and the future-skew rule (judged at its own issue time).
/// Used when folding history: an expired certificate still occupies its id.
pub fn verify_signature_only(
    cert: &ProjectCert,
    trusted_user_pubkey: &[u8; 32],
) -> Result<(), CertError> {
    let at = DateTime::parse_from_rfc3339(&cert.issued_at)
        .map_err(|_| CertError::BadTimestamp("issued_at"))?
        .with_timezone(&Utc);
    cert.verify(trusted_user_pubkey, at)
}

/// Prove possession of the project key: sign
/// `weftos-mesh-local-pop-v2\n<op>\n<user_key_id>\n<nonce>\n<project_id>`.
/// The parts are validated first; the nonce must be one the user daemon
/// issued (single use), which this function cannot check.
pub fn pop_sign(
    project_key: &SigningKey,
    op: PopOp,
    user_key_id: &str,
    nonce: &str,
    project_id: &str,
) -> Result<[u8; 64], CertError> {
    let bytes = pop_signed_bytes(op, user_key_id, nonce, project_id)?;
    Ok(project_key.sign(&bytes).to_bytes())
}

/// Verify a proof of possession by `project_pubkey`.
pub fn pop_verify(
    project_pubkey: &[u8; 32],
    op: PopOp,
    user_key_id: &str,
    nonce: &str,
    project_id: &str,
    sig: &[u8; 64],
) -> Result<(), CertError> {
    let bytes = pop_signed_bytes(op, user_key_id, nonce, project_id)?;
    VerifyingKey::from_bytes(project_pubkey)
        .map_err(|_| CertError::BadHex("pubkey"))?
        .verify_strict(&bytes, &Signature::from_bytes(sig))
        .map_err(|_| CertError::BadSignature)
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
