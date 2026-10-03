//! The mesh id from node config (ADR-106 section 3): the genesis pin and the
//! operator's `mesh_nonce`, both 64 hex chars, both in `kernel.mesh`.

use super::MeshId;
use crate::workload_pkg::codec::hex_decode_exact;

/// Why the configured pin or nonce gives no mesh id.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MeshIdConfigError {
    /// `genesis_hash` is not 64 hex chars.
    #[error("kernel.mesh.genesis_hash must be 64 hex characters")]
    BadGenesis,
    /// `mesh_nonce` is not 64 hex chars.
    #[error("kernel.mesh.mesh_nonce must be 64 hex characters (32 random bytes)")]
    BadNonce,
}

/// Derive the local mesh id. `Ok(None)` while either value is absent: the
/// licence path then stays inert (the policy equals `ManifestPolicy`). A
/// malformed value is an error, never a silent `None`, so the caller can say
/// why checkout stays off.
pub fn mesh_id_from_config(
    genesis_hash: Option<&str>,
    mesh_nonce: Option<&str>,
) -> Result<Option<MeshId>, MeshIdConfigError> {
    let nonce = match mesh_nonce {
        Some(n) => Some(hex_decode_exact::<32>(n).ok_or(MeshIdConfigError::BadNonce)?),
        None => None,
    };
    let pin = match genesis_hash {
        Some(g) => Some(hex_decode_exact::<32>(g).ok_or(MeshIdConfigError::BadGenesis)?),
        None => None,
    };
    Ok(match (pin, nonce) {
        (Some(p), Some(n)) => Some(MeshId::derive(&p, &n)),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PIN: &str = "aa00000000000000000000000000000000000000000000000000000000000001";
    const NONCE: &str = "bb00000000000000000000000000000000000000000000000000000000000002";

    #[test]
    fn needs_both_and_matches_derive() {
        assert_eq!(mesh_id_from_config(Some(PIN), None), Ok(None));
        assert_eq!(mesh_id_from_config(None, Some(NONCE)), Ok(None));
        let id = mesh_id_from_config(Some(PIN), Some(NONCE)).unwrap().unwrap();
        assert_eq!(id, MeshId::derive(&[0xaa, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1], &[0xbb, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2]));
        let other = "cc00000000000000000000000000000000000000000000000000000000000002";
        assert_ne!(Some(id), mesh_id_from_config(Some(PIN), Some(other)).unwrap());
    }

    #[test]
    fn malformed_values_are_errors() {
        assert_eq!(mesh_id_from_config(Some(PIN), Some("zz")), Err(MeshIdConfigError::BadNonce));
        assert_eq!(mesh_id_from_config(Some("12"), Some(NONCE)), Err(MeshIdConfigError::BadGenesis));
        assert_eq!(mesh_id_from_config(None, Some("zz")), Err(MeshIdConfigError::BadNonce));
    }
}
