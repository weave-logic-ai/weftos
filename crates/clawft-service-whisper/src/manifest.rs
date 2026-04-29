//! Voice model integrity (WEFT-209 / SC-7).
//!
//! `clawft-service-whisper` is an HTTP client against a `whisper.cpp`
//! daemon — the whisper.cpp process loads the model file (`ggml-*.bin`
//! / `model.onnx`) directly from disk. The integrity guarantee we need
//! is therefore "the model files on this substrate node have not been
//! tampered with between when an operator placed them there and when
//! the service starts up." That guarantee splits into two checks:
//!
//! 1. **SHA-256 over each on-disk model file**, computed at startup,
//!    matched against the hash claimed in a manifest file sitting
//!    alongside the model directory (`model.manifest.json`).
//! 2. **Ed25519 signature over the manifest**, in `model.manifest.sig`,
//!    verified against a trusted public key under
//!    `~/.clawft/trust-roots/voice/<key-id>.pub`.
//!
//! If the manifest is missing, the signature doesn't verify, or any
//! file's SHA-256 disagrees with the manifest, the loader returns an
//! `Err` so the service can refuse to start. This replaces the
//! previously-shipped "PLACEHOLDER_SHA256_*" strings in
//! `clawft-plugin/src/voice/models.rs`, which were never actually
//! checked at runtime.
//!
//! # Manifest shape
//!
//! ```json
//! {
//!   "version": 1,
//!   "model_id": "ggml-base.en",
//!   "files": [
//!     {"path": "ggml-base.en.bin", "sha256": "<64-hex>"}
//!   ],
//!   "signed_at_unix": 1745925600,
//!   "key_id": "voice-root-2026-04"
//! }
//! ```
//!
//! The signature file is the raw 64-byte Ed25519 signature over the
//! exact JSON bytes of the manifest. Hex-encoded would also work, but
//! raw bytes match the convention used by `clawft-core::security::signing`.

use std::fs;
use std::path::{Path, PathBuf};

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

/// Filename of the manifest emitted next to a model directory.
pub const MANIFEST_FILENAME: &str = "model.manifest.json";
/// Filename of the raw Ed25519 signature emitted next to the manifest.
pub const MANIFEST_SIG_FILENAME: &str = "model.manifest.sig";
/// Default trust-root directory under `$HOME` (overridable via API).
pub const DEFAULT_TRUST_ROOT_REL: &str = ".clawft/trust-roots/voice";

/// Errors returned by [`verify_model_dir`].
#[derive(Debug, thiserror::Error)]
pub enum ModelIntegrityError {
    /// Manifest file was not present in the model directory.
    #[error("model manifest missing at {0}")]
    ManifestMissing(PathBuf),
    /// Signature file was not present.
    #[error("model manifest signature missing at {0}")]
    SignatureMissing(PathBuf),
    /// Signature did not verify against the trusted key.
    #[error("model manifest signature verification failed: {0}")]
    SignatureInvalid(String),
    /// Manifest claimed a key id we do not have a trusted public key for.
    #[error("no trust root for key_id {key_id:?} (looked under {root})")]
    UnknownKeyId {
        /// `key_id` value pulled from the manifest's signing block.
        key_id: String,
        /// Trust-root directory we searched for `<key_id>.pub`.
        root: String,
    },
    /// A file claimed in the manifest was not present on disk.
    #[error("manifest claims file {0} but it is missing on disk")]
    FileMissing(PathBuf),
    /// On-disk SHA-256 disagreed with the manifest.
    #[error("sha-256 mismatch for {file}: manifest={want}, computed={got}")]
    HashMismatch {
        /// Model file whose hash failed to match.
        file: PathBuf,
        /// Hash claimed by the manifest.
        want: String,
        /// Hash computed from the on-disk file.
        got: String,
    },
    /// Manifest bytes were not valid JSON.
    #[error("manifest parse error: {0}")]
    ManifestParse(String),
    /// I/O error walking the model directory.
    #[error("model directory I/O error: {0}")]
    Io(String),
}

