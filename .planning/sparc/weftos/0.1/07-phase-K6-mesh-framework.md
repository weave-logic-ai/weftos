# Phase K6: Transport-Agnostic Encrypted Mesh Network

**Phase ID**: K6
**Workstream**: W-KERNEL
**Duration**: Weeks 11-16 (6 sub-phases)
**Goal**: Extend WeftOS from a single-node kernel into a multi-node cluster with encrypted peer-to-peer networking, distributed IPC, chain replication, tree synchronization, and cluster-wide service discovery
**Gate from**: K2 C10, K5 Symposium (2026-03-25)
**Symposium Decisions**: D1-D15, Commitments C1-C5

---

## S -- Specification

### What Changes

This phase adds a complete mesh networking stack to WeftOS. The mesh is
transport-agnostic: Cloud and Edge nodes use QUIC (quinn), Browser and WASI
nodes use WebSocket, and the protocol treats the transport as a pluggable
implementation detail. All inter-node traffic is encrypted using the Noise
Protocol (snow). Peer discovery uses Kademlia DHT and mDNS. The existing
single-node kernel continues to work unchanged when the `mesh` feature gate
is disabled.

The architecture follows the 5-layer model approved by the K5 Symposium
(Panel 1, Decision D1):

```
APPLICATION    WeftOS IPC (A2ARouter), Chain Sync, Tree Sync
DISCOVERY      Kademlia DHT, mDNS, Bootstrap Peers
ENCRYPTION     Noise Protocol (snow) -- XX/IK handshakes, Ed25519 keys
TRANSPORT      quinn (QUIC) | tokio-tungstenite (WS) | webrtc-rs
IDENTITY       Ed25519 keypair = node identity, governance.genesis = trust root
```

### Files to Create

> **Layout standard: flat.** All mesh modules live directly under
> `crates/clawft-kernel/src/` as `mesh_*.rs`. There is no `mesh/`
> subdirectory; any reference to `mesh/<name>.rs` in older drafts is
> superseded by `mesh_<name>.rs`. The single exception in the tree
> (`assessment/mesh.rs`) is part of the unrelated `assessment/` module
> and intentionally not part of the mesh-framework namespace.

| File | Phase | Purpose |
|------|-------|---------|
| `crates/clawft-kernel/src/mesh.rs` | K6.1 | `MeshTransport` trait, `MeshStream`, `TransportListener` |
| `crates/clawft-kernel/src/mesh_quic.rs` | K6.1 | QUIC transport implementation via quinn |
| `crates/clawft-kernel/src/mesh_noise.rs` | K6.1 | Noise Protocol wrapper via snow (XX + IK handshakes) |
| `crates/clawft-kernel/src/mesh_framing.rs` | K6.1 | Length-prefix framing + message type dispatch |
| `crates/clawft-kernel/src/mesh_listener.rs` | K6.1 | Accept loop, handshake orchestration, peer registration |
| `crates/clawft-kernel/src/mesh_discovery.rs` | K6.2 | Discovery trait + coordinator |
| `crates/clawft-kernel/src/mesh_kad.rs` | K6.2 | Kademlia DHT wrapper (libp2p-kad) |
| `crates/clawft-kernel/src/mesh_mdns.rs` | K6.2 | mDNS local discovery (libp2p-mdns) |
| `crates/clawft-kernel/src/mesh_bootstrap.rs` | K6.2 | Static seed peer bootstrap |
| `crates/clawft-kernel/src/mesh_adapter.rs` | K6.3 | `MeshAdapter` -- incoming mesh dispatch through local A2ARouter |
| `crates/clawft-kernel/src/mesh_ipc.rs` | K6.3 | Serialize/deserialize KernelMessage over mesh streams |
| `crates/clawft-kernel/src/mesh_service.rs` | K6.3 | Cross-node service registry query protocol |
| `crates/clawft-kernel/src/mesh_dedup.rs` | K6.3 | Message deduplication (bloom filter on message IDs) |
| `crates/clawft-kernel/src/mesh_chain.rs` | K6.4 | Chain replication: delta sync, bridge events, subscription forwarding |
| `crates/clawft-kernel/src/mesh_tree.rs` | K6.4 | Tree sync: snapshot transfer, Merkle proof exchange |
| `crates/clawft-kernel/src/mesh_handshake.rs` | K6.4b | Optional ML-KEM-768 hybrid KEM upgrade after Noise XX |
| `crates/clawft-kernel/src/mesh_process.rs` | K6.5 | Distributed process table (CRDT gossip) |
| `crates/clawft-kernel/src/mesh_service_adv.rs` | K6.5 | Service advertisement and cluster-wide resolution |
| `crates/clawft-kernel/src/mesh_heartbeat.rs` | K6.5 | SWIM-style heartbeat + failure detection |

### Files to Modify

| File | Change | Phase |
|------|--------|-------|
| `crates/clawft-kernel/Cargo.toml` | Add quinn, snow, x25519-dalek (optional); feature gates mesh/mesh-discovery/mesh-full | K6.0 |
| `crates/clawft-kernel/src/ipc.rs` | Add `MessageTarget::RemoteNode` variant; add `GlobalPid` struct | K6.0 |
| `crates/clawft-kernel/src/cluster.rs` | Add `bind_address`, `seed_peers`, `identity_key_path` to `ClusterConfig`; add `NodeIdentity` | K6.0 |
| `crates/clawft-kernel/src/chain.rs` | Add `tail_from(seq)` for incremental replication; add chain event subscription | K6.0, K6.4 |
| `crates/clawft-kernel/src/tree_manager.rs` | Add `snapshot()`, `apply_remote_mutation()` with signature verification; sign `MutationEvent.signature` | K6.0, K6.4 |
| `crates/clawft-kernel/src/lib.rs` | Re-export mesh modules behind `#[cfg(feature = "mesh")]` | K6.1 |
| `crates/clawft-kernel/src/a2a.rs` | Add cluster-aware service resolution; remote inbox delivery bridge | K6.3 |
| `crates/clawft-kernel/src/boot.rs` | Add mesh listener startup + peer discovery to boot sequence | K6.1 |
| `crates/clawft-kernel/src/service.rs` | Add cross-node service advertisement | K6.5 |
| `Cargo.toml` (workspace) | Add quinn, snow, x25519-dalek, optionally libp2p-kad/libp2p-mdns | K6.0 |

### Key Types

**MeshTransport trait** (`mesh.rs`) -- Symposium C3:
```rust
#[async_trait]
pub trait MeshTransport: Send + Sync + 'static {
    fn name(&self) -> &str;
    async fn listen(&self, addr: &str) -> Result<TransportListener>;
    async fn connect(&self, addr: &str) -> Result<MeshStream>;
    fn supports(&self, addr: &str) -> bool;
}
```

**NoiseChannel** (`mesh_noise.rs`) -- Symposium D3:
```rust
pub struct NoiseChannel {
    session: snow::TransportState,
    stream: Box<dyn MeshStream>,
}
```

**PeerId / NodeIdentity** (`cluster.rs`) -- Symposium D2:
```rust
pub struct NodeIdentity {
    keypair: ed25519_dalek::SigningKey,
    node_id: String,  // hex(SHAKE-256(pubkey)[0..16])
}
```

**GlobalPid** (`ipc.rs`) -- Symposium C2:
```rust
pub struct GlobalPid {
    pub node_id: String,
    pub pid: Pid,
}
```

**MeshConfig** (`cluster.rs`):
```rust
pub struct MeshConfig {
    pub bind_address: String,
    pub seed_peers: Vec<String>,
    pub identity_key_path: Option<PathBuf>,
    pub max_message_size: usize,  // default 16 MiB (D8)
}
```

#### Transport Types

```rust
/// A bidirectional byte stream over any transport.
pub trait MeshStream: AsyncRead + AsyncWrite + Send + Unpin + 'static {}

/// Listens for incoming mesh connections.
#[async_trait]
pub trait TransportListener: Send + Sync {
    type Stream: MeshStream;
    async fn accept(&mut self) -> Result<(Self::Stream, SocketAddr), MeshError>;
    fn local_addr(&self) -> Result<SocketAddr, MeshError>;
}
```

#### Protocol Message Types

