# K5 Panel 4: K6 Implementation Plan

**Date**: 2026-03-25
**Panel**: Implementation Planning
**Status**: COMPLETE

---

## 1. Overview

K6 delivers the transport-agnostic encrypted mesh network for WeftOS in 6
sub-phases. Each phase is independently testable and builds on the previous.

### Phase Summary

| Phase | Scope | New Lines | Changed Lines | New Deps |
|-------|-------|:---------:|:------------:|----------|
| K6.0 | Prep changes to existing code | ~50 | ~150 | None |
| K6.1 | Transport layer + Noise encryption | ~400 | ~20 | quinn, snow, x25519-dalek |
| K6.2 | Discovery (Kademlia + mDNS) | ~300 | ~30 | libp2p-kad, libp2p-mdns (optional) |
| K6.3 | Cross-node IPC (A2A over mesh) | ~300 | ~80 | None |
| K6.4 | Chain replication + tree sync | ~250 | ~50 | None |
| K6.5 | Distributed process table + service discovery | ~200 | ~40 | None |
| **Total** | | **~1,500** | **~370** | |

---

## 2. K6.0: Prep Changes (~200 lines total)

**Goal**: Modify existing K0-K5 code to accept K6 extensions without
breaking existing single-node behavior.

### 2.1 Add `RemoteNode` to `MessageTarget`

**File**: `crates/clawft-kernel/src/ipc.rs`

```rust
// Add to MessageTarget enum:
pub enum MessageTarget {
    // ... existing variants ...

    /// Route to a specific process on a remote node.
    RemoteNode {
        node_id: String,
        target: Box<MessageTarget>,
    },
}
```

**Impact**: `A2ARouter::send()` gains a new match arm that returns
`Err(IpcError::RemoteNotAvailable)` until K6.1 wires the mesh transport.
All existing code continues to work -- the new variant is never constructed
by K0-K5 code.

### 2.2 Add `GlobalPid`

**File**: `crates/clawft-kernel/src/ipc.rs`

```rust
/// Globally unique process identifier: (node_id, local_pid).
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct GlobalPid {
    pub node_id: String,
    pub pid: Pid,
}

impl GlobalPid {
    pub fn local(pid: Pid, node_id: &str) -> Self {
        Self { node_id: node_id.to_string(), pid }
    }

    pub fn is_local(&self, my_node_id: &str) -> bool {
        self.node_id == my_node_id
    }
}
```

### 2.3 Extend `ClusterConfig`

**File**: `crates/clawft-kernel/src/cluster.rs`

```rust
// Add to ClusterConfig:
pub struct ClusterConfig {
    // ... existing fields ...

    /// Address to bind the mesh listener (e.g., "0.0.0.0:9470")
    pub bind_address: Option<String>,

    /// Seed peers for bootstrap discovery
    pub seed_peers: Vec<String>,

    /// Path to Ed25519 identity key file
    pub identity_key_path: Option<PathBuf>,
}
```

### 2.4 Add `NodeIdentity`

**File**: `crates/clawft-kernel/src/cluster.rs` (or new `identity.rs`)

```rust
/// Node identity derived from Ed25519 keypair.
pub struct NodeIdentity {
    keypair: ed25519_dalek::SigningKey,
    node_id: String,  // hex(SHAKE-256(pubkey)[0..16])
}
```

Uses `ed25519-dalek` which is already a dependency via chain signing.

### 2.5 Test Strategy for K6.0

| Test | Verifies |
|------|----------|
| `RemoteNode` serde roundtrip | New variant serializes/deserializes |
| `GlobalPid` equality | `(node_a, pid_5) != (node_b, pid_5)` |
| `ClusterConfig` defaults | New fields default to `None` / empty |
| `NodeIdentity` generate + sign + verify | Ed25519 keypair operations |
| Existing tests pass | No regression from new fields |

---

## 3. K6.1: Transport Layer + Noise Encryption (~420 lines)

