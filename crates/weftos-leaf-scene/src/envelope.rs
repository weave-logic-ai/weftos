//! Wire envelopes — see
//! [vector-leaf-display.md §4.3 / §4.4](../../../docs/design/vector-leaf-display.md).
//!
//! Two envelopes ride on two distinct mesh topics:
//!
//! - `SceneEnvelope`: host → leaf on `mesh.leaf.<pk>.push`.
//! - `InputEnvelope`: leaf → host on `mesh.leaf.<pk>.input`.
//!
//! Both carry a version byte (current: [`WIRE_VERSION`]) that the
//! decoder rejects on mismatch. This lets us bump the wire format
//! without breaking old peers silently.

use alloc::vec::Vec;

use serde::{Deserialize, Serialize};

use crate::id::{DisplayId, NodeId};
use crate::op::SceneOp;

/// Current wire-format version. Bump when any wire type changes.
pub const WIRE_VERSION: u8 = 1;

/// Host → leaf scene-graph mutation envelope. Targets one
/// `display_id`. Batches one or more `SceneOp`s for atomic apply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneEnvelope {
    pub version: u8,
    pub display_id: DisplayId,
    pub ops: Vec<SceneOp>,
}

impl SceneEnvelope {
    /// Construct an envelope tagged with the current wire version.
    pub fn new(display_id: DisplayId, ops: Vec<SceneOp>) -> Self {
        Self {
            version: WIRE_VERSION,
            display_id,
            ops,
        }
    }

    /// Single-op convenience.
    pub fn single(display_id: DisplayId, op: SceneOp) -> Self {
        Self::new(display_id, alloc::vec![op])
    }
}

/// One pointer-input event. The leaf publishes these in
/// [`InputEnvelope`]; the host treats them as immutable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputEvent {
    /// Pointer or touch-down.
    PointerDown {
        /// Multi-touch slot id.
        pointer_id: u8,
        /// Q24.8 display coords.
        x: i32,
        /// Q24.8 display coords.
        y: i32,
        /// 0..=0xFF00; 0xFF00 = full pressure. 0 = unknown.
        pressure_q8: u16,
    },
    /// Movement (drag or hover).
    PointerMove {
        pointer_id: u8,
        x: i32,
        y: i32,
        pressure_q8: u16,
    },
    /// Pointer up / release.
    PointerUp { pointer_id: u8, x: i32, y: i32 },
    /// Pointer cancelled (gesture conflict, OS interruption).
    PointerCancel { pointer_id: u8 },
}

/// Leaf → host input envelope. Optionally carries a hit-test result
/// (`node_id`) so the host doesn't replay scene-state lookups.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputEnvelope {
    pub version: u8,
    pub display_id: DisplayId,
    /// Resolved by `SceneStore::hit_test` before publishing. `None`
    /// when the pointer is outside any interactive region.
    pub node_id: Option<NodeId>,
    pub event: InputEvent,
}

impl InputEnvelope {
    pub fn new(display_id: DisplayId, node_id: Option<NodeId>, event: InputEvent) -> Self {
        Self {
            version: WIRE_VERSION,
            display_id,
            node_id,
            event,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_tags_current_version() {
        let e = SceneEnvelope::new(0, Vec::new());
        assert_eq!(e.version, WIRE_VERSION);
        let i = InputEnvelope::new(
            0,
            None,
            InputEvent::PointerDown {
                pointer_id: 0,
                x: 0,
                y: 0,
                pressure_q8: 0,
            },
        );
        assert_eq!(i.version, WIRE_VERSION);
    }
}
