//! Package verification: manifest frame, signatures against pinned anchors,
//! then every listed file by size and BLAKE3.
//!
//! Signatures are checked before any file is read, so an unsigned or
//! mis-signed package never causes file I/O beyond the manifest itself.
//! Each failure class has its own [`VerifyError`] variant and stable
//! [`VerifyError::code`].

use std::io::Read;
use std::path::{Path, PathBuf};

use ed25519_dalek::{Signature, VerifyingKey};

use crate::workload_kind::{KindRegistry, validate_envelope};

use super::codec::hex_decode_exact;
use super::cognitum;
use super::manifest::{
    CogPackageBody, FileRef, MANIFEST_FILE, MAX_MANIFEST_BYTES, ManifestEnvelope, ManifestError,
};
use super::trust::{KeyOrigin, TrustAnchors};

/// Largest single package file read during verification.
pub const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;

/// Why a package was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VerifyError {
    /// Manifest unreadable or structurally invalid.
    #[error("manifest invalid: {0}")]
    Manifest(String),
    /// No signature entries and no accepted external attestation.
    #[error("package has no signature")]
    MissingSignature,
    /// Signatures exist but none is from a pinned signer.
    #[error("no signature from a pinned signer (saw: {key_ids})")]
    UntrustedSigner {
        /// Key ids seen on the envelope.
        key_ids: String,
    },
    /// A pinned signer's signature does not verify (manifest tampered or
    /// key id / key mismatch).
    #[error("signature from {key_id} does not verify")]
    BadSignature {
        /// The offending key id.
        key_id: String,
    },
    /// A listed file is absent.
    #[error("package file missing: {path}")]
    FileMissing {
        /// Package-relative path.
        path: String,
    },
    /// A listed file's content does not match its pinned hash or size.
    #[error("package file tampered: {path} (expected {expected}, got {actual})")]
    HashMismatch {
        /// Package-relative path.
        path: String,
        /// Pinned value.
        expected: String,
        /// Observed value.
        actual: String,
    },
    /// The Cognitum release record was supplied and rejected.
    #[error("cognitum release record rejected: {0}")]
    CognitumRecord(String),
    /// I/O failure other than a missing file.
    #[error("io error on {path}: {msg}")]
    Io {
        /// Path involved.
        path: String,
        /// Error text.
        msg: String,
    },
}

impl VerifyError {
    /// Stable machine-readable code for CLI output and audit payloads.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Manifest(_) => "manifest-invalid",
            Self::MissingSignature => "missing-signature",
            Self::UntrustedSigner { .. } => "untrusted-signer",
            Self::BadSignature { .. } => "bad-signature",
            Self::FileMissing { .. } => "file-missing",
            Self::HashMismatch { .. } => "file-tampered",
            Self::CognitumRecord(_) => "cognitum-record-rejected",
            Self::Io { .. } => "io-error",
        }
    }
}

impl From<ManifestError> for VerifyError {
    fn from(e: ManifestError) -> Self {
        Self::Manifest(e.to_string())
    }
}

/// Verification options.
#[derive(Debug, Clone, Default)]
pub struct VerifyPolicy {
    /// Opt in to the Cognitum ADR-154/155 release-record verifier. Off by
    /// default; when on, a valid record from a pinned Cognitum key that
    /// binds one of the package binaries counts as a signature.
    pub accept_cognitum_release: bool,
}

/// A signer whose signature was accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedSigner {
    /// Key id.
    pub key_id: String,
    /// Anchor set it came from.
    pub origin: KeyOrigin,
}

/// A package that passed verification.
#[derive(Debug, Clone)]
pub struct VerifiedPackage {
    /// BLAKE3 of the signed statement.
    pub package_id: String,
    /// The envelope as read.
    pub envelope: ManifestEnvelope,
    /// Typed cog body.
    pub body: CogPackageBody,
    /// Accepted signers (at least one).
    pub signers: Vec<AcceptedSigner>,
}

/// Where package files are read from.
pub trait FileSource {
    /// Read `file` (the source may use its path or its hash).
    fn read(&self, file: &FileRef) -> Result<Vec<u8>, VerifyError>;
}

/// Reads files from an unpacked package directory.
pub struct DirSource {
    root: PathBuf,
}

