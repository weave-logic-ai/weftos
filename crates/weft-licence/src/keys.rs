//! The grant key: generated once by `init` over USB, stored 0600 in a 0700
//! directory, never overwritten, never learned over the network.

use std::path::{Path, PathBuf};

use ed25519_dalek::SigningKey;
use weft_licence_wire::{hex_decode_exact, hex_encode, key_id};

use crate::error::SvcError;
use crate::fsio;

/// File name of the grant key inside the state directory.
pub const KEY_FILE: &str = "grant.key";

/// What `init` made, for the operator to confirm before signing a binding.
#[derive(Debug, Clone)]
pub struct InitReport {
    /// `ed25519:` plus 16 hex chars of `sha256(public key)`.
    pub fingerprint: String,
    /// The grant public key, 64 hex chars.
    pub grant_pubkey: String,
    /// Where the key was written.
    pub key_path: PathBuf,
}

/// The key file path under `state_dir`.
pub fn key_path(state_dir: &Path) -> PathBuf {
    state_dir.join(KEY_FILE)
}

/// Generate the grant key. Refuses when a key (or a dangling file in its
/// place) already exists: an existing key is never overwritten.
pub fn init(state_dir: &Path) -> Result<InitReport, SvcError> {
    fsio::ensure_private_dir(state_dir).map_err(|e| SvcError::Io(e.to_string()))?;
    let dir_mode = fsio::mode_of(state_dir).map_err(|e| SvcError::Io(e.to_string()))?;
    if cfg!(unix) && dir_mode & 0o077 != 0 {
        return Err(SvcError::KeyPerms(format!(
            "state dir {} has mode {dir_mode:o}; it must be 0700",
            state_dir.display()
        )));
    }
    let path = key_path(state_dir);
    if path.exists() {
        return Err(SvcError::KeyExists);
    }
    let seed = fsio::random_bytes::<32>().map_err(|e| SvcError::Io(e.to_string()))?;
    let sk = SigningKey::from_bytes(&seed);
    use std::io::Write;
    let mut f = fsio::create_new_private(&path).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => SvcError::KeyExists,
        _ => SvcError::Io(e.to_string()),
    })?;
    f.write_all(format!("{}\n", hex_encode(&seed)).as_bytes())
        .and_then(|_| f.sync_all())
        .map_err(|e| SvcError::Io(e.to_string()))?;
    let pk = sk.verifying_key().to_bytes();
    Ok(InitReport { fingerprint: key_id(&pk), grant_pubkey: hex_encode(&pk), key_path: path })
}

/// Load the grant key, refusing a key file or state directory that another
/// user could read (any group or other permission bit).
pub fn load(state_dir: &Path) -> Result<SigningKey, SvcError> {
    let path = key_path(state_dir);
    let dir_mode = fsio::mode_of(state_dir).map_err(|e| SvcError::Io(e.to_string()))?;
    if cfg!(unix) && dir_mode & 0o077 != 0 {
        return Err(SvcError::KeyPerms(format!("state dir mode {dir_mode:o}, want 0700")));
    }
    let file_mode = fsio::mode_of(&path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => SvcError::NoKey,
        _ => SvcError::Io(e.to_string()),
    })?;
    if cfg!(unix) && file_mode & 0o177 != 0 {
        return Err(SvcError::KeyPerms(format!("key file mode {file_mode:o}, want 0600")));
    }
    let raw = fsio::read_capped(&path, 256)
        .map_err(|e| SvcError::Io(e.to_string()))?
        .ok_or(SvcError::NoKey)?;
    let text = String::from_utf8(raw).map_err(|_| SvcError::BadKey)?;
    let seed = hex_decode_exact::<32>(text.trim()).ok_or(SvcError::BadKey)?;
    Ok(SigningKey::from_bytes(&seed))
}

/// Delete the key (unbind: `init` runs again for the next mesh).
pub fn delete(state_dir: &Path) -> Result<(), SvcError> {
    match std::fs::remove_file(key_path(state_dir)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(SvcError::Io(e.to_string())),
    }
}
