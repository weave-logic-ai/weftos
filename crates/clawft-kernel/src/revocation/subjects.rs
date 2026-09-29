//! Revocation of package ids, signer keys and artifact hashes (ADR-099
//! section 7).
//!
//! Extends [`RevocationList`] beyond host bans. Subjects persist in a
//! sibling file, `revoked_subjects.json`, next to the host ban file, so
//! the existing host file format is unchanged.
//!
//! Failure handling is fail-closed: if the subjects file exists but cannot
//! be read or parsed, the list is marked *poisoned*. A poisoned list refuses
//! further writes (so the unreadable evidence is never overwritten) and
//! [`RevocationList::subjects_error`] reports it, which the workload gate
//! turns into a deny for every request.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use super::{RevocationInner, RevocationList};

/// File name of the subject revocation list, next to the host ban file.
pub const SUBJECTS_FILE_NAME: &str = "revoked_subjects.json";
/// Maximum package id length.
pub const MAX_PACKAGE_ID_LEN: usize = 128;

/// What a subject revocation targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevocationKind {
    /// A package id (e.g. a cog id or model package id).
    Package,
    /// An Ed25519 signer public key, 64 hex chars.
    SignerKey,
    /// A BLAKE3 artifact hash, 64 hex chars.
    ArtifactHash,
}

impl RevocationKind {
    /// Validate `id` for this kind and return its canonical form.
    ///
    /// Hex ids are lowercased so `AB..` and `ab..` revoke the same key.
    pub fn normalize(self, id: &str) -> Result<String, RevocationError> {
        match self {
            RevocationKind::Package => {
                let ok_len = !id.is_empty() && id.len() <= MAX_PACKAGE_ID_LEN;
                let ok_first = id.chars().next().is_some_and(|c| c.is_ascii_alphanumeric());
                let ok_chars = id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '@' | '/' | ':' | '+'));
                if ok_len && ok_first && ok_chars {
                    Ok(id.to_owned())
                } else {
                    Err(RevocationError::InvalidId {
                        kind: self,
                        reason: format!(
                            "package id must be 1..={MAX_PACKAGE_ID_LEN} chars of [A-Za-z0-9._-@/:+] starting alphanumeric"
                        ),
                    })
                }
            }
            RevocationKind::SignerKey | RevocationKind::ArtifactHash => {
                if id.len() == 64 && id.chars().all(|c| c.is_ascii_hexdigit()) {
                    Ok(id.to_ascii_lowercase())
                } else {
                    Err(RevocationError::InvalidId {
                        kind: self,
                        reason: "must be exactly 64 hex characters".into(),
                    })
                }
            }
        }
    }
}

impl std::fmt::Display for RevocationKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RevocationKind::Package => write!(f, "package"),
            RevocationKind::SignerKey => write!(f, "signer_key"),
            RevocationKind::ArtifactHash => write!(f, "artifact_hash"),
        }
    }
}

/// One revoked subject.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevokedSubject {
    /// What kind of subject.
    pub kind: RevocationKind,
    /// Canonical id (see [`RevocationKind::normalize`]).
    pub id: String,
    /// Unix timestamp (seconds) of the revocation.
    pub revoked_at: u64,
    /// Human-readable reason.
    pub reason: String,
}

/// Errors from subject revocation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RevocationError {
    /// The id failed validation.
    #[error("invalid {kind} id: {reason}")]
    InvalidId {
        /// Subject kind.
        kind: RevocationKind,
        /// Why it was rejected.
        reason: String,
    },
    /// The subjects file was unreadable at load; writes are refused.
    #[error("subject revocation list is poisoned: {0}")]
    Poisoned(String),
    /// Persisting to disk failed. The in-memory entry is kept (fail-closed).
    #[error("failed to persist subject revocations: {0}")]
    Persist(String),
}

/// Subject state held inside [`RevocationInner`].
#[derive(Debug, Default)]
pub(super) struct SubjectState {
    pub(super) subjects: Vec<RevokedSubject>,
    pub(super) path: PathBuf,
    pub(super) poisoned: Option<String>,
}

impl SubjectState {
    /// Path of the subjects file for a given host ban file path.
    pub(super) fn path_for(host_path: &Path) -> PathBuf {
        host_path.with_file_name(SUBJECTS_FILE_NAME)
    }

    /// Empty state (no load).
    pub(super) fn empty(host_path: &Path) -> Self {
        Self {
            subjects: Vec::new(),
            path: Self::path_for(host_path),
            poisoned: None,
        }
    }

