//! Maps [`LicenceEvent`]s onto the node's chain (ADR-106 phase 1b).

use std::sync::Arc;

use serde_json::{Value, json};

use super::{LicenceEvent, LicenceEventSink};
use crate::chain::ChainManager;

/// Chain event kinds are this prefix plus [`LicenceEvent::name`], for example
/// `licence.binding_conflict`.
pub const LICENCE_EVENT_PREFIX: &str = "licence.";

/// Chain source of licence events.
const SOURCE: &str = "licence";

/// Appends every [`LicenceEvent`] to the chain.
pub struct ChainLicenceSink {
    chain: Arc<ChainManager>,
}

impl ChainLicenceSink {
    /// A sink appending to `chain`.
    pub fn new(chain: Arc<ChainManager>) -> Self {
        Self { chain }
    }

    /// The chain event kind for `event`.
    pub fn kind(event: &LicenceEvent) -> String {
        format!("{LICENCE_EVENT_PREFIX}{}", event.name())
    }

    /// The chain payload for `event`.
    pub fn payload(event: &LicenceEvent) -> Value {
        match event {
            LicenceEvent::BindingRefused(why) => json!({ "reason": why }),
            LicenceEvent::BindingConflict(seq) => json!({ "seq": seq }),
            LicenceEvent::BindingOrphaned { stored, local } => {
                json!({ "stored_mesh_id": stored, "local_mesh_id": local })
            }
            LicenceEvent::GrantConflict { cog_id, version, seq } => {
                json!({ "cog_id": cog_id, "version": version, "seq": seq })
            }
            LicenceEvent::FloorClamped { from, to } => json!({ "from": from, "to": to }),
            LicenceEvent::FloorReset(to) => json!({ "to": to }),
            LicenceEvent::SyncBadSignature { peer } => json!({ "peer": peer }),
        }
    }
}

impl LicenceEventSink for ChainLicenceSink {
    fn emit(&self, event: LicenceEvent) {
        self.chain
            .append(SOURCE, &Self::kind(&event), Some(Self::payload(&event)));
    }
}
