//! Static seed peer bootstrap discovery (K6.2).
//!
//! The simplest discovery mechanism: connect to a preconfigured list
//! of seed peer addresses. Always available, no extra dependencies.
//!
//! # Status (WEFT-151 audit)
//!
//! **Orphan — library only.** No production callers outside this module
//! and re-exports. `boot.rs` connects `mesh_config.seed_peers` by
//! iterating addresses and calling `MeshTransport::connect` directly;
//! it never constructs [`BootstrapDiscovery`] or
//! [`PeerExchangeDiscovery`]. [`crate::mesh_discovery::DiscoveryCoordinator`]
//! is itself only exercised in its unit tests (no daemon registration).
//!
//! **Disposition:** keep as `DiscoveryBackend` implementations.
//! **Wiring schedule (0.8.x):** in boot mesh init, build a
//! `DiscoveryCoordinator`, `add_backend(Box::new(BootstrapDiscovery::new(seed_peers)))`,
//! optionally PEX after handshake, and feed discovered peers into
//! the existing seed-connect loop / `MeshRuntime`.

use crate::mesh_discovery::{DiscoveredPeer, DiscoveryBackend, DiscoveryError, DiscoverySource};
use async_trait::async_trait;

/// Bootstrap discovery from static seed peers.
pub struct BootstrapDiscovery {
    /// Seed peer addresses (e.g., "quic://192.168.1.100:9489").
    seed_addresses: Vec<String>,
    /// Peers discovered from seed connections.
    discovered: Vec<DiscoveredPeer>,
    /// Whether initial bootstrap has been performed.
    bootstrapped: bool,
}

impl BootstrapDiscovery {
    /// Create a new bootstrap discovery with seed peer addresses.
    pub fn new(seed_addresses: Vec<String>) -> Self {
        Self {
            seed_addresses,
            discovered: Vec::new(),
            bootstrapped: false,
        }
    }

    /// Get the configured seed addresses.
    pub fn seed_addresses(&self) -> &[String] {
        &self.seed_addresses
    }

    /// Add a seed address dynamically.
    pub fn add_seed(&mut self, address: String) {
        if !self.seed_addresses.contains(&address) {
            self.seed_addresses.push(address);
        }
    }
}

#[async_trait]
impl DiscoveryBackend for BootstrapDiscovery {
    fn name(&self) -> &str {
        "bootstrap"
    }

    async fn start(&mut self) -> Result<(), DiscoveryError> {
        // On start, produce DiscoveredPeer for each seed address.
        // Actual connection happens in the mesh listener, not here.
        // Discovery just reports "these peers exist, try connecting."
        if !self.bootstrapped {
            for (i, addr) in self.seed_addresses.iter().enumerate() {
                self.discovered.push(DiscoveredPeer {
                    node_id: format!("seed-{i}"),
                    address: addr.clone(),
                    platform: "unknown".into(),
                    source: DiscoverySource::SeedPeer,
                });
            }
            self.bootstrapped = true;
        }
        Ok(())
    }

    async fn poll(&mut self) -> Vec<DiscoveredPeer> {
        // Return any undrained discovered peers, then clear.
        std::mem::take(&mut self.discovered)
    }

    async fn stop(&mut self) -> Result<(), DiscoveryError> {
        self.discovered.clear();
        Ok(())
    }
}

/// Peer exchange discovery: learns about peers from other peers' lists.
pub struct PeerExchangeDiscovery {
    /// Peers learned from peer list exchange during handshake.
    pending: Vec<DiscoveredPeer>,
}

impl PeerExchangeDiscovery {
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
        }
    }

    /// Add peers learned from a remote node's peer list.
    pub fn add_peers_from_exchange(&mut self, peers: Vec<DiscoveredPeer>) {
        for mut peer in peers {
            peer.source = DiscoverySource::PeerExchange;
            self.pending.push(peer);
        }
    }
}

impl Default for PeerExchangeDiscovery {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DiscoveryBackend for PeerExchangeDiscovery {
    fn name(&self) -> &str {
        "peer-exchange"
    }

    async fn start(&mut self) -> Result<(), DiscoveryError> {
        Ok(())
    }

    async fn poll(&mut self) -> Vec<DiscoveredPeer> {
        std::mem::take(&mut self.pending)
    }

    async fn stop(&mut self) -> Result<(), DiscoveryError> {
        self.pending.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bootstrap_start_produces_peers() {
        let seeds = vec![
            "quic://10.0.0.1:9489".to_string(),
            "quic://10.0.0.2:9489".to_string(),
        ];
        let mut disc = BootstrapDiscovery::new(seeds);
        disc.start().await.unwrap();

        let peers = disc.poll().await;
        assert_eq!(peers.len(), 2);
        assert_eq!(peers[0].node_id, "seed-0");
        assert_eq!(peers[0].address, "quic://10.0.0.1:9489");
        assert_eq!(peers[0].source, DiscoverySource::SeedPeer);
        assert_eq!(peers[1].node_id, "seed-1");
    }

    #[tokio::test]
    async fn bootstrap_poll_drains() {
        let mut disc = BootstrapDiscovery::new(vec!["addr".into()]);
        disc.start().await.unwrap();

        let first = disc.poll().await;
        assert_eq!(first.len(), 1);

        let second = disc.poll().await;
        assert!(second.is_empty(), "second poll should be empty");
    }

    #[tokio::test]
    async fn bootstrap_double_start_no_duplicate() {
        let mut disc = BootstrapDiscovery::new(vec!["a".into()]);
        disc.start().await.unwrap();
        disc.start().await.unwrap(); // second start is a no-op

        let peers = disc.poll().await;
        assert_eq!(peers.len(), 1, "should not duplicate on second start");
    }

    #[tokio::test]
    async fn bootstrap_add_seed() {
        let mut disc = BootstrapDiscovery::new(vec!["a".into()]);
        disc.add_seed("b".into());
        disc.add_seed("a".into()); // duplicate, should be ignored
        assert_eq!(disc.seed_addresses().len(), 2);
    }

    #[tokio::test]
    async fn bootstrap_stop_clears() {
        let mut disc = BootstrapDiscovery::new(vec!["a".into()]);
        disc.start().await.unwrap();
        disc.stop().await.unwrap();

        let peers = disc.poll().await;
        assert!(peers.is_empty());
    }

    #[tokio::test]
    async fn peer_exchange_add_and_poll() {
        let mut pex = PeerExchangeDiscovery::new();
        assert_eq!(pex.name(), "peer-exchange");

        pex.start().await.unwrap();

        let incoming = vec![DiscoveredPeer {
            node_id: "remote-1".into(),
            address: "quic://192.168.1.50:9489".into(),
            platform: "darwin".into(),
            source: DiscoverySource::Manual, // will be overwritten
        }];
        pex.add_peers_from_exchange(incoming);

        let peers = pex.poll().await;
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].source, DiscoverySource::PeerExchange);
        assert_eq!(peers[0].node_id, "remote-1");

        let second = pex.poll().await;
        assert!(second.is_empty());
    }

    #[tokio::test]
    async fn peer_exchange_stop() {
        let mut pex = PeerExchangeDiscovery::new();
        pex.add_peers_from_exchange(vec![DiscoveredPeer {
            node_id: "n".into(),
            address: "a".into(),
            platform: "p".into(),
            source: DiscoverySource::Manual,
        }]);
        pex.stop().await.unwrap();
        let peers = pex.poll().await;
        assert!(peers.is_empty());
    }
}