impl DirSource {
    /// Source rooted at `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl FileSource for DirSource {
    fn read(&self, file: &FileRef) -> Result<Vec<u8>, VerifyError> {
        // Paths were validated as relative and traversal-free; also refuse a
        // symlinked directory that would resolve outside the package.
        let path = self.root.join(&file.path);
        if let (Ok(root), Ok(real)) = (self.root.canonicalize(), path.canonicalize())
            && !real.starts_with(&root)
        {
            return Err(VerifyError::Io {
                path: file.path.clone(),
                msg: "resolves outside the package".into(),
            });
        }
        read_bounded(&path, &file.path, MAX_FILE_BYTES)
    }
}

pub(crate) fn read_bounded(path: &Path, label: &str, max: u64) -> Result<Vec<u8>, VerifyError> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| io_or_missing(e, label))?;
    if !meta.is_file() {
        return Err(VerifyError::Io {
            path: label.into(),
            msg: "not a regular file".into(),
        });
    }
    if meta.len() > max {
        return Err(VerifyError::Io {
            path: label.into(),
            msg: format!("larger than {max} bytes"),
        });
    }
    // Bounded read: the file may grow between the metadata check and here.
    let mut out = Vec::new();
    std::fs::File::open(path)
        .map_err(|e| io_or_missing(e, label))?
        .take(max + 1)
        .read_to_end(&mut out)
        .map_err(|e| io_or_missing(e, label))?;
    if out.len() as u64 > max {
        return Err(VerifyError::Io {
            path: label.into(),
            msg: format!("larger than {max} bytes"),
        });
    }
    Ok(out)
}

fn io_or_missing(e: std::io::Error, label: &str) -> VerifyError {
    if e.kind() == std::io::ErrorKind::NotFound {
        VerifyError::FileMissing { path: label.into() }
    } else {
        VerifyError::Io {
            path: label.into(),
            msg: e.to_string(),
        }
    }
}

/// Verify the package in directory `dir` (manifest at `dir/cogpkg.json`).
pub fn verify_dir(
    dir: &Path,
    anchors: &TrustAnchors,
    policy: &VerifyPolicy,
) -> Result<VerifiedPackage, VerifyError> {
    verify_dir_in(dir, anchors, policy, &KindRegistry::builtin())
}

/// [`verify_dir`] against a caller-supplied kind registry.
pub fn verify_dir_in(
    dir: &Path,
    anchors: &TrustAnchors,
    policy: &VerifyPolicy,
    kinds: &KindRegistry,
) -> Result<VerifiedPackage, VerifyError> {
    let bytes = read_bounded(
        &dir.join(MANIFEST_FILE),
        MANIFEST_FILE,
        MAX_MANIFEST_BYTES as u64,
    )?;
    verify_with_source_in(&bytes, &DirSource::new(dir), anchors, policy, kinds)
}

/// Verify manifest bytes, reading listed files through `source`.
pub fn verify_with_source(
    manifest: &[u8],
    source: &dyn FileSource,
    anchors: &TrustAnchors,
    policy: &VerifyPolicy,
) -> Result<VerifiedPackage, VerifyError> {
    verify_with_source_in(manifest, source, anchors, policy, &KindRegistry::builtin())
}

/// [`verify_with_source`] against a caller-supplied kind registry.
pub fn verify_with_source_in(
    manifest: &[u8],
    source: &dyn FileSource,
    anchors: &TrustAnchors,
    policy: &VerifyPolicy,
    kinds: &KindRegistry,
) -> Result<VerifiedPackage, VerifyError> {
    let (envelope, body, mut signers) = parse_and_check(manifest, anchors, kinds)?;

    if signers.is_empty()
        && policy.accept_cognitum_release
        && let Some(signer) = cognitum::accept_from_package(&body, source, anchors)?
    {
        signers.push(signer);
    }
    if signers.is_empty() {
        return Err(no_signer_error(&envelope));
    }

    for file in body.files() {
        check_file(file, &source.read(file)?)?;
    }
    Ok(VerifiedPackage {
        package_id: envelope.package_id()?,
        envelope,
        body,
        signers,
    })
}

