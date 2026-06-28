//! Ed25519 skill signing and content hashing.
//!
//! Provides key generation, content hashing (SHA-256), digital signature
//! creation, and verification for skill packages published to ClawHub.
//!
//! All operations use Ed25519 (via `ed25519-dalek`) and SHA-256 (via `sha2`).
//! Keys are stored as hex-encoded files with restricted permissions (0o600).

use std::path::Path;

use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use clawft_types::error::ClawftError;

/// SHA-256 content hash over a skill directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillContentHash {
    /// Hex-encoded SHA-256 hash of the concatenated file contents.
    pub sha256: String,
    /// Sorted list of file paths included in the hash.
    pub files: Vec<String>,
}

/// A digital signature over a content hash.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillSignature {
    /// Hex-encoded Ed25519 signature.
    pub signature: String,
    /// Hex-encoded public key that can verify this signature.
    pub public_key: String,
    /// Algorithm identifier (always `"ed25519"`).
    pub algorithm: String,
}

/// Name of the private key file.
const PRIVATE_KEY_FILE: &str = "skill-signing.key";
/// Name of the public key file.
const PUBLIC_KEY_FILE: &str = "skill-signing.pub";

/// Generate an Ed25519 key pair and save to `output_dir`.
///
/// Creates two files:
/// - `skill-signing.key` (hex-encoded 32-byte private key, mode 0o600)
/// - `skill-signing.pub` (hex-encoded 32-byte public key)
pub fn generate_keypair(output_dir: &Path) -> Result<(), ClawftError> {
    std::fs::create_dir_all(output_dir)?;

    let mut csprng = rand::thread_rng();
    let signing_key = SigningKey::generate(&mut csprng);
    let verifying_key = signing_key.verifying_key();

    let priv_path = output_dir.join(PRIVATE_KEY_FILE);
    let pub_path = output_dir.join(PUBLIC_KEY_FILE);

    // Write private key as hex.
    let priv_hex = hex_encode(signing_key.as_bytes());
    std::fs::write(&priv_path, &priv_hex)?;

    // Set restrictive permissions (Unix only).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&priv_path, std::fs::Permissions::from_mode(0o600))?;
    }

    // Write public key as hex.
    let pub_hex = hex_encode(verifying_key.as_bytes());
    std::fs::write(&pub_path, &pub_hex)?;

    Ok(())
}

/// Compute a deterministic SHA-256 content hash over all files in `dir`.
///
/// Files are sorted by relative path, then their contents are concatenated
/// (with a length-prefixed header per file) to produce a single hash. Hidden
/// files (starting with `.`) and directories named `target` are excluded.
pub fn compute_content_hash(dir: &Path) -> Result<SkillContentHash, ClawftError> {
    let mut entries = Vec::new();
    collect_files(dir, dir, &mut entries)?;
    entries.sort();

    let mut hasher = Sha256::new();
    let mut file_list = Vec::new();

    for rel_path in &entries {
        let abs_path = dir.join(rel_path);
        let content = std::fs::read(&abs_path)?;

        // Length-prefix each file to prevent boundary confusion.
        hasher.update(rel_path.as_bytes());
        hasher.update(b"\0");
        hasher.update((content.len() as u64).to_le_bytes());
        hasher.update(&content);

        file_list.push(rel_path.clone());
    }

    let hash = hasher.finalize();
    Ok(SkillContentHash {
        sha256: hex_encode(&hash),
        files: file_list,
    })
}

/// Sign a content hash string with a private key.
pub fn sign_content(hash: &str, private_key_bytes: &[u8]) -> Result<SkillSignature, ClawftError> {
    let key_bytes: [u8; 32] =
        private_key_bytes
            .try_into()
            .map_err(|_| ClawftError::SecurityViolation {
                reason: "private key must be exactly 32 bytes".into(),
            })?;

    let signing_key = SigningKey::from_bytes(&key_bytes);
    let verifying_key = signing_key.verifying_key();
    let signature = signing_key.sign(hash.as_bytes());

    Ok(SkillSignature {
        signature: hex_encode(&signature.to_bytes()),
        public_key: hex_encode(verifying_key.as_bytes()),
        algorithm: "ed25519".into(),
    })
}

