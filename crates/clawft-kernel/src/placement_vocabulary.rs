//! Governed loading and changing of the placement capability vocabulary
//! (ADR-099 decision 6; card mesh-placement-02).
//!
//! `config/capabilities.toml` is advisory for matching but its content is
//! governed: it changes only through this path, never by free edits.
//!
//! - [`load_governed`] / [`load_governed_dir`] load the vocabulary only when
//!   its SHA-256 digest and version equal the pin in
//!   `config/capabilities.pin.toml` AND the pin is authentic. A freely edited
//!   file does not load, and editing the file and the pin together does not
//!   either: a pin is authentic when it is signed by a key in the
//!   [`TrustAnchors`] and bound to a chain event, or when it is the version 1
//!   baseline whose digest is compiled in ([`BASELINE_VOCABULARY_DIGEST`]).
//! - [`VocabularyGovernor::propose`] checks a proposed vocabulary as the
//!   existing governed `config.set` action on
//!   `placement/capabilities.toml` (with the `ConfigSet` effect vector), and
//!   only on `Permit` appends a `config.set` chain event carrying the change
//!   record, then issues the new pin signed over that event's hash. `Deny`
//!   and `Defer` issue nothing, and a governor without a chain or a signing
//!   key refuses to issue a pin at all.
//! - [`verify_pin_event`] checks a pin against the chain it claims.
//!
//! The vocabulary reuses the config governance action and chain event kind
//! rather than a new name: it is a config file, and the `workload.*` action
//! family (ADR-099) is for workloads.

use std::path::Path;
use std::sync::Arc;

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

use clawft_types::placement::vocabulary::MAX_VOCAB_BYTES;
use clawft_types::placement::vocabulary_pin::{GENESIS_EVENT, MAX_PIN_BYTES};
use clawft_types::placement::{
    VOCABULARY_CONFIG_KEY, VOCABULARY_CONFIG_NAMESPACE, Vocabulary, VocabularyChange, VocabularyPin,
};

use crate::chain::{ChainManager, EVENT_KIND_CONFIG_SET};
use crate::error::KernelError;
use crate::gate::{GateBackend, GateDecision};
use crate::governance::{GateEffectKind, with_effect_context};
use crate::workload_pkg::codec::{hex_decode_exact, hex_encode, is_lower_hex};
use crate::workload_pkg::{TrustAnchors, key_id_for};

/// SHA-256 digest of the version 1 vocabulary (`config/capabilities.toml`).
///
/// The baseline predates any signer or chain event, so its pin carries no
/// signature; it is trusted because this constant is compiled in. Changing it
/// is a code change, reviewed like any other. Every later version needs a
/// signed pin bound to a chain event.
pub const BASELINE_VOCABULARY_DIGEST: &str =
    "927ed3db1359a4d0193277bf53efb424280e49f18e204c295bb47b8a885ff4c5";

/// Governance action checked for a vocabulary change (the config action).
pub const VOCABULARY_GATE_ACTION: &str = "config.set";
/// Vocabulary file name inside a config directory.
pub const VOCABULARY_FILE: &str = "capabilities.toml";
/// Pin file name inside a config directory.
pub const VOCABULARY_PIN_FILE: &str = "capabilities.pin.toml";

fn config_err(e: impl std::fmt::Display) -> KernelError {
    KernelError::Config(format!("capability vocabulary: {e}"))
}

