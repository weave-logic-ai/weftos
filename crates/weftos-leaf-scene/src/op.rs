//! `SceneOp` — the unit of mutation on the wire. See
//! [vector-leaf-display.md §4.3 Display types](../../../docs/design/vector-leaf-display.md).
//!
//! Ops are CBOR-encoded, batched into a [`SceneEnvelope`](crate::envelope::SceneEnvelope),
//! and applied to a [`SceneStore`](crate::store::SceneStore) at the
//! leaf. Every op produces a [`DamageSet`](crate::damage::DamageSet).
//!
//! Producers always emit deltas in steady state. On mesh-connect (and
//! every ~5 s as a self-healing snapshot) they emit a single
//! `SceneOp::Replace(Scene)` to make the leaf state authoritative.

use alloc::vec::Vec;

use serde::{Deserialize, Serialize};

use crate::id::NodeId;
use crate::node::Node;
use crate::primitive::{BlendMode, Layer};
use crate::scene::Scene;
use crate::tween::{AnimatableProperty, PropertyValue};

/// One scene-graph mutation.
///
/// # Variants
///
/// - `Insert` / `Update`: upsert a node. The store dedupes by `NodeId`.
/// - `Remove`: drop a node by id.
/// - `SetLayerBlend`: change a layer's compositing mode without
///   touching its nodes.
/// - `Tween` / `CancelTween`: animation; see §5.6.
/// - `Clear`: drop every node in this op's display (background colour
///   is preserved).
/// - `Replace(Scene)`: full snapshot — atomically swap state. The
///   damage set is always `full_repaint`. Used for mesh-reconnect and
///   the 5-second self-healing cadence.
///
/// # Tween coalescing contract
///
/// When `Tween { id, property, .. }` arrives for a `(id, property)`
/// that already has an active tween:
///
/// - The old tween is **cancelled** (no completion event is emitted).
/// - The new tween's `from` is rewritten in-store to the current
///   interpolated value of the old tween. In v1 the interpolated
///   value is always `old.from` (snap-on-tick), so the visual is
///   wrong but the data flow is correct. v1.1 fills in real
///   interpolation in [`SceneStore::tick`](crate::store::SceneStore::tick).
///
/// Producers wanting strict sequencing should send `CancelTween`
/// explicitly between `Tween` ops.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SceneOp {
    /// Insert or update a node.
    Insert(Node),
    /// Convenience alias for `Insert` — separate variant so deltas
    /// reading "Update" are self-documenting on the wire.
    Update(Node),
    Remove(NodeId),
    SetLayerBlend {
        layer: Layer,
        mode: BlendMode,
    },
    Tween {
        id: NodeId,
        property: AnimatableProperty,
        /// Origin value. Producers can omit by emitting whatever the
        /// current state is; the store rewrites it to the interpolated
        /// state on coalesce.
        from: PropertyValue,
        to: PropertyValue,
        duration_ms: u32,
        /// Optional delay before the tween becomes active. Relative
        /// milliseconds from the leaf's "ingest" timestamp (the
        /// `now_ms` passed to the next `tick` call).
        start_at: Option<u32>,
        /// Easing curve. v1 ignores (snap-to-`to`); v1.1 honours.
        curve: crate::primitive::EaseCurve,
    },
    /// Cancel an in-flight tween. `property: None` cancels every
    /// active tween on `id`.
    CancelTween {
        id: NodeId,
        property: Option<AnimatableProperty>,
    },
    /// Drop every node on this display. Background colour is preserved.
    Clear,
    /// Full snapshot — atomically replace state.
    Replace(Scene),
    /// Batch of ops applied atomically. Damage is the union of each.
    Batch(Vec<SceneOp>),
}
