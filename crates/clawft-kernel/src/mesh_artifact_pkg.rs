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
//!   each listed file that it holds verified (pieces assembled to the
//!   pinned hash here); a peer's descriptor alone never makes content
//!   servable.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::mesh_artifact::{ArtifactExchange, ExchangeError};
use crate::mesh_artifact_transfer::{FetchError, PeerSet};
use crate::mesh_artifact_wire::{ArtifactId, ArtifactKey};
use crate::workload_kind::KindRegistry;
use crate::workload_pkg::codec::{hex_decode_exact, hex_encode};
use crate::workload_pkg::manifest::{FileRef, MANIFEST_FILE, MAX_MANIFEST_BYTES};
use crate::workload_pkg::verify::{
    VerifiedPackage, VerifyError, read_bounded, verify_manifest_signatures,
    verify_manifest_signatures_in,
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

/// Lower-case hex public keys of the manifest's accepted signers, looked up
/// in `anchors` (a signer revocation names the key, not the key id).
pub(crate) fn signer_keys(verified: &VerifiedPackage, anchors: &TrustAnchors) -> Vec<String> {
    verified
        .signers
        .iter()
        .filter_map(|s| {
            anchors
                .signers
                .iter()
                .chain(anchors.cognitum.iter())
                .find(|k| k.key_id == s.key_id)
        })
        .map(|k| hex_encode(&k.public_key))
        .collect()
}

/// May this package be handed to other nodes? No when it carries Cognitum
/// provenance (a release-record attestation, or a `cognitum` release URL):
/// those cogs are licence-gated, and nothing here checks a licence or that a
/// peer belongs to the same operator, so they are never seeded or advertised.
pub(crate) fn redistributable(verified: &VerifiedPackage) -> bool {
    let cognitum_record = verified
        .body
        .attestations
        .iter()
        .any(|a| a.kind.to_ascii_lowercase().starts_with("cognitum."));
    let cognitum_url = verified
        .body
        .source
        .release_url
        .as_deref()
        .is_some_and(|u| u.to_ascii_lowercase().contains("cognitum"));
    !(cognitum_record || cognitum_url)
}

pub(crate) fn pinned(file: &FileRef) -> Result<([u8; 32], u64), VerifyError> {
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
        self.seed_package_in(manifest, anchors, &KindRegistry::builtin(), open)
    }

    /// [`Self::seed_package`] against a caller-supplied kind registry.
    pub fn seed_package_in(
        &self,
        manifest: &[u8],
        anchors: &TrustAnchors,
        kinds: &KindRegistry,
        open: &mut dyn FnMut(&FileRef) -> std::io::Result<Box<dyn Read>>,
    ) -> Result<ExchangedPackage, PackageExchangeError> {
        let verified = verify_manifest_signatures_in(manifest, anchors, kinds)?;
        self.refuse_if_revoked(&verified, anchors, blake3::hash(manifest).as_bytes())?;
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
        Ok(self.authorize(verified, m.content_hash, files, anchors))
    }

    /// [`Self::seed_package`] from an unpacked package directory.
    pub fn seed_package_dir(
        &self,
        dir: &Path,
        anchors: &TrustAnchors,
    ) -> Result<ExchangedPackage, PackageExchangeError> {
        self.seed_package_dir_in(dir, anchors, &KindRegistry::builtin())
    }

    /// [`Self::seed_package_dir`] against a caller-supplied kind registry.
    pub fn seed_package_dir_in(
        &self,
        dir: &Path,
        anchors: &TrustAnchors,
        kinds: &KindRegistry,
    ) -> Result<ExchangedPackage, PackageExchangeError> {
        let manifest = read_bounded(
            &dir.join(MANIFEST_FILE),
            MANIFEST_FILE,
            MAX_MANIFEST_BYTES as u64,
        )?;
        let root = dir
            .canonicalize()
            .map_err(|e| ExchangeError::Io(e.to_string()))?;
        self.seed_package_in(&manifest, anchors, kinds, &mut |file| {
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
    /// and hash. Once the manifest verifies, this node may serve the listed
    /// files it holds verified.
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
        self.refuse_if_revoked(&verified, anchors, &mh)?;
        // The manifest verified: its pinned content may be served from here
        // on, once each file is verified on this node.
        let grant = self.authorize(verified.clone(), mh, Vec::new(), anchors);

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

    /// Refuse a package whose id, signers or manifest hash is revoked.
    pub(crate) fn refuse_if_revoked(
        &self,
        verified: &VerifiedPackage,
        anchors: &TrustAnchors,
        manifest_hash: &[u8; 32],
    ) -> Result<(), ExchangeError> {
        let signers = signer_keys(verified, anchors);
        if self.is_revoked_subject(&verified.package_id, &signers, manifest_hash) {
            return Err(ExchangeError::Revoked(format!(
                "package {} (or its signer or manifest) is revoked",
                verified.package_id
            )));
        }
        Ok(())
    }

    pub(crate) fn authorize(
        &self,
        verified: VerifiedPackage,
        manifest_hash: [u8; 32],
        files: Vec<(String, ArtifactId)>,
        anchors: &TrustAnchors,
    ) -> ExchangedPackage {
        let signers = signer_keys(&verified, anchors);
        let open = redistributable(&verified);
        self.grant_with(manifest_hash, &verified.package_id, signers.clone(), open);
        for file in verified.body.files() {
            if let Ok((hash, _)) = pinned(file) {
                self.grant_with(hash, &verified.package_id, signers.clone(), open);
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