/// Check that a pin is authentic: the compiled-in baseline, or signed by a
/// key in `anchors` and bound to a chain event.
pub fn verify_pin(pin: &VocabularyPin, anchors: &TrustAnchors) -> Result<(), KernelError> {
    let Some(sig_hex) = &pin.signature else {
        let baseline = pin.version == 1
            && pin.event_hash.as_deref() == Some(GENESIS_EVENT)
            && pin.event_seq.is_none()
            && pin.key_id.is_none()
            && pin.digest == BASELINE_VOCABULARY_DIGEST;
        return if baseline {
            Ok(())
        } else {
            Err(config_err(
                "pin is unsigned and is not the compiled-in baseline",
            ))
        };
    };
    let event = pin.event_hash.as_deref().unwrap_or("");
    if !is_lower_hex(event, 64) || pin.event_seq.is_none() {
        return Err(config_err("signed pin is not bound to a chain event"));
    }
    let key_id = pin.key_id.as_deref().unwrap_or("");
    let signer = anchors
        .signers
        .iter()
        .find(|k| k.key_id == key_id)
        .ok_or_else(|| config_err(format!("pin signer {key_id:?} is not a trusted key")))?;
    let key = VerifyingKey::from_bytes(&signer.public_key).map_err(config_err)?;
    let sig = hex_decode_exact::<64>(sig_hex)
        .map(|b| Signature::from_bytes(&b))
        .ok_or_else(|| config_err("pin signature is malformed"))?;
    key.verify(&pin.signed_statement(), &sig)
        .map_err(|_| config_err("pin signature does not verify"))
}

/// Check that the chain holds the event a pin claims to be bound to: same
/// sequence and hash, a `config.set` from `placement`, naming this digest.
pub fn verify_pin_event(pin: &VocabularyPin, chain: &ChainManager) -> Result<(), KernelError> {
    let (Some(seq), Some(hash)) = (pin.event_seq, pin.event_hash.as_deref()) else {
        return Err(config_err("pin is not bound to a chain event"));
    };
    let ev = chain
        .tail_from(seq.saturating_sub(1))
        .into_iter()
        .find(|e| e.sequence == seq)
        .ok_or_else(|| config_err(format!("chain has no event {seq}")))?;
    let digest_ok = ev
        .payload
        .as_ref()
        .and_then(|p| p.pointer("/pin/digest"))
        .and_then(|d| d.as_str())
        == Some(pin.digest.as_str());
    if hex_encode(&ev.hash) != hash
        || ev.source != "placement"
        || ev.kind != EVENT_KIND_CONFIG_SET
        || !digest_ok
    {
        return Err(config_err(format!(
            "chain event {seq} does not match the pin"
        )));
    }
    Ok(())
}

/// Load a vocabulary only if its pin is authentic ([`verify_pin`]) and it
/// matches that pin.
pub fn load_governed(
    vocab_text: &str,
    pin_text: &str,
    anchors: &TrustAnchors,
) -> Result<Vocabulary, KernelError> {
    let pin = VocabularyPin::from_toml_str(pin_text).map_err(config_err)?;
    verify_pin(&pin, anchors)?;
    Vocabulary::from_toml_pinned(vocab_text, &pin).map_err(config_err)
}

fn read_bounded(path: &Path, max: usize) -> Result<String, KernelError> {
    let len = std::fs::metadata(path)
        .map_err(|e| config_err(format!("{}: {e}", path.display())))?
        .len();
    if len > max as u64 {
        return Err(config_err(format!(
            "{} is larger than {max} bytes",
            path.display()
        )));
    }
    std::fs::read_to_string(path).map_err(|e| config_err(format!("{}: {e}", path.display())))
}

/// Load `capabilities.toml` from `config_dir`, checked against
/// `capabilities.pin.toml` in the same directory, trusting only the
/// compiled-in WeftOS signers (the baseline loads; later versions need a
/// provisioned signer, see [`load_governed_dir_with`]).
pub fn load_governed_dir(config_dir: &Path) -> Result<Vocabulary, KernelError> {
    let anchors = TrustAnchors::weftos_default().map_err(config_err)?;
    load_governed_dir_with(config_dir, &anchors)
}

/// [`load_governed_dir`] against caller-supplied trust anchors, such as the
/// operator trust file's pinned signers.
pub fn load_governed_dir_with(
    config_dir: &Path,
    anchors: &TrustAnchors,
) -> Result<Vocabulary, KernelError> {
    let vocab = read_bounded(&config_dir.join(VOCABULARY_FILE), MAX_VOCAB_BYTES)?;
    let pin = read_bounded(&config_dir.join(VOCABULARY_PIN_FILE), MAX_PIN_BYTES)?;
    load_governed(&vocab, &pin, anchors)
}

