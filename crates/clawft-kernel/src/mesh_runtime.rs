//! Mesh runtime orchestrator for WeftOS node-to-node communication (K6).
//!
//! The [`MeshRuntime`] wires together transport connections, serialization
//! via [`MeshIpcEnvelope`], and the local [`A2ARouter`] so that a
//! `RemoteNode` message target actually delivers across the network.

use std::sync::{Arc, Mutex};

use dashmap::DashMap;
use tracing::{debug, warn};

use crate::a2a::A2ARouter;
use crate::error::{KernelError, KernelResult};
use crate::ipc::{KernelMessage, MessagePayload, MessageTarget};
use crate::mesh_chain::{ChainSyncRequest, ChainSyncResponse};
use crate::mesh_heartbeat::{ClockSource, HeartbeatConfig, HeartbeatTracker, MeshClockSync};
use crate::mesh_ipc::MeshIpcEnvelope;
use crate::mesh_kad::KademliaTable;

/// A handle to a connected peer, holding the sender half of an mpsc
/// channel whose receiver is read by a background write loop.
pub struct PeerConnection {
    /// Remote node identifier.
    pub node_id: String,
    /// When the connection was established.
    pub connected_at: chrono::DateTime<chrono::Utc>,
    /// Sender for outbound serialized messages.
    pub sender: tokio::sync::mpsc::Sender<Vec<u8>>,
}

/// Discovery state for mesh peer lookup and health tracking.
pub struct DiscoveryState {
    /// Kademlia routing table for peer lookup.
    pub kademlia: Mutex<KademliaTable>,
    /// Known peer addresses: node_id -> socket addr string.
    pub peer_addresses: DashMap<String, String>,
    /// Heartbeat tracker for failure detection.
    pub heartbeat: Mutex<HeartbeatTracker>,
}

/// The mesh runtime orchestrates transport, connections, and message bridging.
///
/// It maintains a set of active peer connections (keyed by node ID) and
/// provides the plumbing to:
/// 1. Send a [`MeshIpcEnvelope`] to a connected peer.
/// 2. Receive an envelope from a peer and inject it into the local
///    [`A2ARouter`].
pub struct MeshRuntime {
    /// Local node identifier.
    node_id: String,
    /// Active peer connections: node_id -> PeerConnection.
    peers: DashMap<String, PeerConnection>,
    /// Reference to the local A2A router for injecting remote messages.
    local_router: Option<Arc<A2ARouter>>,
    /// Optional discovery state (Kademlia + heartbeat).
    discovery: Option<DiscoveryState>,
    /// Mesh time synchronization state.
    clock: std::sync::Mutex<MeshClockSync>,
    /// Late-bound chain manager. When set (via
    /// [`set_chain_manager`]), every successful `handle_incoming`
    /// appends a `peer.envelope` event to the ExoChain so mesh
    /// activity is auditable via `weaver chain local`.
    #[cfg(feature = "exochain")]
    chain_manager: std::sync::OnceLock<Arc<crate::chain::ChainManager>>,
    /// Peer topic subscription registry: topic → set of peer node IDs.
    ///
    /// Populated when a peer sends a `mesh.subscribe` control envelope.
    /// Consulted by the A2A router's Topic handler to forward published
    /// messages to remote nodes that subscribed to the same topic.
    mesh_subscriptions: DashMap<String, Vec<String>>,
}

impl MeshRuntime {
    /// Create a new mesh runtime for the given local node.
    pub fn new(node_id: String) -> Self {
        Self {
            node_id,
            peers: DashMap::new(),
            local_router: None,
            discovery: None,
            clock: std::sync::Mutex::new(MeshClockSync::new(ClockSource::Local)),
            #[cfg(feature = "exochain")]
            chain_manager: std::sync::OnceLock::new(),
            mesh_subscriptions: DashMap::new(),
        }
    }

    /// Create a mesh runtime with discovery state initialized.
    ///
    /// The `kademlia_id` is used as the local key in the Kademlia routing
    /// table. Heartbeat tracking starts with default configuration.
    pub fn with_discovery(node_id: String, kademlia_id: [u8; 32]) -> Self {
        Self {
            node_id,
            peers: DashMap::new(),
            local_router: None,
            discovery: Some(DiscoveryState {
                kademlia: Mutex::new(KademliaTable::new(kademlia_id)),
                peer_addresses: DashMap::new(),
                heartbeat: Mutex::new(HeartbeatTracker::new(HeartbeatConfig::default())),
            }),
            clock: std::sync::Mutex::new(MeshClockSync::new(ClockSource::Local)),
            #[cfg(feature = "exochain")]
            chain_manager: std::sync::OnceLock::new(),
            mesh_subscriptions: DashMap::new(),
        }
    }

    /// Return this node's identifier.
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    /// Attach the local A2A router for incoming message injection.
    pub fn set_local_router(&mut self, router: Arc<A2ARouter>) {
        self.local_router = Some(router);
    }

    // ── Peer topic subscription registry ─────────────────────────

