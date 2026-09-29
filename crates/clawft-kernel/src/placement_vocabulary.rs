//! Governed loading and changing of the placement capability vocabulary
//! (ADR-099 decision 6; card mesh-placement-02).
//!
//! `config/capabilities.toml` is advisory for matching but its content is
//! governed: it changes only through this path, never by free edits.
//!
//! - [`load_governed`] / [`load_governed_dir`] load the vocabulary only when
//!   its SHA-256 digest and version equal the pin in
//!   `config/capabilities.pin.toml`. A freely edited file does not load.
//! - [`VocabularyGovernor::propose`] checks a proposed vocabulary as the
//!   existing governed `config.set` action on
//!   `placement/capabilities.toml` (with the `ConfigSet` effect vector), and
//!   only on `Permit` appends a `config.set` chain event carrying the change
//!   record and issues the new pin. `Deny` and `Defer` issue nothing.
//!
//! The vocabulary reuses the config governance action and chain event kind
//! rather than a new name: it is a config file, and the `workload.*` action
//! family (ADR-099) is for workloads.

use std::path::Path;
use std::sync::Arc;

use clawft_types::placement::vocabulary::MAX_VOCAB_BYTES;
use clawft_types::placement::vocabulary_pin::MAX_PIN_BYTES;
use clawft_types::placement::{
    VOCABULARY_CONFIG_KEY, VOCABULARY_CONFIG_NAMESPACE, Vocabulary, VocabularyChange, VocabularyPin,
};

use crate::chain::{ChainManager, EVENT_KIND_CONFIG_SET};
use crate::error::KernelError;
use crate::gate::{GateBackend, GateDecision};
use crate::governance::{GateEffectKind, with_effect_context};

/// Governance action checked for a vocabulary change (the config action).
pub const VOCABULARY_GATE_ACTION: &str = "config.set";
/// Vocabulary file name inside a config directory.
pub const VOCABULARY_FILE: &str = "capabilities.toml";
/// Pin file name inside a config directory.
pub const VOCABULARY_PIN_FILE: &str = "capabilities.pin.toml";

fn config_err(e: impl std::fmt::Display) -> KernelError {
    KernelError::Config(format!("capability vocabulary: {e}"))
}

/// Load a vocabulary only if it matches its governance pin.
pub fn load_governed(vocab_text: &str, pin_text: &str) -> Result<Vocabulary, KernelError> {
    let pin = VocabularyPin::from_toml_str(pin_text).map_err(config_err)?;
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
/// `capabilities.pin.toml` in the same directory.
pub fn load_governed_dir(config_dir: &Path) -> Result<Vocabulary, KernelError> {
    let vocab = read_bounded(&config_dir.join(VOCABULARY_FILE), MAX_VOCAB_BYTES)?;
    let pin = read_bounded(&config_dir.join(VOCABULARY_PIN_FILE), MAX_PIN_BYTES)?;
    load_governed(&vocab, &pin)
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

/// Gates vocabulary changes and records approved ones on ExoChain.
pub struct VocabularyGovernor {
    gate: Arc<dyn GateBackend>,
    chain: Option<Arc<ChainManager>>,
}

impl VocabularyGovernor {
    /// A governor that checks changes with `gate`.
    pub fn new(gate: Arc<dyn GateBackend>) -> Self {
        Self { gate, chain: None }
    }

    /// Record approved changes on this chain.
    pub fn with_chain(mut self, chain: Arc<ChainManager>) -> Self {
        self.chain = Some(chain);
        self
    }

    /// Propose replacing `current` with `proposed_text` on behalf of
    /// `requester`. The proposal must parse, bump `meta.version` and pass
    /// the governance gate; only then is a chain event written and a pin
    /// issued. `Defer` is not approval: nothing is issued until a later
    /// proposal is permitted.
    pub fn propose(
        &self,
        requester: &str,
        current: &Vocabulary,
        proposed_text: &str,
    ) -> Result<ApprovedVocabularyChange, KernelError> {
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
        let pin = VocabularyPin::of(&proposed);
        if let Some(cm) = &self.chain {
            cm.append(
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
        }
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
