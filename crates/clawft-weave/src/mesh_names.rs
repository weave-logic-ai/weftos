//! Which `weftos://` authority names this node's own mesh (ADR-114).
//!
//! The authority of a name is the mesh's `MeshId`, 64 lowercase hex, and
//! nothing else: friendly mesh names are dashboard display labels and never
//! appear in a stored, chained, granted or transmitted name (owner decision,
//! 2026-10-09). A name whose authority is another mesh is refused with
//! [`UNKNOWN_NAME`], the same words as for a thing this mesh does not have,
//! so a name reveals nothing about what exists elsewhere.
//!
//! The mesh id comes from the licence store (ADR-106). A node without one
//! cannot mint or resolve names and says so.

use crate::weftos_uri::Authority;

/// The one refusal for a name this node cannot resolve.
pub const UNKNOWN_NAME: &str = "no such name on this mesh";

/// This node's mesh.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MeshNames {
    pub mesh: Option<[u8; 32]>,
}

impl MeshNames {
    /// From the running daemon: the licence store's mesh id.
    pub fn local() -> Self {
        let mesh = crate::licence_boot::store().and_then(|s| s.local_mesh_id().get()).and_then(|m| {
            let mut id = [0u8; 32];
            hex::decode_to_slice(m.to_hex(), &mut id).ok().map(|_| id)
        });
        Self { mesh }
    }

    /// Whether `a` names this mesh.
    pub fn accepts(&self, a: &Authority) -> bool {
        self.mesh.as_ref() == Some(&a.0)
    }

    /// The authority this node writes into names it mints, when it has a mesh.
    pub fn authority(&self) -> Option<Authority> {
        self.mesh.map(Authority)
    }
}
