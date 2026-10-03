//! The `model` manifest body and operator attestation (ADR-101 section 3).
//!
//! Upstream rarely signs weights, so trust is an operator attestation over a
//! hash manifest: the operator hashes the shards where they lie, signs the
//! body with a key pinned in [`TrustAnchors`], and a shard whose hash later
//! differs is refused. The envelope, signing domain and signature check are
//! the ones signed workload packages use ([`crate::workload_pkg`]); only the
//! body differs.

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::workload_pkg::codec::is_lower_hex;
use crate::workload_pkg::manifest::{valid_token, validate_rel_path};
use crate::workload_pkg::verify::{AcceptedSigner, VerifyError, check_signatures};
use crate::workload_pkg::{
    FileRef, ManifestEnvelope, ManifestError, TrustAnchors, sign_envelope,
};

/// Workload kind string of a model manifest.
pub const KIND_MODEL: &str = "model";
/// Most shard files one model may list. One `model.present` capability
/// carries every shard hash plus a model marker in a list attribute, which
/// is bounded at 256 entries.
pub const MAX_SHARDS: usize = 255;
/// Prefix of the identity marker entry in the `model.present` `shards` list.
pub const MODEL_MARKER_PREFIX: &str = "model:";

/// Weight file layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelFormat {
    /// MLX quantisation directory (safetensors plus config).
    Mlx,
    /// One or more GGUF files.
    Gguf,
    /// Plain safetensors shards (an HF snapshot).
    Safetensors,
    /// Ollama blobs (GGUF content addressed by the Ollama store).
    Ollama,
}

impl ModelFormat {
    /// Wire name, also the `format.*` capability suffix.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mlx => "mlx",
            Self::Gguf => "gguf",
            Self::Safetensors => "safetensors",
            Self::Ollama => "ollama",
        }
    }
}

/// Where the weights came from. Provenance, never trust.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSource {
    /// Hugging Face repo id (`org/name`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hf_repo: Option<String>,
    /// Hugging Face revision (commit hash or ref).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hf_revision: Option<String>,
    /// Ollama tag (`name:tag`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ollama_tag: Option<String>,
}

impl ModelSource {
    fn validate(&self) -> Result<(), ManifestError> {
        if self.hf_repo.is_none() && self.ollama_tag.is_none() {
            return Err(invalid("source needs an hf_repo or an ollama_tag"));
        }
        if self.hf_revision.is_some() && self.hf_repo.is_none() {
            return Err(invalid("hf_revision needs an hf_repo"));
        }
        for s in [&self.hf_repo, &self.hf_revision, &self.ollama_tag]
            .into_iter()
            .flatten()
        {
            if s.is_empty() || s.len() > 256 || s.chars().any(|c| c.is_control() || c == ' ') {
                return Err(invalid("source strings must be 1..256 printable chars"));
            }
        }
        Ok(())
    }
}

/// Body of a `kind = "model"` manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelPackageBody {
    /// Model name (`Qwen3-Coder-Next-4bit`); the `model` parameter of
    /// `perf.infer.tok_s`.
    pub name: String,
    /// Layout.
    pub format: ModelFormat,
    /// Weight shards, relative to the model root, sorted by path.
    pub shards: Vec<FileRef>,
    /// BLAKE3 of the tokenizer file, when the model has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokenizer_blake3: Option<String>,
    /// BLAKE3 of the chat template (or the config that carries it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_blake3: Option<String>,
    /// Provenance.
    pub source: ModelSource,
    /// The signer states the weights may be handed to other nodes. Absent
    /// means false: weights are never shared unless explicitly opted in. It
    /// is part of the signed statement.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub redistributable: bool,
}

fn invalid(msg: impl Into<String>) -> ManifestError {
    ManifestError::Invalid(msg.into())
}

impl ModelPackageBody {
    /// Validate names, paths, hashes and bounds.
    pub fn validate(&self) -> Result<(), ManifestError> {
        if !valid_token(&self.name, 64) {
            return Err(invalid(format!("bad model name {:?}", self.name)));
        }
        if self.shards.is_empty() || self.shards.len() > MAX_SHARDS {
            return Err(invalid(format!("a model lists 1..={MAX_SHARDS} shards")));
        }
        for s in &self.shards {
            validate_rel_path(&s.path)?;
            if !is_lower_hex(&s.blake3, 64) {
                return Err(invalid(format!("{}: blake3 must be 64 lower-case hex", s.path)));
            }
        }
        if self.shards.windows(2).any(|w| w[0].path >= w[1].path) {
            return Err(invalid("shards must be sorted by path with no duplicates"));
        }
        for h in [&self.tokenizer_blake3, &self.template_blake3].into_iter().flatten() {
            if !is_lower_hex(h, 64) {
                return Err(invalid("tokenizer/template hashes are 64 lower-case hex"));
            }
        }
        self.source.validate()
    }