impl ModelIntegrityError {
    fn io<E: std::fmt::Display>(e: E) -> Self {
        ModelIntegrityError::Io(e.to_string())
    }
}

/// One file claimed in the manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestFile {
    /// Path of the file relative to the model directory.
    pub path: String,
    /// 64-character lower-hex SHA-256 hash.
    pub sha256: String,
}

/// Signed manifest describing the on-disk model files.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelManifest {
    /// Manifest schema version.
    pub version: u32,
    /// Identifier of the model bundle (e.g. `"ggml-base.en"`).
    pub model_id: String,
    /// Files this manifest covers.
    pub files: Vec<ManifestFile>,
    /// Unix epoch second when the manifest was signed.
    pub signed_at_unix: i64,
    /// Identifier of the signing key. Used to look up the trust root.
    pub key_id: String,
}

/// Outcome of a successful verification.
#[derive(Debug, Clone)]
pub struct ModelIntegrityReport {
    /// The verified manifest as parsed off disk.
    pub manifest: ModelManifest,
    /// Number of files checked.
    pub files_checked: usize,
}

/// Verify the model directory at `model_dir` against its manifest +
/// signature, using trusted keys under `trust_root`.
///
/// The function:
///
/// 1. Reads `model_dir/model.manifest.json` + `model.manifest.sig`.
/// 2. Looks up `trust_root/<manifest.key_id>.pub` (32 raw bytes).
/// 3. Verifies the Ed25519 signature over the manifest JSON.
/// 4. Computes SHA-256 of every file the manifest names, compares.
///
/// Returns a [`ModelIntegrityReport`] on success or a typed error on
/// any failure, so the daemon can refuse to start with a clear log.
pub fn verify_model_dir(
    model_dir: &Path,
    trust_root: &Path,
) -> Result<ModelIntegrityReport, ModelIntegrityError> {
    let manifest_path = model_dir.join(MANIFEST_FILENAME);
    let sig_path = model_dir.join(MANIFEST_SIG_FILENAME);

    if !manifest_path.is_file() {
        return Err(ModelIntegrityError::ManifestMissing(manifest_path));
    }
    if !sig_path.is_file() {
        return Err(ModelIntegrityError::SignatureMissing(sig_path));
    }

    let manifest_bytes =
        fs::read(&manifest_path).map_err(ModelIntegrityError::io)?;
    let sig_bytes = fs::read(&sig_path).map_err(ModelIntegrityError::io)?;

    let manifest: ModelManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|e| ModelIntegrityError::ManifestParse(e.to_string()))?;

    let key_path = trust_root.join(format!("{}.pub", manifest.key_id));
    if !key_path.is_file() {
        return Err(ModelIntegrityError::UnknownKeyId {
            key_id: manifest.key_id.clone(),
            root: trust_root.display().to_string(),
        });
    }
    let key_bytes = fs::read(&key_path).map_err(ModelIntegrityError::io)?;
    let key_array: [u8; 32] = key_bytes
        .as_slice()
        .try_into()
        .map_err(|_| {
            ModelIntegrityError::SignatureInvalid(format!(
                "trust root key {} is not 32 bytes (got {})",
                key_path.display(),
                key_bytes.len()
            ))
        })?;
    let verifying_key = VerifyingKey::from_bytes(&key_array)
        .map_err(|e| ModelIntegrityError::SignatureInvalid(e.to_string()))?;

    let sig_array: [u8; 64] = sig_bytes
        .as_slice()
        .try_into()
        .map_err(|_| {
            ModelIntegrityError::SignatureInvalid(format!(
                "signature file {} is not 64 bytes (got {})",
                sig_path.display(),
                sig_bytes.len()
            ))
        })?;
    let signature = Signature::from_bytes(&sig_array);

    verifying_key
        .verify(&manifest_bytes, &signature)
        .map_err(|e| ModelIntegrityError::SignatureInvalid(e.to_string()))?;

    // Hash check.
    for entry in &manifest.files {
        let file_path = model_dir.join(&entry.path);
        if !file_path.is_file() {
            return Err(ModelIntegrityError::FileMissing(file_path));
        }
        let got = sha256_hex(&file_path)?;
        if !got.eq_ignore_ascii_case(&entry.sha256) {
            return Err(ModelIntegrityError::HashMismatch {
                file: file_path,
                want: entry.sha256.clone(),
                got,
            });
        }
    }

    info!(
        model_id = %manifest.model_id,
        key_id = %manifest.key_id,
        files_checked = manifest.files.len(),
        "voice model integrity verified"
    );

    let files_checked = manifest.files.len();
    Ok(ModelIntegrityReport {
        manifest,
        files_checked,
    })
}

