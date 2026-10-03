//! Cog ingest bridge (mesh-placement-10; ADR-100 section 4 and the
//! "Decision 5 resolved" amendment).
//!
//! A placed cog posts its feature vectors to `POST /api/v1/store/ingest`
//! with its `COGNITUM_COG_TOKEN`. This module is where that request ends:
//!
//! - [`types`]: the request contract and its validation.
//! - [`registry`]: per-instance tokens bound to the placing project, the
//!   per-instance rate budget, and the [`StoreRouter`].
//! - [`bridge`]: the node-local HTTP server.
//! - [`forward`]: [`LocalForwarder`] and [`MeshForwarder`] to the store owner
//!   as signed `MeshIpcEnvelope`s.
//! - [`owner`]: the owner's [`StoreOwnerService`] and its policy.
//! - [`store`]: the [`IngestStore`] a project's kernel owns.
//! - [`udp`]: the optional ESP32 feed relay into a container.
//!
//! Ingested vectors belong to the project that placed the cog. Whatever the
//! cog prints on stdout is evidence for logs only and never reaches a store.

pub mod bridge;
pub mod forward;
pub mod hooks;
pub mod owner;
pub mod registry;
pub mod store;
pub mod store_log;
pub mod types;
pub mod udp;
#[cfg(feature = "ecc")]
pub mod vector_dir;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_forward;
#[cfg(all(test, feature = "ecc"))]
mod tests_persist;

pub use bridge::{BridgeConfig, BridgeHandle, BridgeScope, BridgeStats, IngestBridge};
pub use forward::{
    ForwardRefusal, ForwardRequest, Forwarder, LocalForwarder, MeshForwarder, STORE_INGEST_METHOD,
    STORE_SERVICE,
};
pub use hooks::{IngestHooks, IngestLease, ProjectDirectory};
pub use owner::{ForwardPolicy, KeyPolicy, OwnerConnector, StoreOwnerService};
pub use registry::{InstanceBinding, RateBudget, StaticRouter, StoreRouter, TokenRegistry};
#[cfg(feature = "ecc")]
pub use store::VectorBackendStore;
#[cfg(feature = "ecc")]
pub use vector_dir::VectorDirectory;
pub use store::{
    Hit, IngestOutcome, IngestStore, MemoryIngestStore, Provenance, StaticDirectory, StoreDirectory,
    StoreError,
};
pub use types::{
    DIMS, INGEST_PATH, IngestBatch, IngestError, IngestVector, parse_batch, valid_project_id,
};
pub use udp::{UdpForwardConfig, UdpForwarder};