**Goal**: Build the core mesh transport with encrypted connections.

### 3.1 New Files

| File | Purpose | Lines |
|------|---------|:-----:|
| `mesh.rs` | `MeshTransport` trait, `MeshStream`, `TransportListener` | ~60 |
| `mesh_quic.rs` | QUIC transport via quinn | ~120 |
| `mesh_noise.rs` | Noise Protocol wrapper via snow | ~100 |
| `mesh_framing.rs` | Length-prefix framing + message type dispatch | ~60 |
| `mesh_listener.rs` | Accept loop, handshake, peer registration | ~80 |

All behind `#[cfg(feature = "mesh")]`.

### 3.2 New Dependencies

```toml
[dependencies]
quinn = { version = "0.11", optional = true }
snow = { version = "0.9", optional = true }
x25519-dalek = { version = "2.0", optional = true, features = ["static_secrets"] }

[features]
mesh = ["quinn", "snow", "x25519-dalek"]
```

### 3.3 Connection Flow

```
1. quinn::Endpoint::server(config, bind_addr)     -- start QUIC listener
2. endpoint.accept() -> Connection                  -- accept incoming
3. connection.accept_bi() -> (SendStream, RecvStream) -- get byte stream
4. snow::Builder::new(params).build_responder()     -- create Noise session
5. Noise XX handshake over the bi-directional stream
6. NoiseStream wraps (SendStream, RecvStream)       -- encrypted I/O
7. Read framed messages from NoiseStream
8. Dispatch to IPC / chain sync / tree sync handlers
```

### 3.4 Test Strategy for K6.1

| Test | Verifies |
|------|----------|
| Noise XX handshake roundtrip | Two snow sessions complete handshake |
| Noise IK handshake roundtrip | Known-peer 1-RTT handshake |
| QUIC connect + send + recv | quinn transport works end-to-end |
| Framing encode/decode | Length-prefix framing correctness |
| Max message size enforcement | Messages >16 MiB rejected |
| Invalid Noise handshake rejected | Corrupted handshake data fails cleanly |

---

## 4. K6.2: Discovery (~330 lines)

**Goal**: Peer discovery via Kademlia DHT and mDNS.

### 4.1 New Files

| File | Purpose | Lines |
|------|---------|:-----:|
| `mesh_discovery.rs` | Discovery trait + coordinator | ~80 |
| `mesh_kad.rs` | Kademlia DHT wrapper (libp2p-kad) | ~120 |
| `mesh_mdns.rs` | mDNS local discovery (libp2p-mdns) | ~80 |
| `mesh_bootstrap.rs` | Static seed peer bootstrap | ~50 |

### 4.2 New Dependencies (Optional)

```toml
[dependencies]
libp2p-kad = { version = "0.46", optional = true }
libp2p-mdns = { version = "0.46", optional = true }

[features]
mesh-discovery = ["libp2p-kad", "libp2p-mdns"]
mesh-full = ["mesh", "mesh-discovery"]
```

Discovery is optional because some deployments (e.g., Kubernetes) use
external service discovery. Static seed peers (`ClusterConfig.seed_peers`)
work without libp2p dependencies.

### 4.3 Discovery Flow

```
Boot:
  1. Load seed_peers from ClusterConfig
  2. Connect to each seed peer (K6.1 transport)
  3. Exchange peer lists during WeftOS handshake
  4. If mesh-discovery enabled:
     a. Start Kademlia DHT with discovered peers as initial routing table
     b. Start mDNS listener for LAN peers
  5. Periodically:
     a. Query DHT for peers near own NodeId
     b. Process mDNS announcements
     c. Connect to newly discovered peers
     d. Update ClusterMembership
```

### 4.4 Test Strategy for K6.2

| Test | Verifies |
|------|----------|
| Bootstrap from seed peers | Connect to 3 seeds, discover 5 more |
| mDNS announcement + discovery | Two nodes on same LAN find each other |
| Kademlia put/get | Store and retrieve peer info from DHT |
| Peer list exchange | WeftOS handshake shares known peers |
| Discovery event -> ClusterMembership | Discovered peers appear in membership |