    /// Register a remote peer as a subscriber for the given topic.
    ///
    /// Called from [`handle_incoming`] when a `mesh.subscribe` control
    /// envelope is received. De-duplicates: if the peer is already
    /// registered for this topic, the call is a no-op.
    pub fn register_peer_topic(&self, topic: &str, peer_node_id: &str) {
        let mut entry = self
            .mesh_subscriptions
            .entry(topic.to_string())
            .or_default();
        if !entry.contains(&peer_node_id.to_string()) {
            entry.push(peer_node_id.to_string());
            debug!(topic, peer = %peer_node_id, "registered peer topic subscription");
        }
    }

    /// Remove a peer's subscription for the given topic.
    ///
    /// Called when a peer disconnects or sends an explicit unsubscribe.
    pub fn unregister_peer_topic(&self, topic: &str, peer_node_id: &str) {
        if let Some(mut subs) = self.mesh_subscriptions.get_mut(topic) {
            subs.retain(|id| id != peer_node_id);
        }
    }

    /// Remove all topic subscriptions for the given peer.
    ///
    /// Called when `disconnect_peer` is invoked so stale entries are
    /// cleaned up and future publishes don't attempt dead channels.
    pub fn unregister_all_peer_topics(&self, peer_node_id: &str) {
        for mut entry in self.mesh_subscriptions.iter_mut() {
            entry.retain(|id| id != peer_node_id);
        }
    }

    /// Return the list of peer node IDs subscribed to the given topic.
    ///
    /// Used by the A2A router's Topic handler to forward published
    /// messages to remote nodes.
    pub fn peers_for_topic(&self, topic: &str) -> Vec<String> {
        self.mesh_subscriptions
            .get(topic)
            .map(|subs| subs.clone())
            .unwrap_or_default()
    }

    /// Attach the ExoChain manager for auditable peer-envelope logging.
    ///
    /// Idempotent (first call wins; subsequent calls are no-ops). Uses
    /// interior mutability so this can be called after the runtime has
    /// been wrapped in [`Arc`], which matches the boot-sequence ordering
    /// where the chain manager is constructed after the mesh listener.
    #[cfg(feature = "exochain")]
    pub fn set_chain_manager(&self, cm: Arc<crate::chain::ChainManager>) {
        let _ = self.chain_manager.set(cm);
    }

    /// Register a peer connection using an already-established channel.
    ///
    /// This is the low-level entry point used after a TCP (or other
    /// transport) connection has been set up and the node-ID exchange
    /// has completed. Higher-level helpers like `connect_peer` build
    /// on top of this.
    pub fn add_peer(&self, node_id: String, sender: tokio::sync::mpsc::Sender<Vec<u8>>) {
        debug!(peer = %node_id, "adding peer connection");
        self.peers.insert(
            node_id.clone(),
            PeerConnection {
                node_id,
                connected_at: chrono::Utc::now(),
                sender,
            },
        );
    }

    /// Send a [`MeshIpcEnvelope`] to a connected peer.
    ///
    /// Serializes the envelope to JSON bytes and pushes them into the
    /// peer's outbound channel. Returns an error if the peer is not
    /// connected or the channel is closed/full.
    pub async fn send_to_peer(
        &self,
        node_id: &str,
        envelope: MeshIpcEnvelope,
    ) -> KernelResult<()> {
        let peer = self
            .peers
            .get(node_id)
            .ok_or_else(|| KernelError::Mesh(format!("peer not connected: {node_id}")))?;

        let bytes = envelope
            .to_bytes()
            .map_err(|e| KernelError::Mesh(format!("serialization error: {e}")))?;

        peer.sender
            .send(bytes)
            .await
            .map_err(|_| KernelError::Mesh(format!("send channel closed for peer {node_id}")))?;

        debug!(peer = %node_id, "sent envelope to peer");
        Ok(())
    }

    /// Build and send a message to a remote node.
    ///
    /// Wraps a [`KernelMessage`] in a [`MeshIpcEnvelope`] with the
    /// correct source/dest and sends it to the named peer.
    pub async fn route_to_remote(
        &self,
        node_id: &str,
        message: KernelMessage,
    ) -> KernelResult<()> {
        let envelope =
            MeshIpcEnvelope::new(self.node_id.clone(), node_id.to_string(), message);
        self.send_to_peer(node_id, envelope).await
    }

    /// Handle incoming raw bytes from a peer.
    ///
    /// Deserializes the data into a [`MeshIpcEnvelope`], then injects
    /// the inner [`KernelMessage`] into the local A2A router. The
    /// message target is unwrapped from `RemoteNode` if present so
    /// that the local router delivers to the correct local process.
    pub async fn handle_incoming(&self, data: &[u8]) -> KernelResult<()> {
        let envelope = MeshIpcEnvelope::from_bytes(data)
            .map_err(|e| KernelError::Mesh(format!("deserialization error: {e}")))?;
        self.handle_envelope(envelope).await
    }