/// Verify the manifest envelope and its signatures without reading any
/// listed file (mesh-placement-11).
///
/// Used where file content is proven some other way: the artifact
/// exchange checks every file's size and BLAKE3 while streaming it, so a
/// multi-GB payload never has to sit in memory. Only pinned Ed25519
/// signers count here; the Cognitum release-record path needs a binary
/// and is only available through [`verify_with_source`].
pub fn verify_manifest_signatures(
    manifest: &[u8],
    anchors: &TrustAnchors,
) -> Result<VerifiedPackage, VerifyError> {
    verify_manifest_signatures_in(manifest, anchors, &KindRegistry::builtin())
}

/// [`verify_manifest_signatures`] against a caller-supplied kind registry.
pub fn verify_manifest_signatures_in(
    manifest: &[u8],
    anchors: &TrustAnchors,
    kinds: &KindRegistry,
) -> Result<VerifiedPackage, VerifyError> {
    let (envelope, body, signers) = parse_and_check(manifest, anchors, kinds)?;
    if signers.is_empty() {
        return Err(no_signer_error(&envelope));
    }
    Ok(VerifiedPackage {
        package_id: envelope.package_id()?,
        envelope,
        body,
        signers,
    })
}

type Checked = (ManifestEnvelope, CogPackageBody, Vec<AcceptedSigner>);

fn parse_and_check(
    manifest: &[u8],
    anchors: &TrustAnchors,
    kinds: &KindRegistry,
) -> Result<Checked, VerifyError> {
    let envelope = ManifestEnvelope::from_bytes(manifest)?;
    // The kind must be registered and accept the body. The verified package
    // is still cog-shaped until a kind brings its own verified body, so a
    // registered kind must also parse as a cog body.
    validate_envelope(kinds, &envelope)?;
    let body = envelope.cog_shaped_body()?;
    let signers = check_signatures(&envelope, anchors)?;
    Ok((envelope, body, signers))
}

fn no_signer_error(envelope: &ManifestEnvelope) -> VerifyError {
    if envelope.signatures.is_empty() {
        VerifyError::MissingSignature
    } else {
        let key_ids: Vec<&str> = envelope
            .signatures
            .iter()
            .map(|s| s.key_id.as_str())
            .collect();
        VerifyError::UntrustedSigner {
            key_ids: key_ids.join(","),
        }
    }
}

/// Check one file's size and hash.
pub fn check_file(file: &FileRef, content: &[u8]) -> Result<(), VerifyError> {
    let actual = blake3::hash(content).to_hex().to_string();
    if actual != file.blake3 {
        return Err(VerifyError::HashMismatch {
            path: file.path.clone(),
            expected: file.blake3.clone(),
            actual,
        });
    }
    if content.len() as u64 != file.size {
        return Err(VerifyError::HashMismatch {
            path: file.path.clone(),
            expected: format!("{} bytes", file.size),
            actual: format!("{} bytes", content.len()),
        });
    }
    Ok(())
}

/// Returns accepted pinned signers. Any signature from a pinned key that
/// fails is fatal ([`VerifyError::BadSignature`]); entries from unpinned
/// keys are ignored here and only reported if nothing else is accepted.
fn check_signatures(
    env: &ManifestEnvelope,
    anchors: &TrustAnchors,
) -> Result<Vec<AcceptedSigner>, VerifyError> {
    let msg = env.signed_statement()?;
    let mut accepted = Vec::new();
    for entry in &env.signatures {
        if entry.algorithm != "ed25519" {
            return Err(VerifyError::Manifest(format!(
                "unsupported algorithm {:?}",
                entry.algorithm
            )));
        }
        let pk = hex_decode_exact::<32>(&entry.public_key).ok_or_else(|| {
            VerifyError::Manifest(format!("{}: bad public key encoding", entry.key_id))
        })?;
        let Some(pinned) = anchors.signer(&pk) else {
            continue;
        };
        let bad = || VerifyError::BadSignature {
            key_id: entry.key_id.clone(),
        };
        if pinned.key_id != entry.key_id {
            return Err(bad());
        }
        let sig = hex_decode_exact::<64>(&entry.signature).ok_or_else(bad)?;
        let vk = VerifyingKey::from_bytes(&pk).map_err(|_| bad())?;
        vk.verify_strict(&msg, &Signature::from_bytes(&sig))
            .map_err(|_| bad())?;
        if !accepted
            .iter()
            .any(|a: &AcceptedSigner| a.key_id == pinned.key_id)
        {
            accepted.push(AcceptedSigner {
                key_id: pinned.key_id.clone(),
                origin: pinned.origin,
            });
        }
    }
    Ok(accepted)
}
