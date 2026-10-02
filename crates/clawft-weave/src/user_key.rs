//! The user key (`~/.weftos/user.key`, ADR-103 decision D-5).
//!
//! One identity: `user.key` carries the same 32-byte seed as the migrated
//! `~/.weftos/chain/chain.key`, so the user id is unchanged. The chain loader
//! prefers `user.key` and falls back to `chain.key`
//! ([`clawft_types::config::chain_paths::chain_key_for_checkpoint`]); this
//! module resolves the same key for the mesh client, migrates one into the
//! other (`weaver migrate user-key`) and reports both for `weaver doctor`.
//!
//! Nothing here deletes or rewrites `chain.key`.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use clawft_rpc::doctor::{Component, Finding, Severity};
use clawft_types::config::chain_paths::user_key_path;
use clawft_types::runtime_paths::user_chain_checkpoint;
use ed25519_dalek::SigningKey;
use rand::RngCore;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Receipt written beside `user.key` by the migration.
pub const MIGRATED_FROM_FILE: &str = "user.key.MIGRATED_FROM.json";

/// Failures resolving or migrating the user key.
#[derive(Debug, thiserror::Error)]
pub enum UserKeyError {
    /// Filesystem failure.
    #[error("{path}: {source}")]
    Io {
        /// File involved.
        path: String,
        /// Underlying error.
        #[source]
        source: io::Error,
    },
    /// A key file is not 32 bytes.
    #[error("{path} is malformed: expected 32 bytes, got {got}")]
    Malformed {
        /// File involved.
        path: String,
        /// Bytes found.
        got: usize,
    },
    /// A key file is a symlink or not a regular file.
    #[error("{path} is not a regular file; refusing to use it as a key")]
    NotRegular {
        /// File involved.
        path: String,
    },
    /// A key file is readable beyond its owner.
    #[error("{path} is mode {mode:o}, readable beyond its owner; refusing to use it (chmod 600 {path})")]
    LooseMode {
        /// File involved.
        path: String,
        /// Its permission bits.
        mode: u32,
    },
    /// A legacy chain exists but has not been migrated: a fresh key would split the identity.
    #[error("a legacy chain exists at {path} and has not been migrated; run `weaver migrate user-chain` first (a freshly generated user.key would split your identity)")]
    LegacyChainPending {
        /// The legacy chain checkpoint.
        path: String,
    },
    /// Nothing to migrate from.
    #[error("no chain.key at {path}: nothing to migrate (run `weaver migrate user-chain` first if the chain still lives in ~/.clawft)")]
    NoSource {
        /// Expected source.
        path: String,
    },
    /// `user.key` already holds a different key.
    #[error("{path} already exists with a different key than chain.key; refusing to overwrite (a split identity: fix by hand, doctor reports both)")]
    Diverged {
        /// The existing `user.key`.
        path: String,
    },
    /// The copy did not produce the same public key.
    #[error("migration verification failed: the public keys differ after the copy")]
    VerifyFailed,
}

fn io_err(path: &Path) -> impl Fn(io::Error) -> UserKeyError + '_ {
    move |source| UserKeyError::Io { path: path.display().to_string(), source }
}

/// Where the user's seed came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeySource {
    /// `~/.weftos/user.key`.
    UserKey(PathBuf),
    /// `~/.weftos/chain/chain.key` (not yet migrated).
    ChainKey(PathBuf),
    /// Newly generated `~/.weftos/user.key` (nothing existed).
    Generated(PathBuf),
}

/// `<home>/.weftos/chain/chain.key`, the migrated user chain's key.
pub fn chain_key_path(home: &Path) -> PathBuf {
    user_chain_checkpoint(home).with_extension("key")
}

/// Read a 32-byte seed, refusing symlinks, non-regular files and keys
/// readable beyond their owner (refused, not repaired: a key that was exposed
/// is not made safe by a chmod).
pub fn read_seed(path: &Path) -> Result<[u8; 32], UserKeyError> {
    let meta = std::fs::symlink_metadata(path).map_err(io_err(path))?;
    if !meta.file_type().is_file() {
        return Err(UserKeyError::NotRegular { path: path.display().to_string() });
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            return Err(UserKeyError::LooseMode { path: path.display().to_string(), mode });
        }
    }
    let bytes = std::fs::read(path).map_err(io_err(path))?;
    <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| UserKeyError::Malformed {
        path: path.display().to_string(),
        got: bytes.len(),
    })
}

fn private_options() -> std::fs::OpenOptions {
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    o
}

