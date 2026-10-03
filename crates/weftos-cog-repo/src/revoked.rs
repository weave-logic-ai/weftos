//! Signer-key revocations (ADR-099 section 7) as seen by the signed-only cog verifiers.
//!
//! The kernel keeps revoked signer keys in `revoked_subjects.json` next to the host ban file in
//! the runtime dir. This reads just the `signer_key` entries from that file, so `weft-cog-repo`
//! and `weaver cog install` refuse a release or private-repo key an operator revoked, without
//! linking the kernel. Fail-closed: a file that exists but cannot be read or parsed is an error,
//! never an empty list.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// File name of the kernel's subject revocation list.
pub const SUBJECTS_FILE_NAME: &str = "revoked_subjects.json";

/// The set of revoked signer public keys (lower-case hex).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RevokedKeys {
    keys: BTreeSet<String>,
}

#[derive(Deserialize)]
struct Entry {
    kind: String,
    id: String,
}

impl RevokedKeys {
    /// No revocations.
    pub fn none() -> Self {
        Self::default()
    }

    /// From explicit keys (hex, any case).
    pub fn from_keys<I: IntoIterator<Item = S>, S: AsRef<str>>(keys: I) -> Self {
        Self { keys: keys.into_iter().map(|k| k.as_ref().to_ascii_lowercase()).collect() }
    }

    /// Load `path`. A missing file is an empty list; an unreadable or malformed one is an error.
    pub fn load(path: &Path) -> Result<Self, String> {
        if !path.exists() {
            return Ok(Self::none());
        }
        let data = std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let entries: Vec<Entry> = serde_json::from_str(&data).map_err(|e| format!("parse {}: {e}", path.display()))?;
        Ok(Self::from_keys(entries.iter().filter(|e| e.kind == "signer_key").map(|e| e.id.as_str())))
    }

    /// Where the kernel keeps the list, as far as a standalone tool can tell:
    /// `$WEFTOS_RUNTIME_DIR` when set, else the per-user daemon dir `~/.weftos/run`.
    pub fn default_path() -> Option<PathBuf> {
        let dir = match std::env::var("WEFTOS_RUNTIME_DIR").ok().filter(|d| !d.is_empty()) {
            Some(d) => PathBuf::from(d),
            None => PathBuf::from(std::env::var("HOME").ok().filter(|h| !h.is_empty())?).join(".weftos").join("run"),
        };
        Some(dir.join(SUBJECTS_FILE_NAME))
    }

    /// Whether a signer public key (hex, any case) is revoked.
    pub fn contains(&self, pubkey_hex: &str) -> bool {
        self.keys.contains(&pubkey_hex.to_ascii_lowercase())
    }

    /// Number of revoked keys.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// True when nothing is revoked.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_only_signer_keys_from_the_kernel_format() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(SUBJECTS_FILE_NAME);
        let key = "ab".repeat(32);
        std::fs::write(
            &p,
            format!(
                r#"[{{"kind":"signer_key","id":"{key}","revoked_at":1,"reason":"leaked"}},
                    {{"kind":"package","id":"x","revoked_at":1,"reason":"r"}}]"#
            ),
        )
        .unwrap();
        let r = RevokedKeys::load(&p).unwrap();
        assert_eq!(r.len(), 1);
        assert!(r.contains(&key.to_uppercase()));
        assert!(!r.contains("x"));
    }

    #[test]
    fn missing_is_empty_and_malformed_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        assert!(RevokedKeys::load(&dir.path().join("nope.json")).unwrap().is_empty());
        let p = dir.path().join(SUBJECTS_FILE_NAME);
        std::fs::write(&p, "{not json").unwrap();
        assert!(RevokedKeys::load(&p).is_err());
    }
}
