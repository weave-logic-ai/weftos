//! Node and display identifiers — see
//! [vector-leaf-display.md §5.2 NodeId namespacing](../../../docs/design/vector-leaf-display.md).
//!
//! `NodeId` is a packed `u32` carrying two fields:
//!
//! ```text
//!   31         24 23                                0
//!   ┌────────────┬─────────────────────────────────┐
//!   │ DisplayId  │            PathHash             │
//!   │   (u8)     │              (u24)              │
//!   └────────────┴─────────────────────────────────┘
//! ```
//!
//! The `PathHash` is computed from a producer-prefixed path (e.g.
//! `"kernel.ps"` + `[3, 2]`) via [`rustc_hash::FxHasher`]. The hash
//! MUST be deterministic and stable across leaf reboots; the glyph
//! cache and AABB cache in the renderer (Phase B) key off NodeId and
//! cannot tolerate drift.
//!
//! Collisions in the 24-bit space are statistically rare at the
//! expected scale (~10³ nodes per display); producers that need
//! collision-freedom should keep paths short and prefix-discrimi­nated.

use core::hash::Hasher;

use rustc_hash::FxHasher;
use serde::{Deserialize, Serialize};

/// Per-leaf display index. 0 is the implicit single-display leaf.
pub type DisplayId = u8;

/// Packed `[DisplayId:8 | PathHash:24]`. See module docs.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NodeId(pub u32);

impl NodeId {
    /// Construct directly from raw bits. Prefer [`NodeId::from_parts`]
    /// or [`path_to_id`] in producer code.
    #[inline]
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// Compose a NodeId from its `DisplayId` byte and a 24-bit path
    /// hash. The hash's top 8 bits are discarded.
    #[inline]
    pub const fn from_parts(display: DisplayId, path_hash_u24: u32) -> Self {
        Self(((display as u32) << 24) | (path_hash_u24 & 0x00FF_FFFF))
    }

    /// Extract the DisplayId byte.
    #[inline]
    pub const fn display_id(self) -> DisplayId {
        ((self.0 >> 24) & 0xFF) as u8
    }

    /// Extract the 24-bit path hash.
    #[inline]
    pub const fn path_hash(self) -> u32 {
        self.0 & 0x00FF_FFFF
    }

    /// Raw `u32` for wire / map keys.
    #[inline]
    pub const fn raw(self) -> u32 {
        self.0
    }
}

/// Producer-side helper: hash `producer` + `path` into a stable
/// 24-bit value and combine with `display` to form a NodeId.
///
/// This is the canonical path for host producers. Embedded code that
/// receives NodeIds over the wire does not call this — it just stores
/// the bits.
///
/// # Determinism
///
/// `FxHasher` is seeded from the empty state; the same `(producer,
/// path)` always maps to the same NodeId on any platform. This is the
/// reason `fxhash` / `rustc-hash` is preferred over `SipHash` (which
/// is random-seeded per process).
pub fn path_to_id(display: DisplayId, producer: &str, path: &[u16]) -> NodeId {
    let mut h = FxHasher::default();
    h.write(producer.as_bytes());
    // Length separator so ("ab", []) ≠ ("a", [0x62]).
    h.write_u8(b':');
    for p in path {
        h.write_u16(*p);
    }
    let full = h.finish() as u32;
    NodeId::from_parts(display, full)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_unpack_roundtrip() {
        let id = NodeId::from_parts(7, 0x00AB_CDEF);
        assert_eq!(id.display_id(), 7);
        assert_eq!(id.path_hash(), 0x00AB_CDEF);
        assert_eq!(id.raw(), 0x07AB_CDEF);
    }

    #[test]
    fn from_parts_drops_high_bits_of_path() {
        // path_hash must be masked to 24 bits.
        let id = NodeId::from_parts(0, 0xFFFF_FFFF);
        assert_eq!(id.path_hash(), 0x00FF_FFFF);
        assert_eq!(id.display_id(), 0);
    }

    #[test]
    fn path_to_id_is_deterministic() {
        let a = path_to_id(0, "kernel.ps", &[3, 2]);
        let b = path_to_id(0, "kernel.ps", &[3, 2]);
        assert_eq!(a, b);
    }

    #[test]
    fn path_to_id_distinguishes_producer() {
        let a = path_to_id(0, "kernel.ps", &[3, 2]);
        let b = path_to_id(0, "kernel.log", &[3, 2]);
        assert_ne!(a, b);
    }

    #[test]
    fn path_to_id_distinguishes_path() {
        let a = path_to_id(0, "kernel.ps", &[3, 2]);
        let b = path_to_id(0, "kernel.ps", &[3, 3]);
        assert_ne!(a, b);
    }

    #[test]
    fn path_to_id_distinguishes_display() {
        let a = path_to_id(0, "kernel.ps", &[3, 2]);
        let b = path_to_id(1, "kernel.ps", &[3, 2]);
        assert_ne!(a, b);
        assert_eq!(a.path_hash(), b.path_hash());
        assert_eq!(a.display_id(), 0);
        assert_eq!(b.display_id(), 1);
    }

    #[test]
    fn length_separator_disambiguates() {
        // Without the b':' separator, ("ab", []) and ("a", [0x62])
        // could collide. Confirm they don't.
        let a = path_to_id(0, "ab", &[]);
        let b = path_to_id(0, "a", &[0x62]);
        assert_ne!(a, b);
    }
}
