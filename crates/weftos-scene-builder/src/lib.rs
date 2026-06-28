//! # weftos-scene-builder
//!
//! Phase E host-side ergonomics for the WeftOS vector-first leaf
//! display. Three things, in order of use:
//!
//! 1. [`SceneBuilder`] — fluent producer API that accumulates `(path,
//!    Node)` records into a [`SceneStore`].
//! 2. [`diff`] — compute the minimal `Vec<SceneOp>` that transitions
//!    one [`SceneStore`] to another.
//! 3. [`snapshot::to_envelope`] — wrap a [`SceneStore`] into a single
//!    `Replace(Scene)` [`SceneEnvelope`].
//!
//! Canonical design doc: `docs/design/vector-leaf-display.md` §10
//! Migration Plan (Phase B "weftos-scene-builder host crate" + Phase E
//! "Replace `kernel.ps` renderer with a `SceneBuilder` producing
//! patches").
//!
//! ## Shape at a glance
//!
//! ```text
//!   producer  ──► SceneBuilder::insert("ps.row[0]", Node{...})
//!                 SceneBuilder::insert("ps.row[1]", Node{...})
//!                 SceneBuilder::build() → SceneStore
//!                         │
//!                         ├── on first run / on reconnect
//!                         │      to_envelope(store) → SceneEnvelope{Replace(Scene)}
//!                         │
//!                         └── steady state
//!                                diff(prev, next) → Vec<SceneOp>
//!                                wrap in SceneEnvelope::new(display_id, ops)
//! ```
//!
//! `SceneBuilder` keys nodes by **string path** (e.g.
//! `"ps.row[0].agent"`). Internally it calls
//! [`weftos_leaf_scene::path_to_id`] with a producer prefix to assign
//! stable [`NodeId`]s — so the leaf's glyph cache and AABB cache survive
//! reruns and reboots.
//!
//! ## Why string paths
//!
//! NodeIds on the wire are u32. Picking them by hand at the producer is
//! error-prone (collisions, drift across refactors). Strings are the
//! ergonomic input; the deterministic [`path_to_id`] hash is the wire
//! shape. Producers think in paths, the wire moves bits.

pub mod builder;
pub mod diff;
pub mod snapshot;

pub use builder::{NodeBuilder, SceneBuilder};
pub use diff::diff;
pub use snapshot::to_envelope;

// Re-export the most commonly needed Phase A types so producers can
// `use weftos_scene_builder::*` without a second `use
// weftos_leaf_scene::*` line.
pub use weftos_leaf_scene::{
    path_to_id, BlendMode, BuiltinFont, CursorHint, DisplayId, EaseCurve, FontFace, HitShape,
    InputRegion, KerningHint, Layer, Node, NodeId, Point, Primitive, Rect, Rgb, Rgba, Scene,
    SceneEnvelope, SceneOp, SceneStore, Size, Style, Transform,
};
