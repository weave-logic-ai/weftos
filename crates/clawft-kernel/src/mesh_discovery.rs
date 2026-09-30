//! Peer discovery for mesh networking (K6.2).
//!
//! Discovery is the mechanism by which nodes find each other.
//! Three strategies are supported:
//! - Static seed peers (always available, no extra deps)
//! - mDNS for LAN discovery (behind mesh-discovery feature)
//! - Kademlia DHT for WAN discovery (behind mesh-discovery feature)
//!
//! # Peer event stream (WEFT-120)
//!
//! Live mesh membership changes are published as [`MeshPeerEvent`] on a
//! [`MeshPeerEventBus`]. [`crate::mesh_runtime::MeshRuntime`] emits events
//! on connect / disconnect / health transitions; [`crate::cluster::ClusterMembership`]
//! (and `ClusterService` when the `cluster` feature is on) subscribe so
//! cluster membership reflects live mesh state without manual pull-sync.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

/// A discovered peer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredPeer {
    /// Node identifier.
    pub node_id: String,
    /// Network address.
    pub address: String,
    /// Platform type.
    pub platform: String,
    /// How this peer was discovered.
    pub source: DiscoverySource,
}

/// How a peer was discovered.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiscoverySource {
    /// Static seed peer from configuration.
    SeedPeer,
    /// Discovered via mDNS on the local network.
    Mdns,
    /// Discovered via Kademlia DHT.
    Kademlia,
    /// Learned from another peer's peer list exchange.
    PeerExchange,
    /// Manually added by operator.
    Manual,
}

/// Live mesh peer membership / health change (WEFT-120).
///
/// Published by the mesh runtime whenever a peer joins, leaves, or
/// transitions health state. Consumers (notably `ClusterService` /
/// `ClusterMembership`) apply these atomically so cluster membership
/// mirrors mesh state without a pull-based `sync_to_membership` loop.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MeshPeerEvent {
    /// Peer established a mesh connection (or reconnected after drop).
    Joined {
        /// Remote node id.
        node_id: String,
        /// Network address if known (seed / discovery map).
        address: Option<String>,
        /// Platform label when known.
        platform: Option<String>,
    },
    /// Peer disconnected (graceful leave or connection drop).
    Left {
        /// Remote node id.
        node_id: String,
    },
    /// Heartbeat / successful probe — peer is alive.
    Alive {
        /// Remote node id.
        node_id: String,
    },
    /// SWIM / heartbeat marked the peer suspect.
    Suspect {
        /// Remote node id.
        node_id: String,
    },
    /// Peer confirmed unreachable (partition or dead).
    Unreachable {
        /// Remote node id.
        node_id: String,
    },
    /// Peer recovered after Unreachable / Suspect (partition recovery).
    Recovered {
        /// Remote node id.
        node_id: String,
        /// Address if known on recovery path.
        address: Option<String>,
    },
}

impl MeshPeerEvent {
    /// Node id referenced by this event.
    pub fn node_id(&self) -> &str {
        match self {
            MeshPeerEvent::Joined { node_id, .. }
            | MeshPeerEvent::Left { node_id }
            | MeshPeerEvent::Alive { node_id }
            | MeshPeerEvent::Suspect { node_id }
            | MeshPeerEvent::Unreachable { node_id }
            | MeshPeerEvent::Recovered { node_id, .. } => node_id.as_str(),
        }
    }
}

/// Default capacity for the peer-event broadcast bus.
const DEFAULT_PEER_EVENT_CAPACITY: usize = 256;

/// Fan-out bus for [`MeshPeerEvent`] (tokio broadcast).
///
/// Multiple subscribers (cluster membership, observability, tests) can
/// each hold a receiver. Slow consumers lag and skip old events rather
/// than blocking mesh I/O.
#[derive(Clone, Debug)]
pub struct MeshPeerEventBus {
    tx: broadcast::Sender<MeshPeerEvent>,
}

impl MeshPeerEventBus {
    /// Create a bus with the default capacity.
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_PEER_EVENT_CAPACITY)
    }

    /// Create a bus with an explicit ring capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity.max(1));
        Self { tx }
    }

    /// Publish a peer event (best-effort; no-op when no receivers).
    pub fn emit(&self, event: MeshPeerEvent) {
        let _ = self.tx.send(event);
    }

    /// Subscribe to the peer-event stream.
    pub fn subscribe(&self) -> broadcast::Receiver<MeshPeerEvent> {
        self.tx.subscribe()
    }

    /// Number of active receivers.
    pub fn receiver_count(&self) -> usize {
        self.tx.receiver_count()
    }

    /// Shared sender for advanced wiring (e.g. external discovery).
    pub fn sender(&self) -> broadcast::Sender<MeshPeerEvent> {
        self.tx.clone()
    }
}

