//! Persistent operator secret store for per-Seed tokens.
//!
//! One file per Seed under a private directory (`0700`), each file `0600`,
//! written atomically (temp file, fsync, rename). Tokens survive daemon
//! restarts and never expire on their own; a new pairing replaces them.
//! Reads refuse a symlink, a non-regular file or a file other users can
//! read, so a token planted or exposed by someone else is not used.
//!
//! The operator default is `<runtime dir>/secrets/workload.seed/`, where the
//! runtime dir is `$WEFTOS_RUNTIME_DIR` or `~/.clawft` (the same root the
//! chain uses), so probe and test daemons never read the operator's tokens.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use clawft_types::config::chain_paths::{chain_root, runtime_dir_from_env};
use clawft_types::secret::SecretString;

use super::seed_http::SeedCredentials;
use super::types::RuntimeError;
use crate::workload_pkg::manifest::valid_token;

/// Sub-path of the runtime dir that holds Seed tokens.
pub const SEED_SECRETS_SUBDIR: &str = "secrets/workload.seed";
/// Largest token accepted.
const MAX_TOKEN: u64 = 512;

/// File-backed [`SeedCredentials`].
#[derive(Debug, Clone)]
pub struct FileCredentials {
    dir: PathBuf,
}

fn io(what: &str, e: std::io::Error) -> RuntimeError {
    RuntimeError::Backend(format!("seed credential {what}: {e}"))
}

impl FileCredentials {
    /// Store in `dir` (created `0700` on first write).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The operator store: `$WEFTOS_RUNTIME_DIR` or `~/.clawft`, then
    /// [`SEED_SECRETS_SUBDIR`].
    pub fn operator_default() -> Result<Self, RuntimeError> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let root = chain_root(runtime_dir_from_env().as_deref(), home.as_deref())
            .ok_or_else(|| RuntimeError::InvalidConfig("no runtime dir or home".into()))?;
        Ok(Self::new(root.join(SEED_SECRETS_SUBDIR)))
    }

    /// Directory holding the tokens.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path_for(&self, node_id: &str) -> Result<PathBuf, RuntimeError> {
        if !valid_token(node_id, 64) || node_id.starts_with('.') {
            return Err(RuntimeError::InvalidConfig(
                "seed node id must be a plain token".into(),
            ));
        }
        Ok(self.dir.join(format!("{node_id}.token")))
    }

    fn ensure_dir(&self) -> Result<(), RuntimeError> {
        match fs::symlink_metadata(&self.dir) {
            Ok(m) if m.file_type().is_symlink() || !m.is_dir() => Err(RuntimeError::InvalidConfig(
                "seed secret dir is not a plain directory".into(),
            )),
            Ok(_) => fs::set_permissions(&self.dir, fs::Permissions::from_mode(0o700))
                .map_err(|e| io("dir", e)),
            Err(_) => fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&self.dir)
                .map_err(|e| io("dir", e)),
        }
    }
}

impl SeedCredentials for FileCredentials {
    fn get(&self, node_id: &str) -> Result<SecretString, RuntimeError> {
        let path = self.path_for(node_id)?;
        let meta = fs::symlink_metadata(&path).map_err(|_| {
            RuntimeError::InvalidConfig(format!("no credential for {node_id}; pair the Seed"))
        })?;
        if !meta.file_type().is_file() {
            return Err(RuntimeError::InvalidConfig(
                "seed credential is not a regular file".into(),
            ));
        }
        if meta.mode() & 0o077 != 0 {
            return Err(RuntimeError::InvalidConfig(
                "seed credential is readable by other users; re-pair".into(),
            ));
        }
        if meta.len() > MAX_TOKEN {
            return Err(RuntimeError::InvalidConfig(
                "seed credential too large".into(),
            ));
        }
        let raw = fs::read_to_string(&path).map_err(|e| io("read", e))?;
        let token = raw.trim();
        if token.is_empty() || !token.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(RuntimeError::InvalidConfig(
                "seed credential is malformed".into(),
            ));
        }
        Ok(SecretString::new(token))
    }

    fn put(&self, node_id: &str, token: SecretString) -> Result<(), RuntimeError> {
        let path = self.path_for(node_id)?;
        let t = token.expose();
        if t.is_empty() || t.len() as u64 > MAX_TOKEN || !t.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(RuntimeError::InvalidConfig("refusing a malformed token".into()));
        }
        self.ensure_dir()?;
        let tmp = self.dir.join(format!(".{node_id}.token.tmp"));
        let _ = fs::remove_file(&tmp);
        let mut f = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|e| io("write", e))?;
        f.write_all(t.as_bytes())
            .and_then(|_| f.sync_all())
            .map_err(|e| io("write", e))?;
        fs::rename(&tmp, &path).map_err(|e| io("rename", e))
    }
}