---

## 5. K6.3: Cross-Node IPC (~380 lines)

**Goal**: Route `KernelMessage` across nodes transparently.

### 5.1 Changes to Existing Files

| File | Change | Lines |
|------|--------|:-----:|
| `ipc.rs` | `KernelIpc::send()` transport fork for `RemoteNode` | ~40 |
| `a2a.rs` | `A2ARouter` cluster-aware service resolution | ~50 |
| `a2a.rs` | Remote inbox delivery bridge | ~60 |

### 5.2 New Files

| File | Purpose | Lines |
|------|---------|:-----:|
| `mesh_ipc.rs` | Serialize/deserialize KernelMessage over mesh streams | ~80 |
| `mesh_service.rs` | Cross-node service registry query protocol | ~70 |
| `mesh_dedup.rs` | Message deduplication (bloom filter on message IDs) | ~80 |

### 5.3 Remote Message Flow

```
Node A                                    Node B
  |                                         |
  Agent sends KernelMessage with target:    |
    RemoteNode { node_id: "B", target:      |
      Service("myservice") }                |
  |                                         |
  KernelIpc::send()                         |
    -> detects RemoteNode target            |
    -> looks up Node B in ClusterMembership |
    -> gets MeshStream for Node B           |
    -> serializes KernelMessage as framed   |
       RVF segment                          |
    -> sends over encrypted MeshStream      |
  |                                         |
  |---------- encrypted wire ------------->|
  |                                         |
  |                           mesh_ipc handler receives
  |                           -> deserializes KernelMessage
  |                           -> checks message dedup
  |                           -> unwraps RemoteNode target
  |                           -> passes inner target to local
  |                              A2ARouter::send()
  |                           -> GovernanceGate evaluates
  |                           -> delivers to local inbox
  |                                         |
  |<-------- response (if correlation_id) --|
```

### 5.4 Test Strategy for K6.3

| Test | Verifies |
|------|----------|
| Remote message roundtrip | Send from A, receive on B, response back |
| Service resolution across nodes | Service on B discoverable from A |
| Governance gate on remote messages | Remote message denied by gate |
| Message deduplication | Duplicate message ID rejected on second delivery |
| GlobalPid in response | Response carries correct GlobalPid |

---

## 6. K6.4: Chain Replication + Tree Sync (~300 lines)

**Goal**: Synchronize chain events and resource tree state across nodes.

### 6.1 Chain Replication

**File changes**: `chain.rs` + new `mesh_chain.rs`

```rust
// Add to LocalChain:
impl LocalChain {
    /// Return events from sequence `after` to head (inclusive).
    /// Used for incremental replication.
    pub fn tail_from(&self, after: u64) -> Vec<ChainEvent> {
        self.events.iter()
            .filter(|e| e.sequence > after)
            .cloned()
            .collect()
    }

    /// Subscribe to new chain events (push notification).
    pub fn subscribe(&self) -> broadcast::Receiver<ChainEvent> {
        self.event_tx.subscribe()
    }
}
```

New `mesh_chain.rs` (~120 lines):
- `ChainSyncRequest { chain_id, after_sequence }` -- pull delta
- `ChainSyncResponse { events: Vec<ChainEvent> }` -- delta payload
- `BridgeEvent` chain event kind -- anchors remote chain head hash
- Push-based subscription forwarding over mesh streams

### 6.2 Tree Sync

**File changes**: `tree_manager.rs` + new `mesh_tree.rs`

```rust
// Add to TreeManager:
impl TreeManager {
    /// Serializable snapshot of the full tree state.
    pub fn snapshot(&self) -> TreeSnapshot { /* ... */ }

    /// Apply a remote mutation with signature verification.
    pub fn apply_remote_mutation(
        &self,
        event: MutationEvent,
        node_pubkey: &[u8; 32],
    ) -> Result<()> { /* ... */ }
}
```