```rust
/// WeftOS mesh handshake payload (sent inside Noise XX step 3).
#[derive(Serialize, Deserialize)]
pub struct WeftHandshake {
    pub node_id: [u8; 32],
    pub governance_genesis_hash: [u8; 32],
    pub governance_version: String,
    pub capabilities: u32,
    pub kem_supported: bool,
    pub kem_public_key: Option<Vec<u8>>,
    pub supported_sync_streams: Vec<u8>,
    pub chain_seq: u64,
}

/// Request to join a mesh cluster.
#[derive(Serialize, Deserialize)]
pub struct JoinRequest {
    pub node_id: [u8; 32],
    pub governance_genesis_hash: [u8; 32],
    pub platform: String,
    pub transports: Vec<String>,
    pub chain_seq: u64,
    pub tree_root_hash: [u8; 32],
}

/// Response to a join request.
#[derive(Serialize, Deserialize)]
pub struct JoinResponse {
    pub accepted: bool,
    pub reason: Option<String>,
    pub peer_list: Vec<PeerInfo>,
    pub governance_rule_count: u32,
}

/// Chain sync request.
#[derive(Serialize, Deserialize)]
pub struct ChainSyncRequest {
    pub from_seq: u64,
    pub from_hash: [u8; 32],
    pub max_events: u32,
}

/// Chain sync response.
#[derive(Serialize, Deserialize)]
pub struct ChainSyncResponse {
    pub events: Vec<Vec<u8>>,  // RVF-serialized chain events
    pub has_more: bool,
    pub tip_seq: u64,
    pub tip_hash: [u8; 32],
}

/// DHT service advertisement.
#[derive(Serialize, Deserialize)]
pub struct ServiceAdvertisement {
    pub node_id: [u8; 32],
    pub version: u64,
    pub capabilities: u32,
    pub methods: Vec<String>,
}

/// DHT process advertisement.
#[derive(Serialize, Deserialize)]
pub struct ProcessAdvertisement {
    pub global_pid: GlobalPid,
    pub agent_id: String,
    pub node_id: [u8; 32],
    pub state: String,
}
```

### Mesh RPC via ServiceApi Reuse

Cross-node RPCs (including service resolution) reuse the existing `ServiceApi`
pattern rather than introducing a dedicated mesh protocol. The mesh transport
is simply another protocol adapter alongside Shell and MCP.

#### Architecture

```
Protocol Adapters (all dispatch through ServiceApi):
  ├── ShellAdapter      → "service.method args"
  ├── McpAdapter        → MCP tool_call mapping
  ├── DaemonRpcAdapter  → Unix socket JSON-RPC
  └── MeshAdapter       → incoming mesh KernelMessage dispatch   (K6.3)
```

On the receiving node, incoming mesh messages route through the same
`A2ARouter` as local messages. No separate handler needed.

#### New Components (K6.3)

**`RegistryQueryService`** (~50 lines):
Wraps `ServiceRegistry` as a queryable `SystemService` registered at boot.
Exposes `resolve(name)`, `list()`, `health(name)` methods via the standard
`ServiceApi::call("registry", method, params)` path.

