//! The local model registry: which attested models are adopted on this node,
//! where their files lie, and whether they are usable right now.
//!
//! Adoption never copies. The registry stores the signed envelope and the
//! absolute root (local only; never advertised), plus a `(size, mtime)`
//! stamp per file recorded when its hash was last confirmed. Re-hashing is
//! lazy: [`CheckMode::Lazy`] re-hashes a file only when its stamp changed or
//! was never recorded, so a 45 GB shard is hashed once, not on every start.
//! A file whose hash differs from the attestation refuses the model until an
//! operator-run full check passes again.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use super::adopt::{FileRole, ScannedModel, io_err};
use super::resolve::{consistency, files_of_body};
use super::check::{CheckMode, ModelCheck, ModelState, check_entry};
use super::body::{ModelError, ModelFormat, ModelPackageBody, VerifiedModel, attest, verify_model};
use crate::revocation::RevocationList;
use crate::workload_pkg::{ManifestEnvelope, TrustAnchors};

/// Registry file schema id.
pub const REGISTRY_SCHEMA: &str = "weftos.model-registry.v1";
/// Largest registry file read.
pub const MAX_REGISTRY_BYTES: u64 = 16 * 1024 * 1024;

/// A file of an adopted model and when its hash was last confirmed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalFile {
    /// Path relative to the model root.
    pub path: String,
    /// Role.
    pub role: FileRole,
    /// Attested size.
    pub size: u64,
    /// Attested BLAKE3.
    pub blake3: String,
    /// `(size, mtime_ns)` when the hash was last confirmed on this node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified: Option<(u64, u64)>,
}

/// One adopted model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelEntry {
    /// The signed attestation.
    pub envelope: ManifestEnvelope,
    /// Canonical root directory (local only).
    pub root: PathBuf,
    /// Files with their verification stamps.
    pub files: Vec<LocalFile>,
    /// Set when a file failed its hash: the model is refused until a full
    /// check passes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
    /// Local opt-out: when true the model is not advertised in node facts
    /// (shard hashes fingerprint which models a node holds). Not signed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

/// What a node trusts when it lets a model be used: pinned signers and the
/// revocation list. Held by the registry; replaced with
/// [`ModelRegistry::set_trust`] when keys or revocations change.
#[derive(Clone)]
pub struct ModelTrust {
    /// Pinned signer keys.
    pub anchors: TrustAnchors,
    /// Revocation list consulted for the package, its signers and shard hashes.
    pub revocations: Option<Arc<RevocationList>>,
}

impl std::fmt::Debug for ModelTrust {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelTrust")
            .field("signers", &self.anchors.signers.len())
            .field("revocations", &self.revocations.is_some())
            .finish()
    }
}

impl ModelTrust {
    /// Anchors with no revocation list.
    pub fn new(anchors: TrustAnchors) -> Self {
        Self { anchors, revocations: None }
    }
}

impl ModelEntry {
    /// The typed body (the envelope was verified before insertion).
    pub fn body(&self) -> Result<ModelPackageBody, ModelError> {
        serde_json::from_value(self.envelope.body.clone())
            .map_err(|e| ModelError::Registry(format!("stored body: {e}")))
    }

    /// Package id of the attestation.
    pub fn package_id(&self) -> Result<String, ModelError> {
        Ok(self.envelope.package_id()?)
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub(super) struct RegistryFile {
    pub(super) schema: String,
    pub(super) models: BTreeMap<String, ModelEntry>,
}

/// A usable model, for an inference adapter to consume.
#[derive(Debug, Clone)]
pub struct ResolvedModel {
    /// Package id (the manifest hash adapters put in `ModelRef.manifest`).
    pub package_id: String,
    /// Model name.
    pub name: String,
    /// Layout.
    pub format: ModelFormat,
    /// Directory holding the files (what `--model` takes for MLX/HF).
    pub root: PathBuf,
    /// Absolute path of each shard, in manifest order.
    pub shards: Vec<PathBuf>,
    /// Absolute tokenizer path, when the model has one.
    pub tokenizer: Option<PathBuf>,
    /// The attested body.
    pub body: ModelPackageBody,
}

/// Result of [`ModelRegistry::adopt`] / [`ModelRegistry::attach`].
#[derive(Debug, Clone)]
pub struct AdoptedModel {
    /// Package id.
    pub package_id: String,
    /// The signed envelope.
    pub envelope: ManifestEnvelope,
    /// Model root.
    pub root: PathBuf,
    /// The verified model.
    pub verified: VerifiedModel,
}

/// Adopted models, persisted as JSON when opened on a path.
pub struct ModelRegistry {
    path: Option<PathBuf>,
    pub(super) inner: Mutex<RegistryFile>,
    pub(super) trust: Mutex<Option<ModelTrust>>,
    /// Models whose every byte was hashed in this process. Not persisted:
    /// the first [`ModelRegistry::resolve`] after a start does a full check.
    pub(super) fully_verified: Mutex<BTreeSet<String>>,
}

impl std::fmt::Debug for ModelRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelRegistry").field("path", &self.path).finish()
    }
}