New `mesh_tree.rs` (~80 lines):
- `TreeSyncRequest { root_hash }` -- compare roots
- `TreeSyncResponse { diff: Vec<MutationEvent> }` or full snapshot
- Merkle proof generation for lightweight verification

### 6.3 Test Strategy for K6.4

| Test | Verifies |
|------|----------|
| `tail_from(0)` returns all events | Full chain export |
| `tail_from(n)` returns only new events | Incremental sync |
| Chain sync over mesh | Node B pulls events from Node A |
| Bridge event anchoring | Node A embeds Node B's chain head |
| Tree snapshot roundtrip | Snapshot serializes and deserializes |
| Remote mutation with valid signature | Applied to tree |
| Remote mutation with invalid signature | Rejected |
| Root hash comparison | Matching hashes skip sync |

### 6.4 K6.4b: Hybrid Post-Quantum Key Exchange (~100 lines)

**Goal**: Protect mesh transport against store-now-decrypt-later quantum attacks
by adding ML-KEM-768 key encapsulation on top of the classical Noise XX channel.

**Protocol**:
1. Noise XX (X25519 DH) completes → classical shared secret established
2. Initiator sends ML-KEM-768 ephemeral pubkey (1,184 bytes) over Noise channel
3. Responder encapsulates → sends ciphertext (1,088 bytes) back
4. Both derive: `final_key = HKDF(classical_ss || pq_ss || "weftos-hybrid-kem-v1")`
5. Rekey the transport with `final_key`

**Negotiation**: Advertised via `kem_supported: bool` in the Noise handshake
payload (`WeftHandshake` struct). Nodes that don't support KEM stay on
classical Noise -- graceful degradation, no connection failure.

**Dependencies**: `ruvector-dag` with `production-crypto` feature provides
`MlKem768::generate_keypair()`, `encapsulate()`, `decapsulate()` via
`ruvector-dag/src/qudag/crypto/ml_kem.rs`.

**Files**:

| File | Purpose | Lines |
|------|---------|:-----:|
| `mesh_handshake.rs` | KEM upgrade step after Noise XX completes (flat layout: `crates/clawft-kernel/src/mesh_handshake.rs`) | ~60 |
| Changes to `mesh_noise.rs` | Add `kem_supported` to handshake payload, rekey after KEM | ~30 |
| Changes to `mesh_listener.rs` | Wire KEM upgrade into accept path | ~10 |

**Cost**: ~2.4KB extra per connection handshake, ~1ms latency. Zero per-message
overhead after rekey.

**Test Strategy for K6.4b**:

| Test | Verifies |
|------|----------|
| Hybrid handshake completes (both KEM-capable) | Full PQ upgrade path |
| Graceful fallback (one side lacks KEM) | Classical-only still works |
| Rekey produces different key than classical-only | KEM material contributes |
| Wrong KEM key fails decapsulation | Ciphertext integrity |
| `kem_supported: false` in payload | Negotiation flag respected |

---

## 7. K6.5: Distributed Process Table + Service Discovery (~240 lines)

**Goal**: Cluster-wide process and service visibility.

### 7.1 New Files

| File | Purpose | Lines |
|------|---------|:-----:|
| `mesh_process.rs` | Distributed process table (CRDT-based) | ~100 |
| `mesh_service_adv.rs` | Service advertisement and resolution | ~80 |
| `mesh_heartbeat.rs` | SWIM-style heartbeat + failure detection | ~60 |

### 7.2 Distributed Process Table

Each node maintains a local `ProcessTable`. The mesh layer gossips process
summaries using ruvector-delta-consensus CRDTs:

```rust
/// Process summary advertised to peer nodes.
#[derive(Serialize, Deserialize, Clone)]
pub struct ProcessAdvertisement {
    pub global_pid: GlobalPid,
    pub agent_type: String,
    pub capabilities: Vec<String>,
    pub services: Vec<String>,
    pub status: ProcessStatus,
    pub last_updated: u64,
}
```

