//! `weaver migrate user-key --rotate`: replace the user key and keep
//! verifying what the old one sealed (ADR-103 A13).
//!
//! Offline: the user daemon must be stopped (the user chain's lock is
//! probed), because it holds the old key in memory and would go on signing
//! with it. Steps, each idempotent so a crash is finished by running the
//! verb again:
//!
//! 1. the new seed is written to `user.key.next` (0600);
//! 2. a dual-signed [`RotationRecord`] is appended to the rotation log in the
//!    manifest store (`user-key-rotations.jsonl`);
//! 3. the old `user.key` is moved to `user.key.retired-<seq>` (0600; never
//!    deleted by code: delete it yourself once you are satisfied) and
//!    `user.key.next` becomes `user.key`.
//!
//! The daemon appends the record to the user chain (`user.key.rotated`) the
//! next time it builds its certificate environment. A user key that lives
//! only in the migrated `chain.key` is rotated by writing a new `user.key`,
//! which the chain loader prefers; `chain.key` stays untouched.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use clawft_kernel::project_identity::{RotationLog, RotationRecord};
use clawft_types::config::chain_paths::user_key_path;
use ed25519_dalek::SigningKey;
use rand::RngCore;
use rand::rngs::OsRng;

use crate::user_key::{UserKeyError, read_seed, resolve_user_key};

/// Suffix of the staged new key.
pub const NEXT_SUFFIX: &str = "next";

/// What a rotation did or would do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RotateOutcome {
    /// Dry run.
    WouldRotate {
        /// `key_id` of the key in use.
        old_key_id: String,
    },
    /// Rotated (or a half-finished rotation was completed).
    Rotated {
        /// The record's sequence.
        seq: u64,
        /// `key_id` retired.
        old_key_id: String,
        /// `key_id` now in use.
        new_key_id: String,
        /// Where the retired private key was kept, if it was a `user.key`.
        retired: Option<PathBuf>,
    },
}

/// Why a rotation was refused.
#[derive(Debug, thiserror::Error)]
pub enum RotateError {
    /// Key file problem.
    #[error(transparent)]
    Key(#[from] UserKeyError),
    /// The user daemon holds the chain.
    #[error("{0}; stop the user daemon first (it keeps the old key in memory)")]
    DaemonRunning(String),
    /// The rotation log refused the record.
    #[error(transparent)]
    Log(#[from] clawft_kernel::project_identity::RotationError),
    /// Filesystem failure.
    #[error("{path}: {source}")]
    Io {
        /// File involved.
        path: String,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
}

fn io(path: &Path) -> impl Fn(std::io::Error) -> RotateError + '_ {
    move |source| RotateError::Io { path: path.display().to_string(), source }
}

fn pk(seed: &[u8; 32]) -> [u8; 32] {
    SigningKey::from_bytes(seed).verifying_key().to_bytes()
}

fn write_new(path: &Path, seed: &[u8; 32]) -> Result<(), RotateError> {
    clawft_kernel::project_identity::write_private_atomic(path, seed, false)
        .map_err(|e| RotateError::Io { path: path.display().to_string(), source: std::io::Error::other(e.to_string()) })
}

/// Rotate the user key under `home`, recording the handover in
/// `manifests_dir`.
pub fn rotate_user_key(
    home: &Path,
    manifests_dir: &Path,
    dry_run: bool,
    now: DateTime<Utc>,
) -> Result<RotateOutcome, RotateError> {
    let ckpt = clawft_types::runtime_paths::user_chain_checkpoint(home);
    clawft_kernel::chain_storage::ChainLock::probe(&ckpt).map_err(RotateError::DaemonRunning)?;
    let (old, _) = resolve_user_key(home, false)?;
    let old_pk = old.verifying_key().to_bytes();
    let user = user_key_path(home);
    let next = user.with_file_name(format!("user.key.{NEXT_SUFFIX}"));
    let log = RotationLog::new(manifests_dir);
    let records = log.read()?;
    let old_id = clawft_types::project::cert::key_id(&old_pk);

    // A half-finished rotation: the log already hands over from the key in
    // use to the staged key (crash between steps 2 and 3).
    let staged = std::fs::symlink_metadata(&next).ok().map(|_| read_seed(&next)).transpose()?;
    let pending = records.last().filter(|r| {
        r.old_pubkey == clawft_types::project::canon::hex_encode(&old_pk)
            && staged.is_some_and(|s| r.new_pubkey == clawft_types::project::canon::hex_encode(&pk(&s)))
    });
    if dry_run {
        return Ok(RotateOutcome::WouldRotate { old_key_id: old_id });
    }
    let (record, new_seed) = if let (Some(r), Some(s)) = (pending, staged) {
        (r.clone(), s)
    } else {
        let mut seed = [0u8; 32];
        OsRng.fill_bytes(&mut seed);
        let new = SigningKey::from_bytes(&seed);
        // The log must already end at the key in use (or be empty).
        log.history(&old_pk)?;
        let rec = RotationRecord::sign(&old, &new, records.last(), now);
        write_new(&next, &seed)?;
        log.append(&rec)?;
        (rec, seed)
    };
    let retired = if std::fs::symlink_metadata(&user).is_ok() {
        let to = user.with_file_name(format!("user.key.retired-{}", record.seq));
        std::fs::rename(&user, &to).map_err(io(&user))?;
        Some(to)
    } else {
        None
    };
    std::fs::rename(&next, &user).map_err(io(&next))?;
    debug_assert_eq!(read_seed(&user).ok(), Some(new_seed));
    Ok(RotateOutcome::Rotated {
        seq: record.seq,
        old_key_id: record.old_key_id,
        new_key_id: record.new_key_id,
        retired,
    })
}

#[cfg(all(test, unix))]
#[path = "user_key_rotate_tests.rs"]
mod tests;