/// Convenience: resolve `trust_root` to `$HOME/.clawft/trust-roots/voice`.
pub fn default_trust_root() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(DEFAULT_TRUST_ROOT_REL))
}

/// Compute the lower-hex SHA-256 of a file's contents, streaming the
/// read so we don't slurp 1.5 GB ggml weights into memory.
pub fn sha256_hex(path: &Path) -> Result<String, ModelIntegrityError> {
    use std::io::Read;
    let mut f = fs::File::open(path).map_err(ModelIntegrityError::io)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(ModelIntegrityError::io)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Like [`verify_model_dir`] but logs a warning + returns `Ok(None)`
/// when no manifest is present (so a deployment that hasn't yet rolled
/// out signed manifests can run with a clear audit trail). Use
/// [`verify_model_dir`] directly for hard-fail.
pub fn verify_model_dir_soft(
    model_dir: &Path,
    trust_root: &Path,
) -> Result<Option<ModelIntegrityReport>, ModelIntegrityError> {
    match verify_model_dir(model_dir, trust_root) {
        Ok(r) => Ok(Some(r)),
        Err(ModelIntegrityError::ManifestMissing(p)) => {
            warn!(
                missing = %p.display(),
                "voice model integrity check skipped: no manifest present \
                 (deploy a signed model.manifest.json to enforce SC-7)"
            );
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use std::io::Write;
    use tempfile::TempDir;

    fn make_signing_key() -> (SigningKey, [u8; 32]) {
        // Deterministic key for tests — we don't care about randomness
        // here, just that the signature math works.
        let secret_bytes = [7u8; 32];
        let signing = SigningKey::from_bytes(&secret_bytes);
        let verifying = signing.verifying_key().to_bytes();
        (signing, verifying)
    }

    fn write_file(p: &Path, contents: &[u8]) {
        let mut f = fs::File::create(p).unwrap();
        f.write_all(contents).unwrap();
    }

    /// Build a (model_dir, trust_root) pair carrying a valid manifest +
    /// signature for a single model file. The model file is `model.bin`
    /// with a known SHA-256.
    fn fixture_valid() -> (TempDir, TempDir, ModelManifest) {
        let model_dir = TempDir::new().unwrap();
        let trust_root = TempDir::new().unwrap();

        // Write a fake model file.
        let model_bytes = b"hello whisper model";
        write_file(&model_dir.path().join("model.bin"), model_bytes);

        // Compute the file's real SHA-256.
        let mut hasher = Sha256::new();
        hasher.update(model_bytes);
        let want_hash = hex::encode(hasher.finalize());

        // Build the manifest.
        let key_id = "voice-test-key-1".to_string();
        let manifest = ModelManifest {
            version: 1,
            model_id: "ggml-test".into(),
            files: vec![ManifestFile {
                path: "model.bin".into(),
                sha256: want_hash,
            }],
            signed_at_unix: 1_745_925_600,
            key_id: key_id.clone(),
        };
        let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        write_file(&model_dir.path().join(MANIFEST_FILENAME), &manifest_bytes);

        // Sign.
        let (signing, verifying) = make_signing_key();
        let sig: Signature = signing.sign(&manifest_bytes);
        write_file(
            &model_dir.path().join(MANIFEST_SIG_FILENAME),
            &sig.to_bytes(),
        );

        // Place trust root key.
        write_file(
            &trust_root.path().join(format!("{key_id}.pub")),
            &verifying,
        );

        (model_dir, trust_root, manifest)
    }

    #[test]
    fn valid_manifest_verifies() {
        let (model_dir, trust_root, _m) = fixture_valid();
        let report =
            verify_model_dir(model_dir.path(), trust_root.path()).expect("ok");
        assert_eq!(report.files_checked, 1);
        assert_eq!(report.manifest.model_id, "ggml-test");
    }

    #[test]
    fn tampered_model_fails_verify() {
        let (model_dir, trust_root, _m) = fixture_valid();
        // Tamper with the model file *without* re-signing.
        write_file(&model_dir.path().join("model.bin"), b"corrupted!");
        let err = verify_model_dir(model_dir.path(), trust_root.path())
            .expect_err("must fail");
        match err {
            ModelIntegrityError::HashMismatch { .. } => {}
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn missing_signature_refuses_load() {
        let (model_dir, trust_root, _m) = fixture_valid();
        fs::remove_file(model_dir.path().join(MANIFEST_SIG_FILENAME)).unwrap();
        let err = verify_model_dir(model_dir.path(), trust_root.path())
            .expect_err("must fail");
        match err {
            ModelIntegrityError::SignatureMissing(_) => {}
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn missing_manifest_hard_fails_strict() {
        let (model_dir, trust_root, _m) = fixture_valid();
        fs::remove_file(model_dir.path().join(MANIFEST_FILENAME)).unwrap();
        let err = verify_model_dir(model_dir.path(), trust_root.path())
            .expect_err("must fail");
        match err {
            ModelIntegrityError::ManifestMissing(_) => {}
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn missing_manifest_soft_skips() {
        let (model_dir, trust_root, _m) = fixture_valid();
        fs::remove_file(model_dir.path().join(MANIFEST_FILENAME)).unwrap();
        let r = verify_model_dir_soft(model_dir.path(), trust_root.path())
            .expect("soft must not error on missing manifest");
        assert!(r.is_none());
    }

    #[test]
    fn unknown_key_id_refuses_load() {
        let (model_dir, trust_root, _m) = fixture_valid();
        // Remove the trust-root key file so the manifest's key_id can't
        // be resolved.
        for entry in fs::read_dir(trust_root.path()).unwrap().flatten() {
            fs::remove_file(entry.path()).unwrap();
        }
        let err = verify_model_dir(model_dir.path(), trust_root.path())
            .expect_err("must fail");
        match err {
            ModelIntegrityError::UnknownKeyId { .. } => {}
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn forged_signature_with_wrong_key_fails() {
        let (model_dir, trust_root, manifest) = fixture_valid();
        // Re-sign the manifest with a *different* key, but leave the
        // trust-root key (the original one) in place. Verify must
        // refuse: signature won't validate against the placed pubkey.
        let attacker_secret = [9u8; 32];
        let attacker = SigningKey::from_bytes(&attacker_secret);
        let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        let bad_sig = attacker.sign(&manifest_bytes);
        write_file(
            &model_dir.path().join(MANIFEST_SIG_FILENAME),
            &bad_sig.to_bytes(),
        );
        let err = verify_model_dir(model_dir.path(), trust_root.path())
            .expect_err("must fail");
        match err {
            ModelIntegrityError::SignatureInvalid(_) => {}
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn file_missing_fails() {
        let (model_dir, trust_root, _m) = fixture_valid();
        fs::remove_file(model_dir.path().join("model.bin")).unwrap();
        let err = verify_model_dir(model_dir.path(), trust_root.path())
            .expect_err("must fail");
        match err {
            ModelIntegrityError::FileMissing(_) => {}
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn sha256_hex_matches_known_value() {
        // Standard "abc" -> SHA-256 vector.
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("x");
        write_file(&p, b"abc");
        let h = sha256_hex(&p).unwrap();
        assert_eq!(
            h,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