Peers merge advertisements using last-writer-wins semantics. A process that
stops heartbeating is marked `Unreachable` after `suspect_threshold`.

### 7.3 Service Advertisement

```rust
/// Service advertised to the cluster.
#[derive(Serialize, Deserialize, Clone)]
pub struct ServiceAdvertisement {
    pub name: String,
    pub methods: Vec<String>,
    pub node_id: String,
    pub global_pid: GlobalPid,
    pub version: String,
    pub metadata: HashMap<String, String>,
}
```

When `A2ARouter` cannot resolve a service locally, it queries the distributed
service table. If the service exists on a remote node, the router wraps the
message in a `RemoteNode` target and sends it over the mesh.

### 7.4 Test Strategy for K6.5

| Test | Verifies |
|------|----------|
| Process advertisement gossip | Node A's process visible on Node B |
| Service discovery across nodes | Service on B resolvable from A |
| Failure detection | Stopped node marked Unreachable |
| CRDT merge | Concurrent updates converge |
| Service resolution fallback | Local first, then remote |

---

## 8. Dependency Summary

### Required (mesh feature)

| Crate | Version | Purpose |
|-------|---------|---------|
| `quinn` | 0.11 | QUIC transport with multiplexing |
| `snow` | 0.9 | Noise Protocol Framework |
| `x25519-dalek` | 2.0 | X25519 Diffie-Hellman for Noise |

### Optional (mesh-discovery feature)

| Crate | Version | Purpose |
|-------|---------|---------|
| `libp2p-kad` | 0.46 | Kademlia DHT peer discovery |
| `libp2p-mdns` | 0.46 | mDNS LAN peer discovery |

### Already Available

| Crate | Used For |
|-------|----------|
| `ed25519-dalek` | Node identity signing (already in chain.rs) |
| `rvf-wire` | Zero-copy wire format (already in workspace) |
| `tokio` | Async runtime (already used throughout) |
| `serde` / `serde_json` | Serialization (already used throughout) |
| `dashmap` | Concurrent maps (already in cluster.rs) |

---

## 9. Feature Gate Structure

```toml
[features]
default = []

# Core mesh transport (QUIC + Noise)
mesh = ["quinn", "snow", "x25519-dalek"]

# DHT + mDNS discovery
mesh-discovery = ["mesh", "libp2p-kad", "libp2p-mdns"]

# Full networking stack
mesh-full = ["mesh", "mesh-discovery"]
```

### Build Configurations

| Build | Features | Use Case |
|-------|----------|----------|
| Single-node | (none) | Development, testing, embedded |
| Static cluster | `mesh` | Known peers, seed-peer bootstrap |
| Dynamic cluster | `mesh-full` | DHT + mDNS auto-discovery |

---

## 10. Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|-----------|
| quinn API instability | Low | Medium | Pin to 0.11, vendor if needed |
| snow Noise bugs | Very Low | High | snow is WireGuard-grade, well-audited |
| libp2p-kad without full libp2p | Medium | Medium | Fallback to static discovery |
| CRDT convergence delays | Medium | Low | Bounded convergence with anti-entropy |
| Browser WebSocket limitations | Low | Medium | Degrade gracefully, restrict capabilities |
| Post-quantum transition | Low | Low | Dual signing + hybrid KEM in K6.4b (D11) |

---

## 11. Success Criteria

K6 is complete when:

1. Two Cloud nodes connect via QUIC with Noise encryption
2. A Browser node connects via WebSocket with Noise encryption
3. Nodes discover each other via seed peers (and optionally DHT/mDNS)
4. `KernelMessage` routes transparently between nodes
5. Chain events replicate incrementally between nodes
6. Resource tree state synchronizes between nodes
7. Services on any node are discoverable from any other node
8. GovernanceGate enforces policy on all remote operations
9. All existing single-node tests pass unchanged
10. The `mesh` feature gate compiles to zero networking code when disabled
