//! The governance pin for the vocabulary file (ADR-099 decision 6).
//!
//! `config/capabilities.toml` changes only through the governance path. The
//! mechanism has three parts:
//!
//! 1. A **pin** ([`VocabularyPin`], committed as
//!    `config/capabilities.pin.toml`) names the one approved digest and
//!    version. [`Vocabulary::from_toml_pinned`] refuses any other file, so a
//!    free edit to the vocabulary no longer loads (and the committed-file
//!    tests fail). The pin is also **signed** (Ed25519, over
//!    [`VocabularyPin::signed_statement`]) and bound to the chain event that
//!    approved it, so editing the vocabulary and its pin together does not
//!    forge one. The kernel verifies the signature against compiled-in
//!    signer keys; this module only carries and shapes the fields.
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

/// `event_hash` value of the baseline pin, which predates any chain event.
/// Accepted only for version 1.
pub const GENESIS_EVENT: &str = "genesis";

/// Domain separator at the start of every signed pin statement.
pub const PIN_STATEMENT_DOMAIN: &str = "weftos.vocabulary-pin.v1";

/// The governance-approved digest and version of the vocabulary file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VocabularyPin {
    /// Approved `meta.version`.
    pub version: u32,
    /// Approved SHA-256 hex digest of the exact file bytes (64 lowercase hex).
    pub digest: String,
    /// Id of the signing key (resolved against the compiled-in signers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_id: Option<String>,
    /// Ed25519 signature over [`Self::signed_statement`] (128 lowercase hex).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// Hash of the chain event that approved this pin (64 lowercase hex), or
    /// [`GENESIS_EVENT`] for the version 1 baseline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_hash: Option<String>,
    /// Sequence number of that chain event (absent for the baseline).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_seq: Option<u64>,
}

fn is_lower_hex(s: &str, len: usize) -> bool {
    s.len() == len
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn is_sha256_hex(s: &str) -> bool {
    is_lower_hex(s, 64)
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
        pin.validate_signature_fields()?;
        Ok(pin)
    }

    fn validate_signature_fields(&self) -> Result<(), VocabularyError> {
        let bad = |entry: &str, reason: &str| VocabularyError::Invalid {
            entry: entry.into(),
            reason: reason.into(),
        };
        if let Some(sig) = &self.signature
            && !is_lower_hex(sig, 128)
        {
            return Err(bad("signature", "must be 128 lowercase hex characters"));
        }
        if let Some(id) = &self.key_id
            && (id.is_empty() || id.len() > 128 || !id.is_ascii() || id.contains('\n'))
        {
            return Err(bad("key_id", "must be 1-128 printable ASCII characters"));
        }
        if let Some(ev) = &self.event_hash
            && ev != GENESIS_EVENT
            && !is_sha256_hex(ev)
        {
            return Err(bad("event_hash", "must be 64 lowercase hex or \"genesis\""));
        }
        Ok(())
    }

    /// The pin for an already loaded vocabulary, unsigned and unbound.
    pub fn of(v: &Vocabulary) -> Self {
        Self {
            version: v.meta().version,
            digest: v.digest().to_string(),
            key_id: None,
            signature: None,
            event_hash: None,
            event_seq: None,
        }
    }

    /// The exact bytes the signature covers: domain, version, digest, signer
    /// key id, and the chain event it is bound to. A missing event or key id
    /// signs as the empty string, which no valid pin carries.
    pub fn signed_statement(&self) -> Vec<u8> {
        format!(
            "{PIN_STATEMENT_DOMAIN}\nversion={}\ndigest={}\nkey_id={}\nevent_seq={}\nevent_hash={}\n",
            self.version,
            self.digest,
            self.key_id.as_deref().unwrap_or(""),
            self.event_seq.map(|n| n.to_string()).unwrap_or_default(),
            self.event_hash.as_deref().unwrap_or(""),
        )
        .into_bytes()
    }

    /// Serialize as the committed pin file, with its header comment.
    pub fn to_toml_string(&self) -> String {
        let mut out = format!(
            "# Governance pin for config/capabilities.toml (ADR-099 decision 6).\n\
             # Issued only by an approved `config.set` on {VOCABULARY_CONFIG_NAMESPACE}/\
             {VOCABULARY_CONFIG_KEY} (clawft_kernel::placement_vocabulary).\n\
             # Later versions are signed and bound to that chain event. This baseline (version 1)\n\
             # is trusted by its compiled-in digest. Do not edit by hand: a vocabulary whose\n\
             # digest differs, or a pin whose signature fails, does not load.\n\
             version = {}\ndigest = \"{}\"\n",
            self.version, self.digest
        );
        if let Some(v) = &self.key_id {
            out.push_str(&format!("key_id = \"{v}\"\n"));
        }
        if let Some(v) = self.event_seq {
            out.push_str(&format!("event_seq = {v}\n"));
        }
        if let Some(v) = &self.event_hash {
            out.push_str(&format!("event_hash = \"{v}\"\n"));
        }
        if let Some(v) = &self.signature {
            out.push_str(&format!("signature = \"{v}\"\n"));
        }
        out
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