    /// Handle incoming bytes while auto-registering the sending peer.
    ///
    /// Used by the mesh accept loop: the kernel doesn't know the peer's
    /// node ID until the first envelope arrives, so we opportunistically
    /// register `envelope.source_node → outbound` before dispatching.
    /// Idempotent — a no-op if the peer is already registered under the
    /// same ID. This is what wires `A2ARouter` topic forwarding to
    /// inbound leaf subscribers (they send `mesh.subscribe`, then the
    /// kernel can call `send_to_peer` back over this connection).
    pub async fn handle_incoming_from(
        &self,
        data: &[u8],
        outbound: tokio::sync::mpsc::Sender<Vec<u8>>,
    ) -> KernelResult<()> {
        let envelope = MeshIpcEnvelope::from_bytes(data)
            .map_err(|e| KernelError::Mesh(format!("deserialization error: {e}")))?;

        if !self.peers.contains_key(&envelope.source_node) {
            debug!(peer = %envelope.source_node, "auto-registering inbound peer");
            self.add_peer(envelope.source_node.clone(), outbound);
        }

        self.handle_envelope(envelope).await
    }

    async fn handle_envelope(&self, envelope: MeshIpcEnvelope) -> KernelResult<()> {
        debug!(
            from_node = %envelope.source_node,
            dest_node = %envelope.dest_node,
            "received mesh envelope"
        );

        // Auditable record: append a peer.envelope event to the local
        // chain before routing. Captures source_node (leaf identity),
        // dest_node, envelope_id, and the resolved topic/service when
        // known. The chain append is best-effort — a failure here must
        // not block message delivery.
        #[cfg(feature = "exochain")]
        if let Some(cm) = self.chain_manager.get() {
            let topic = match &envelope.message.target {
                MessageTarget::Topic(t) => Some(t.clone()),
                MessageTarget::Service(s) => Some(format!("service:{s}")),
                MessageTarget::ServiceMethod { service, method } => {
                    Some(format!("service:{service}#{method}"))
                }
                MessageTarget::RemoteNode { target, .. } => match target.as_ref() {
                    MessageTarget::Topic(t) => Some(t.clone()),
                    _ => None,
                },
                _ => None,
            };
            let payload = serde_json::json!({
                "source_node": envelope.source_node,
                "dest_node": envelope.dest_node,
                "envelope_id": envelope.envelope_id,
                "topic": topic,
                "hop_count": envelope.hop_count,
            });
            let _ = cm.append("mesh", "peer.envelope", Some(payload));
        }

        let router = self.local_router.as_ref().ok_or_else(|| {
            KernelError::Mesh("no local router attached to mesh runtime".into())
        })?;

        // Unwrap the RemoteNode wrapper so the local router sees the
        // inner target (Process, Service, Topic, etc.).
        let mut message = envelope.message;
        if let MessageTarget::RemoteNode { target, .. } = message.target {
            message.target = *target;
        }

        // Intercept mesh.subscribe control envelopes before local routing.
        //
        // A peer sends `Topic("mesh.subscribe")` with a JSON payload of the
        // form `{"topic": "<topic-name>"}` to register interest. We record
        // the subscription and consume the message — it does not propagate
        // to the local router or any local subscribers.
        if let MessageTarget::Topic(ref ctrl_topic) = message.target
            && ctrl_topic == "mesh.subscribe" {
                if let MessagePayload::Json(ref payload) = message.payload
                    && let Some(topic) = payload.get("topic").and_then(|v| v.as_str()) {
                        self.register_peer_topic(topic, &envelope.source_node);
                        return Ok(());
                    }
                // Malformed subscribe — drop silently rather than routing to
                // local subscribers who have no idea what to do with it.
                warn!(
                    from = %envelope.source_node,
                    "received malformed mesh.subscribe envelope (missing 'topic' field)"
                );
                return Ok(());
            }

        router.send(message).await
    }

