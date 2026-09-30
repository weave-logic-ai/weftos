# ADR-025: Ed25519 Public Key as Node Identity

**Date**: 2026-04-03
**Status**: Accepted
**Deciders**: K5 Symposium Security Panel (D2, S1), Mesh Architecture Panel (M2)
**Depends-On**: None (foundational)
**Amended by**: ADR-103 D11 (node-id hash is SHA-256, not SHAKE-256)

## Context

WeftOS nodes have historically used UUID-based `NodeId` values generated at startup. UUIDs provide uniqueness but not authentication -- any node can claim any UUID, and there is no mechanism to prove identity without a separate authentication layer. K6 introduces multi-node mesh networking where nodes must authenticate to each other during cluster join, chain replication, and remote service invocation. Building a separate authentication system on top of UUIDs (e.g., shared secrets, certificate authority) would add complexity and infrastructure. Meanwhile, `chain.rs` already uses Ed25519 signing via `ed25519-dalek` for ExoChain event signatures, so cryptographic identity primitives are already in the dependency tree.

## Decision

Node identity is derived from an Ed25519 keypair. The node ID is computed as `hex(SHA-256(pubkey)[0..16])` -- the first 16 bytes of the SHA-256 hash of the Ed25519 public key, rendered as a 32-character hex string. The short display form uses the first 8 bytes (`hex(node_id[0..8])`, 16 characters) for human readability.

The `NodeIdentity` struct in the kernel holds the full keypair:

```rust
pub struct NodeIdentity {
    keypair: ed25519_dalek::SigningKey,
    node_id: [u8; 16],       // SHA-256(pubkey)[0..16]
    short_id: String,         // hex(node_id[0..8])
}
```

Key operations:
- `NodeIdentity::generate()` -- create a new random identity at first boot
- `NodeIdentity::from_file(path)` -- load from a persisted key file (`$WEFTOS_DATA/identity.key`, mode 0600)
- `NodeIdentity::public_key()` -- returns the `ed25519_dalek::VerifyingKey` (32 bytes)
- `NodeIdentity::sign(data)` -- signs arbitrary bytes with the node's private key

No UUID, no certificate authority, no shared secret distribution. The trust root is `governance.genesis`, which records the founding node's public key and cluster policy. The cluster ID itself is `SHAKE-256(genesis_payload)` (unchanged by D11, which covers the node-id hash only).

### Identity Hierarchy

```
governance.genesis
    +-- Founding node pubkey (trust root)
    +-- Governance rules (who can join, what capabilities are valid)
    +-- Cluster ID = SHAKE-256(genesis payload)
         +-- Node A pubkey (joined, signed genesis_hash)
         +-- Node B pubkey (joined, signed genesis_hash)
```

### Cluster Join Authentication

When a new node sends a `JoinRequest`, the receiving node verifies:
1. The `genesis_hash` matches the local genesis (same cluster)
2. The `JoinRequest` signature is valid against the joining node's Ed25519 public key
3. The `GovernanceGate` approves the join via `cluster.join` action evaluation
4. The timestamp is within 60-second skew tolerance

On acceptance, the peer ID is computed as `hex(SHA-256(pubkey)[0..16])` and added to `ClusterMembership`.

## Consequences

### Positive
- Every node can authenticate itself by signing a challenge -- no shared secret distribution or CA infrastructure needed
- Node ID is deterministically derived from the public key: given a pubkey, anyone can compute the expected node ID and verify consistency
- Consistent with Ed25519 signing already used in `chain.rs` for ExoChain events -- no new cryptographic primitives
- `governance.genesis` as the trust root requires no new infrastructure; it already exists as the first chain event
- Key rotation is auditable: a `KeyRotation` chain event dual-signed by old and new keys provides verifiable identity transition

### Negative
- Every module that references a node (`cluster.rs`, `mesh_*.rs`, `ipc.rs` `GlobalPid`, `PeerNode`) must use the key-derived identity instead of UUID -- broad refactor across the kernel
- Cannot change the identity derivation (SHA-256 truncation, Ed25519 curve) without a cluster-wide re-keying event
- Key storage security becomes critical: compromising `identity.key` compromises the node's mesh identity, chain signing authority, and cluster membership
- 16-byte truncated hash has a collision space of 2^128; sufficient for practical purposes but theoretically weaker than the full 32-byte Ed25519 public key

### Neutral
- Platform-specific key storage: CloudNative/Edge use file permissions (0600), Browser uses IndexedDB or session-scoped ephemeral keys, WASI receives keys via host injection
- Browser nodes default to session-scoped ephemeral keypairs (new identity each session) unless the user opts into IndexedDB persistence
- The node-id hash is SHA-256 (amended by ADR-103 D11, matching `cluster.rs` and `node_id_from_pubkey`); the cluster ID still uses SHAKE-256