/// An approved vocabulary change.
#[derive(Debug, Clone)]
pub struct ApprovedVocabularyChange {
    /// The approved vocabulary.
    pub vocabulary: Vocabulary,
    /// The pin to commit as `capabilities.pin.toml`.
    pub pin: VocabularyPin,
    /// The change record that was gated and chained.
    pub change: VocabularyChange,
}

/// Gates vocabulary changes, records approved ones on ExoChain and signs the
/// pin it issues.
pub struct VocabularyGovernor {
    gate: Arc<dyn GateBackend>,
    chain: Option<Arc<ChainManager>>,
    signer: Option<(SigningKey, String)>,
}

impl VocabularyGovernor {
    /// A governor that checks changes with `gate`. It issues nothing until it
    /// also has a chain and a signing key.
    pub fn new(gate: Arc<dyn GateBackend>) -> Self {
        Self {
            gate,
            chain: None,
            signer: None,
        }
    }

    /// Sign issued pins with `key` (id derived with [`key_id_for`]). The
    /// public half must be in the trust anchors used to load the pin.
    pub fn with_signer(mut self, key: SigningKey) -> Self {
        let id = key_id_for(key.verifying_key().as_bytes());
        self.signer = Some((key, id));
        self
    }

    /// Record approved changes on this chain.
    pub fn with_chain(mut self, chain: Arc<ChainManager>) -> Self {
        self.chain = Some(chain);
        self
    }

    /// Propose replacing `current` with `proposed_text` on behalf of
    /// `requester`. The proposal must parse, bump `meta.version` and pass
    /// the governance gate; only then is a chain event written and a signed
    /// pin issued, bound to that event. `Defer` is not approval: nothing is
    /// issued until a later proposal is permitted. A governor with no chain
    /// or no signing key refuses before the gate is consulted.
    pub fn propose(
        &self,
        requester: &str,
        current: &Vocabulary,
        proposed_text: &str,
    ) -> Result<ApprovedVocabularyChange, KernelError> {
        let (Some(chain), Some((key, key_id))) = (&self.chain, &self.signer) else {
            return Err(config_err(
                "governor has no chain or signing key; refusing to issue a pin",
            ));
        };
        let proposed = Vocabulary::from_toml_str(proposed_text).map_err(config_err)?;
        let change = current.change_to(&proposed).map_err(config_err)?;
        let ctx = with_effect_context(
            serde_json::json!({
                "namespace": VOCABULARY_CONFIG_NAMESPACE,
                "key": VOCABULARY_CONFIG_KEY,
                "requester": requester,
                "from_digest": change.from_digest,
                "to_digest": change.to_digest,
                "from_version": change.from_version,
                "to_version": change.to_version,
            }),
            &GateEffectKind::ConfigSet.effect_vector(),
        );
        match self.gate.check(requester, VOCABULARY_GATE_ACTION, &ctx) {
            GateDecision::Permit { .. } => {}
            GateDecision::Deny { reason, .. } => {
                return Err(KernelError::GovernanceDenied(format!(
                    "vocabulary change denied: {reason}"
                )));
            }
            GateDecision::Defer { reason } => {
                return Err(KernelError::GovernanceDenied(format!(
                    "vocabulary change deferred for review: {reason}"
                )));
            }
            #[allow(unreachable_patterns)]
            _ => {
                return Err(KernelError::GovernanceDenied(
                    "vocabulary change: unrecognised gate decision".into(),
                ));
            }
        }
        let mut pin = VocabularyPin::of(&proposed);
        let event = chain.append(
            "placement",
            EVENT_KIND_CONFIG_SET,
            Some(serde_json::json!({
                "namespace": VOCABULARY_CONFIG_NAMESPACE,
                "key": VOCABULARY_CONFIG_KEY,
                "requester": requester,
                "change": change,
                "pin": pin,
            })),
        );
        pin.key_id = Some(key_id.clone());
        pin.event_seq = Some(event.sequence);
        pin.event_hash = Some(hex_encode(&event.hash));
        pin.signature = Some(hex_encode(&key.sign(&pin.signed_statement()).to_bytes()));
        Ok(ApprovedVocabularyChange {
            vocabulary: proposed,
            pin,
            change,
        })
    }
}

#[cfg(test)]
#[path = "placement_vocabulary_tests.rs"]
mod tests;
