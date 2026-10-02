//! The one node-id derivation (ADR-103 D11, ADR-025).
//!
//! `node_id = hex(SHA-256(ed25519 pubkey)[..16])`: 32 lowercase hex
//! characters. Every place that names a node (mesh handshake, cluster
//! membership, heartbeats, chain event sources, status RPC, substrate
//! `<node-id>` path segments, leaf registration) uses this value. Do
//! not hash a pubkey into an id anywhere else.

use sha2::{Digest, Sha256};

/// Number of hash bytes kept in a node id (16 bytes = 32 hex chars).
pub const NODE_ID_BYTES: usize = 16;

/// Length of a node id in hex characters.
pub const NODE_ID_HEX_LEN: usize = NODE_ID_BYTES * 2;

/// Derive the node id from an Ed25519 public key.
pub fn node_id_from_pubkey(pubkey: &[u8; 32]) -> String {
    use std::fmt::Write;
    let hash = Sha256::digest(pubkey);
    hash[..NODE_ID_BYTES]
        .iter()
        .fold(String::with_capacity(NODE_ID_HEX_LEN), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// True when `s` has the shape of a node id produced by
/// [`node_id_from_pubkey`] (32 lowercase hex characters).
pub fn is_node_id(s: &str) -> bool {
    s.len() == NODE_ID_HEX_LEN && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_and_32_hex() {
        let a = node_id_from_pubkey(&[7u8; 32]);
        assert_eq!(a, node_id_from_pubkey(&[7u8; 32]));
        assert_eq!(a.len(), 32);
        assert!(is_node_id(&a));
    }

    #[test]
    fn matches_the_project_key_id() {
        // A child's node_id must equal its certified project_key_id.
        for seed in [0u8, 2, 7, 255] {
            let pk = ed25519_dalek::SigningKey::from_bytes(&[seed; 32])
                .verifying_key()
                .to_bytes();
            assert_eq!(
                node_id_from_pubkey(&pk),
                clawft_types::project::cert::key_id(&pk)
            );
        }
    }

    #[test]
    fn differs_per_key() {
        assert_ne!(
            node_id_from_pubkey(&[1u8; 32]),
            node_id_from_pubkey(&[2u8; 32])
        );
    }

    #[test]
    fn matches_sha256_prefix_vector() {
        // SHA-256 of 32 zero bytes starts 66687aadf862bd776c8fc18b8e9f8e20.
        assert_eq!(
            node_id_from_pubkey(&[0u8; 32]),
            "66687aadf862bd776c8fc18b8e9f8e20"
        );
    }

    #[test]
    fn shape_check_rejects_legacy_and_uuid() {
        assert!(!is_node_id("n-3a7f9c"));
        assert!(!is_node_id("550e8400-e29b-41d4-a716-446655440000"));
        assert!(!is_node_id("66687AADF862BD776C8FC18B8E9F8E20"));
    }
}
