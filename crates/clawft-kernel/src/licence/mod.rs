//! Seed licence proxy, member side (ADR-106 phase 1a).
//!
//! Types, stores and policy for the mesh checkout of Cognitum cogs, as pure
//! in-process code (no network, no transport):
//!
//! - [`MeshId`] and [`LocalMeshId`]: the mesh a binding, grant or approval is for.
//! - [`BindingRecord`]: the operator-signed Seed to mesh binding (v2), with the
//!   *member* verification profile ([`verify_binding_member`]). The *steward*
//!   profile (age window, live identity match, fingerprint) is phase 1d and
//!   plugs in through [`BindingExtraCheck`].
//! - [`CheckoutGrant`]: signed by the Seed's grant key; highest `seq` wins.
//! - [`Approval`]: the operator's additive, content-addressed hash approval.
//! - [`CheckoutGrantStore`] and [`ApprovalStore`]: persisted, atomic, 0600,
//!   size-capped, fail-closed.
//! - [`MeshCheckoutPolicy`]: the swarm redistribution policy. With no binding
//!   it behaves exactly like [`crate::mesh_swarm_state::ManifestPolicy`].
//! - [`may_run`]: the run gate (valid grant AND an approval covering the sha256).
//!
//! Every signature is Ed25519, checked with `verify_strict`, over
//! `domain tag || "\n" || canonical JSON payload`. The signer keys are the
//! existing [`crate::workload_pkg::TrustAnchors`] operator keys; the grant key
//! is the one the binding names.

mod approval;
mod approval_store;
mod binding;
mod chain_sink;
mod client;
mod exchange;
mod exchange_sync;
mod exchange_types;
mod floor;
mod floor_preview;
mod gate;
mod mesh_config;
mod persist;
mod policy;
mod relay;
mod request;
mod store;
mod steward;
mod store_accept;
mod store_load;
mod store_sync;

#[cfg(test)]
mod tests_common;
#[cfg(test)]
mod tests_exchange;
#[cfg(test)]
mod tests_fixes;
#[cfg(test)]
mod tests_sync;
#[cfg(test)]
mod tests_policy;
#[cfg(test)]
mod tests_relay;
#[cfg(test)]
mod tests_request;
#[cfg(test)]
mod tests_stub;
#[cfg(test)]
mod tests_review;
#[cfg(test)]
mod tests_seed_service;
#[cfg(test)]
mod tests_store;
#[cfg(test)]
mod tests_types;

use std::sync::{Arc, Mutex, RwLock};

#[allow(unused_imports)]
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
#[allow(unused_imports)]
use serde::{Deserialize, Serialize};

#[allow(unused_imports)]
use crate::workload_pkg::codec::{hex_decode_exact, hex_encode};

pub use approval::{Approval, SignedApproval, sign_approval, verify_approval};
pub use approval_store::ApprovalStore;
pub use binding::{
    AdmissionPosture, BindState, BindingExtraCheck, BindingRecord, NoExtraChecks, SignedBinding,
    sign_binding, verify_binding_member,
};
pub use chain_sink::{ChainLicenceSink, LICENCE_EVENT_PREFIX};
pub use client::{
    ARTIFACT_PATH, CHECKOUT_PATH, CheckoutWire, ClockMs, system_clock_ms, GRANTS_PATH, LicenceClient, LicenceClientError,
    LicenceResponse, LicenceTransport, SignedLicenceClient,
};
pub use exchange::{
    CtxAdmission, ExchangeError, LicenceExchange, LicenceExchangeConfig, LicenceExchangeParts,
    PeerAdmission, PostureFn, Receipt, Spend, sign_unbind,
};
pub use exchange_sync::{GrantCursor, SYNC_MAX_BYTES, SYNC_MAX_ENTRIES, SyncMsg};
pub use floor::FloorState;
pub use floor_preview::{FloorPreview, RevivedGrant};
pub use gate::{RunDenied, RunPermit, RunRequest, may_run};
pub use weft_licence_wire::{
    APPROVAL_DOMAIN, BINDING_DOMAIN, CheckoutGrant, FAR_FUTURE_CLAMP_SECS, GRANT_DOMAIN,
    GRANT_SKEW_SECS, GrantArtifact, LicenceError, LicenceRef, MAX_APPROVALS, MAX_GRANT_SLOTS,
    MAX_ARTIFACT_BYTES, MAX_FLOORS, MAX_GRANT_TTL_SECS, MAX_PAYLOAD_BYTES, MAX_STORE_BYTES, MAX_UNIX_TIME, MESH_ID_DOMAIN, MeshId,
    SignedEnvelope, SignedGrant, key_id, sha256_hex, sign_grant, verify_grant,
};
#[allow(unused_imports)]
pub(crate) use weft_licence_wire::{
    envelope_key, parse_canonical, sign_envelope, signed_bytes, valid_hex32, valid_token,
    verify_envelope, verify_grant_signature,
};
pub use mesh_config::{MeshIdConfigError, mesh_id_from_config};
pub use policy::MeshCheckoutPolicy;
pub use relay::{
    CheckoutCaller, CheckoutRefusal, CheckoutRelay, RelayLimits, EVENT_KIND_CHECKOUT_GRANTED,
    EVENT_KIND_CHECKOUT_REFUSED, GATE_ACTION, GrantFlood, NoFlood, install_grant,
};
pub use request::{
    CLOCK_FLOOR_SECS, LicenceRequest, REQUEST_DOMAIN, REQUEST_WINDOW_MS, ReplayGuard,
    RequestAuth, RequestRefused, sign_request, signing_string, valid_nonce, verify_request,
};
pub use steward::StewardCheck;
pub use store::{CheckoutGrantStore, VerifiedCheckoutGrant};

