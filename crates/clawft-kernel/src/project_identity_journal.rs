//! The identity journal: an append-only, fsynced record of every
//! certification, rekey and revocation, beside the manifests
//! (`<manifests_dir>/identity.journal.jsonl`, mode 0600).
//!
//! The user chain is saved only on clean shutdown, so after a crash the
//! chain can be missing events the daemon already acted on (a registered
//! project would look unbound and a second key could be certified). Every
//! operation therefore appends here first, then to the chain. Fail closed:
//! a journal that exists but is unreadable, empty, or has a line that does
//! not parse is an error, never "no records", and so is a journal that is
//! missing while anything shows it should exist (the lock file, a
//! certificate file, a `user.projects` chain event). Nothing is issued or
//! verified until the owner runs the repair RPC (`project.identity.repair`),
//! and [`IdentityJournal::append`] never touches a journal it could not
//! parse. Readers take a shared `flock`, writers an exclusive one. Certificates in records are re-verified
//! against the user key when the view is built, so forging a `register`
//! line needs the user key; a forged `revoke` can only deny service.

use std::io::{Read as _, Write as _};
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
    /// Written once when the journal is created; carries no state.
    Init {},
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
/// exclude each other as well as two processes. Appending needs one.
#[derive(Debug)]
pub struct JournalLock {
    _file: std::fs::File,
}

/// A shared (read) lock; held while a view is built.
#[derive(Debug)]
pub struct SharedLock {
    _file: Option<std::fs::File>,
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

    /// Does the journal file exist?
    pub fn exists(&self) -> bool {
        self.path().exists()
    }

    fn lock_path(&self) -> PathBuf {
        self.dir.join(LOCK_FILE)
    }

    /// Take the exclusive lock (blocking). On a brand-new install (no lock
    /// file and no journal yet) this also creates the journal with an
    /// `init` record, so from then on a missing journal is an error.
    pub fn lock(&self) -> Result<JournalLock, IdentityError> {
        let fresh = !self.lock_path().exists() && !self.path().exists();
        if fresh {
            // Journal first, lock file second: a crash between the two
            // leaves a journal without a lock file (harmless), never a lock
            // file without a journal.
            match write_private_atomic(&self.path(), &init_line(), true) {
                Ok(()) | Err(IdentityError::Exists(_)) => {}
                Err(e) => return Err(e),
            }
        }
        self.lock_inner(false, false)?
            .map(|f| JournalLock { _file: f })
            .ok_or_else(|| IdentityError::Io(std::io::ErrorKind::WouldBlock.into()))
    }

    /// Take the exclusive lock if free; `None` when someone holds it.
    pub fn try_lock(&self) -> Result<Option<JournalLock>, IdentityError> {
        Ok(self.lock_inner(true, false)?.map(|f| JournalLock { _file: f }))
    }

    /// Take a shared lock for reading. Never creates the lock file (a
    /// reader must not make an unused install look initialised).
    pub fn lock_shared(&self) -> Result<SharedLock, IdentityError> {
        Ok(SharedLock { _file: self.lock_inner(false, true)? })
    }

    fn lock_inner(&self, nonblocking: bool, shared: bool) -> Result<Option<std::fs::File>, IdentityError> {
        let mut opts = std::fs::OpenOptions::new();
        if shared {
            opts.read(true);
            match opts.open(self.lock_path()) {
                Ok(f) => return Self::flock(f, nonblocking, true),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(e.into()),
            }
        }
        std::fs::create_dir_all(&self.dir)?;
        opts.create(true).truncate(false).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        Self::flock(opts.open(self.lock_path())?, nonblocking, false)
    }

    #[allow(unused_variables)]
    fn flock(file: std::fs::File, nonblocking: bool, shared: bool) -> Result<Option<std::fs::File>, IdentityError> {
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let mut flags = if shared { libc::LOCK_SH } else { libc::LOCK_EX };
            if nonblocking {
                flags |= libc::LOCK_NB;
            }
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
        Ok(Some(file))
    }