**`MeshAdapter`** (~80 lines):
Receives `KernelMessage` from the mesh transport and feeds it into the local
`A2ARouter::send()`. Responses flow back via `correlation_id` matching
(reusing K2's `A2ARouter::request()` pattern).

**`mesh.request()`** (~30 lines):
Extension on the mesh transport that sends a `KernelMessage` to a remote node
and awaits a correlated response with timeout. Wraps the Noise channel send/receive.

#### Service Resolution via ServiceApi

```rust
// Node A resolves "cache" on Node B:
let msg = KernelMessage::new(
    0,
    MessageTarget::ServiceMethod { service: "registry", method: "resolve" },
    MessagePayload::Json(json!({"name": "cache"})),
);
let response = mesh.request(node_b, msg, Duration::from_secs(5)).await?;
// → { pid: 12, methods: ["get", "set"], contract_hash: "abc..." }
```

#### Implication: All Services Remotely Callable

Because ServiceApi is the universal dispatch interface, every kernel service
becomes automatically remotely callable once the mesh is operational:

| Service | Remote Call | Use Case |
|---------|-----------|----------|
| registry | resolve, list, health | Service discovery |
| chain | status, local, verify | Cross-node chain queries |
| ecc | status, search, causal | Distributed cognitive queries |
| kernel | status, ps, services | Remote kernel introspection |

No service-specific remote protocol needed. `weaver console --attach` could
work across the mesh by dialing a remote node's daemon service.

#### Symposium Decision

**D13**: Mesh RPCs reuse ServiceApi pattern. No dedicated mesh protocol type.
`RegistryQueryService` + `MeshAdapter` + `mesh.request()` are the only new
components (~160 lines). All existing services automatically become remotely
callable.

### Cross-Mesh Service Resolution

#### DHT Key Namespacing

All DHT keys are prefixed with the governance genesis hash for cluster isolation:

```
svc:<genesis_hash_hex[0..16]>:<service_name>   → service advertisement
node:<genesis_hash_hex[0..16]>:<node_id_hex>   → node presence/transports
agent:<genesis_hash_hex[0..16]>:<app>/<id>     → agent directory
```

Even though the join protocol already gates on governance genesis, key namespacing
provides defense-in-depth: a node that accidentally connects to a different cluster's
DHT cannot resolve or pollute service records.

#### Resolution Flow (9 steps)

```
Agent calls Service("cache")
  │
  ├─ 1. LOCAL: ServiceRegistry.resolve_target("cache")
  │     → Found? Deliver to local inbox. DONE.
  │
  ├─ 2. NEGATIVE CACHE: Check if service is known-missing (30s TTL)
  │     → Found? Return ServiceNotFound immediately. DONE.
  │
  ├─ 3. RESOLUTION CACHE: Check cached RemoteNode resolution
  │     ├─ Hit + not expired + circuit CLOSED → Use cached. Go to step 6.
  │     └─ Miss/expired/circuit OPEN → Continue to step 4.
  │
  ├─ 4. DHT LOOKUP: dht.get("svc:<genesis>:<cache>")
  │     ├─ Not found → Add to negative cache. Return error.
  │     └─ Found: [(node_B, v=5), (node_C, v=3)]
  │         → Filter by governance genesis hash
  │         → Filter by circuit breaker state
  │         → K6.3: Round-robin or lowest-latency selection
  │         → K6.5+: Affinity → connection pool → latency → load
  │
  ├─ 5. SECONDARY RESOLVE: RPC to selected node for full endpoint metadata
  │     → { pid, methods, contract_hash }
  │     → Cache result (30s TTL). Set affinity.
  │
  ├─ 6. CONNECTION: pool.get_or_dial(node_id)
  │     → Reuse existing NoiseChannel or establish new one
  │
  ├─ 7. GOVERNANCE GATE: gate.check("ipc.remote.send", context)
  │     → Remote calls carry elevated EffectVector.risk
  │
  ├─ 8. DELIVER: Serialize KernelMessage via RVF, send over mesh
  │
  └─ 9. WITNESS: chain.append("ipc.remote.send", {...})
```

#### Replicated Services

When multiple nodes advertise the same service name, the DHT returns a list.
Selection strategy progresses with K6 phases:

| Phase | Strategy | Implementation |
|-------|----------|---------------|
| K6.3  | Round-robin | Simple `AtomicU64` counter mod replica count |
| K6.3  | Lowest-latency | Prefer nodes with lowest recent ping RTT |
| K6.5  | Connection affinity | Prefer nodes with existing pool connection |
| K6.5  | Circuit-breaker aware | Skip nodes in OPEN state |
| K7    | Load-aware | Use `NodeEccCapability.headroom_ratio` from gossip |

#### Service Resolution Types

```rust
/// Cached resolution of a remote service.
pub struct ResolvedService {
    pub node_id: [u8; 32],
    pub pid: Pid,
    pub endpoint: ServiceEndpoint,
    pub methods: Vec<String>,
    pub resolved_at: Instant,
    pub ttl: Duration,
}

/// Connection pool entry for a mesh peer.
pub struct MeshConnection {
    pub node_id: [u8; 32],
    pub channel: NoiseChannel,
    pub established_at: Instant,
    pub last_used: Instant,
    pub active_streams: AtomicU32,
}

/// Circuit breaker for a remote node or service.
pub enum CircuitState {
    Closed { error_count: u32, window_start: Instant },
    Open { opened_at: Instant, cooldown: Duration },
    HalfOpen { test_in_progress: bool },
}

/// Negative cache for missing services and unreachable nodes.
pub struct NegativeCache {
    missing_services: DashMap<String, (Instant, Duration)>,
    unreachable_nodes: DashMap<[u8; 32], (Instant, Duration)>,
}
```

**MessageTarget::RemoteNode** (`ipc.rs`) -- Symposium C1:
```rust
pub enum MessageTarget {
    // ... existing variants ...
    RemoteNode {
        node_id: String,
        target: Box<MessageTarget>,
    },
}
```

### New Rust Dependencies

| Crate | Version | Feature Gate | Purpose | Symposium Ref |
|-------|---------|-------------|---------|---------------|
| `quinn` | 0.11 | `mesh` | QUIC transport with multiplexing | D1, D6 |
| `snow` | 0.9 | `mesh` | Noise Protocol Framework | D1, D3 |
| `x25519-dalek` | 2.0 | `mesh` | X25519 Diffie-Hellman for Noise | D1 |
| `libp2p-kad` | 0.46 | `mesh-discovery` | Kademlia DHT peer discovery | D1 |
| `libp2p-mdns` | 0.46 | `mesh-discovery` | mDNS LAN peer discovery | D1 |
| `ruvector-dag` | workspace | `mesh` | ML-KEM-768 via `production-crypto` feature (K6.4b) | D11 |

### Feature Gate Structure -- Symposium D5, C4

```toml
[features]
mesh = ["quinn", "snow", "x25519-dalek"]
mesh-discovery = ["mesh", "libp2p-kad", "libp2p-mdns"]
mesh-full = ["mesh", "mesh-discovery"]
```

| Build Config | Features | Use Case |
|-------------|----------|----------|
| Single-node | (none) | Development, testing, embedded, WASI |
| Static cluster | `mesh` | Known peers, seed-peer bootstrap |
| Dynamic cluster | `mesh-full` | DHT + mDNS auto-discovery |

---

## P -- Pseudocode

### Connection Establishment (K6.1) -- Symposium D1, D3, D6

```
fn dial(addr: &str, identity: &NodeIdentity) -> Result<EncryptedPeer>:
    // 1. Select transport based on address scheme
    transport = match addr:
        "quic://*" => QuicTransport
        "ws://*"   => WebSocketTransport
        "tcp://*"  => TcpTransport

    // 2. Establish raw byte stream
    stream = transport.connect(addr).await?

    // 3. Noise XX handshake (first contact) or IK (known peer) [D3]
    noise_builder = snow::Builder::new("Noise_XX_25519_ChaChaPoly_BLAKE2b")
    initiator = noise_builder
        .local_private_key(identity.x25519_secret())
        .build_initiator()?

    // XX pattern: -> e, s  |  <- e, ee, se, s, es  |  -> payload
    buf = [0u8; 65535]
    len = initiator.write_message(&[], &mut buf)?
    stream.send(&buf[..len]).await?

    resp = stream.recv().await?
    initiator.read_message(&resp, &mut buf)?

    // Final message: send our capabilities + chain_head
    payload = serialize(WeftHandshake {
        node_id: identity.node_id(),
        capabilities: local_capabilities(),
        chain_head: chain.head_hash(),
        genesis_hash: governance.genesis_hash(),  // D4: cluster trust root
    })
    len = initiator.write_message(&payload, &mut buf)?
    stream.send(&buf[..len]).await?

    transport_state = initiator.into_transport_mode()?

    // 4. Wrap in NoiseChannel for encrypted I/O
    channel = NoiseChannel::new(transport_state, stream)

    // 5. Verify genesis_hash matches (D4: reject foreign clusters)
    remote_handshake = channel.recv_handshake().await?
    if remote_handshake.genesis_hash != governance.genesis_hash():
        return Err(MeshError::GenesisMismatch)

    // 6. Register peer in ClusterMembership
    cluster.add_peer(PeerNode::from(remote_handshake))?

    Ok(EncryptedPeer { channel, remote: remote_handshake })
```

### Peer Discovery (K6.2) -- Symposium D1

```
fn discover_peers(config: &MeshConfig, cluster: &ClusterMembership):
    // Phase 1: Static seed peers (always available, no extra deps)
    for seed in config.seed_peers:
        spawn(async {
            peer = dial(seed, identity).await?
            exchange_peer_lists(peer).await?
        })

    // Phase 2: mDNS for LAN (if mesh-discovery enabled)
    #[cfg(feature = "mesh-discovery")]
    mdns_task = spawn(async {
        mdns = MdnsDiscovery::new(WEFTOS_SERVICE_NAME)
        loop:
            peer_info = mdns.next().await
            if !cluster.has_peer(peer_info.node_id):
                peer = dial(peer_info.addr, identity).await?
                cluster.add_peer(peer)?
    })

    // Phase 3: Kademlia DHT (if mesh-discovery enabled)
    #[cfg(feature = "mesh-discovery")]
    kad_task = spawn(async {
        kad = KademliaDht::new(identity.node_id_bytes())
        for peer in cluster.active_peers():
            kad.add_address(peer.id, peer.address)
        loop:
            sleep(30s)
            closest = kad.find_node(identity.node_id_bytes()).await
            for found in closest:
                if !cluster.has_peer(found.id):
                    peer = dial(found.addr, identity).await?
                    cluster.add_peer(peer)?
    })
```

### Cross-Node Message Routing (K6.3) -- Symposium C1, C2

```
fn send_message(msg: KernelMessage, target: MessageTarget):
    match target:
        MessageTarget::RemoteNode { node_id, inner_target }:
            // 1. Look up mesh connection for node_id
            peer = mesh_connections.get(node_id)
                .ok_or(MeshError::PeerNotConnected)?

            // 2. Apply governance gate on outbound message
            gate_decision = governance.evaluate(
                GovernanceRequest::new("ipc.cross_node")
                    .with_agent(msg.from)
                    .with_target(node_id)
            )?
            if gate_decision != Permit: return Err(Denied)

            // 3. Frame message with length prefix [D8: 16 MiB max]
            frame = Frame {
                len: serialized_size(&msg),
                msg_type: 0x02,  // KernelMessage (RVF segment)
                payload: serialize_rvf(&msg),  // D8: rvf-wire format
            }
            if frame.len > MAX_MESSAGE_SIZE: return Err(MessageTooLarge)

            // 4. Send over encrypted mesh stream
            peer.channel.send(frame).await?

            // 5. Log to chain
            chain.append("mesh", "ipc.forward", json!({
                "target_node": node_id,
                "msg_id": msg.id,
            }))

        // All other targets: existing local dispatch
        _ => existing_local_send(msg, target)
```

### Chain Sync Protocol (K6.4) -- Symposium D9

```
fn sync_chain_with_peer(peer: &EncryptedPeer):
    // 1. Compare chain heads
    local_head = chain.head_sequence()
    remote_head = peer.handshake.chain_head_seq

    if local_head == remote_head: return  // already in sync

    if local_head < remote_head:
        // Pull missing events from peer
        request = ChainSyncRequest {
            chain_id: 0,  // local chain
            after_sequence: local_head,
        }
        response = peer.channel.request(request).await?

        for event in response.events:
            // Verify dual signature (D9: Ed25519 + ML-DSA-65)
            verify_ed25519(event.signature, event.hash)?
            verify_ml_dsa(event.pq_signature, event.hash)?
            chain.append_verified(event)?

    else:
        // Push our newer events to peer
        events = chain.tail_from(remote_head)
        peer.channel.send(ChainSyncResponse { events }).await?

    // 2. Create bridge event anchoring remote chain head
    chain.append("mesh", "chain.bridge", json!({
        "remote_node": peer.node_id,
        "remote_head_hash": peer.handshake.chain_head_hash,
        "remote_head_seq": remote_head,
    }))
```

### Cluster Join with Governance Verification (K6.0/K6.1) -- Symposium C5, D4

```
fn handle_join_request(request: JoinRequest, peer: &EncryptedPeer):
    // 1. Verify genesis hash matches cluster [D4]
    if request.genesis_hash != governance.genesis_hash():
        peer.send(JoinResponse::Rejected("genesis mismatch")).await?
        return

    // 2. Verify Ed25519 signature on join request [D2]
    verify_ed25519(request.pubkey, request.signature, request.payload)?

    // 3. Evaluate via GovernanceGate [C5]
    decision = governance.evaluate(
        GovernanceRequest::new("cluster.join")
            .with_agent(request.node_id)
            .with_capabilities(request.capabilities)
            .with_platform(request.platform)
    )?

    match decision:
        Permit:
            cluster.add_peer(peer_from_request(request))?
            peer.send(JoinResponse::Accepted {
                peer_list: cluster.active_peers(),
                governance_rules: governance.rules(),
                chain_head: chain.head(),
            }).await?
            // Start chain + tree sync
            spawn(sync_chain_with_peer(peer))
            spawn(sync_tree_with_peer(peer))

        Deny(reason):
            peer.send(JoinResponse::Rejected(reason)).await?
```

### MeshAdapter Dispatch (K6.3)

```
fn MeshAdapter::handle_incoming(msg: KernelMessage):
    // 1. Validate sender is authenticated (Noise channel verified)
    // 2. Check governance gate (ipc.remote.receive)
    gate.check(msg.from, "ipc.remote.receive", context)?

    // 3. If it has a correlation_id, try to complete a pending request
    if a2a.try_complete_request(msg.clone()):
        return  // response delivered to waiting future

    // 4. Otherwise, route through normal A2ARouter dispatch
    a2a.send(msg).await
```

### mesh.request() Correlation (K6.3)

```
fn mesh.request(node_id, msg, timeout):
    // 1. Assign correlation_id if not set
    msg.correlation_id = Some(uuid::new_v4())

    // 2. Register in pending_requests (reuse A2ARouter pattern)
    let (tx, rx) = oneshot::channel()
    pending.insert(msg.correlation_id, tx)

    // 3. Send over Noise channel
    channel = pool.get_or_dial(node_id)
    channel.send(serialize(msg))

    // 4. Wait with timeout
    match timeout(duration, rx).await:
        Ok(response) -> response
        Err(_) -> remove pending, return Timeout error
```

### Tree Merkle Diff (K6.4)

```
fn tree_merkle_diff(local_tree, peer_digest):
    if local_tree.root_hash() == peer_digest.tree_root_hash:
        return  // trees are identical

    // Walk tree breadth-first, compare node hashes
    queue = [(root_id, peer_root_hash)]
    diff_nodes = []

    while queue not empty:
        (node_id, peer_hash) = queue.pop()
        local_node = local_tree.get(node_id)

        if local_node.hash != peer_hash:
            diff_nodes.push(local_node)
            for child in local_node.children:
                queue.push((child.id, request_child_hash(peer, child.id)))

    // Send only the differing nodes
    send_tree_diff(diff_nodes)
```

### SWIM Heartbeat Protocol (K6.5)

```
fn swim_heartbeat_loop(cluster: ClusterMembership, interval: Duration):
    loop:
        sleep(interval)  // default 1s

        // 1. Pick a random peer to probe
        target = cluster.random_active_peer()
        if target is None: continue

        // 2. Direct ping
        result = timeout(500ms, ping(target))
        if result.is_ok():
            target.update_last_seen(now())
            continue

        // 3. Direct ping failed -- indirect probe via k random peers
        witnesses = cluster.random_peers(k=3, excluding=target)
        indirect_ok = false
        for witness in witnesses:
            result = timeout(1s, request_ping(witness, target))
            if result.is_ok():
                indirect_ok = true
                break

        // 4. Update membership state
        if indirect_ok:
            target.update_last_seen(now())
        else:
            target.mark_suspect()
            // After suspicion_timeout (5s), promote to Unreachable
            spawn_after(5s, || {
                if target.is_still_suspect():
                    target.mark_unreachable()
                    cluster.broadcast_unreachable(target)
            })
```

---

## A -- Architecture

### 5-Layer Diagram -- Symposium Panel 1

```
+------------------------------------------------------------------+
|                    APPLICATION LAYER                               |
|  A2ARouter (cross-node IPC), ChainSync, TreeSync, ServiceDiscovery|
|  Integration: ipc.rs, a2a.rs, chain.rs, tree_manager.rs           |
+------------------------------------------------------------------+
|                    DISCOVERY LAYER                                 |
|  Kademlia DHT (libp2p-kad), mDNS (libp2p-mdns), Bootstrap Peers  |
|  Files: mesh_discovery.rs, mesh_kad.rs, mesh_mdns.rs              |
|  Feature gate: mesh-discovery                                     |
+------------------------------------------------------------------+
|                    ENCRYPTION LAYER                                |
|  Noise Protocol (snow) -- XX for first contact, IK for known      |
|  Ed25519 static keys, X25519 ephemeral DH                         |
|  File: mesh_noise.rs                                              |
+------------------------------------------------------------------+
|                    TRANSPORT LAYER                                 |
|  quinn 0.11 (QUIC) | tokio-tungstenite (WS) | raw TCP            |
|  Files: mesh.rs (trait), mesh_quic.rs                             |
|  Feature gate: mesh                                               |
+------------------------------------------------------------------+
|                    IDENTITY LAYER                                  |
|  Ed25519 keypair = node identity [D2]                             |
|  governance.genesis = cluster trust root [D4]                     |
|  NodeIdentity in cluster.rs                                       |
+------------------------------------------------------------------+
```

### Component Relationships

```
                    ┌──────────────────────────────┐
                    │         boot.rs               │
                    │   (start mesh listener)       │
                    └──────┬───────────────────────┘
                           │
              ┌────────────┼────────────────┐
              │            │                │
    ┌─────────▼───┐  ┌─────▼──────┐  ┌─────▼─────────────┐
    │ mesh_       │  │ mesh_      │  │ mesh_discovery.rs  │
    │ listener.rs │  │ quic.rs    │  │ mesh_kad.rs        │
    │ (accept)    │  │ (transport)│  │ mesh_mdns.rs       │
    └──────┬──────┘  └─────┬──────┘  └──────┬─────────────┘
           │               │                │
           └───────┬───────┘                │
                   │                        │
           ┌───────▼───────┐       ┌────────▼──────────┐
           │ mesh_noise.rs │       │ cluster.rs         │
           │ (encryption)  │       │ (ClusterMembership)│
           └───────┬───────┘       └────────┬──────────┘
                   │                        │
           ┌───────▼───────┐                │
           │ mesh_         │                │
           │ framing.rs    │                │
           │ (wire format) │                │
           └───────┬───────┘                │
                   │                        │
    ┌──────────────┼──────────────┬─────────┘
    │              │              │
┌───▼─────┐  ┌────▼─────┐  ┌────▼──────────┐
│ mesh_   │  │ mesh_    │  │ mesh_         │
│ ipc.rs  │  │ chain.rs │  │ process.rs    │
│         │  │ (sync)   │  │ (distributed) │
└───┬─────┘  └────┬─────┘  └────┬──────────┘
    │              │              │
┌───▼─────┐  ┌────▼─────┐  ┌────▼──────────┐
│ a2a.rs  │  │ chain.rs │  │ service.rs    │
│ ipc.rs  │  │ tree_    │  │ (cross-node   │
│ (exist.)│  │ manager  │  │  adverts)     │
└─────────┘  └──────────┘  └───────────────┘
```

### Integration with Existing Kernel Modules

| Existing Module | Integration Point | Symposium Ref |
|----------------|-------------------|---------------|
| `cluster.rs` | `ClusterMembership` receives peer updates from mesh discovery | D4 |
| `ipc.rs` | `KernelIpc::send()` forks to mesh transport for `RemoteNode` targets | C1 |
| `a2a.rs` | `A2ARouter` gains cluster-aware service resolution | C1 |
| `chain.rs` | `LocalChain` gains `tail_from(seq)` for incremental replication | D9 |
| `tree_manager.rs` | `TreeManager` gains `snapshot()` / `apply_remote_mutation()` | -- |
| `governance.rs` | `GovernanceEngine` distributes rules via mesh; gates remote ops | C5 |
| `boot.rs` | Boot sequence adds mesh listener + peer discovery when feature enabled | D5 |
| `service.rs` | `ServiceRegistry` gains cross-node service advertisement | D10 |

### CMVG Cognitive Sync Architecture

The Causal Merkle Vector Graph (CMVG) from K3c syncs across the mesh using
multiplexed QUIC streams over a single Noise-encrypted connection per node pair.

#### Stream Multiplexing

| Stream | Structure | Sync Mode | Phase |
|--------|-----------|-----------|-------|
| 0 | Control | Ping, capability, handshake | K6.1 |
| 1 | ExoChain | Ordered log replication | K6.4 |
| 2 | ResourceTree | Merkle diff anti-entropy | K6.4 |
| 3 | CausalGraph | CRDT delta merge (G-Set) | K7 |
| 4 | HNSW Index | Vector entry batch transfer | K7 |
| 5 | CrossRefs | Add-only edge gossip | K7 |
| 6 | Impulses | Ephemeral flood (TTL-bounded) | K7 |
| 7+ | IPC | KernelMessage / ServiceApi | K6.3 |

#### Stream Prioritization

QUIC supports per-stream weighting. Higher-priority streams get bandwidth
preference when the connection is congested:

| Priority | Stream | Rationale |
|----------|--------|-----------|
| 0 (highest) | Control (0) | Heartbeat, capability exchange |
| 1 | Chain (1) | Foundation — all other sync depends on chain state |
| 2 | Tree (2) | Service discovery depends on tree state |
| 3 | IPC (7+) | User-facing agent communication |
| 4 | Causal (3) | Cognitive — can tolerate slight delay |
| 4 | CrossRef (5) | Cognitive — same priority as causal |
| 5 | HNSW (4) | Batch — tolerates delay, large payloads |
| 6 (lowest) | Impulse (6) | Ephemeral — TTL-bounded, loss-tolerant |

Priority is set via QUIC's `set_priority()` on stream creation. Adjustable
at runtime if cognitive load increases (e.g., during ECC spectral analysis,
promote Causal to priority 2).

#### Backpressure and Flow Control

QUIC provides connection-level and stream-level flow control natively.
Additional WeftOS-specific backpressure:

- **Chain sync**: If receiver falls >1000 events behind, switch from
  event-by-event to checkpoint-based catch-up (bulk RVF transfer)
- **HNSW batch**: Limit to 100 vectors per batch frame. Receiver ACKs
  before next batch.
- **Impulse flood**: Drop impulses older than TTL without forwarding.
  Monitor impulse queue depth — if >1000 pending, throttle emission rate.
- **DeFi high-churn**: In oracle-heavy scenarios (rapid price updates),
  impulse stream may spike. The TTL mechanism ensures self-limiting behavior.
  Monitor via `ecc.tick.drift` chain events.

#### Why One Connection, Not Separate Protocols

QUIC provides native stream multiplexing. Using separate protocols would
duplicate connection management, authentication, governance gates, and
chain witnessing. One connection per node pair covers everything.

#### Sync Modes by Structure

**ExoChain (Stream 1)**: Log replication. Peer reports its sequence + hash,
sender sends missing events as RVF segments. Linear chain = no merge conflict.
Uses `ruvector-delta-consensus` with LWW on sequence numbers for leaderless mesh.

**ResourceTree (Stream 2)**: Merkle tree anti-entropy. Compare root hashes,
recurse on differing subtrees, transfer only changed nodes. O(changed) not O(total).
The existing `exo-resource-tree` already computes Merkle root hashes.

**CausalGraph (Stream 3)**: CRDT add-only G-Set. Edges have unique
(source, target, type) keys and are never deleted. Merge = union.
`CausalEdge.timestamp` (HLC) and `chain_seq` provide causal ordering.
Uses `ruvector-delta-consensus::CausalDelta` with vector clocks.

**HNSW Index (Stream 4)**: Vector entries (id, embedding, metadata) transferred
as insert batches. HNSW graph structure is NOT synced — it rebuilds lazily
on the receiving node. Chain events (`ecc.hnsw.insert`) provide the insert log.

**CrossRefs (Stream 5)**: Add-only cross-structure edges gossipped with HLC
deduplication. Same semantics as CausalGraph edges.

**Impulses (Stream 6)**: Ephemeral events with short TTL (e.g., 5 cognitive ticks).
Not persisted to chain. Gossipped via mesh GossipSub. Deduplicated by (impulse_id, hlc).

#### Delta Computation: Peer State Exchange

When a sync stream opens, peers exchange a `SyncStateDigest` to determine
what needs syncing:

```rust
/// Compact summary of a node's CMVG state, exchanged on stream open.
#[derive(Serialize, Deserialize)]
pub struct SyncStateDigest {
    /// Chain: highest sequence number + hash
    pub chain_seq: u64,
    pub chain_hash: [u8; 32],
    /// Tree: Merkle root hash
    pub tree_root_hash: [u8; 32],
    /// Causal: edge count + vector clock summary
    pub causal_edge_count: u64,
    pub causal_vclock_hash: [u8; 32],  // hash of serialized vector clock
    /// HNSW: vector count + last insert chain_seq
    pub hnsw_count: u32,
    pub hnsw_last_seq: u64,
    /// CrossRef: count + latest HLC
    pub crossref_count: u64,
    pub crossref_latest_hlc: u64,
}
```

Delta computation per structure:

| Structure | Digest Compare | Delta Action |
|-----------|---------------|-------------|
| Chain | `peer.chain_seq < local.chain_seq` | Send events `[peer_seq+1..local_seq]` |
| Tree | `peer.tree_root_hash != local` | Initiate Merkle diff walk |
| Causal | `peer.causal_vclock_hash != local` | Exchange vector clocks, send missing deltas |
| HNSW | `peer.hnsw_last_seq < local` | Send vector entries added after peer's seq |
| CrossRef | `peer.crossref_latest_hlc < local` | Send crossrefs with hlc > peer's latest |

The digest is ~140 bytes — exchanged once on stream open, then periodically
(every 30s) to detect drift without full re-sync.

#### Sync Message Framing

Sync messages use RVF wire segments with a `SyncStreamType` discriminator
in the segment type byte. This reuses the existing `rvf-wire` zero-copy
serialization with content hash integrity:

```rust
/// Sync frame header (inside RVF segment payload)
#[derive(Serialize, Deserialize)]
pub struct SyncFrame {
    /// Which sync stream this frame belongs to
    pub stream_type: u8,  // matches SyncStreamType discriminant
    /// Frame sequence within the stream (for ordering/dedup)
    pub frame_seq: u64,
    /// Payload type within the stream
    pub payload_type: SyncPayloadType,
}

#[repr(u8)]
pub enum SyncPayloadType {
    StateDigest = 0x00,     // SyncStateDigest exchange
    ChainEvents = 0x01,     // batch of ChainEvent
    TreeDiff = 0x02,        // Merkle diff nodes
    CausalDelta = 0x03,     // CRDT delta with vector clock
    HnswBatch = 0x04,       // vector entry batch
    CrossRefBatch = 0x05,   // crossref edge batch
    ImpulseFlood = 0x06,    // impulse batch
    Ack = 0x0F,             // acknowledgment for backpressure
}
```

RVF segment layout for sync:
```
[SegmentHeader (64B)] [SyncFrame (10B)] [payload (variable)] [padding]
```

Content hash in the segment header covers `SyncFrame + payload`,
providing integrity verification without additional checksumming.
This means `rvf-wire::validate_segment()` verifies sync frame integrity
using the same path as chain segment verification — no new validation code.

#### Chain Replication as Implicit CMVG Sync

Chain events include `ecc.hnsw.insert`, `ecc.causal.link`, `ecc.crossref.create`,
`ecc.impulse.emit`. Replaying the chain reconstructs ~80% of CMVG state.
Dedicated streams (3-6) are optimization for real-time cognitive coordination —
they provide lower latency than waiting for chain replication.

#### CmvgSyncService (K7)

```rust
pub struct CmvgSyncService {
    chain: Arc<ChainManager>,
    tree: Arc<TreeManager>,
    causal: Arc<CausalGraph>,
    hnsw: Arc<HnswService>,
    crossrefs: Arc<CrossRefStore>,
    impulses: Arc<ImpulseQueue>,
}

enum SyncStreamType {
    Chain,    // Stream 1
    Tree,     // Stream 2
    Causal,   // Stream 3
    Hnsw,     // Stream 4
    CrossRef, // Stream 5
    Impulse,  // Stream 6
}
```

Registered as `SystemService`, remotely queryable via `ServiceApi` (D13):
- `cmvg.sync_status` → current state hashes for all structures
- `cmvg.delta { since_seq }` → changes since a given chain sequence

### Ruvector Reuse -- Symposium D7

Ruvector algorithms are pure computation, producing messages to send and
consuming messages received. The mesh layer provides the I/O bridge:

| Ruvector Crate | Algorithm | Mesh Integration |
|---------------|-----------|-----------------|
| ruvector-cluster | SWIM membership | Drive with mesh heartbeats |
| ruvector-raft | Raft consensus | Use mesh transport for AppendEntries/RequestVote |
| ruvector-replication | Log replication | Replicate chain events over mesh streams |
| ruvector-delta-consensus | CRDT gossip | Gossip CRDT deltas over mesh pub/sub (K6.5) |
| rvf-wire | Zero-copy segments | Wire format for mesh messages (D8) |

### Platform-Transport Matrix -- Symposium D6

| Platform | Primary Transport | Fallback | Discovery |
|----------|------------------|----------|-----------|
| CloudNative | QUIC (quinn) | TCP | Kademlia DHT + bootstrap |
| Edge | QUIC (quinn) | TCP, BLE | Kademlia DHT + mDNS |
| Browser | WebSocket | WebRTC | Bootstrap peers via WS |
| Wasi | WebSocket | -- | Bootstrap peers via WS |
| Embedded | BLE, LoRa | TCP | mDNS, static config |

---

## R -- Refinement

### Edge Cases

**NAT Traversal**:
- QUIC (quinn) handles connection migration and NAT rebinding natively.
- For nodes behind symmetric NATs, designate relay nodes with the `Relay`
  capability (see doc 12 `NodeCapability::Relay`). Browser nodes always
  connect outbound via WebSocket to a relay.
- Future: WebRTC ICE for browser-to-browser direct connections.

**Split Brain / Network Partition**:
- Open question Q5 from symposium. For K6, the approach is:
  - Chain replication uses eventual consistency (not consensus).
  - Each partition continues to extend its local chain independently.
  - On reconnection, chains are reconciled via bridge events anchoring
    each side's head hash. Events are ordered by HLC (Hybrid Logical Clock).
  - Governance rules require judicial branch quorum for irreversible
    operations; partitioned nodes that lack quorum defer such decisions.
- Full consensus (ruvector-raft) for shared metadata is deferred to K6+.

**Network Partition Recovery**:
- When two partitions rejoin, the discovery layer detects new peers.
- Chain sync identifies divergence point (common ancestor by sequence + hash).
- Events from both sides are merged into a DAG structure (bridge events
  record the divergence). No events are lost.
- Tree sync uses Merkle root comparison: differing roots trigger a diff
  exchange of only the changed subtrees.

### Security Boundaries -- Symposium D3, D9

- All inter-node traffic encrypted with Noise Protocol (D3). No plaintext.
  This supersedes doc 12's "encryption is opt-in" approach.
- Dual signing (Ed25519 + ML-DSA-65) required for cross-node chain events (D9).
- GovernanceGate evaluates all remote operations identically to local ones (C5).
- Maximum message size: 16 MiB (prevents memory exhaustion attacks).
- Message deduplication via bloom filter on message IDs.
- Remote capability claims verified against source node's signed advertisement.
- Rate limiting on cluster join requests and governance evaluation requests.

### Browser Node Support

- Browser nodes connect via WebSocket to a cloud/edge relay node.
- The relay terminates WebSocket and bridges to the QUIC mesh.
- Browser nodes participate in the same mesh protocol, same governance,
  same chain verification -- but with limited capabilities:
  - Cannot listen for incoming connections (no server sockets in browsers).
  - Storage is IndexedDB/OPFS (limited, ephemeral).
  - Identity persisted via browser storage (Q3 from symposium).
- Browser transport implementation deferred to K6.3+ but the `MeshTransport`
  trait design accommodates it from K6.1.

### Backward Compatibility

- The `mesh` feature gate (D5) means all mesh code compiles to zero when
  disabled. The default build is unchanged.
- The `RemoteNode` variant in `MessageTarget` (C1) returns
  `Err(IpcError::RemoteNotAvailable)` until K6.1 wires the transport.
  Existing code never constructs this variant.
- All existing single-node tests pass without modification.
- `GlobalPid` (C2) is used only at mesh boundaries. Local code continues
  to use bare `Pid`.

### Resolved Questions

**Q1 (Chain merge strategy)**: Leader-based Raft for metadata consensus
(who owns which PID, service registry state). Delta-consensus with CRDT
for CMVG state (causal graph edges, crossrefs). Chain events are linear
and append-only -- no merge needed, just replication catch-up.

**Q2 (Wire format)**: RVF segments for sync frames (SyncFrame header +
payload, validated by rvf-wire). Bincode for lightweight RPC messages
(JoinRequest, ChainSyncRequest). JSON for ServiceApi calls (existing pattern).

**Q5 (Split-brain handling)**: Governance genesis hash acts as partition
identifier. During a network partition, nodes can only communicate with
peers sharing the same genesis hash. On reconnect, the partition with the
longer chain (higher sequence) is authoritative. Nodes in the shorter
partition catch up via chain replication.

**Q3 (NAT traversal)**: QUIC hole-punching via coordinating relay node.
Relay nodes are well-known bootstrap peers that forward connection setup
packets. Once direct connection established, relay is no longer involved.

**Q4 (Browser-to-browser)**: WebRTC DataChannel with signaling through
any connected native node. Browser nodes cannot be relay nodes.

### Key Rotation Protocol (S10, K6.5)

Nodes must support rolling key updates without losing mesh identity:

1. Generate new Ed25519 keypair
2. Create `key.rotate` chain event:
   - Signed by BOTH old and new keys (dual-signed for continuity)
   - Contains old_pubkey, new_pubkey, effective_after (chain_seq + grace_period)
3. Broadcast key rotation announcement via mesh gossip
4. During grace period: peers accept signatures from either key
5. After grace period: old key revoked, only new key accepted
6. DHT records updated with new node_id

```rust
#[derive(Serialize, Deserialize)]
pub struct KeyRotationEvent {
    pub old_key: [u8; 32],
    pub new_key: [u8; 32],
    pub effective_after_seq: u64,
    pub old_key_signature: [u8; 64],  // old key signs new_key
    pub new_key_signature: [u8; 64],  // new key signs old_key
}
```

### Browser Node Restrictions (S7)

Browser nodes run in WASM sandbox with restricted capabilities:
- Default `IpcScope::Restricted` -- can only message whitelisted services
- Cannot be relay nodes or DHT bootstrap nodes
- All connections through WebSocket to a native coordinator
- Governance `browser_policy` rules enforce additional constraints:
  - No direct filesystem access
  - No agent spawning (can request via IPC to coordinator)
  - Rate-limited IPC (max 100 msg/s)

### Doc 12 Deviations

The symposium refined several decisions from doc 12:

| Doc 12 Position | Symposium Decision | Status |
|----------------|-------------------|--------|
| TCP is default transport, encryption opt-in | Noise encryption mandatory for ALL inter-node traffic (D3) | **Superseded** |
| libp2p multiaddr addressing | Direct address strings (quic://host:port, ws://host:port) | **Simplified** |
| DeFi-style bonds and trust levels | Deferred post-K6; governance.genesis as trust root (D4) | **Deferred** |
| 5-step pairing handshake (HELLO/CHALLENGE/PROVE/ACCEPT/BOND) | Noise XX handshake + WeftOS handshake (2 phases) | **Simplified** |
| Post-quantum ML-KEM-768 for bonded channels | Post-quantum via dual signing (D9), not per-channel PQ | **Narrowed** |

---

## C -- Completion

### Exit Criteria

- [x] Two CloudNative nodes connect via TCP transport (QUIC deferred)
- [x] A Browser node connects via WebSocket (mesh_ws.rs)
- [x] Nodes discover each other via seed peers (BootstrapDiscovery)
- [x] Nodes discover each other via mDNS on LAN (mesh_mdns.rs, UDP multicast)
- [x] Nodes discover each other via Kademlia DHT (mesh_kad.rs, XOR distance, k-buckets)
- [x] `KernelMessage` routes transparently between nodes via `RemoteNode` target
- [x] Remote messages pass through GovernanceGate before delivery
- [x] Chain events replicate incrementally between nodes (`tail_from`)
- [x] Cross-node chain events carry dual signatures (DualSignature: Ed25519 + ML-DSA-65)
- [x] Bridge events anchor remote chain head hashes (ChainBridgeEvent)
- [x] Resource tree state synchronizes between nodes (Merkle root comparison)
- [x] Remote tree mutations verified against node's Ed25519 signature
- [x] Services on any node are discoverable from any other node (ClusterServiceRegistry)
- [x] Process advertisements gossip via CRDT-based distributed table
- [x] Stopped nodes detected as Unreachable via SWIM-style heartbeats
- [x] All existing single-node tests pass unchanged (560 without mesh)
- [x] `mesh` feature gate compiles to zero networking code when disabled
- [x] Maximum message size (16 MiB) enforced at deserialization
- [x] Message deduplication prevents double-delivery (DedupFilter)
- [x] Hybrid Noise + ML-KEM-768 handshake protects against store-now-decrypt-later (HybridKeyExchange, KemUpgradeProtocol)
- [x] KEM negotiation degrades gracefully when unsupported (negotiate_kem: BothSupported/GracefulDegradation/ClassicalOnly)
- [x] DHT keys namespaced with governance genesis hash prefix (NamespacedDhtKey with genesis_prefix)
- [x] Service resolution cache with TTL-based expiry
- [x] Negative cache prevents DHT storms for missing services
- [x] Replicated services resolve with round-robin selection (resolve_round_robin with AtomicU64)
- [x] Connection pool reuses channels across calls (get_or_insert with reuse tracking)
- [x] Circuit breaker prevents cascade failures from slow nodes (CircuitState: Closed/Open/HalfOpen)
- [x] RegistryQueryService exposes service resolution via ServiceApi (resolve/list/health methods)
- [x] MeshAdapter dispatches incoming mesh messages through local A2ARouter
- [x] mesh.request() supports correlated request-response with timeout (MeshRequest + PendingRequests)
- [x] Remote service calls use same governance gate as local calls
- [x] Chain log replication syncs events between mesh peers (K6.4)
- [x] Tree Merkle diff transfers only changed subtrees (K6.4)
- [x] QUIC stream priorities set per SyncStreamType (stream_priority() function, 0=highest to 6=lowest)
- [x] Backpressure: chain checkpoint catch-up when >1000 events behind (sync_strategy with CheckpointCatchup)
- [x] SyncStateDigest exchanged on stream open for delta computation (D15)
- [x] Sync frames use RVF wire segments with SyncStreamType discriminator (SyncFrame + SyncPayloadType)
- [x] PeerMetrics tracks observability dimensions for affinity scoring (affinity_score = RTT + error_rate*1000)
- [x] KEM upgrade completes before sync streams are opened (requires_upgrade() gates sync)
- [x] Key rotation protocol allows rolling key updates with grace period (KeyRotationState)
- [x] Browser nodes default to IpcScope::Restricted (browser_default())
- [x] Browser capability elevation requires governance gate approval (CapabilityElevationRequest + needs_elevation)
- [x] ruvector-cluster ConsistentHashRing used for PID-to-node assignment (ConsistentHashRing with virtual nodes)
- [x] ruvector-raft used for metadata consensus (MetadataConsensus with ConsensusRole/ConsensusEntry/ConsensusOp)
- [x] ruvector-delta-consensus used for CRDT state gossip (CrdtGossipState with LWW merge + delta_since)
- [x] InMemoryTransport enables mesh tests without real networking
- [x] MockPeer simulates remote nodes for protocol testing
- [x] Clock trait enables deterministic timeout testing

### Testing Verification Commands

```bash
# Build with mesh feature
scripts/build.sh native --features mesh

# Build with full mesh + discovery
scripts/build.sh native --features mesh-full

# Run mesh-specific tests
scripts/build.sh test -- --features mesh mesh_

# Verify single-node build unchanged (no mesh deps)
scripts/build.sh check

# Full phase gate
scripts/build.sh gate
```

### 6-Phase Breakdown -- Symposium D10

#### K6.0: Prep Changes (~200 lines, 0 new deps)

Modify existing K0-K5 code to accept K6 extensions. All prep changes
maintain backward compatibility. No new crate dependencies.

| Item | File | Lines | Symposium Ref |
|------|------|:-----:|---------------|
| Add `RemoteNode` to `MessageTarget` | `ipc.rs` | ~10 | C1 |
| Add `GlobalPid` struct | `ipc.rs` | ~20 | C2 |
| Add mesh fields to `ClusterConfig` | `cluster.rs` | ~10 | -- |
| Add `NodeIdentity` struct | `cluster.rs` | ~40 | D2 |
| Add `tail_from()` to `LocalChain` | `chain.rs` | ~10 | -- |
| Add `mesh` feature gate definition | `Cargo.toml` | ~5 | C4, D5 |
| Sign `MutationEvent.signature` with node key | `tree_manager.rs` | ~15 | -- |

**Test**: Existing tests pass. New variant serde roundtrips. GlobalPid equality.

#### Test Infrastructure (K6.0)

Built BEFORE any mesh code, used by ALL subsequent phases:

```rust
/// In-memory transport for unit/integration tests.
/// No real networking -- immediate delivery between test nodes.
pub struct InMemoryTransport {
    peers: DashMap<[u8; 32], mpsc::Sender<Vec<u8>>>,
}

impl MeshTransport for InMemoryTransport { ... }

/// Simulated remote node for protocol testing.
pub struct MockPeer {
    pub identity: NodeIdentity,
    pub chain: ChainManager,
    pub services: ServiceRegistry,
    pub governance_genesis_hash: [u8; 32],
}

/// Injectable clock for deterministic timeout/TTL testing.
pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
    fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send>>;
}

pub struct SystemClock;  // real clock (production)
pub struct MockClock;    // controllable clock (tests)
```

All timeout-dependent code (cache TTL, circuit breaker cooldown,
heartbeat intervals) must use the `Clock` trait, not `Instant::now()`
directly. This enables deterministic testing.

#### K6.1: Transport + Noise Encryption (~420 lines, 3 new deps)

Build the core mesh transport with encrypted connections.

| File | Purpose | Lines |
|------|---------|:-----:|
| `mesh.rs` | MeshTransport trait, MeshStream, TransportListener | ~60 |
| `mesh_quic.rs` | QUIC transport via quinn | ~120 |
| `mesh_noise.rs` | Noise wrapper via snow (XX + IK) | ~100 |
| `mesh_framing.rs` | Length-prefix framing + message type dispatch | ~60 |
| `mesh_listener.rs` | Accept loop, handshake, peer registration | ~80 |

Includes `MeshConnectionPool` with idle timeout (60s) and exponential backoff
for failed dial attempts. Pool entries track `last_used` and `active_streams`
for connection lifecycle management.

**Test**: Noise handshake roundtrip, QUIC connect+send+recv, framing encode/decode,
max message size enforcement, invalid handshake rejection, pool idle eviction.

#### K6.2: Discovery (~330 lines, 2 optional deps)

Peer discovery via Kademlia DHT and mDNS.

| File | Purpose | Lines |
|------|---------|:-----:|
| `mesh_discovery.rs` | Discovery trait + coordinator | ~80 |
| `mesh_kad.rs` | Kademlia DHT wrapper | ~120 |
| `mesh_mdns.rs` | mDNS local discovery | ~80 |
| `mesh_bootstrap.rs` | Static seed peer bootstrap | ~50 |

**Test**: Bootstrap from seeds, mDNS announcement+discovery, Kademlia put/get,
peer list exchange, discovery -> ClusterMembership update.

#### K6.3: Cross-Node IPC (~380 lines)

Route KernelMessage across nodes transparently.

| File | Purpose | Lines |
|------|---------|:-----:|
| `mesh_ipc.rs` | KernelMessage serialize/deserialize over mesh | ~80 |
| `mesh_service.rs` | Cross-node service registry query + `RegistryQueryService` (~50 lines) | ~120 |
| `mesh_dedup.rs` | Message deduplication (bloom filter) | ~80 |
| `mesh_adapter.rs` | `MeshAdapter` — incoming mesh dispatch through local A2ARouter (~80 lines) | ~80 |
| Changes to `ipc.rs` | Transport fork for RemoteNode | ~40 |
| Changes to `a2a.rs` | Cluster-aware service resolution + remote inbox bridge | ~110 |

Service resolution cache (30s TTL) and negative cache (30s TTL) prevent
redundant DHT lookups. Genesis-hash-prefixed DHT keys (`svc:<genesis[0..16]>:<name>`)
provide cross-cluster isolation. Replicated services use round-robin selection
via `AtomicU64` counter, with lowest-latency as an alternative strategy.

`RegistryQueryService` is registered at boot (~50 lines), exposing service
resolution via the standard `ServiceApi::call()` path (D13). `MeshAdapter`
dispatches incoming mesh messages through the local `A2ARouter` (~80 lines).
`mesh.request()` provides correlated request-response with timeout (~30 lines).

**Test**: Remote message roundtrip, cross-node service resolution, governance gate
on remote messages, dedup rejection, GlobalPid in responses, resolution cache hit/miss,
negative cache prevents DHT storm, round-robin across replicated services,
RegistryQueryService resolves via ServiceApi, MeshAdapter dispatches through A2ARouter,
mesh.request() supports correlated RPC with timeout.

#### K6.4: Chain Replication + Tree Sync (~300 lines)

Synchronize chain events and resource tree state across nodes. This phase
also establishes the first two CMVG cognitive sync streams (D14):

- **Chain log replication (Stream 1)**: Ordered event replication using
  `ruvector-delta-consensus` with LWW on sequence numbers. Chain events
  implicitly carry CMVG mutations via `ecc.*` event kinds, providing ~80%
  of CMVG state sync without dedicated cognitive streams.
- **Tree Merkle diff sync (Stream 2)**: Anti-entropy using Merkle root hash
  comparison, recursing on differing subtrees, transferring only changed nodes.

| File | Purpose | Lines |
|------|---------|:-----:|
| `mesh_chain.rs` | ChainSyncRequest/Response, BridgeEvent, push subscription | ~120 |
| `mesh_tree.rs` | TreeSyncRequest/Response, Merkle proof, remote mutation | ~80 |
| Changes to `chain.rs` | `tail_from()`, `subscribe()` | ~50 |
| Changes to `tree_manager.rs` | `snapshot()`, `apply_remote_mutation()` | ~50 |

**Test**: tail_from returns correct slices, chain sync over mesh, bridge events,
tree snapshot roundtrip, remote mutation with valid/invalid signature, root hash
comparison short-circuit.

#### K6.4b: Hybrid Post-Quantum Key Exchange (~100 lines)

**Goal**: Protect mesh transport against store-now-decrypt-later quantum attacks.

**Implementation**: After the Noise XX handshake establishes a classical channel,
perform an ML-KEM-768 key encapsulation upgrade inside the encrypted channel.
The final session key combines both secrets via HKDF.

**Protocol**:
1. Noise XX (X25519 DH) → classical shared secret
2. Initiator sends ML-KEM-768 ephemeral pubkey (1,184 bytes) over Noise channel
3. Responder encapsulates → sends ciphertext (1,088 bytes) back
4. Both derive: `final_key = HKDF(classical_ss || pq_ss || "weftos-hybrid-kem-v1")`
5. Rekey the transport with `final_key`

#### KEM Upgrade Timing

The hybrid Noise + ML-KEM-768 upgrade runs once per connection establishment,
**before** any sync streams are opened. Sequence:

1. TCP/QUIC connection established
2. Noise XX handshake (mutual auth, classical key)
3. ML-KEM-768 encapsulate/decapsulate (PQ key)
4. HKDF combine → final session key
5. Rekey transport
6. NOW open sync streams (0-7+)

All streams inherit the hybrid-encrypted transport. No per-stream encryption
needed — QUIC encrypts at the connection level.

**Negotiation**: Advertised via `kem_supported: bool` in the Noise handshake payload.
Graceful degradation — nodes that don't support KEM stay on classical Noise.

**Dependencies**: `ruvector-dag` with `production-crypto` feature (ML-KEM-768 already implemented)

**Files**:
- `crates/clawft-kernel/src/mesh_handshake.rs` — KEM upgrade step after Noise XX (flat layout; see "Files to Create" preamble)
- Reuses `ruvector-dag/src/qudag/crypto/ml_kem.rs` (MlKem768::encapsulate/decapsulate)

**Tests**:
- Hybrid handshake completes with both sides KEM-capable
- Graceful fallback when one side lacks KEM support
- Rekey produces different key than classical-only
- KEM ciphertext verified (wrong key fails decapsulation)

**Cost**: ~2.4KB extra per connection handshake, ~1ms latency. Zero per-message overhead.

#### K6.5: Distributed Process Table + Service Discovery (~240 lines)

Cluster-wide process and service visibility.

| File | Purpose | Lines |
|------|---------|:-----:|
| `mesh_process.rs` | ProcessAdvertisement CRDT gossip | ~100 |
| `mesh_service_adv.rs` | ServiceAdvertisement + resolution | ~80 |
| `mesh_heartbeat.rs` | SWIM-style heartbeat + failure detection | ~60 |

Circuit breaker (`CircuitState`) tracks error rate per remote node. Transitions:
CLOSED -> OPEN (>50% errors over 10 calls), OPEN -> HALF-OPEN (30s cooldown),
HALF-OPEN -> CLOSED (test succeeds) or OPEN (test fails). Affinity routing
prefers nodes with existing pool connections. Connection-aware selection
combines pool state, circuit state, and latency metrics.

**Note**: K7 graduates to load-aware resolution using `NodeEccCapability.headroom_ratio`
from ECC gossip (see M9).

**Test**: Process advertisement gossip, cross-node service discovery, failure
detection, CRDT merge convergence, service resolution fallback (local-first),
circuit breaker state transitions, affinity routing preference.

#### Observability Scoring for Affinity and Circuit Decisions

Mesh connections track per-peer metrics that feed into service resolution
ranking and circuit breaker decisions:

```rust
/// Per-peer observability metrics, updated on every interaction.
pub struct PeerMetrics {
    /// Rolling average RTT from ping/pong (microseconds)
    pub avg_rtt_us: u64,
    /// P95 RTT over last 100 interactions
    pub p95_rtt_us: u64,
    /// Success rate: successful calls / total calls (0.0 - 1.0)
    pub success_rate: f64,
    /// Error rate over sliding window (last 60s)
    pub error_rate_60s: f64,
    /// Risk delta: change in EffectVector magnitude after remote calls
    /// (tracks whether this peer's responses increase governance risk)
    pub risk_delta_avg: f64,
    /// Governance deny rate: fraction of calls denied by remote gate
    pub gate_deny_rate: f64,
    /// Last seen timestamp (for staleness detection)
    pub last_seen: Instant,
    /// Consecutive failures (for circuit breaker)
    pub consecutive_failures: u32,
    /// NodeEccCapability.headroom_ratio (from gossip, K7)
    pub headroom_ratio: Option<f32>,
}
```

**Dimensions used per phase:**

| Phase | Dimensions | Decision |
|-------|-----------|----------|
| K6.3 | `avg_rtt_us`, `success_rate` | Lowest-latency + round-robin tiebreak |
| K6.5 | + `consecutive_failures`, `error_rate_60s` | Circuit breaker threshold (>50% error → OPEN) |
| K6.5 | + `gate_deny_rate` | Avoid peers that frequently deny (governance mismatch) |
| K7 | + `headroom_ratio`, `risk_delta_avg` | Load-aware + risk-aware selection |

**Circuit breaker thresholds (initial):**
- OPEN trigger: `error_rate_60s > 0.5` OR `consecutive_failures > 5`
- Cooldown: 30s
- HALF-OPEN test: 1 ping + 1 lightweight ServiceApi call

**Affinity scoring (K6.5):**
```
score = (1.0 / avg_rtt_us) * success_rate * (1.0 - gate_deny_rate)
        * (has_pool_connection ? 1.5 : 1.0)   // prefer existing connections
```
Highest score wins. Recalculated every 30s or on circuit state change.

#### K7 (Future): Full CMVG Cognitive Sync

Full cognitive sync via dedicated QUIC streams (D14):

- **Streams 3-6**: CausalGraph CRDT merge, HNSW vector batch transfer,
  CrossRef edge gossip, Impulse ephemeral flood
- **CmvgSyncService**: Registered as `SystemService`, remotely queryable
  via `ServiceApi` (D13) — exposes `cmvg.sync_status` and `cmvg.delta`
- **Load-aware resolution**: Uses `NodeEccCapability.headroom_ratio` from
  ECC gossip (see M9)

#### Manual Testing (K6)

The manual testing guide (`.planning/development_notes/manual-testing-guide.md`)
needs two new passes:

- **Pass 6: Mesh Networking** -- multi-node startup, cross-node service calls,
  chain sync verification, peer discovery, connection pool behavior
- **Pass 7: Mesh Security** -- unauthorized peer rejection, governance gate
  enforcement on remote calls, Noise encryption verification, foreign cluster
  isolation via genesis hash prefix

### Line Count Summary

| Phase | New Lines | Changed Lines | New Deps |
|-------|:---------:|:------------:|----------|
| K6.0 | ~50 | ~150 | None |
| K6.1 | ~400 | ~20 | quinn, snow, x25519-dalek |
| K6.2 | ~300 | ~30 | libp2p-kad, libp2p-mdns (optional) |
| K6.3 | ~300 | ~80 | None |
| K6.4 | ~250 | ~50 | None |
| K6.4b | ~100 | ~10 | ruvector-dag (production-crypto) |
| K6.5 | ~200 | ~40 | None |
| **Total** | **~1,500** | **~370** | **5 (2 optional)** |

---

## Open Questions (Inherited from Symposium)

| # | Question | Impact | Resolve By | Status |
|---|----------|--------|-----------|--------|
| Q1 | Chain merge: leader-based consensus or DAG? | K6.4 architecture | Before K6.4 | **Resolved** -- see Refinement "Resolved Questions" |
| Q2 | Wire format: JSON or RVF for KernelMessage? | Performance vs debuggability | K6.1 design | **Resolved** -- see Refinement "Resolved Questions" |
| Q3 | Browser identity persistence across sessions | UX + security | K6.1 browser transport | **Resolved** -- see Refinement "Resolved Questions" |
| Q4 | Full libp2p-kad or lighter custom DHT? | Dep weight | K6.2 design | **Resolved** -- see Refinement "Resolved Questions" |
| Q5 | Split-brain handling on network partition | Consistency vs availability | Before K6.4 | **Resolved** -- see Refinement "Resolved Questions" |
| Q6 | BLAKE3 (ECC D6) or stay with SHAKE-256? | Hash migration | K6.0 design | Open |
| Q7 | Maximum practical cluster size? | Config defaults, test scenarios | K6.2 testing | Open |
| Q8 | Tree sync: full snapshot or Merkle proof exchange? | Bandwidth vs complexity | K6.4 design | Open |

---

## Cross-References

| Document | Relationship |
|----------|-------------|
| `01-phase-K0-kernel-foundation.md` | Process table, service registry (base for mesh extensions) |
| `03-phase-K2-a2a-ipc.md` | IPC architecture extended with RemoteNode routing |
| `07-ruvector-deep-integration.md` | Ruvector algorithms composed with mesh I/O layer (D7) |
| `08-ephemeral-os-architecture.md` | Multi-node fabric vision; mesh makes it real |
| `10-agent-first-single-user.md` | Agent lifecycle unchanged; services work identically over mesh |
| `12-networking-and-pairing.md` | Original networking vision; refined/superseded by symposium |
| `14-exochain-substrate.md` | Chain manager extended with replication + bridge events |
| `docs/weftos/k5-symposium/01-mesh-architecture.md` | Authoritative 5-layer architecture |
| `docs/weftos/k5-symposium/04-k6-implementation-plan.md` | Authoritative phase plan |
| `docs/weftos/k5-symposium/05-symposium-results.md` | Decisions D1-D15, Commitments C1-C5 |
| `docs/weftos/sparc/k6-cluster-networking.md` | Earlier K6 sketch (superseded by this plan) |
| `.planning/development_notes/k6-readiness-audit.md` | Readiness matrix (41 GREEN, 22 YELLOW, 21 RED) |
| `docs/weftos/k5-symposium/05-symposium-results.md` | Decision D14: CMVG cognitive sync via multiplexed QUIC streams |