/// Publish `seed` at `path` (0600) atomically: private temp file, fsync, then
/// a hard link that fails if `path` exists. Falls back to `create_new` on
/// filesystems without hard links. Returns `AlreadyExists` on a lost race.
fn publish_seed(path: &Path, seed: &[u8; 32]) -> io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".user.key.{}.{:016x}.tmp", std::process::id(), OsRng.next_u64()));
    let mut f = private_options().open(&tmp)?;
    f.write_all(seed)?;
    f.sync_all()?;
    drop(f);
    match std::fs::hard_link(&tmp, path) {
        Ok(()) => {
            let _ = std::fs::remove_file(&tmp);
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
        // No hard links here (EPERM, ENOTSUP, FAT, some FUSE mounts): rename
        // the complete, synced temp file into place, so a reader never sees a
        // partial key. The existence check keeps a racing creator's key.
        Err(_) => {
            if std::fs::symlink_metadata(path).is_ok() {
                let _ = std::fs::remove_file(&tmp);
                return Err(io::Error::from(io::ErrorKind::AlreadyExists));
            }
            std::fs::rename(&tmp, path).inspect_err(|_| {
                let _ = std::fs::remove_file(&tmp);
            })
        }
    }
}

fn pubkey_of(seed: &[u8; 32]) -> [u8; 32] {
    SigningKey::from_bytes(seed).verifying_key().to_bytes()
}

/// The user's signing key: `user.key`, else the migrated `chain.key`, else
/// (when `create`) a fresh `user.key`. Never touches `chain.key`.
pub fn resolve_user_key(home: &Path, create: bool) -> Result<(SigningKey, KeySource), UserKeyError> {
    let user = user_key_path(home);
    if std::fs::symlink_metadata(&user).is_ok() {
        let seed = read_seed(&user)?;
        return Ok((SigningKey::from_bytes(&seed), KeySource::UserKey(user)));
    }
    let chain = chain_key_path(home);
    if std::fs::symlink_metadata(&chain).is_ok() {
        let seed = read_seed(&chain)?;
        return Ok((SigningKey::from_bytes(&seed), KeySource::ChainKey(chain)));
    }
    if !create {
        return Err(UserKeyError::NoSource { path: chain.display().to_string() });
    }
    let legacy = home.join(".clawft").join(clawft_types::runtime_paths::CHAIN_CHECKPOINT_FILE);
    if std::fs::symlink_metadata(&legacy).is_ok() {
        return Err(UserKeyError::LegacyChainPending { path: legacy.display().to_string() });
    }
    let mut seed = [0u8; 32];
    OsRng.fill_bytes(&mut seed);
    match publish_seed(&user, &seed) {
        Ok(()) => Ok((SigningKey::from_bytes(&seed), KeySource::Generated(user))),
        // Lost a race with another creator: use theirs.
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            let seed = read_seed(&user)?;
            Ok((SigningKey::from_bytes(&seed), KeySource::UserKey(user)))
        }
        Err(e) => Err(io_err(&user)(e)),
    }
}

/// `user.key.MIGRATED_FROM.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MigratedFrom {
    /// SHA-256 (hex) of the shared public key.
    pub source_sha256_of_pubkey: String,
    /// The chain key the seed was copied from.
    pub source: String,
    /// Unix seconds.
    pub at: u64,
}