/// Verify a signature against a content hash.
pub fn verify_signature(hash: &str, sig: &SkillSignature) -> Result<bool, ClawftError> {
    if sig.algorithm != "ed25519" {
        return Err(ClawftError::SecurityViolation {
            reason: format!("unsupported signature algorithm: {}", sig.algorithm),
        });
    }

    let pub_bytes = hex_decode(&sig.public_key).map_err(|e| ClawftError::SecurityViolation {
        reason: format!("invalid public key hex: {e}"),
    })?;
    let sig_bytes = hex_decode(&sig.signature).map_err(|e| ClawftError::SecurityViolation {
        reason: format!("invalid signature hex: {e}"),
    })?;

    let pub_array: [u8; 32] = pub_bytes
        .try_into()
        .map_err(|_| ClawftError::SecurityViolation {
            reason: "public key must be exactly 32 bytes".into(),
        })?;
    let sig_array: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| ClawftError::SecurityViolation {
            reason: "signature must be exactly 64 bytes".into(),
        })?;

    let verifying_key =
        VerifyingKey::from_bytes(&pub_array).map_err(|e| ClawftError::SecurityViolation {
            reason: format!("invalid public key: {e}"),
        })?;
    let signature = ed25519_dalek::Signature::from_bytes(&sig_array);

    Ok(verifying_key.verify(hash.as_bytes(), &signature).is_ok())
}

/// Load a private signing key from `keys_dir/skill-signing.key`.
///
/// Returns `Ok(None)` if the file does not exist.
pub fn load_signing_key(keys_dir: &Path) -> Result<Option<Vec<u8>>, ClawftError> {
    let path = keys_dir.join(PRIVATE_KEY_FILE);
    if !path.exists() {
        return Ok(None);
    }
    let hex_str = std::fs::read_to_string(&path)?;
    let bytes = hex_decode(hex_str.trim()).map_err(|e| ClawftError::SecurityViolation {
        reason: format!("invalid private key file: {e}"),
    })?;
    Ok(Some(bytes))
}

/// Load a public verification key from `keys_dir/skill-signing.pub`.
///
/// Returns `Ok(None)` if the file does not exist.
pub fn load_public_key(keys_dir: &Path) -> Result<Option<Vec<u8>>, ClawftError> {
    let path = keys_dir.join(PUBLIC_KEY_FILE);
    if !path.exists() {
        return Ok(None);
    }
    let hex_str = std::fs::read_to_string(&path)?;
    let bytes = hex_decode(hex_str.trim()).map_err(|e| ClawftError::SecurityViolation {
        reason: format!("invalid public key file: {e}"),
    })?;
    Ok(Some(bytes))
}

// ── Helpers ──────────────────────────────────────────────────────────

