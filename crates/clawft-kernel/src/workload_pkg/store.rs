//! Persist verified packages in [`ArtifactStore`] and re-verify from it.
//!
//! Every file (and the manifest itself) is stored under its BLAKE3 hash.
//! Nothing is stored before the package verifies.

use std::path::Path;

use crate::artifact_store::{ArtifactStore, ArtifactType};
use crate::workload_kind::KindRegistry;

use super::manifest::{FileRef, MANIFEST_FILE, MAX_MANIFEST_BYTES};
use super::trust::TrustAnchors;
use super::verify::{
    DirSource, FileSource, VerifiedPackage, VerifyError, VerifyPolicy, read_bounded,
    verify_with_source, verify_with_source_in,
};

/// Result of [`store_package`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPackage {
    /// BLAKE3 of the signed statement.
    pub package_id: String,
    /// `ArtifactStore` hash of the `cogpkg.json` bytes; pass it to
    /// [`verify_stored`].
    pub manifest_hash: String,
    /// `(package path, artifact hash)` for every file.
    pub files: Vec<(String, String)>,
}

/// Reads package files from an [`ArtifactStore`] by hash.
pub struct StoreSource<'a> {
    store: &'a ArtifactStore,
}

impl<'a> StoreSource<'a> {
    /// Source over `store`.
    pub fn new(store: &'a ArtifactStore) -> Self {
        Self { store }
    }
}

impl FileSource for StoreSource<'_> {
    fn read(&self, file: &FileRef) -> Result<Vec<u8>, VerifyError> {
        if !self.store.contains(&file.blake3) {
            return Err(VerifyError::FileMissing {
                path: file.path.clone(),
            });
        }
        self.store.load(&file.blake3).map_err(|e| {
            let msg = e.to_string();
            if msg.contains("integrity error") {
                // The store re-hashes on load; a mismatch means the stored
                // bytes were altered.
                VerifyError::HashMismatch {
                    path: file.path.clone(),
                    expected: file.blake3.clone(),
                    actual: "stored content no longer matches its hash".into(),
                }
            } else {
                VerifyError::Io {
                    path: file.path.clone(),
                    msg,
                }
            }
        })
    }
}

fn store_err(path: &str, e: impl std::fmt::Display) -> VerifyError {
    VerifyError::Io {
        path: path.to_string(),
        msg: e.to_string(),
    }
}

/// Verify the package in `dir`, then store its manifest and files.
pub fn store_package(
    store: &ArtifactStore,
    dir: &Path,
    anchors: &TrustAnchors,
    policy: &VerifyPolicy,
) -> Result<StoredPackage, VerifyError> {
    let manifest = read_bounded(
        &dir.join(MANIFEST_FILE),
        MANIFEST_FILE,
        MAX_MANIFEST_BYTES as u64,
    )?;
    let source = DirSource::new(dir);
    let verified = verify_with_source(&manifest, &source, anchors, policy)?;

    let mut files = Vec::new();
    for file in verified.body.files() {
        // Re-read and re-check: the directory could change after verify.
        let content = source.read(file)?;
        super::verify::check_file(file, &content)?;
        let kind = if file.path == "cog.toml" {
            ArtifactType::ConfigBundle
        } else {
            ArtifactType::Generic
        };
        let hash = store
            .store(&content, kind)
            .map_err(|e| store_err(&file.path, e))?;
        files.push((file.path.clone(), hash));
    }
    let manifest_hash = store
        .store(&manifest, ArtifactType::AppManifest)
        .map_err(|e| store_err(MANIFEST_FILE, e))?;
    Ok(StoredPackage {
        package_id: verified.package_id,
        manifest_hash,
        files,
    })
}

/// Load a manifest from `store` by its artifact hash and verify it, reading
/// every file from the store.
pub fn verify_stored(
    store: &ArtifactStore,
    manifest_hash: &str,
    anchors: &TrustAnchors,
    policy: &VerifyPolicy,
) -> Result<VerifiedPackage, VerifyError> {
    verify_stored_in(
        store,
        manifest_hash,
        anchors,
        policy,
        &KindRegistry::builtin(),
    )
}

/// [`verify_stored`] against a caller-supplied kind registry.
pub fn verify_stored_in(
    store: &ArtifactStore,
    manifest_hash: &str,
    anchors: &TrustAnchors,
    policy: &VerifyPolicy,
    kinds: &KindRegistry,
) -> Result<VerifiedPackage, VerifyError> {
    if !store.contains(manifest_hash) {
        return Err(VerifyError::FileMissing {
            path: format!("{MANIFEST_FILE} ({manifest_hash})"),
        });
    }
    let manifest = store
        .load(manifest_hash)
        .map_err(|e| store_err(MANIFEST_FILE, e))?;
    verify_with_source_in(&manifest, &StoreSource::new(store), anchors, policy, kinds)
}