/// What `weaver migrate user-key` did (or would do).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrateOutcome {
    /// Dry run: nothing written.
    WouldCopy { from: PathBuf, to: PathBuf },
    /// `user.key` created from `chain.key`.
    Copied { from: PathBuf, to: PathBuf },
    /// `user.key` already holds the same key.
    AlreadyMigrated { to: PathBuf },
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Copy the seed of `chain.key` into `user.key`. Idempotent; never modifies
/// `chain.key`; refuses to overwrite a `user.key` that holds another key.
pub fn migrate_user_key(home: &Path, dry_run: bool) -> Result<MigrateOutcome, UserKeyError> {
    let from = chain_key_path(home);
    let to = user_key_path(home);
    if std::fs::symlink_metadata(&from).is_err() {
        return Err(UserKeyError::NoSource { path: from.display().to_string() });
    }
    let seed = read_seed(&from)?;
    if std::fs::symlink_metadata(&to).is_ok() {
        let existing = read_seed(&to)?;
        return if pubkey_of(&existing) == pubkey_of(&seed) {
            Ok(MigrateOutcome::AlreadyMigrated { to })
        } else {
            Err(UserKeyError::Diverged { path: to.display().to_string() })
        };
    }
    if dry_run {
        return Ok(MigrateOutcome::WouldCopy { from, to });
    }
    match publish_seed(&to, &seed) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            // A concurrent run won: accept it only if it is the same key.
            return if pubkey_of(&read_seed(&to)?) == pubkey_of(&seed) {
                Ok(MigrateOutcome::AlreadyMigrated { to })
            } else {
                Err(UserKeyError::Diverged { path: to.display().to_string() })
            };
        }
        Err(e) => return Err(io_err(&to)(e)),
    }
    let copied = read_seed(&to)?;
    if pubkey_of(&copied) != pubkey_of(&seed) {
        return Err(UserKeyError::VerifyFailed);
    }
    let receipt = MigratedFrom {
        source_sha256_of_pubkey: crate::user_key::hex(&Sha256::digest(pubkey_of(&seed))),
        source: from.display().to_string(),
        at: now_secs(),
    };
    let receipt_path = to.with_file_name(MIGRATED_FROM_FILE);
    let json = serde_json::to_vec_pretty(&receipt).unwrap_or_default();
    std::fs::write(&receipt_path, json).map_err(io_err(&receipt_path))?;
    Ok(MigrateOutcome::Copied { from, to })
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `weaver doctor` findings for the two key files. Reads both seeds (to
/// compare public keys) and prints neither.
pub fn doctor_findings(home: &Path) -> Vec<Finding> {
    let c = Component::Runtime;
    let user = user_key_path(home);
    let chain = chain_key_path(home);
    let user_present = std::fs::symlink_metadata(&user).is_ok();
    let chain_present = std::fs::symlink_metadata(&chain).is_ok();
    let mut out = Vec::new();
    match (user_present, chain_present) {
        (false, false) => out.push(Finding::new(c, "user_key", Severity::Ok, "no user.key or chain.key yet (created on first boot)")),
        (true, false) => out.push(Finding::new(c, "user_key", Severity::Ok, format!("user.key present: {}", user.display()))),
        (false, true) => out.push(
            Finding::new(c, "user_key", Severity::Ok, format!("chain.key only: {} (user.key not migrated yet)", chain.display()))
                .remedy("weaver migrate user-key --dry-run"),
        ),
        (true, true) => match (read_seed(&user), read_seed(&chain)) {
            (Ok(u), Ok(ch)) if pubkey_of(&u) == pubkey_of(&ch) => out.push(Finding::new(
                c,
                "user_key",
                Severity::Ok,
                format!("user.key and chain.key present, same identity: {}, {}", user.display(), chain.display()),
            )),
            (Ok(_), Ok(_)) => out.push(
                Finding::new(
                    c,
                    "user_key_split",
                    Severity::Warn,
                    format!("{} and {} hold different keys (split identity): the chain signs with user.key, older events carry the chain.key identity", user.display(), chain.display()),
                )
                .remedy(format!(
                    "to keep the chain's identity: mv {} {}.bak && weaver migrate user-key   (to adopt user.key instead, keep it and start a new chain; doctor never deletes keys)",
                    user.display(),
                    user.display()
                )),
            ),
            (u, ch) => {
                let why = [u.err(), ch.err()]
                    .into_iter()
                    .flatten()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; ");
                out.push(Finding::new(c, "user_key", Severity::Warn, format!("cannot compare user.key and chain.key: {why}")));
            }
        },
    }
    out
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn write_key(path: &Path, seed: [u8; 32]) {
        std::fs::write(path, seed).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    fn setup(seed: Option<[u8; 32]>) -> (tempfile::TempDir, PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().to_path_buf();
        if let Some(s) = seed {
            let chain = chain_key_path(&home);
            std::fs::create_dir_all(chain.parent().unwrap()).unwrap();
            std::fs::write(&chain, s).unwrap();
            std::fs::set_permissions(&chain, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        (t, home)
    }

    #[test]
    fn migration_is_idempotent_with_equal_pubkeys_and_leaves_chain_key_alone() {
        let (_t, home) = setup(Some([7u8; 32]));
        let chain = chain_key_path(&home);
        let before = (std::fs::read(&chain).unwrap(), std::fs::metadata(&chain).unwrap().modified().unwrap());

        let dry = migrate_user_key(&home, true).unwrap();
        assert!(matches!(dry, MigrateOutcome::WouldCopy { .. }));
        assert!(!user_key_path(&home).exists(), "dry run must not write");

        assert!(matches!(migrate_user_key(&home, false).unwrap(), MigrateOutcome::Copied { .. }));
        let user_seed = read_seed(&user_key_path(&home)).unwrap();
        assert_eq!(pubkey_of(&user_seed), pubkey_of(&[7u8; 32]));
        let mode = std::fs::metadata(user_key_path(&home)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let receipt: MigratedFrom =
            serde_json::from_slice(&std::fs::read(user_key_path(&home).with_file_name(MIGRATED_FROM_FILE)).unwrap()).unwrap();
        assert_eq!(receipt.source_sha256_of_pubkey.len(), 64);

        // Second run: nothing changes.
        assert!(matches!(migrate_user_key(&home, false).unwrap(), MigrateOutcome::AlreadyMigrated { .. }));
        let after = (std::fs::read(&chain).unwrap(), std::fs::metadata(&chain).unwrap().modified().unwrap());
        assert_eq!(before, after, "chain.key must never be touched");
    }

    #[test]
    fn a_diverged_user_key_is_never_overwritten() {
        let (_t, home) = setup(Some([7u8; 32]));
        write_key(&user_key_path(&home), [9u8; 32]);
        let e = migrate_user_key(&home, false).unwrap_err();
        assert!(matches!(e, UserKeyError::Diverged { .. }), "{e}");
        assert_eq!(std::fs::read(user_key_path(&home)).unwrap(), vec![9u8; 32]);
    }

    #[test]
    fn missing_source_is_reported() {
        let (_t, home) = setup(None);
        assert!(matches!(migrate_user_key(&home, false), Err(UserKeyError::NoSource { .. })));
    }

    #[test]
    fn resolve_prefers_user_key_then_chain_key_then_generates() {
        let (_t, home) = setup(Some([7u8; 32]));
        let (k, src) = resolve_user_key(&home, false).unwrap();
        assert!(matches!(src, KeySource::ChainKey(_)));
        assert_eq!(k.to_bytes(), [7u8; 32]);
        write_key(&user_key_path(&home), [3u8; 32]);
        let (k, src) = resolve_user_key(&home, false).unwrap();
        assert!(matches!(src, KeySource::UserKey(_)));
        assert_eq!(k.to_bytes(), [3u8; 32]);

        let (_t2, home2) = setup(None);
        assert!(resolve_user_key(&home2, false).is_err());
        let (_, src) = resolve_user_key(&home2, true).unwrap();
        assert!(matches!(src, KeySource::Generated(_)));
        // The generated key is then found, not regenerated.
        let (k1, _) = resolve_user_key(&home2, true).unwrap();
        let (k2, _) = resolve_user_key(&home2, true).unwrap();
        assert_eq!(k1.to_bytes(), k2.to_bytes());
    }

    #[test]
    fn symlinked_key_is_refused() {
        let (_t, home) = setup(Some([7u8; 32]));
        std::os::unix::fs::symlink(chain_key_path(&home), user_key_path(&home)).unwrap();
        assert!(matches!(resolve_user_key(&home, false), Err(UserKeyError::NotRegular { .. })));
    }

    #[test]
    fn doctor_warns_when_the_two_keys_differ() {
        let (_t, home) = setup(Some([7u8; 32]));
        write_key(&user_key_path(&home), [9u8; 32]);
        let f = doctor_findings(&home);
        assert!(f.iter().any(|x| x.id == "user_key_split" && x.severity == Severity::Warn));
        write_key(&user_key_path(&home), [7u8; 32]);
        let f = doctor_findings(&home);
        assert!(f.iter().all(|x| x.severity == Severity::Ok), "{f:?}");
    }

    #[test]
    fn a_key_readable_beyond_its_owner_is_refused_not_repaired() {
        let (_t, home) = setup(Some([7u8; 32]));
        let chain = chain_key_path(&home);
        std::fs::set_permissions(&chain, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(read_seed(&chain), Err(UserKeyError::LooseMode { mode: 0o644, .. })));
        assert!(matches!(migrate_user_key(&home, false), Err(UserKeyError::LooseMode { .. })));
        let mode = std::fs::metadata(&chain).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644, "not chmodded");
    }

    #[test]
    fn a_pending_legacy_chain_blocks_generating_a_fresh_key() {
        let (_t, home) = setup(None);
        let legacy = home.join(".clawft");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join(clawft_types::runtime_paths::CHAIN_CHECKPOINT_FILE), b"{}").unwrap();
        let e = resolve_user_key(&home, true).err().unwrap();
        assert!(matches!(e, UserKeyError::LegacyChainPending { .. }), "{e}");
        assert!(e.to_string().contains("weaver migrate user-chain"));
        assert!(!user_key_path(&home).exists());
    }
}