    /// Raw bytes, `None` when the file does not exist. Opened `O_NOFOLLOW`.
    fn read_bytes(&self) -> Result<Option<Vec<u8>>, IdentityError> {
        let path = self.path();
        let mut opts = std::fs::OpenOptions::new();
        opts.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.custom_flags(libc::O_NOFOLLOW);
        }
        let mut f = match opts.open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(corrupt(&path, format!("unreadable: {e}"))),
        };
        let mut b = Vec::new();
        f.read_to_end(&mut b).map_err(|e| corrupt(&path, format!("unreadable: {e}")))?;
        Ok(Some(b))
    }

    /// Every record, oldest first (the `init` marker is not returned),
    /// under a shared lock. `other_evidence` says whether anything else
    /// proves the journal should exist (a certificate file or a
    /// `user.projects` chain event). See [`Self::read_locked`].
    pub fn read(&self, other_evidence: bool) -> Result<Vec<JournalRecord>, IdentityError> {
        let _g = self.lock_shared()?;
        self.parse(other_evidence)
    }

    /// [`Self::read`] for a caller already holding the exclusive lock.
    pub fn read_locked(&self, _lock: &JournalLock, other_evidence: bool) -> Result<Vec<JournalRecord>, IdentityError> {
        self.parse(other_evidence)
    }

    fn parse(&self, other_evidence: bool) -> Result<Vec<JournalRecord>, IdentityError> {
        let path = self.path();
        let Some(bytes) = self.read_bytes()? else {
            if other_evidence || self.lock_path().exists() {
                return Err(corrupt(&path, "is missing although this install has used it"));
            }
            return Ok(Vec::new());
        };
        if bytes.is_empty() {
            return Err(corrupt(&path, "exists but is empty"));
        }
        let text = std::str::from_utf8(&bytes).map_err(|_| corrupt(&path, "is not UTF-8"))?;
        let body = text.strip_suffix('\n').unwrap_or(text);
        let recs: Result<Vec<JournalRecord>, _> = body
            .split('\n')
            .enumerate()
            .map(|(i, line)| {
                serde_json::from_str(line).map_err(|e| corrupt(&path, format!("line {}: {e}", i + 1)))
            })
            .collect();
        Ok(recs?.into_iter().filter(|r| !matches!(r, JournalRecord::Init {})).collect())
    }

    /// Append `rec`, fsynced. Parses the existing journal first and refuses
    /// (leaving it untouched) if it does not parse or does not end in a
    /// newline.
    pub fn append(&self, lock: &JournalLock, rec: &JournalRecord) -> Result<(), IdentityError> {
        let path = self.path();
        self.read_locked(lock, false)?;
        let mut line = serde_json::to_vec(rec).map_err(|e| corrupt(&path, e.to_string()))?;
        line.push(b'\n');
        match self.read_bytes()? {
            None => return write_private_atomic(&path, &line, true),
            Some(b) if b.last() != Some(&b'\n') => {
                return Err(corrupt(&path, "does not end in a newline (torn write)"));
            }
            Some(_) => {}
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

    /// Every record of the journal that parses, line by line, ignoring any
    /// line that does not (a torn tail must not hide earlier records).
    /// Unverified: the caller rebuilds a view, which re-verifies certificates.
    pub fn salvage(&self, _lock: &JournalLock) -> Vec<JournalRecord> {
        let Ok(Some(bytes)) = self.read_bytes() else { return Vec::new() };
        String::from_utf8_lossy(&bytes)
            .lines()
            .filter_map(|l| serde_json::from_str::<JournalRecord>(l).ok())
            .filter(|r| !matches!(r, JournalRecord::Init {}))
            .collect()
    }

    /// Repair: move the current journal (if any) aside as
    /// `identity.journal.jsonl.corrupt-<unix secs>` and write a new one from
    /// `records`. Returns the path of the moved file.
    pub fn replace(&self, _lock: &JournalLock, records: &[JournalRecord]) -> Result<Option<PathBuf>, IdentityError> {
        let path = self.path();
        let mut moved = None;
        if path.exists() {
            let secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            let to = self.dir.join(format!("{JOURNAL_FILE}.corrupt-{secs}"));
            std::fs::rename(&path, &to)?;
            moved = Some(to);
        }
        let mut bytes = init_line();
        for r in records {
            bytes.extend(serde_json::to_vec(r).map_err(|e| corrupt(&path, e.to_string()))?);
            bytes.push(b'\n');
        }
        write_private_atomic(&path, &bytes, true)?;
        Ok(moved)
    }
}

fn init_line() -> Vec<u8> {
    let mut v = serde_json::to_vec(&JournalRecord::Init {}).unwrap_or_default();
    v.push(b'\n');
    v
}
