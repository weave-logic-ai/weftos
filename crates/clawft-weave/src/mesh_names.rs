//! Which `weftos://` authorities name this node's own mesh (ADR-114).
//!
//! The authority of a name is the mesh: its `MeshId` as 64 lowercase hex, or
//! a local alias in DNS form that this node maps to its own mesh. A name
//! whose authority is neither is refused with [`UNKNOWN_NAME`]: the same
//! words as for a thing the mesh does not have, so a name reveals nothing
//! about what exists elsewhere.
//!
//! The mesh id comes from the licence store (ADR-106); aliases from the
//! optional `<runtime>/mesh-aliases.json` (a JSON array of lowercase DNS
//! names).

use std::path::Path;

use crate::weftos_uri::Authority;

/// The one refusal for a name this node cannot resolve.
pub const UNKNOWN_NAME: &str = "no such name on this mesh";
/// Aliases file under the runtime dir.
pub const ALIASES_FILE: &str = "mesh-aliases.json";
const MAX_ALIASES: usize = 16;

/// This node's mesh, by id and by alias.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MeshNames {
    pub mesh: Option<[u8; 32]>,
    pub aliases: Vec<String>,
}

impl MeshNames {
    /// From the running daemon: the licence store's mesh id and the aliases
    /// file under `runtime_dir`.
    pub fn local(runtime_dir: Option<&Path>) -> Self {
        let mesh = crate::licence_boot::store().and_then(|s| s.local_mesh_id().get()).and_then(|m| {
            let mut id = [0u8; 32];
            hex::decode_to_slice(m.to_hex(), &mut id).ok().map(|_| id)
        });
        let aliases = runtime_dir.map(aliases_in).unwrap_or_default();
        Self { mesh, aliases }
    }

    /// Whether `a` names this mesh.
    pub fn accepts(&self, a: &Authority) -> bool {
        match a {
            Authority::Mesh(id) => self.mesh.as_ref() == Some(id),
            Authority::Alias(x) => self.aliases.iter().any(|y| y == x),
        }
    }

    /// The authority this node writes into names it mints: the mesh id when
    /// known, else the first alias.
    pub fn authority(&self) -> Option<Authority> {
        self.mesh.map(Authority::Mesh).or_else(|| self.aliases.first().cloned().map(Authority::Alias))
    }
}

/// Lowercase DNS aliases from the file; a malformed file is no aliases.
fn aliases_in(dir: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(dir.join(ALIASES_FILE)) else { return Vec::new() };
    serde_json::from_str::<Vec<String>>(&text)
        .unwrap_or_default()
        .into_iter()
        .filter(|a| matches!(crate::weftos_uri::WeftosUri::parse(&format!("weftos://{a}/nodes/x")), Ok(u) if matches!(u.authority, Authority::Alias(_))))
        .take(MAX_ALIASES)
        .collect()
}
