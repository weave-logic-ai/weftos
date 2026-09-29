//! The governance pin for the vocabulary file (ADR-099 decision 6).
//!
//! `config/capabilities.toml` changes only through the governance path. The
//! mechanism has three parts:
//!
//! 1. A **pin** ([`VocabularyPin`], committed as
//!    `config/capabilities.pin.toml`) names the one approved digest and
//!    version. [`Vocabulary::from_toml_pinned`] refuses any other file, so a
//!    free edit to the vocabulary no longer loads (and the committed-file
//!    tests fail).
//! 2. A **change record** ([`Vocabulary::change_to`]) describes a proposed
//!    change; `meta.version` must increase and the content must differ.
//! 3. The **gate and chain event** live in the kernel
//!    (`clawft_kernel::placement_vocabulary::VocabularyGovernor`): the change
//!    is checked as the existing governed `config.set` action and, when
//!    permitted, written to ExoChain; only then is a new pin issued.
//!
//! This module is pure data: no I/O.

use serde::{Deserialize, Serialize};

use super::vocabulary::{MAX_VOCAB_BYTES, Vocabulary, VocabularyError, digest_of};

/// Config namespace used for the governed `config.set` action and chain event.
pub const VOCABULARY_CONFIG_NAMESPACE: &str = "placement";
/// Config key used for the governed `config.set` action and chain event.
pub const VOCABULARY_CONFIG_KEY: &str = "capabilities.toml";

/// Largest pin file accepted, in bytes.
pub const MAX_PIN_BYTES: usize = 4 * 1024;

/// The governance-approved digest and version of the vocabulary file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VocabularyPin {
    /// Approved `meta.version`.
    pub version: u32,
    /// Approved SHA-256 hex digest of the exact file bytes (64 lowercase hex).
    pub digest: String,
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl VocabularyPin {
    /// Parse and validate a pin file.
    pub fn from_toml_str(text: &str) -> Result<Self, VocabularyError> {
        if text.len() > MAX_PIN_BYTES {
            return Err(VocabularyError::TooLarge(text.len()));
        }
        let pin: VocabularyPin =
            toml::from_str(text).map_err(|e| VocabularyError::Parse(e.to_string()))?;
        if !is_sha256_hex(&pin.digest) {
            return Err(VocabularyError::Invalid {
                entry: "digest".into(),
                reason: "must be 64 lowercase hex characters".into(),
            });
        }
        Ok(pin)
    }

    /// The pin for an already loaded vocabulary.
    pub fn of(v: &Vocabulary) -> Self {
        Self {
            version: v.meta().version,
            digest: v.digest().to_string(),
        }
    }

    /// Serialize as the committed pin file, with its header comment.
    pub fn to_toml_string(&self) -> String {
        format!(
            "# Governance pin for config/capabilities.toml (ADR-099 decision 6).\n\
             # Issued only by an approved `config.set` on {VOCABULARY_CONFIG_NAMESPACE}/\
             {VOCABULARY_CONFIG_KEY} (clawft_kernel::placement_vocabulary).\n\
             # Do not edit by hand: a vocabulary whose digest differs does not load.\n\
             version = {}\ndigest = \"{}\"\n",
            self.version, self.digest
        )
    }
}

/// A governed change from one vocabulary to another, carried in the
/// governance request context and its chain event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VocabularyChange {
    /// Current digest.
    pub from_digest: String,
    /// Proposed digest.
    pub to_digest: String,
    /// Current version.
    pub from_version: u32,
    /// Proposed version.
    pub to_version: u32,
}

impl Vocabulary {
    /// Parse, refusing a file whose digest or version is not the pinned one.
    pub fn from_toml_pinned(text: &str, pin: &VocabularyPin) -> Result<Self, VocabularyError> {
        if text.len() > MAX_VOCAB_BYTES {
            return Err(VocabularyError::TooLarge(text.len()));
        }
        let actual = digest_of(text);
        if actual != pin.digest {
            return Err(VocabularyError::DigestMismatch {
                expected: pin.digest.clone(),
                actual,
            });
        }
        let v = Self::from_toml_str(text)?;
        if v.meta().version != pin.version {
            return Err(VocabularyError::Invalid {
                entry: "meta.version".into(),
                reason: format!(
                    "file says {} but the pin says {}",
                    v.meta().version,
                    pin.version
                ),
            });
        }
        Ok(v)
    }

    /// Describe the change to `next`: the version must increase and the
    /// content must differ.
    pub fn change_to(&self, next: &Vocabulary) -> Result<VocabularyChange, VocabularyError> {
        if next.meta().version <= self.meta().version {
            return Err(VocabularyError::VersionNotIncreased {
                from: self.meta().version,
                to: next.meta().version,
            });
        }
        if next.digest() == self.digest() {
            return Err(VocabularyError::Invalid {
                entry: "digest".into(),
                reason: "proposed vocabulary is identical to the current one".into(),
            });
        }
        Ok(VocabularyChange {
            from_digest: self.digest().to_string(),
            to_digest: next.digest().to_string(),
            from_version: self.meta().version,
            to_version: next.meta().version,
        })
    }
}