    /// Load from disk; a missing file is an empty list, a bad file poisons.
    pub(super) fn load(host_path: &Path) -> Self {
        let path = Self::path_for(host_path);
        if !path.exists() {
            return Self { subjects: Vec::new(), path, poisoned: None };
        }
        let parsed = std::fs::read_to_string(&path)
            .map_err(|e| format!("read {}: {e}", path.display()))
            .and_then(|data| {
                serde_json::from_str::<Vec<RevokedSubject>>(&data)
                    .map_err(|e| format!("parse {}: {e}", path.display()))
            });
        match parsed {
            Ok(subjects) => {
                info!(count = subjects.len(), "loaded subject revocation list");
                Self { subjects, path, poisoned: None }
            }
            Err(e) => {
                warn!(error = %e, "subject revocation list unreadable; failing closed");
                Self { subjects: Vec::new(), path, poisoned: Some(e) }
            }
        }
    }

    fn save(&self) -> Result<(), RevocationError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| RevocationError::Persist(e.to_string()))?;
        }
        let json = serde_json::to_string_pretty(&self.subjects)
            .map_err(|e| RevocationError::Persist(e.to_string()))?;
        // Write-then-rename so a crash never leaves a truncated file.
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, json).map_err(|e| RevocationError::Persist(e.to_string()))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| RevocationError::Persist(e.to_string()))
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl RevocationList {
    fn with_subjects<T>(&self, f: impl FnOnce(&mut SubjectState) -> T) -> T {
        let mut inner: std::sync::MutexGuard<'_, RevocationInner> = self.inner.lock().unwrap();
        f(&mut inner.subjects)
    }

    /// Revoke a package id, signer key or artifact hash and persist it.
    ///
    /// Returns `Ok(true)` if newly added, `Ok(false)` if already revoked.
    pub fn revoke_subject(
        &self,
        kind: RevocationKind,
        id: &str,
        reason: &str,
    ) -> Result<bool, RevocationError> {
        let id = kind.normalize(id)?;
        self.with_subjects(|s| {
            if let Some(p) = &s.poisoned {
                return Err(RevocationError::Poisoned(p.clone()));
            }
            if s.subjects.iter().any(|e| e.kind == kind && e.id == id) {
                return Ok(false);
            }
            s.subjects.push(RevokedSubject {
                kind,
                id: id.clone(),
                revoked_at: now_secs(),
                reason: reason.to_owned(),
            });
            s.save()?;
            info!(%kind, id, reason, "subject revoked");
            Ok(true)
        })
    }

    /// Remove a subject revocation. Returns `Ok(true)` if it was present.
    pub fn unrevoke_subject(&self, kind: RevocationKind, id: &str) -> Result<bool, RevocationError> {
        let id = kind.normalize(id)?;
        self.with_subjects(|s| {
            if let Some(p) = &s.poisoned {
                return Err(RevocationError::Poisoned(p.clone()));
            }
            let before = s.subjects.len();
            s.subjects.retain(|e| !(e.kind == kind && e.id == id));
            if s.subjects.len() == before {
                return Ok(false);
            }
            s.save()?;
            info!(%kind, id, "subject unrevoked");
            Ok(true)
        })
    }

    /// Whether a subject is revoked. Invalid ids are never revoked.
    pub fn is_subject_revoked(&self, kind: RevocationKind, id: &str) -> bool {
        self.find_subject(kind, id).is_some()
    }

    /// Look up a revoked subject by kind and id.
    pub fn find_subject(&self, kind: RevocationKind, id: &str) -> Option<RevokedSubject> {
        let id = kind.normalize(id).ok()?;
        self.with_subjects(|s| {
            s.subjects
                .iter()
                .find(|e| e.kind == kind && e.id == id)
                .cloned()
        })
    }

    /// List revoked subjects, optionally filtered by kind.
    pub fn list_subjects(&self, kind: Option<RevocationKind>) -> Vec<RevokedSubject> {
        self.with_subjects(|s| {
            s.subjects
                .iter()
                .filter(|e| kind.is_none_or(|k| e.kind == k))
                .cloned()
                .collect()
        })
    }

    /// First revoked subject among a package id, signers and artifact hashes.
    pub fn first_revoked<'a>(
        &self,
        package_id: Option<&str>,
        signer_keys: impl IntoIterator<Item = &'a String>,
        artifact_hashes: impl IntoIterator<Item = &'a String>,
    ) -> Option<RevokedSubject> {
        package_id
            .and_then(|p| self.find_subject(RevocationKind::Package, p))
            .or_else(|| {
                signer_keys
                    .into_iter()
                    .find_map(|k| self.find_subject(RevocationKind::SignerKey, k))
            })
            .or_else(|| {
                artifact_hashes
                    .into_iter()
                    .find_map(|h| self.find_subject(RevocationKind::ArtifactHash, h))
            })
    }

    /// Load error of the subjects file, if the list is poisoned.
    pub fn subjects_error(&self) -> Option<String> {
        self.with_subjects(|s| s.poisoned.clone())
    }

    /// Path of the subjects file.
    pub fn subjects_path(&self) -> PathBuf {
        self.with_subjects(|s| s.path.clone())
    }
}
