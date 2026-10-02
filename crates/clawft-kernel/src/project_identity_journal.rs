//! The identity journal: an append-only, fsynced record of every
//! certification, rekey and revocation, beside the manifests
//! (`<manifests_dir>/identity.journal.jsonl`, mode 0600).
//!
//! The user chain is saved only on clean shutdown, so after a crash the
//! chain can be missing events the daemon already acted on (a registered
//! project would look unbound and a second key could be certified). Every
//! operation therefore appends here first, then to the chain. Fail closed:
//! a journal that exists but is unreadable, empty, or has a line that does
//! not parse is an error, never "no records"; nothing is issued or verified
//! until the owner repairs it, and [`IdentityJournal::append`] never touches
//! a journal it could not parse. Certificates in records are re-verified
//! against the user key when the view is built, so forging a `register`
//! line needs the user key; a forged `revoke` can only deny service.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use clawft_types::project::cert::ProjectCert;
use serde::{Deserialize, Serialize};

use super::{IdentityError, write_private_atomic};

/// Journal file name.
pub const JOURNAL_FILE: &str = "identity.journal.jsonl";
const LOCK_FILE: &str = "identity.lock";

/// One journal line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum JournalRecord {
    /// A key was certified.
    Register {
        /// The certificate issued.
        cert: ProjectCert,
    },
    /// A key was replaced: `old_key_id` revoked, `cert` is the new key's.
    Rekey {
        /// Revoked key id.
        old_key_id: String,
        /// The new certificate.
        cert: ProjectCert,
    },
    /// A key was revoked.
    Revoke {
        /// Project the key belonged to.
        project_id: String,
        /// Revoked key id.
        key_id: String,
    },
}

fn corrupt(path: &Path, why: impl Into<String>) -> IdentityError {
    IdentityError::JournalCorrupt {
        path: path.display().to_string(),
        why: why.into(),
    }
}

/// The journal in a manifests directory.
#[derive(Debug, Clone)]
pub struct IdentityJournal {
    dir: PathBuf,
}

/// Exclusive lock over the journal for a read-modify-write. Held until
/// dropped. `flock` locks the open file, so two threads of one process
/// exclude each other as well as two processes.
#[derive(Debug)]
pub struct JournalLock {
    _file: std::fs::File,
}

impl IdentityJournal {
    /// The journal under `manifests_dir`.
    pub fn new(manifests_dir: &Path) -> Self {
        Self {
            dir: manifests_dir.to_path_buf(),
        }
    }

    /// Journal file path.
    pub fn path(&self) -> PathBuf {
        self.dir.join(JOURNAL_FILE)
    }

    /// Take the exclusive lock (blocking).
    pub fn lock(&self) -> Result<JournalLock, IdentityError> {
        self.lock_inner(false)?.ok_or_else(|| IdentityError::Io(std::io::ErrorKind::WouldBlock.into()))
    }

    /// Take the lock if free; `None` when someone holds it.
    pub fn try_lock(&self) -> Result<Option<JournalLock>, IdentityError> {
        self.lock_inner(true)
    }

    fn lock_inner(&self, nonblocking: bool) -> Result<Option<JournalLock>, IdentityError> {
        std::fs::create_dir_all(&self.dir)?;
        let mut opts = std::fs::OpenOptions::new();
        opts.create(true).truncate(false).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let file = opts.open(self.dir.join(LOCK_FILE))?;
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let flags = if nonblocking { libc::LOCK_EX | libc::LOCK_NB } else { libc::LOCK_EX };
            loop {
                // SAFETY: flock on a valid fd owned by `file`.
                if unsafe { libc::flock(file.as_raw_fd(), flags) } == 0 {
                    break;
                }
                let err = std::io::Error::last_os_error();
                match err.kind() {
                    std::io::ErrorKind::Interrupted => {}
                    std::io::ErrorKind::WouldBlock if nonblocking => return Ok(None),
                    _ => return Err(err.into()),
                }
            }
        }
        #[cfg(not(unix))]
        let _ = nonblocking;
        Ok(Some(JournalLock { _file: file }))
    }

    /// Every record, oldest first. A missing journal is empty; one that
    /// exists but cannot be read, is empty, or has any line that does not
    /// parse is [`IdentityError::JournalCorrupt`].
    pub fn read(&self) -> Result<Vec<JournalRecord>, IdentityError> {
        let path = self.path();
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(corrupt(&path, format!("unreadable: {e}"))),
        };
        if bytes.is_empty() {
            return Err(corrupt(&path, "exists but is empty"));
        }
        let text = std::str::from_utf8(&bytes).map_err(|_| corrupt(&path, "is not UTF-8"))?;
        let body = text.strip_suffix('\n').unwrap_or(text);
        body.split('\n')
            .enumerate()
            .map(|(i, line)| {
                serde_json::from_str(line).map_err(|e| corrupt(&path, format!("line {}: {e}", i + 1)))
            })
            .collect()
    }

    /// Append `rec`, fsynced. Parses the existing journal first and refuses
    /// (leaving it untouched) if it does not parse. Call under [`Self::lock`].
    pub fn append(&self, rec: &JournalRecord) -> Result<(), IdentityError> {
        self.read()?;
        let path = self.path();
        let mut line = serde_json::to_vec(rec).map_err(|e| corrupt(&path, e.to_string()))?;
        line.push(b'\n');
        if !path.exists() {
            // First record: created whole, so an empty journal never exists.
            return write_private_atomic(&path, &line, true);
        }
        let mut opts = std::fs::OpenOptions::new();
        opts.append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.custom_flags(libc::O_NOFOLLOW);
        }
        let mut f = opts.open(&path)?;
        f.write_all(&line)?;
        f.sync_all()?;
        Ok(())
    }
}