impl Default for MeshPeerEventBus {
    fn default() -> Self {
        Self::new()
    }
}

/// Discovery backend trait.
#[async_trait]
pub trait DiscoveryBackend: Send + Sync + 'static {
    /// Name of this discovery mechanism.
    fn name(&self) -> &str;
    /// Start the discovery mechanism.
    async fn start(&mut self) -> Result<(), DiscoveryError>;
    /// Poll for newly discovered peers (non-blocking).
    async fn poll(&mut self) -> Vec<DiscoveredPeer>;
    /// Stop the discovery mechanism.
    async fn stop(&mut self) -> Result<(), DiscoveryError>;
}

/// Discovery errors.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error("discovery backend error: {0}")]
    Backend(String),
    #[error("invalid seed address: {0}")]
    InvalidAddress(String),
    #[error("discovery not started")]
    NotStarted,
}

/// Discovery coordinator that manages multiple discovery backends.
pub struct DiscoveryCoordinator {
    backends: Vec<Box<dyn DiscoveryBackend>>,
    known_peers: std::collections::HashSet<String>,
}

impl DiscoveryCoordinator {
    pub fn new() -> Self {
        Self {
            backends: Vec::new(),
            known_peers: std::collections::HashSet::new(),
        }
    }

    /// Add a discovery backend.
    pub fn add_backend(&mut self, backend: Box<dyn DiscoveryBackend>) {
        self.backends.push(backend);
    }

    /// Start all discovery backends.
    pub async fn start_all(&mut self) -> Result<(), DiscoveryError> {
        for backend in &mut self.backends {
            backend.start().await?;
        }
        Ok(())
    }

    /// Poll all backends for new peers. Returns only newly discovered peers.
    pub async fn poll_all(&mut self) -> Vec<DiscoveredPeer> {
        let mut new_peers = Vec::new();
        for backend in &mut self.backends {
            let peers = backend.poll().await;
            for peer in peers {
                if self.known_peers.insert(peer.node_id.clone()) {
                    new_peers.push(peer);
                }
            }
        }
        new_peers
    }

    /// Stop all discovery backends.
    pub async fn stop_all(&mut self) -> Result<(), DiscoveryError> {
        for backend in &mut self.backends {
            backend.stop().await?;
        }
        Ok(())
    }

    /// Number of known peers.
    pub fn known_peer_count(&self) -> usize {
        self.known_peers.len()
    }

    /// Check if a peer is already known.
    pub fn is_known(&self, node_id: &str) -> bool {
        self.known_peers.contains(node_id)
    }
}

