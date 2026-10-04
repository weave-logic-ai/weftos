//! `clawft-bvh` — BVH broad-phase spatial index (ADR-056).
//!
//! Standalone, no `clawft-kernel` dependency. Ships:
//! - AABB / Vec3 / Ray / Frustum primitives
//! - Leaf + LeafId + IdentityKind + BranchId
//! - Top-down median-split tree with point/AABB/sphere/ray/knn queries
//! - Tagged-union narrow-phase registry (stub interpreters)
//! - [`BvhStore`] with [`ChainSink`] hook for kernel audit (Phase C)
//!
//! COW branch node-sharing and determinism-phase seal deepen in Phase D.
//! Splat integration registers `splat.*` leaf tags via `weftos-leaf-types`.

#![warn(missing_docs)]

mod aabb;
mod chain;
mod leaf;
mod query;
mod registry;
mod store;
mod tree;
mod vector_join;

pub use aabb::{Aabb, Frustum, Ray, Vec3};
pub use chain::{BvhChainKind, ChainSink, NullChainSink, RecordingChainSink};
pub use leaf::{BranchId, BranchMeta, DiffEntry, IdentityKind, Leaf, LeafId, tags};
pub use query::{RayHit, query_aabb, query_knn, query_point, query_ray, query_sphere};
pub use registry::{NarrowPhaseFn, SpatialRegistry};
pub use store::{BvhError, BvhResult, BvhStore, BvhStoreConfig};
pub use tree::BvhTree;
pub use vector_join::{
    DualIndexHit, FeatureHit, SpatialJoinHit, filter_feature_hits_by_spatial,
    join_spatial_with_vector_refs, rank_spatial_hits_by_feature_distance, vector_ref_from_payload,
};

/// Crate version string for diagnostics.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