    /// Number of currently connected peers.
    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }

    /// List the node IDs of all connected peers.
    pub fn peer_ids(&self) -> Vec<String> {
        self.peers
            .iter()
            .map(|entry| entry.key().clone())
            .collect()
    }

    /// Disconnect a peer, dropping its send channel.
    ///
    /// Also cleans up any topic subscriptions the peer registered so
    /// future publishes don't attempt to send to a closed channel.
    pub fn disconnect_peer(&self, node_id: &str) {
        if self.peers.remove(node_id).is_some() {
            self.unregister_all_peer_topics(node_id);
            debug!(peer = %node_id, "disconnected peer");
        } else {
            warn!(peer = %node_id, "disconnect_peer: peer not found");
        }
    }

    // ── Discovery integration ─────────────────────────────────────

    /// Register a peer's network address for future connection.
    ///
    /// Stored in the discovery state's address map. If discovery is not
    /// initialized this is a no-op.
    pub fn register_peer_address(&self, node_id: &str, addr: &str) {
        if let Some(ref disc) = self.discovery {
            disc.peer_addresses
                .insert(node_id.to_string(), addr.to_string());
        }
    }

    /// Return known peers from the discovery address map.
    ///
    /// Each entry is `(node_id, address)`. Returns an empty vec if
    /// discovery is not initialized.
    pub fn discover_peers(&self) -> Vec<(String, String)> {
        match self.discovery.as_ref() {
            Some(disc) => disc
                .peer_addresses
                .iter()
                .map(|entry| (entry.key().clone(), entry.value().clone()))
                .collect(),
            None => Vec::new(),
        }
    }

    /// Record a heartbeat from the given peer (marks it alive).
    ///
    /// If the peer is not yet tracked by the heartbeat system it will
    /// be added automatically.
    pub fn record_heartbeat(&self, node_id: &str) {
        if let Some(ref disc) = self.discovery {
            let mut hb = disc.heartbeat.lock().unwrap();
            if hb.peer_state(node_id).is_none() {
                hb.add_peer(node_id.to_string());
            }
            hb.record_alive(node_id);
        }
    }

    // ── Time synchronization ──────────────────────────────────────

    /// Get the current mesh-synchronized time in microseconds since epoch.
    ///
    /// If time sync is active (synced from authority), returns the
    /// authority-aligned time. Otherwise returns local system time.
    pub fn mesh_time_us(&self) -> u64 {
        self.clock.lock().unwrap().mesh_time_us()
    }

    /// Get the clock uncertainty estimate in microseconds.
    pub fn clock_uncertainty_us(&self) -> u64 {
        self.clock.lock().unwrap().uncertainty_us
    }

    /// Get the current clock source quality.
    pub fn clock_source(&self) -> ClockSource {
        self.clock.lock().unwrap().local_source
    }

    /// Set the local clock source (e.g., after NTP sync is confirmed).
    pub fn set_clock_source(&self, source: ClockSource) {
        self.clock.lock().unwrap().local_source = source;
    }

    /// Process a time sync sample from a peer's heartbeat.
    pub fn sync_clock_from_peer(
        &self,
        peer_id: &str,
        peer_time_us: u64,
        peer_source: ClockSource,
    ) {
        let local_time = crate::mesh_heartbeat::system_time_us();
        self.clock.lock().unwrap().process_sync(
            peer_id,
            peer_time_us,
            peer_source,
            local_time,
        );
    }

    /// Check if this node is the time authority.
    pub fn is_time_authority(&self) -> bool {
        self.clock.lock().unwrap().is_authority(&self.node_id)
    }

    /// Return node IDs of peers that heartbeat considers suspect or dead.
    pub fn check_peer_health(&self) -> Vec<String> {
        match self.discovery.as_ref() {
            Some(disc) => {
                let hb = disc.heartbeat.lock().unwrap();
                let mut unhealthy: Vec<String> = hb
                    .suspect_peers()
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect();
                unhealthy.extend(
                    hb.dead_peers()
                        .into_iter()
                        .map(|s| s.to_string()),
                );
                unhealthy
            }
            None => Vec::new(),
        }
    }

    /// Disconnect peers that the heartbeat tracker considers dead.
    pub fn remove_dead_peers(&self) {
        if let Some(ref disc) = self.discovery {
            let dead: Vec<String> = {
                let hb = disc.heartbeat.lock().unwrap();
                hb.dead_peers()
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect()
            };
            for node_id in &dead {
                self.disconnect_peer(node_id);
                disc.peer_addresses.remove(node_id.as_str());
            }
        }
    }

    /// Mutable access to discovery state (for tests and setup code).
    pub fn discovery_mut(&mut self) -> Option<&mut DiscoveryState> {
        self.discovery.as_mut()
    }

    /// Shared access to discovery state.
    pub fn discovery(&self) -> Option<&DiscoveryState> {
        self.discovery.as_ref()
    }

    // ── Chain sync stubs ──────────────────────────────────────────

    /// Build a serialized [`ChainSyncRequest`] for sending to a peer.
    ///
    /// The request asks for chain events starting after `from_seq`.
    /// This is a stub — the actual chain integration will replay
    /// received events into the local chain manager.
    pub fn build_chain_sync_request(&self, from_seq: u64) -> Vec<u8> {
        let req = ChainSyncRequest {
            chain_id: 0,
            after_sequence: from_seq,
            after_hash: String::new(),
            max_events: 256,
        };
        serde_json::to_vec(&req).unwrap_or_default()
    }

    /// Handle an incoming chain sync response.
    ///
    /// Deserializes the data and returns the number of events contained.
    /// This is a stub — the actual implementation will replay events
    /// into the local chain manager.
    pub fn handle_chain_sync_response(&self, data: &[u8]) -> KernelResult<usize> {
        let resp: ChainSyncResponse = serde_json::from_slice(data)
            .map_err(|e| KernelError::Mesh(format!("chain sync deserialize error: {e}")))?;
        Ok(resp.events.len())
    }
}

// ── Tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{AgentCapabilities, CapabilityChecker};
    use crate::ipc::{KernelMessage, MessagePayload, MessageTarget};
    use crate::mesh_heartbeat::HeartbeatState;
    use crate::mesh_ipc::MeshIpcEnvelope;
    use crate::process::{ProcessEntry, ProcessState, ProcessTable, ResourceUsage};
    use crate::topic::TopicRouter;
    use tokio_util::sync::CancellationToken;

    /// Helper: build a minimal A2ARouter with one registered process and return
    /// (router, pid, inbox_receiver).
    fn make_router_with_process() -> (
        Arc<A2ARouter>,
        crate::process::Pid,
        tokio::sync::mpsc::Receiver<KernelMessage>,
    ) {
        let table = Arc::new(ProcessTable::new(64));
        let entry = ProcessEntry {
            pid: 0,
            agent_id: "test-agent".into(),
            state: ProcessState::Running,
            capabilities: AgentCapabilities::default(),
            resource_usage: ResourceUsage::default(),
            cancel_token: CancellationToken::new(),
            parent_pid: None,
        };
        let pid = table.insert(entry).unwrap();
        let checker = Arc::new(CapabilityChecker::new(table.clone()));
        let topics = Arc::new(TopicRouter::new(table.clone()));
        let router = Arc::new(A2ARouter::new(table, checker, topics));
        let rx = router.create_inbox(pid);
        (router, pid, rx)
    }

    // ── Test 1: empty peer list on creation ─────────────────────

    #[test]
    fn new_runtime_has_no_peers() {
        let rt = MeshRuntime::new("node-local".into());
        assert_eq!(rt.peer_count(), 0);
        assert!(rt.peer_ids().is_empty());
        assert_eq!(rt.node_id(), "node-local");
    }

    // ── Test 2: add mock peer, verify peer_count ────────────────

    #[test]
    fn add_peer_increases_count() {
        let rt = MeshRuntime::new("local".into());
        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        rt.add_peer("peer-1".into(), tx);
        assert_eq!(rt.peer_count(), 1);
        assert_eq!(rt.peer_ids(), vec!["peer-1".to_string()]);
    }

    // ── Test 3: send envelope to peer (mock channel) ────────────

    #[tokio::test]
    async fn send_to_peer_delivers_serialized_bytes() {
        let rt = MeshRuntime::new("local".into());
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        rt.add_peer("peer-1".into(), tx);

        let msg = KernelMessage::text(0, MessageTarget::Broadcast, "hello");
        let envelope = MeshIpcEnvelope::new("local".into(), "peer-1".into(), msg);
        let expected_id = envelope.envelope_id.clone();

        rt.send_to_peer("peer-1", envelope).await.unwrap();

        let received = rx.recv().await.unwrap();
        let decoded = MeshIpcEnvelope::from_bytes(&received).unwrap();
        assert_eq!(decoded.envelope_id, expected_id);
        assert_eq!(decoded.source_node, "local");
        assert_eq!(decoded.dest_node, "peer-1");
    }

    // ── Test 4: receive envelope, inject into local router ──────

    #[tokio::test]
    async fn handle_incoming_injects_into_local_router() {
        let (router, pid, mut inbox_rx) = make_router_with_process();

        let mut rt = MeshRuntime::new("node-b".into());
        rt.set_local_router(router);

        // Build an envelope targeting a local process via RemoteNode wrapper
        let inner_target = MessageTarget::Process(pid);
        let remote_target = MessageTarget::RemoteNode {
            node_id: "node-b".into(),
            target: Box::new(inner_target),
        };
        let msg = KernelMessage::text(pid, remote_target, "from-remote");
        let envelope = MeshIpcEnvelope::new("node-a".into(), "node-b".into(), msg);
        let bytes = envelope.to_bytes().unwrap();

        rt.handle_incoming(&bytes).await.unwrap();

        let delivered = inbox_rx.try_recv().unwrap();
        assert!(matches!(delivered.target, MessageTarget::Process(p) if p == pid));
        match &delivered.payload {
            MessagePayload::Text(s) => assert_eq!(s, "from-remote"),
            other => panic!("expected Text payload, got: {other:?}"),
        }
    }

    // ── Test 5: disconnect peer ─────────────────────────────────

    #[test]
    fn disconnect_peer_removes_it() {
        let rt = MeshRuntime::new("local".into());
        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        rt.add_peer("peer-1".into(), tx);
        assert_eq!(rt.peer_count(), 1);

        rt.disconnect_peer("peer-1");
        assert_eq!(rt.peer_count(), 0);
    }

    // ── Test 6: round-trip (serialize -> deserialize -> inject) ──

    #[tokio::test]
    async fn round_trip_send_receive() {
        let (router, pid, mut inbox_rx) = make_router_with_process();

        // "Node A" side: build the runtime and a mock peer channel
        let rt_a = MeshRuntime::new("node-a".into());
        let (tx, mut peer_rx) = tokio::sync::mpsc::channel(16);
        rt_a.add_peer("node-b".into(), tx);

        // Send from A to B
        let msg = KernelMessage::text(
            pid,
            MessageTarget::RemoteNode {
                node_id: "node-b".into(),
                target: Box::new(MessageTarget::Process(pid)),
            },
            "round-trip",
        );
        rt_a.route_to_remote("node-b", msg).await.unwrap();

        // "Node B" side: receive the bytes and inject
        let wire_bytes = peer_rx.recv().await.unwrap();
        let mut rt_b = MeshRuntime::new("node-b".into());
        rt_b.set_local_router(router);
        rt_b.handle_incoming(&wire_bytes).await.unwrap();

        let delivered = inbox_rx.try_recv().unwrap();
        match &delivered.payload {
            MessagePayload::Text(s) => assert_eq!(s, "round-trip"),
            other => panic!("expected Text payload, got: {other:?}"),
        }
    }

    // ── Test 7: node ID exchange metadata ───────────────────────

    #[test]
    fn peer_connection_stores_node_id_and_timestamp() {
        let rt = MeshRuntime::new("local".into());
        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        rt.add_peer("remote-42".into(), tx);

        let entry = rt.peers.get("remote-42").unwrap();
        assert_eq!(entry.node_id, "remote-42");
        // connected_at should be very recent (within 1 second)
        let age = chrono::Utc::now() - entry.connected_at;
        assert!(age.num_seconds() < 2);
    }

    // ── Test 8: multiple peers ──────────────────────────────────

    #[test]
    fn multiple_peers_connected() {
        let rt = MeshRuntime::new("local".into());
        for i in 0..5 {
            let (tx, _rx) = tokio::sync::mpsc::channel(16);
            rt.add_peer(format!("peer-{i}"), tx);
        }
        assert_eq!(rt.peer_count(), 5);

        let mut ids = rt.peer_ids();
        ids.sort();
        assert_eq!(ids, vec!["peer-0", "peer-1", "peer-2", "peer-3", "peer-4"]);
    }

    // ── Test 9: send to unknown peer returns error ──────────────

    #[tokio::test]
    async fn send_to_unknown_peer_errors() {
        let rt = MeshRuntime::new("local".into());
        let msg = KernelMessage::text(0, MessageTarget::Broadcast, "hi");
        let envelope = MeshIpcEnvelope::new("local".into(), "ghost".into(), msg);

        let err = rt.send_to_peer("ghost", envelope).await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("peer not connected"), "got: {msg}");
    }

    // ── Test 10: handle_incoming with malformed data ────────────

    #[tokio::test]
    async fn handle_incoming_malformed_data_errors() {
        let mut rt = MeshRuntime::new("local".into());
        let (router, _pid, _rx) = make_router_with_process();
        rt.set_local_router(router);

        let err = rt.handle_incoming(b"not valid json").await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("deserialization"), "got: {msg}");
    }

    // ── Test 11: handle_incoming without router errors ──────────

    #[tokio::test]
    async fn handle_incoming_without_router_errors() {
        let rt = MeshRuntime::new("local".into());
        let msg = KernelMessage::text(0, MessageTarget::Broadcast, "hi");
        let envelope = MeshIpcEnvelope::new("a".into(), "local".into(), msg);
        let bytes = envelope.to_bytes().unwrap();

        let err = rt.handle_incoming(&bytes).await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no local router"), "got: {msg}");
    }

    // ── Test 12: disconnect nonexistent peer is safe ────────────

    #[test]
    fn disconnect_nonexistent_peer_is_noop() {
        let rt = MeshRuntime::new("local".into());
        rt.disconnect_peer("ghost"); // should not panic
        assert_eq!(rt.peer_count(), 0);
    }

    // ── Test 13: two-node message exchange ──────────────────────

    #[tokio::test]
    async fn two_nodes_exchange_messages() {
        // Create two MeshRuntime instances with different node IDs.
        let (router_b, pid_b, mut inbox_b) = make_router_with_process();

        let rt_a = MeshRuntime::new("node-a".into());
        let mut rt_b = MeshRuntime::new("node-b".into());
        rt_b.set_local_router(router_b);

        // Create channels to simulate network transport.
        let (a_to_b_tx, mut a_to_b_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
        let (b_to_a_tx, mut b_to_a_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);

        // Each node knows the other as a peer.
        rt_a.add_peer("node-b".into(), a_to_b_tx);
        rt_b.add_peer("node-a".into(), b_to_a_tx);

        // Node A sends a message targeting a process on node B.
        let msg_a = KernelMessage::text(
            pid_b,
            MessageTarget::RemoteNode {
                node_id: "node-b".into(),
                target: Box::new(MessageTarget::Process(pid_b)),
            },
            "hello-from-a",
        );
        rt_a.route_to_remote("node-b", msg_a).await.unwrap();

        // Simulate the wire: read from a_to_b channel, inject into rt_b.
        let wire_bytes = a_to_b_rx.recv().await.unwrap();
        rt_b.handle_incoming(&wire_bytes).await.unwrap();

        // The message should be in node B's local inbox.
        let delivered = inbox_b.try_recv().unwrap();
        match &delivered.payload {
            MessagePayload::Text(s) => assert_eq!(s, "hello-from-a"),
            other => panic!("expected Text, got: {other:?}"),
        }

        // Node B replies back to node A (we just verify send succeeds).
        let msg_b = KernelMessage::text(0, MessageTarget::Broadcast, "reply-from-b");
        rt_b.route_to_remote("node-a", msg_b).await.unwrap();

        let reply_bytes = b_to_a_rx.recv().await.unwrap();
        let reply_env = MeshIpcEnvelope::from_bytes(&reply_bytes).unwrap();
        assert_eq!(reply_env.source_node, "node-b");
        assert_eq!(reply_env.dest_node, "node-a");
    }

    // ── Test 14: discover_peers returns registered addresses ─────

    #[test]
    fn discover_peers_returns_registered_addresses() {
        let rt = MeshRuntime::with_discovery("node-x".into(), [0u8; 32]);
        rt.register_peer_address("peer-1", "10.0.0.1:9470");
        rt.register_peer_address("peer-2", "10.0.0.2:9470");
        rt.register_peer_address("peer-3", "10.0.0.3:9470");

        let mut peers = rt.discover_peers();
        peers.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(peers.len(), 3);
        assert_eq!(peers[0], ("peer-1".into(), "10.0.0.1:9470".into()));
        assert_eq!(peers[1], ("peer-2".into(), "10.0.0.2:9470".into()));
        assert_eq!(peers[2], ("peer-3".into(), "10.0.0.3:9470".into()));
    }

    // ── Test 15: heartbeat tracking detects suspect peer ─────────

    #[test]
    fn heartbeat_tracking_detects_dead_peer() {
        let mut rt = MeshRuntime::with_discovery("local".into(), [0u8; 32]);

        // Configure heartbeat with zero suspect timeout for instant transition.
        {
            let disc = rt.discovery_mut().unwrap();
            let mut hb = disc.heartbeat.lock().unwrap();
            *hb = HeartbeatTracker::new(crate::mesh_heartbeat::HeartbeatConfig {
                suspect_timeout: std::time::Duration::from_secs(0),
                ..crate::mesh_heartbeat::HeartbeatConfig::default()
            });
            hb.add_peer("healthy".into());
            hb.add_peer("dying".into());
        }

        // Record heartbeat for healthy peer.
        rt.record_heartbeat("healthy");

        // Simulate misses for dying peer -> suspect -> dead.
        {
            let disc = rt.discovery_mut().unwrap();
            let mut hb = disc.heartbeat.lock().unwrap();
            hb.record_miss("dying");
            hb.record_miss("dying");
        }

        let unhealthy = rt.check_peer_health();
        assert!(unhealthy.contains(&"dying".to_string()));
        assert!(!unhealthy.contains(&"healthy".to_string()));
    }

    // ── Test 16: remove_dead_peers cleans connections ────────────

    #[test]
    fn remove_dead_peers_cleans_connections() {
        let mut rt = MeshRuntime::with_discovery("local".into(), [0u8; 32]);

        // Add a peer connection.
        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        rt.add_peer("dead-peer".into(), tx);
        rt.register_peer_address("dead-peer", "10.0.0.99:9470");

        // Force the peer into Dead state.
        {
            let disc = rt.discovery_mut().unwrap();
            let mut hb = disc.heartbeat.lock().unwrap();
            *hb = HeartbeatTracker::new(crate::mesh_heartbeat::HeartbeatConfig {
                suspect_timeout: std::time::Duration::from_secs(0),
                ..crate::mesh_heartbeat::HeartbeatConfig::default()
            });
            hb.add_peer("dead-peer".into());
            hb.record_miss("dead-peer");
            hb.record_miss("dead-peer");
            assert_eq!(
                hb.peer_state("dead-peer"),
                Some(HeartbeatState::Dead)
            );
        }

        assert_eq!(rt.peer_count(), 1);
        rt.remove_dead_peers();
        assert_eq!(rt.peer_count(), 0);
        assert!(rt.discover_peers().is_empty());
    }

    // ── Test 17: Kademlia routing finds closest peers ────────────

    #[test]
    fn kademlia_routing_finds_closest_peers() {
        let mut rt = MeshRuntime::with_discovery("local".into(), [0u8; 32]);

        let disc = rt.discovery_mut().unwrap();
        let mut kad = disc.kademlia.lock().unwrap();
        // Insert peers with different XOR distances from the local key [0;32].
        for i in 1..=5u8 {
            let mut peer_key = [0u8; 32];
            peer_key[0] = i;
            kad.add_peer(
                peer_key,
                crate::mesh_kad::DhtEntry {
                    key: format!("peer-{i}"),
                    node_id: format!("peer-{i}"),
                    address: format!("10.0.0.{i}:9470"),
                    platform: "linux".into(),
                    last_seen: 1000,
                    governance_genesis_prefix: "0000000000000000".into(),
                },
            );
        }

        // Find the 3 closest to the local key.
        let closest = kad.find_closest(&[0u8; 32], 3);
        assert_eq!(closest.len(), 3);
        // The closest by XOR distance to [0;32] should be the ones with
        // the smallest first byte, which maps to node_id strings that
        // start with the smallest byte values.
    }

    // ── Test 18: with_discovery initializes state ────────────────

    #[test]
    fn with_discovery_initializes_state() {
        let rt = MeshRuntime::with_discovery("disc-node".into(), [0xAB; 32]);
        assert_eq!(rt.node_id(), "disc-node");
        assert!(rt.discovery().is_some());

        let disc = rt.discovery().unwrap();
        assert_eq!(*disc.kademlia.lock().unwrap().local_key(), [0xAB; 32]);
        assert_eq!(disc.heartbeat.lock().unwrap().peer_count(), 0);
    }

    // ── Test 19: discover_peers empty without discovery ──────────

    #[test]
    fn discover_peers_empty_without_discovery() {
        let rt = MeshRuntime::new("plain-node".into());
        assert!(rt.discover_peers().is_empty());
        // These should be safe no-ops.
        rt.register_peer_address("x", "y");
        rt.record_heartbeat("x");
        assert!(rt.check_peer_health().is_empty());
    }

    // ── Test 20: chain sync request round-trip ───────────────────

    #[test]
    fn chain_sync_request_round_trip() {
        let rt = MeshRuntime::new("sync-node".into());
        let bytes = rt.build_chain_sync_request(42);

        let req: crate::mesh_chain::ChainSyncRequest =
            serde_json::from_slice(&bytes).unwrap();
        assert_eq!(req.chain_id, 0);
        assert_eq!(req.after_sequence, 42);
        assert_eq!(req.max_events, 256);
    }

    // ── Test 21: chain sync response handling ────────────────────

    #[test]
    fn chain_sync_response_handling() {
        let rt = MeshRuntime::new("sync-node".into());

        let resp = crate::mesh_chain::ChainSyncResponse {
            chain_id: 0,
            events: vec![
                serde_json::json!({"type": "write"}),
                serde_json::json!({"type": "delete"}),
            ],
            has_more: false,
            tip_sequence: 100,
            tip_hash: "abc".into(),
        };
        let data = serde_json::to_vec(&resp).unwrap();

        let count = rt.handle_chain_sync_response(&data).unwrap();
        assert_eq!(count, 2);
    }

    // ── Test 22: chain sync response malformed errors ────────────

    #[test]
    fn chain_sync_response_malformed_errors() {
        let rt = MeshRuntime::new("sync-node".into());
        let err = rt.handle_chain_sync_response(b"bad json").unwrap_err();
        assert!(err.to_string().contains("chain sync deserialize"));
    }

    // ── Test 23: inbound peer auto-registered, topic forwarded ───

    /// End-to-end: a leaf peer sends a `mesh.subscribe` envelope over
    /// an inbound connection; the kernel auto-registers the peer, records
    /// the subscription, and then forwards a locally-published topic
    /// message back through the peer's outbound channel.
    #[tokio::test]
    async fn subscribe_then_topic_forwards_to_inbound_peer() {
        let (router, pid, _inbox) = make_router_with_process();

        // Wire a mesh runtime to the router so the Topic handler's
        // forwarder can see it.
        let mut rt = MeshRuntime::new("kernel".into());
        rt.set_local_router(router.clone());
        let rt = Arc::new(rt);
        router.set_mesh_runtime(rt.clone());

        // Inbound peer's outbound channel (what the accept loop would drain).
        let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(16);

        // Leaf sends mesh.subscribe to register interest in topic "push.leaf-x".
        let sub_msg = KernelMessage::new(
            pid,
            MessageTarget::Topic("mesh.subscribe".into()),
            MessagePayload::Json(serde_json::json!({"topic": "push.leaf-x"})),
        );
        let sub_env =
            MeshIpcEnvelope::new("leaf-x".into(), "kernel".into(), sub_msg);
        let sub_bytes = sub_env.to_bytes().unwrap();

        rt.handle_incoming_from(&sub_bytes, out_tx.clone())
            .await
            .unwrap();

        // Peer should now be registered in both the peer table and the
        // subscription registry.
        assert!(
            rt.peer_ids().contains(&"leaf-x".to_string()),
            "inbound peer should be auto-registered by source_node"
        );
        assert_eq!(rt.peers_for_topic("push.leaf-x"), vec!["leaf-x".to_string()]);

        // Locally publish to the subscribed topic. The A2A router's Topic
        // handler should forward to the mesh peer via send_to_peer.
        let push = KernelMessage::new(
            pid,
            MessageTarget::Topic("push.leaf-x".into()),
            MessagePayload::Text("hello-leaf".into()),
        );
        router.send(push).await.unwrap();

        // The outbound drain should see the forwarded envelope.
        let forwarded_bytes = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            out_rx.recv(),
        )
        .await
        .expect("forward should arrive within timeout")
        .expect("channel should not be closed");

        let forwarded = MeshIpcEnvelope::from_bytes(&forwarded_bytes).unwrap();
        assert_eq!(forwarded.source_node, "kernel");
        assert_eq!(forwarded.dest_node, "leaf-x");
        match &forwarded.message.target {
            MessageTarget::Topic(t) => assert_eq!(t, "push.leaf-x"),
            other => panic!("expected Topic target, got {other:?}"),
        }
        match &forwarded.message.payload {
            MessagePayload::Text(s) => assert_eq!(s, "hello-leaf"),
            other => panic!("expected Text payload, got {other:?}"),
        }

        // Disconnect should also clear the subscription, so a follow-up
        // publish has nowhere to go.
        rt.disconnect_peer("leaf-x");
        assert!(rt.peers_for_topic("push.leaf-x").is_empty());
    }
}