/// What an accepted record did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// New state was recorded.
    Applied,
    /// A record that only restricts (an unbind, a withdrawal) is in force
    /// but could not be saved; `tick` retries. Propagate it like `Applied`.
    AppliedUnsaved,
    /// Already held; nothing changed.
    Duplicate,
    /// A lower `seq` than the one held; ignored.
    Ignored,
}

/// Shared, live handle to this node's mesh id. A change (new nonce or pin)
/// reaches every store holding a clone, which then report orphaned state.
#[derive(Clone, Default)]
pub struct LocalMeshId(Arc<RwLock<Option<MeshId>>>);

impl LocalMeshId {
    /// A handle holding `id`.
    pub fn new(id: MeshId) -> Self {
        Self(Arc::new(RwLock::new(Some(id))))
    }

    /// A handle with no id yet (no mesh nonce configured).
    pub fn unset() -> Self {
        Self::default()
    }

    /// The current id.
    pub fn get(&self) -> Option<MeshId> {
        *self.0.read().unwrap_or_else(|p| p.into_inner())
    }

    /// Replace the id.
    pub fn set(&self, id: Option<MeshId>) {
        *self.0.write().unwrap_or_else(|p| p.into_inner()) = id;
    }
}

/// Source of unix seconds. Injected so tests control the clock.
pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// The wall clock.
pub fn system_clock() -> Clock {
    Arc::new(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    })
}

/// Something worth chaining. The daemon maps these to chain events; the
/// stores only report them through a [`LicenceEventSink`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LicenceEvent {
    /// A binding was refused (`open_membership`, `observe`, ...).
    BindingRefused(String),
    /// Two different bindings at one `seq`.
    BindingConflict(u64),
    /// The stored binding's mesh id no longer matches the local one.
    BindingOrphaned {
        /// Mesh id in the stored binding.
        stored: String,
        /// Mesh id this node computes now.
        local: String,
    },
    /// Two different grants at one `seq` for one (cog, version).
    GrantConflict {
        /// Cog id.
        cog_id: String,
        /// Version.
        version: String,
        /// The contested `seq`.
        seq: u64,
    },
    /// An operator reset the floor.
    FloorReset(u64),
    /// A sync response carried a bad signature; the peer is banned from sync.
    SyncBadSignature {
        /// The peer that sent it.
        peer: String,
    },
}

impl LicenceEvent {
    /// The chain event name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::BindingRefused(_) => "binding_refused",
            Self::BindingConflict(_) => "binding_conflict",
            Self::BindingOrphaned { .. } => "binding_orphaned",
            Self::GrantConflict { .. } => "grant_conflict",
            Self::FloorReset(_) => "floor_reset",
            Self::SyncBadSignature { .. } => "sync_bad_signature",
        }
    }
}

/// Receives [`LicenceEvent`]s. Called after the store's lock is released, but
/// a sink must still not call back into the store.
pub trait LicenceEventSink: Send + Sync {
    /// Handle one event.
    fn emit(&self, event: LicenceEvent);
}

/// Drops every event.
#[derive(Debug, Default)]
pub struct NoopSink;

impl LicenceEventSink for NoopSink {
    fn emit(&self, _: LicenceEvent) {}
}

/// Keeps every event (tests, and callers that drain into the chain).
#[derive(Debug, Default)]
pub struct RecordingSink(Mutex<Vec<LicenceEvent>>);

impl RecordingSink {
    /// The events so far.
    pub fn events(&self) -> Vec<LicenceEvent> {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

impl LicenceEventSink for RecordingSink {
    fn emit(&self, event: LicenceEvent) {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).push(event);
    }
}