impl ModelRegistry {
    /// A registry that is not persisted.
    pub fn in_memory() -> Self {
        Self {
            path: None,
            inner: Mutex::new(RegistryFile {
                schema: REGISTRY_SCHEMA.into(),
                models: BTreeMap::new(),
            }),
            trust: Mutex::new(None),
            fully_verified: Mutex::new(BTreeSet::new()),
        }
    }

    /// Set what [`resolve`](Self::resolve) verifies against.
    pub fn with_trust(self, trust: ModelTrust) -> Self {
        *self.trust.lock().unwrap_or_else(|p| p.into_inner()) = Some(trust);
        self
    }

    /// Replace the trust (a key was pinned or removed, a revocation list was
    /// swapped). Takes effect on the next resolve.
    pub fn set_trust(&self, trust: ModelTrust) {
        *self.trust.lock().unwrap_or_else(|p| p.into_inner()) = Some(trust);
    }

    /// Open (or start) the registry file at `path`.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, ModelError> {
        let path = path.into();
        let file = match std::fs::metadata(&path) {
            Ok(m) if m.len() > MAX_REGISTRY_BYTES => {
                return Err(ModelError::Registry("registry file too large".into()));
            }
            Ok(_) => {
                let bytes = std::fs::read(&path).map_err(|e| io_err(&path, e))?;
                let f: RegistryFile = serde_json::from_slice(&bytes)
                    .map_err(|e| ModelError::Registry(format!("{}: {e}", path.display())))?;
                if f.schema != REGISTRY_SCHEMA {
                    return Err(ModelError::Registry(format!("unknown schema {:?}", f.schema)));
                }
                f
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => RegistryFile {
                schema: REGISTRY_SCHEMA.into(),
                models: BTreeMap::new(),
            },
            Err(e) => return Err(io_err(&path, e)),
        };
        for (id, e) in &file.models {
            consistency(id, e).map_err(|why| {
                ModelError::Registry(format!("{}: entry {id} is inconsistent: {why}", path.display()))
            })?;
        }
        Ok(Self {
            path: Some(path),
            inner: Mutex::new(file),
            trust: Mutex::new(None),
            fully_verified: Mutex::new(BTreeSet::new()),
        })
    }

