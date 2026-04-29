//! Token persistence with secure file permissions.
//!
//! Tokens are stored at `~/.clawft/tokens/<provider>.json` with
//! 0600 file permissions. Rotated refresh tokens are persisted
//! immediately to prevent loss on crash.

use std::fs;
use std::path::{Path, PathBuf};

use tracing::{debug, warn};

use crate::types::{AuthorizationState, StoredTokens};

/// Default token storage directory.
fn default_token_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".clawft")
        .join("tokens")
}

/// Token storage manager.
#[derive(Debug, Clone)]
pub struct TokenStore {
    /// Base directory for token files.
    base_dir: PathBuf,
}

impl Default for TokenStore {
    fn default() -> Self {
        Self {
            base_dir: default_token_dir(),
        }
    }
}

impl TokenStore {
    /// Create a token store with the default directory (~/.clawft/tokens/).
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a token store with a custom directory (for testing).
    pub fn with_dir(dir: PathBuf) -> Self {
        Self { base_dir: dir }
    }

    /// Ensure the token directory exists with proper permissions.
    fn ensure_dir(&self) -> Result<(), String> {
        if !self.base_dir.exists() {
            fs::create_dir_all(&self.base_dir)
                .map_err(|e| format!("failed to create token dir: {e}"))?;

            // Set directory permissions to 0700
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let perms = fs::Permissions::from_mode(0o700);
                fs::set_permissions(&self.base_dir, perms)
                    .map_err(|e| format!("failed to set dir permissions: {e}"))?;
            }
        }
        Ok(())
    }

    /// Path to the token file for a provider.
    fn token_path(&self, provider: &str) -> PathBuf {
        self.base_dir.join(format!("{provider}.json"))
    }

    /// Path to the auth state file for a provider.
    fn state_path(&self, provider: &str) -> PathBuf {
        self.base_dir.join(format!("{provider}.state.json"))
    }

    /// Store tokens for a provider. File permissions are set to 0600.
    pub fn store_tokens(&self, tokens: &StoredTokens) -> Result<(), String> {
        self.ensure_dir()?;

        let path = self.token_path(&tokens.provider);
        let json = serde_json::to_string_pretty(tokens)
            .map_err(|e| format!("failed to serialize tokens: {e}"))?;

        // Write to a temp file first, then rename for atomicity
        let tmp_path = path.with_extension("json.tmp");
        fs::write(&tmp_path, &json)
            .map_err(|e| format!("failed to write token file: {e}"))?;

        // Set file permissions to 0600 before rename
        set_file_permissions_0600(&tmp_path)?;

        fs::rename(&tmp_path, &path)
            .map_err(|e| format!("failed to rename token file: {e}"))?;

        debug!(provider = %tokens.provider, path = %path.display(), "stored tokens");
        Ok(())
    }

    /// Load tokens for a provider.
    pub fn load_tokens(&self, provider: &str) -> Result<Option<StoredTokens>, String> {
        let path = self.token_path(provider);
        if !path.exists() {
            return Ok(None);
        }

        let json =
            fs::read_to_string(&path).map_err(|e| format!("failed to read token file: {e}"))?;
        let tokens: StoredTokens =
            serde_json::from_str(&json).map_err(|e| format!("failed to parse token file: {e}"))?;

        debug!(provider = %provider, "loaded tokens");
        Ok(Some(tokens))
    }

    /// Delete tokens for a provider.
    pub fn delete_tokens(&self, provider: &str) -> Result<bool, String> {
        let path = self.token_path(provider);
        if path.exists() {
            fs::remove_file(&path).map_err(|e| format!("failed to delete token file: {e}"))?;
            debug!(provider = %provider, "deleted tokens");
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Store authorization state (for CSRF validation during callback).
    pub fn store_auth_state(&self, state: &AuthorizationState) -> Result<(), String> {
        self.ensure_dir()?;

        let path = self.state_path(&state.provider);
        let json = serde_json::to_string(state)
            .map_err(|e| format!("failed to serialize auth state: {e}"))?;
        fs::write(&path, &json)
            .map_err(|e| format!("failed to write state file: {e}"))?;

        set_file_permissions_0600(&path)?;

        debug!(provider = %state.provider, "stored auth state");
        Ok(())
    }

    /// Load and consume authorization state (deletes the file after reading).
    pub fn consume_auth_state(&self, provider: &str) -> Result<Option<AuthorizationState>, String> {
        let path = self.state_path(provider);
        if !path.exists() {
            return Ok(None);
        }

        let json =
            fs::read_to_string(&path).map_err(|e| format!("failed to read state file: {e}"))?;
        let state: AuthorizationState = serde_json::from_str(&json)
            .map_err(|e| format!("failed to parse state file: {e}"))?;

        // Delete the state file after reading (single-use)
        if let Err(e) = fs::remove_file(&path) {
            warn!(error = %e, "failed to remove state file after consumption");
        }

        Ok(Some(state))
    }
}

/// Set file permissions to 0600 (owner read/write only).
fn set_file_permissions_0600(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o600);
        fs::set_permissions(path, perms)
            .map_err(|e| format!("failed to set file permissions: {e}"))?;
    }

    #[cfg(not(unix))]
    {
        let _ = path; // Suppress unused warning on non-unix
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_and_load_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let store = TokenStore::with_dir(dir.path().to_path_buf());

        let tokens = StoredTokens {
            access_token: "access-123".to_string(),
            refresh_token: Some("refresh-456".to_string()),
            token_type: "Bearer".to_string(),
            expires_at: Some(9999999999),
            scopes: vec!["email".to_string()],
            provider: "test".to_string(),
        };

        store.store_tokens(&tokens).unwrap();

        let loaded = store.load_tokens("test").unwrap().unwrap();
        assert_eq!(loaded.access_token, "access-123");
        assert_eq!(loaded.refresh_token, Some("refresh-456".to_string()));
        assert_eq!(loaded.provider, "test");
    }

    #[test]
    fn load_nonexistent_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let store = TokenStore::with_dir(dir.path().to_path_buf());

        let loaded = store.load_tokens("nonexistent").unwrap();
        assert!(loaded.is_none());
    }

    #[test]
    fn delete_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let store = TokenStore::with_dir(dir.path().to_path_buf());

        let tokens = StoredTokens {
            access_token: "test".to_string(),
            refresh_token: None,
            token_type: "Bearer".to_string(),
            expires_at: None,
            scopes: vec![],
            provider: "test".to_string(),
        };

        store.store_tokens(&tokens).unwrap();
        assert!(store.delete_tokens("test").unwrap());
        assert!(!store.delete_tokens("test").unwrap());
    }

    #[test]
    fn store_and_consume_auth_state() {
        let dir = tempfile::tempdir().unwrap();
        let store = TokenStore::with_dir(dir.path().to_path_buf());

        let state = AuthorizationState {
            state: "random-state".to_string(),
            pkce_verifier: "verifier".to_string(),
            provider: "test".to_string(),
            created_at: 12345,
        };

        store.store_auth_state(&state).unwrap();

        // First consume should return the state
        let loaded = store.consume_auth_state("test").unwrap().unwrap();
        assert_eq!(loaded.state, "random-state");
        assert_eq!(loaded.pkce_verifier, "verifier");

        // Second consume should return None (file was deleted)
        let second = store.consume_auth_state("test").unwrap();
        assert!(second.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn token_file_has_0600_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let store = TokenStore::with_dir(dir.path().to_path_buf());

        let tokens = StoredTokens {
            access_token: "secret".to_string(),
            refresh_token: None,
            token_type: "Bearer".to_string(),
            expires_at: None,
            scopes: vec![],
            provider: "perms_test".to_string(),
        };

        store.store_tokens(&tokens).unwrap();

        let path = dir.path().join("perms_test.json");
        let metadata = fs::metadata(&path).unwrap();
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected 0600, got {mode:o}");
    }
}