/// Hex-encode a byte slice.
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Hex-decode a string into bytes.
fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) {
        return Err("odd-length hex string".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

/// Recursively collect relative file paths under `base`, excluding hidden
/// files and `target/` directories.
fn collect_files(current: &Path, base: &Path, out: &mut Vec<String>) -> Result<(), ClawftError> {
    if !current.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(current)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Skip hidden files and target directories.
        if name_str.starts_with('.') || name_str == "target" {
            continue;
        }

        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, base, out)?;
        } else if path.is_file() {
            let rel = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            out.push(rel);
        }
    }
    Ok(())
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(all(test, feature = "signing"))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(prefix: &str) -> std::path::PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        std::env::temp_dir().join(format!("clawft_signing_{prefix}_{pid}_{id}"))
    }

    #[test]
    fn keygen_creates_keypair() {
        let dir = temp_dir("keygen");
        generate_keypair(&dir).unwrap();

        let priv_path = dir.join(PRIVATE_KEY_FILE);
        let pub_path = dir.join(PUBLIC_KEY_FILE);

        assert!(priv_path.exists(), "private key file should exist");
        assert!(pub_path.exists(), "public key file should exist");

        // Private key should be 64 hex chars (32 bytes).
        let priv_hex = std::fs::read_to_string(&priv_path).unwrap();
        assert_eq!(priv_hex.len(), 64);

        // Public key should be 64 hex chars (32 bytes).
        let pub_hex = std::fs::read_to_string(&pub_path).unwrap();
        assert_eq!(pub_hex.len(), 64);

        // Verify permissions on Unix.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::metadata(&priv_path).unwrap().permissions();
            assert_eq!(perms.mode() & 0o777, 0o600);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn content_hash_deterministic() {
        let dir = temp_dir("hash_det");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), "---\nname: test\n---\nInstructions.").unwrap();
        std::fs::write(dir.join("extra.txt"), "extra content").unwrap();

        let hash1 = compute_content_hash(&dir).unwrap();
        let hash2 = compute_content_hash(&dir).unwrap();

        assert_eq!(hash1.sha256, hash2.sha256);
        assert_eq!(hash1.files, hash2.files);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn content_hash_detects_changes() {
        let dir = temp_dir("hash_change");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), "original content").unwrap();

        let hash_before = compute_content_hash(&dir).unwrap();

        std::fs::write(dir.join("SKILL.md"), "modified content").unwrap();
        let hash_after = compute_content_hash(&dir).unwrap();

        assert_ne!(hash_before.sha256, hash_after.sha256);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sign_and_verify_roundtrip() {
        let dir = temp_dir("sign_roundtrip");
        generate_keypair(&dir).unwrap();

        let priv_key = load_signing_key(&dir).unwrap().unwrap();
        let hash = "abc123def456";
        let sig = sign_content(hash, &priv_key).unwrap();

        assert_eq!(sig.algorithm, "ed25519");
        assert!(verify_signature(hash, &sig).unwrap());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_wrong_key_fails() {
        let dir1 = temp_dir("wrong_key_1");
        let dir2 = temp_dir("wrong_key_2");

        generate_keypair(&dir1).unwrap();
        generate_keypair(&dir2).unwrap();

        let priv_key1 = load_signing_key(&dir1).unwrap().unwrap();
        let hash = "test_hash_value";
        let mut sig = sign_content(hash, &priv_key1).unwrap();

        // Replace the public key with the one from a different key pair.
        let pub_key2 = load_public_key(&dir2).unwrap().unwrap();
        sig.public_key = hex_encode(&pub_key2);

        assert!(!verify_signature(hash, &sig).unwrap());

        let _ = std::fs::remove_dir_all(&dir1);
        let _ = std::fs::remove_dir_all(&dir2);
    }

    #[test]
    fn verify_tampered_hash_fails() {
        let dir = temp_dir("tampered");
        generate_keypair(&dir).unwrap();

        let priv_key = load_signing_key(&dir).unwrap().unwrap();
        let hash = "original_hash";
        let sig = sign_content(hash, &priv_key).unwrap();

        // Verify with a different hash should fail.
        assert!(!verify_signature("tampered_hash", &sig).unwrap());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_missing_key_returns_none() {
        let dir = temp_dir("no_key");
        std::fs::create_dir_all(&dir).unwrap();

        assert!(load_signing_key(&dir).unwrap().is_none());
        assert!(load_public_key(&dir).unwrap().is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hex_roundtrip() {
        let data = [0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0xFF];
        let encoded = hex_encode(&data);
        assert_eq!(encoded, "deadbeef00ff");
        let decoded = hex_decode(&encoded).unwrap();
        assert_eq!(decoded, data);
    }
}