    /// Sum of shard sizes.
    pub fn total_bytes(&self) -> u64 {
        self.shards.iter().map(|s| s.size).fold(0, u64::saturating_add)
    }

    /// Shard hashes in manifest order.
    pub fn shard_hashes(&self) -> impl Iterator<Item = &str> {
        self.shards.iter().map(|s| s.blake3.as_str())
    }
}

/// A model manifest that passed signature verification.
#[derive(Debug, Clone)]
pub struct VerifiedModel {
    /// BLAKE3 of the signed statement (the model's identity).
    pub package_id: String,
    /// The envelope as read.
    pub envelope: ManifestEnvelope,
    /// Typed body.
    pub body: ModelPackageBody,
    /// Accepted signers (at least one).
    pub signers: Vec<AcceptedSigner>,
}

/// Model manifest failure.
#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    /// Manifest shape.
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    /// Signature check.
    #[error(transparent)]
    Verify(#[from] VerifyError),
    /// Filesystem failure.
    #[error("{path}: {msg}")]
    Io {
        /// Path involved.
        path: String,
        /// Error text.
        msg: String,
    },
    /// A file's bytes differ from the attested hash. The model is refused.
    #[error("{path}: hash mismatch (expected {expected}, found {actual})")]
    HashMismatch {
        /// Relative path.
        path: String,
        /// Attested hash or size.
        expected: String,
        /// Found hash or size.
        actual: String,
    },
    /// Another model already holds this name with a different manifest.
    #[error("model name {0:?} is already adopted with a different manifest")]
    NameConflict(String),
    /// Nothing adopted under this id or name.
    #[error("no adopted model {0:?}")]
    Unknown(String),
    /// The model cannot be used in its current state.
    #[error("model {id} is not ready: {reason}")]
    NotReady {
        /// Package id.
        id: String,
        /// Why.
        reason: String,
    },
    /// Registry persistence or input problem.
    #[error("model registry: {0}")]
    Registry(String),
    /// Nothing recognisable as model weights under the path.
    #[error("{0}")]
    Scan(String),
}

/// Operator attestation: wrap `body` in a `model` envelope and sign it with
/// the operator key. The attestation is only useful if `key_id`'s public key
/// is pinned in the node's [`TrustAnchors`].
pub fn attest(
    body: &ModelPackageBody,
    key: &ed25519_dalek::SigningKey,
    key_id: &str,
) -> Result<ManifestEnvelope, ModelError> {
    body.validate()?;
    let value = serde_json::to_value(body).map_err(|e| ManifestError::Parse(e.to_string()))?;
    let mut env = ManifestEnvelope::new(KIND_MODEL, json!(value));
    sign_envelope(&mut env, key, key_id)?;
    Ok(env)
}

/// Verify a model envelope: kind, body, and at least one signature from a
/// pinned signer. File contents are checked separately (see the registry),
/// because weights are too large to read here.
pub fn verify_model(
    envelope: &ManifestEnvelope,
    anchors: &TrustAnchors,
) -> Result<VerifiedModel, ModelError> {
    if envelope.kind != KIND_MODEL {
        return Err(invalid(format!("kind {:?} is not a model", envelope.kind)).into());
    }
    let body: ModelPackageBody = serde_json::from_value(envelope.body.clone())
        .map_err(|e| ManifestError::Parse(format!("model body: {e}")))?;
    body.validate()?;
    let signers = check_signatures(envelope, anchors)?;
    if signers.is_empty() {
        return Err(if envelope.signatures.is_empty() {
            VerifyError::MissingSignature
        } else {
            VerifyError::UntrustedSigner {
                key_ids: envelope
                    .signatures
                    .iter()
                    .map(|s| s.key_id.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
            }
        }
        .into());
    }
    Ok(VerifiedModel {
        package_id: envelope.package_id()?,
        envelope: envelope.clone(),
        body,
        signers,
    })
}