    pub(super) fn lock(&self) -> std::sync::MutexGuard<'_, RegistryFile> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub(super) fn save(&self, f: &RegistryFile) -> Result<(), ModelError> {
        let Some(path) = &self.path else { return Ok(()) };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| io_err(dir, e))?;
        }
        let bytes = serde_json::to_vec_pretty(f).map_err(|e| ModelError::Registry(e.to_string()))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes).map_err(|e| io_err(&tmp, e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| io_err(&tmp, e))?;
        }
        std::fs::rename(&tmp, path).map_err(|e| io_err(path, e))
    }

    fn insert(
        &self,
        verified: &VerifiedModel,
        root: PathBuf,
        files: Vec<LocalFile>,
        replace: bool,
    ) -> Result<AdoptedModel, ModelError> {
        let entry = ModelEntry {
            envelope: verified.envelope.clone(),
            root: root.clone(),
            files: files.clone(),
            refused: None,
            hidden: false,
        };
        consistency(&verified.package_id, &entry).map_err(ModelError::Registry)?;
        let mut g = self.lock();
        let name = &verified.body.name;
        let clash: Vec<String> = g
            .models
            .iter()
            .filter(|(id, e)| {
                **id != verified.package_id
                    && e.body().map(|b| &b.name == name).unwrap_or(false)
            })
            .map(|(id, _)| id.clone())
            .collect();
        if !clash.is_empty() && !replace {
            return Err(ModelError::NameConflict(name.clone()));
        }
        for id in clash {
            g.models.remove(&id);
        }
        g.models.insert(
            verified.package_id.clone(),
            ModelEntry {
                envelope: verified.envelope.clone(),
                root: root.clone(),
                files,
                refused: None,
                hidden: false,
            },
        );
        self.save(&g)?;
        Ok(AdoptedModel {
            package_id: verified.package_id.clone(),
            envelope: verified.envelope.clone(),
            root,
            verified: verified.clone(),
        })
    }

    /// Attest a scanned model with the operator key and register it. The
    /// key must be pinned in `anchors`, otherwise the attestation does not
    /// verify and nothing is registered. A different manifest under an
    /// existing name is refused unless `replace` is set.
    pub fn adopt(
        &self,
        scanned: ScannedModel,
        key: &ed25519_dalek::SigningKey,
        key_id: &str,
        anchors: &TrustAnchors,
        replace: bool,
    ) -> Result<AdoptedModel, ModelError> {
        let envelope = attest(&scanned.body, key, key_id)?;
        let verified = verify_model(&envelope, anchors)?;
        let files = scanned
            .files
            .iter()
            .map(|f| LocalFile {
                path: f.path.clone(),
                role: f.role,
                size: f.size,
                blake3: f.blake3.clone(),
                verified: Some((f.size, f.mtime_ns)),
            })
            .collect();
        let adopted = self.insert(&verified, scanned.root, files, replace)?;
        // The scan just hashed every byte in this process.
        self.fully_verified
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(adopted.package_id.clone());
        Ok(adopted)
    }

    /// Register an already-attested manifest (for example one fetched from
    /// another node) whose files lie under `root`. Its signature is checked
    /// against `anchors`; file hashes are checked lazily on first use.
    pub fn attach(
        &self,
        envelope: &ManifestEnvelope,
        root: &Path,
        anchors: &TrustAnchors,
        replace: bool,
    ) -> Result<AdoptedModel, ModelError> {
        let verified = verify_model(envelope, anchors)?;
        let root = root.canonicalize().map_err(|e| io_err(root, e))?;
        let files = files_of_body(&verified.body);
        self.insert(&verified, root, files, replace)
    }

    /// Forget a model (files are untouched).
    pub fn remove(&self, id_or_name: &str) -> Result<(), ModelError> {
        let mut g = self.lock();
        let id = Self::find(&g, id_or_name)?;
        g.models.remove(&id);
        self.save(&g)
    }

    pub(super) fn find(g: &RegistryFile, id_or_name: &str) -> Result<String, ModelError> {
        if g.models.contains_key(id_or_name) {
            return Ok(id_or_name.to_string());
        }
        g.models
            .iter()
            .find(|(_, e)| e.body().map(|b| b.name == id_or_name).unwrap_or(false))
            .map(|(id, _)| id.clone())
            .ok_or_else(|| ModelError::Unknown(id_or_name.chars().take(80).collect()))
    }

    /// Snapshot of every entry, keyed by package id.
    pub fn entries(&self) -> Vec<(String, ModelEntry)> {
        self.lock().models.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }

    /// One entry by package id or name.
    pub fn get(&self, id_or_name: &str) -> Result<(String, ModelEntry), ModelError> {
        let g = self.lock();
        let id = Self::find(&g, id_or_name)?;
        Ok((id.clone(), g.models[&id].clone()))
    }

    /// Check a model's files.
    pub fn check(&self, id_or_name: &str, mode: CheckMode) -> Result<ModelCheck, ModelError> {
        let (id, entry) = self.get(id_or_name)?;
        let (check, stamps) = check_entry(&id, &entry, mode)?;
        if mode != CheckMode::Stat {
            let mut g = self.lock();
            if let Some(e) = g.models.get_mut(&id) {
                let mut changed = false;
                for (f, s) in e.files.iter_mut().zip(stamps) {
                    if f.verified != s {
                        f.verified = s;
                        changed = true;
                    }
                }
                match &check.state {
                    ModelState::Refused { reason } if e.refused.as_deref() != Some(reason) => {
                        e.refused = Some(reason.clone());
                        changed = true;
                    }
                    ModelState::Ready if mode == CheckMode::Full && e.refused.is_some() => {
                        e.refused = None;
                        changed = true;
                    }
                    _ => {}
                }
                if changed {
                    self.save(&g)?;
                }
            }
        }
        Ok(check)
    }
}