impl Default for DiscoveryCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mock discovery backend for testing.
    struct MockBackend {
        name: &'static str,
        peers: Vec<DiscoveredPeer>,
        started: bool,
    }

    impl MockBackend {
        fn new(name: &'static str, peers: Vec<DiscoveredPeer>) -> Self {
            Self {
                name,
                peers,
                started: false,
            }
        }
    }

    #[async_trait]
    impl DiscoveryBackend for MockBackend {
        fn name(&self) -> &str {
            self.name
        }
        async fn start(&mut self) -> Result<(), DiscoveryError> {
            self.started = true;
            Ok(())
        }
        async fn poll(&mut self) -> Vec<DiscoveredPeer> {
            std::mem::take(&mut self.peers)
        }
        async fn stop(&mut self) -> Result<(), DiscoveryError> {
            self.started = false;
            Ok(())
        }
    }

    fn make_peer(id: &str, source: DiscoverySource) -> DiscoveredPeer {
        DiscoveredPeer {
            node_id: id.to_string(),
            address: format!("127.0.0.1:900{}", id.len()),
            platform: "linux".into(),
            source,
        }
    }

    #[tokio::test]
    async fn coordinator_add_and_start() {
        let mut coord = DiscoveryCoordinator::new();
        assert_eq!(coord.known_peer_count(), 0);

        let backend = MockBackend::new("mock", vec![make_peer("a", DiscoverySource::Manual)]);
        coord.add_backend(Box::new(backend));
        coord.start_all().await.unwrap();
    }

    #[tokio::test]
    async fn coordinator_poll_deduplicates() {
        let mut coord = DiscoveryCoordinator::new();

        let b1 = MockBackend::new(
            "b1",
            vec![
                make_peer("node-1", DiscoverySource::SeedPeer),
                make_peer("node-2", DiscoverySource::SeedPeer),
            ],
        );
        let b2 = MockBackend::new(
            "b2",
            vec![
                make_peer("node-2", DiscoverySource::Mdns), // duplicate id
                make_peer("node-3", DiscoverySource::Mdns),
            ],
        );
        coord.add_backend(Box::new(b1));
        coord.add_backend(Box::new(b2));
        coord.start_all().await.unwrap();

        let peers = coord.poll_all().await;
        // node-2 appears in both backends; should only be returned once.
        assert_eq!(peers.len(), 3);
        assert_eq!(coord.known_peer_count(), 3);
        assert!(coord.is_known("node-1"));
        assert!(coord.is_known("node-2"));
        assert!(coord.is_known("node-3"));
    }

    #[tokio::test]
    async fn coordinator_second_poll_empty() {
        let mut coord = DiscoveryCoordinator::new();
        let b = MockBackend::new("b", vec![make_peer("x", DiscoverySource::Manual)]);
        coord.add_backend(Box::new(b));
        coord.start_all().await.unwrap();

        let first = coord.poll_all().await;
        assert_eq!(first.len(), 1);

        let second = coord.poll_all().await;
        assert!(second.is_empty());
    }

    #[tokio::test]
    async fn coordinator_stop_all() {
        let mut coord = DiscoveryCoordinator::new();
        let b = MockBackend::new("b", vec![]);
        coord.add_backend(Box::new(b));
        coord.start_all().await.unwrap();
        coord.stop_all().await.unwrap();
    }

    #[test]
    fn discovered_peer_serde_roundtrip() {
        let peer = make_peer("serde-test", DiscoverySource::Kademlia);
        let json = serde_json::to_string(&peer).unwrap();
        let back: DiscoveredPeer = serde_json::from_str(&json).unwrap();
        assert_eq!(back.node_id, "serde-test");
        assert_eq!(back.source, DiscoverySource::Kademlia);
    }

    #[test]
    fn mesh_peer_event_node_id() {
        let e = MeshPeerEvent::Joined {
            node_id: "n1".into(),
            address: Some("127.0.0.1:9".into()),
            platform: None,
        };
        assert_eq!(e.node_id(), "n1");
        assert_eq!(
            MeshPeerEvent::Left {
                node_id: "n2".into()
            }
            .node_id(),
            "n2"
        );
    }

    #[tokio::test]
    async fn peer_event_bus_delivers_to_subscriber() {
        let bus = MeshPeerEventBus::new();
        let mut rx = bus.subscribe();
        bus.emit(MeshPeerEvent::Alive {
            node_id: "p".into(),
        });
        let got = rx.recv().await.unwrap();
        assert_eq!(
            got,
            MeshPeerEvent::Alive {
                node_id: "p".into()
            }
        );
    }

    #[test]
    fn mesh_peer_event_serde_roundtrip() {
        let e = MeshPeerEvent::Recovered {
            node_id: "r1".into(),
            address: Some("10.0.0.2:9489".into()),
        };
        let json = serde_json::to_string(&e).unwrap();
        let back: MeshPeerEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, e);
    }

    #[test]
    fn discovery_source_serde_roundtrip() {
        let sources = [
            DiscoverySource::SeedPeer,
            DiscoverySource::Mdns,
            DiscoverySource::Kademlia,
            DiscoverySource::PeerExchange,
            DiscoverySource::Manual,
        ];
        for src in &sources {
            let json = serde_json::to_string(src).unwrap();
            let back: DiscoverySource = serde_json::from_str(&json).unwrap();
            assert_eq!(&back, src);
        }
    }

    #[test]
    fn discovery_error_display() {
        let e = DiscoveryError::Backend("timeout".into());
        assert!(e.to_string().contains("timeout"));
        let e = DiscoveryError::InvalidAddress("bad:addr".into());
        assert!(e.to_string().contains("bad:addr"));
        let e = DiscoveryError::NotStarted;
        assert!(e.to_string().contains("not started"));
    }
}
