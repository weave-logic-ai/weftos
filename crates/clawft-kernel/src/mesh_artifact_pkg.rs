//! Signed packages over the artifact exchange (mesh-placement-11).
//!
//! Governance for distribution (ADR-099 section 6): a node serves only
//! content listed by a signed workload manifest (wave-1 `workload_pkg`)
//! that verified on that node.
//!
//! - The origin seeds a package with [`ArtifactExchange::seed_package`]:
//!   the manifest's signatures are checked first, then every listed file is
//!   streamed into pieces and must match its pinned size and BLAKE3.
//! - A fetcher uses [`ArtifactExchange::fetch_package`]: it fetches the
//!   manifest by content hash, checks its signatures before fetching any
//!   file, fetches each file by its pinned hash (piece checks plus a
//!   whole-content check). Once the manifest verifies, this node may serve
//!   the pieces it holds, even before the package is complete.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::mesh_artifact::{ArtifactExchange, ExchangeError};
use crate::mesh_artifact_transfer::{FetchError, PeerSet};
use crate::mesh_artifact_wire::{ArtifactId, ArtifactKey};
use crate::workload_pkg::codec::{hex_decode_exact, hex_encode};
use crate::workload_pkg::manifest::{FileRef, MANIFEST_FILE, MAX_MANIFEST_BYTES};
use crate::workload_pkg::verify::{
    VerifiedPackage, VerifyError, read_bounded, verify_manifest_signatures,
};
use crate::workload_pkg::{TrustAnchors, VerifyPolicy, verify_stored};

/// A package this node can serve.
#[derive(Debug, Clone)]
pub struct ExchangedPackage {
    /// BLAKE3 of the signed statement.
    pub package_id: String,
    /// BLAKE3 of the manifest bytes (fetch key for other nodes).
    pub manifest_hash: String,
    /// `(package path, artifact id)` for every file.
    pub files: Vec<(String, ArtifactId)>,
    /// The verified manifest.
    pub verified: VerifiedPackage,
}

/// Why a package could not be seeded or fetched.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PackageExchangeError {
    /// Manifest or signature verification failed.
    #[error(transparent)]
    Verify(#[from] VerifyError),
    /// Local exchange failure.
    #[error(transparent)]
    Exchange(#[from] ExchangeError),
    /// Transfer failure.
    #[error(transparent)]
    Fetch(#[from] FetchError),
}

fn pinned(file: &FileRef) -> Result<([u8; 32], u64), VerifyError> {
    let hash = hex_decode_exact::<32>(&file.blake3)
        .ok_or_else(|| VerifyError::Manifest(format!("{}: bad blake3", file.path)))?;
    Ok((hash, file.size))
}

impl ArtifactExchange {
    /// Seed a signed package. `open` yields each listed file's bytes; files
    /// are streamed, so large payloads never sit in memory whole.
    pub fn seed_package(
        &self,
        manifest: &[u8],
        anchors: &TrustAnchors,
        open: &mut dyn FnMut(&FileRef) -> std::io::Result<Box<dyn Read>>,
    ) -> Result<ExchangedPackage, PackageExchangeError> {
        let verified = verify_manifest_signatures(manifest, anchors)?;
        let mut files = Vec::new();
        for file in verified.body.files() {
            let expect = pinned(file)?;
            let mut reader = open(file).map_err(|e| VerifyError::Io {
                path: file.path.clone(),
                msg: e.to_string(),
            })?;
            let d = self
                .seed_reader(reader.as_mut(), Some(expect))
                .map_err(|e| match e {
                    ExchangeError::Mismatch(m) => {
                        PackageExchangeError::Verify(VerifyError::HashMismatch {
                            path: file.path.clone(),
                            expected: file.blake3.clone(),
                            actual: m,
                        })
                    }
                    other => other.into(),
                })?;
            files.push((file.path.clone(), d.id()));
        }
        let m = self.seed_bytes(manifest)?;
        self.materialize(&m.id())?;
        for (_, id) in &files {
            self.materialize(id)?;
        }
        Ok(self.authorize(verified, m.content_hash, files))
    }

    /// [`Self::seed_package`] from an unpacked package directory.
    pub fn seed_package_dir(
        &self,
        dir: &Path,
        anchors: &TrustAnchors,
    ) -> Result<ExchangedPackage, PackageExchangeError> {
        let manifest = read_bounded(
            &dir.join(MANIFEST_FILE),
            MANIFEST_FILE,
            MAX_MANIFEST_BYTES as u64,
        )?;
        let root = dir
            .canonicalize()
            .map_err(|e| ExchangeError::Io(e.to_string()))?;
        self.seed_package(&manifest, anchors, &mut |file| {
            // Paths were validated as relative and traversal-free; also
            // refuse symlinks that resolve outside the package.
            let real = root.join(&file.path).canonicalize()?;
            if !real.starts_with(&root) {
                return Err(std::io::Error::other("resolves outside the package"));
            }
            Ok(Box::new(File::open(real)?) as Box<dyn Read>)
        })
    }

    /// Fetch a signed package by its manifest hash. Signatures are checked
    /// before any file is requested; each file must match its pinned size
    /// and hash. Once the manifest verifies, this node may serve what it holds.
    pub async fn fetch_package(
        &self,
        peers: &mut PeerSet,
        manifest_hash: &str,
        anchors: &TrustAnchors,
    ) -> Result<ExchangedPackage, PackageExchangeError> {
        let mh = hex_decode_exact::<32>(manifest_hash).ok_or_else(|| {
            VerifyError::Manifest("manifest hash must be 64 lower-case hex".into())
        })?;
        let m = self.fetch(peers, ArtifactKey::Content(mh)).await?;
        if m.descriptor.total_size > MAX_MANIFEST_BYTES as u64 {
            return Err(VerifyError::Manifest("manifest too large".into()).into());
        }
        let manifest = self.read_all(&m.id)?;
        let verified = verify_manifest_signatures(&manifest, anchors)?;
        // The manifest verified: its pinned content may be served from here
        // on, including pieces held before the package is complete.
        let grant = self.authorize(verified.clone(), mh, Vec::new());

        let mut files = Vec::new();
        let mut all_small = true;
        for file in verified.body.files() {
            let (hash, size) = pinned(file)?;
            let got = self.fetch(peers, ArtifactKey::Content(hash)).await?;
            if got.descriptor.total_size != size {
                return Err(VerifyError::HashMismatch {
                    path: file.path.clone(),
                    expected: format!("{size} bytes"),
                    actual: format!("{} bytes", got.descriptor.total_size),
                }
                .into());
            }
            all_small &= size <= self.config().materialize_limit;
            files.push((file.path.clone(), got.id));
        }
        if all_small {
            // Everything is held whole: run the full wave-1 verification too.
            verify_stored(
                self.store(),
                &hex_encode(&mh),
                anchors,
                &VerifyPolicy::default(),
            )?;
        }
        Ok(ExchangedPackage { files, ..grant })
    }

    fn authorize(
        &self,
        verified: VerifiedPackage,
        manifest_hash: [u8; 32],
        files: Vec<(String, ArtifactId)>,
    ) -> ExchangedPackage {
        self.grant(manifest_hash, &verified.package_id);
        for file in verified.body.files() {
            if let Ok((hash, _)) = pinned(file) {
                self.grant(hash, &verified.package_id);
            }
        }
        ExchangedPackage {
            package_id: verified.package_id.clone(),
            manifest_hash: hex_encode(&manifest_hash),
            files,
            verified,
        }
    }
}
