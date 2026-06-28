//! Kademlia DHT for WeftOS mesh peer discovery (K6.2).
//!
//! Implements a simplified Kademlia Distributed Hash Table for
//! wide-area peer discovery. Uses XOR distance metric on 256-bit
//! node keys and k-bucket routing tables.
//!
//! DHT keys are **namespaced** with the governance genesis hash prefix
//! so that nodes from different governance clusters occupy disjoint
//! key-spaces.

use std::collections::HashMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::mesh_discovery::{DiscoveredPeer, DiscoveryBackend, DiscoveryError, DiscoverySource};

// ── Constants ────────────────────────────────────────────────────

/// Maximum peers per k-bucket.
pub const K_BUCKET_SIZE: usize = 20;

/// Parallel lookup factor.
pub const ALPHA: usize = 3;

/// Key-space size in bits (SHA-256).
pub const KEY_BITS: usize = 256;

// ── Key primitives ───────────────────────────────────────────────

/// A 256-bit DHT key.
pub type DhtKey = [u8; 32];

/// XOR distance between two DHT keys.
pub fn xor_distance(a: &DhtKey, b: &DhtKey) -> DhtKey {
    let mut result = [0u8; 32];
    for i in 0..32 {
        result[i] = a[i] ^ b[i];
    }
    result
}

/// Count leading zero bits in a DHT key (0..=256).
///
/// Returns 256 for the all-zero key.
pub fn leading_zeros(key: &DhtKey) -> usize {
    for (i, &byte) in key.iter().enumerate() {
        if byte != 0 {
            return i * 8 + byte.leading_zeros() as usize;
        }
    }
    256
}

/// Bucket index for a given XOR distance.
///
/// Bucket 0 is the *farthest* (MSB set), bucket 255 is the *closest*
/// (only the LSB set). Returns 0 for the all-zero distance (self).
pub fn bucket_index(distance: &DhtKey) -> usize {
    let lz = leading_zeros(distance);
    if lz >= 256 { 0 } else { 255 - lz }
}

// ── DHT entry ────────────────────────────────────────────────────

/// A record stored in the DHT.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DhtEntry {
    /// Record key string.
    pub key: String,
    /// Owning node identifier.
    pub node_id: String,
    /// Network address.
    pub address: String,
    /// Platform type.
    pub platform: String,
    /// Unix timestamp of last announcement.
    pub last_seen: u64,
    /// Governance genesis hash prefix (first 16 hex chars).
    pub governance_genesis_prefix: String,
}

// ── Namespaced DHT keys ──────────────────────────────────────────

/// A governance-namespaced DHT key.
///
/// Format: `<key_type>:<genesis_prefix>:<name>`
///
/// This ensures that nodes from different governance clusters never
/// collide in the DHT keyspace.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NamespacedDhtKey {
    /// Governance genesis hash prefix (first 16 hex chars).
    pub genesis_prefix: String,
    /// Key type (`svc`, `node`, `agent`).
    pub key_type: String,
    /// Key name.
    pub name: String,
}

impl NamespacedDhtKey {
    /// Create a service key: `svc:<genesis[0..16]>:<name>`.
    pub fn service(genesis_hash: &str, name: &str) -> Self {
        Self {
            genesis_prefix: genesis_hash.chars().take(16).collect(),
            key_type: "svc".into(),
            name: name.into(),
        }
    }

    /// Create a node presence key: `node:<genesis[0..16]>:<node_id>`.
    pub fn node(genesis_hash: &str, node_id: &str) -> Self {
        Self {
            genesis_prefix: genesis_hash.chars().take(16).collect(),
            key_type: "node".into(),
            name: node_id.into(),
        }
    }

    /// Create an agent key: `agent:<genesis[0..16]>:<agent_id>`.
    pub fn agent(genesis_hash: &str, agent_id: &str) -> Self {
        Self {
            genesis_prefix: genesis_hash.chars().take(16).collect(),
            key_type: "agent".into(),
            name: agent_id.into(),
        }
    }

    /// Format as a string key.
    pub fn to_key_string(&self) -> String {
        format!("{}:{}:{}", self.key_type, self.genesis_prefix, self.name)
    }

    /// Parse from a string key.
    pub fn from_key_string(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.splitn(3, ':').collect();
        if parts.len() == 3 {
            Some(Self {
                key_type: parts[0].into(),
                genesis_prefix: parts[1].into(),
                name: parts[2].into(),
            })
        } else {
            None
        }
    }

    /// Check if this key belongs to a specific governance cluster.
    pub fn matches_genesis(&self, genesis_hash: &str) -> bool {
        let prefix: String = genesis_hash.chars().take(16).collect();
        self.genesis_prefix == prefix
    }
}

// ── Kademlia routing table ───────────────────────────────────────

/// Simplified Kademlia routing table with k-buckets.
pub struct KademliaTable {
    /// Our node's key.
    local_key: DhtKey,
    /// k-buckets indexed by distance (256 buckets, 0 = closest).
    buckets: Vec<Vec<DhtEntry>>,
    /// All stored records (keyed by string key).
    records: HashMap<String, DhtEntry>,
}

impl KademliaTable {
    /// Create a new routing table with the given local key.
    pub fn new(local_key: DhtKey) -> Self {
        Self {
            local_key,
            buckets: (0..256).map(|_| Vec::new()).collect(),
            records: HashMap::new(),
        }
    }

    /// Our local DHT key.
    pub fn local_key(&self) -> &DhtKey {
        &self.local_key
    }

    /// Add a peer to the routing table.
    pub fn add_peer(&mut self, peer_key: DhtKey, entry: DhtEntry) {
        let dist = xor_distance(&self.local_key, &peer_key);
        let idx = bucket_index(&dist);
        let bucket = &mut self.buckets[idx];

        // Update if already present.
        if let Some(existing) = bucket.iter_mut().find(|e| e.node_id == entry.node_id) {
            *existing = entry.clone();
            self.records.insert(entry.node_id.clone(), entry);
            return;
        }

        // Add if bucket not full.
        if bucket.len() < K_BUCKET_SIZE {
            bucket.push(entry.clone());
        }
        // If full, the entry is still stored in `records` for lookup
        // but not in the routing bucket (standard Kademlia: evict only
        // if the tail peer is unreachable, which we skip in this
        // simplified implementation).

        self.records.insert(entry.node_id.clone(), entry);
    }

    /// Find the `k` closest peers to a target key.
    pub fn find_closest(&self, target: &DhtKey, k: usize) -> Vec<&DhtEntry> {
        let mut all_entries: Vec<(&DhtEntry, DhtKey)> = self
            .records
            .values()
            .map(|e| {
                let mut key = [0u8; 32];
                let bytes = e.node_id.as_bytes();
                let len = bytes.len().min(32);
                key[..len].copy_from_slice(&bytes[..len]);
                let dist = xor_distance(target, &key);
                (e, dist)
            })
            .collect();

        all_entries.sort_by(|a, b| a.1.cmp(&b.1));
        all_entries.into_iter().take(k).map(|(e, _)| e).collect()
    }

    /// Store a record in the DHT.
    pub fn put(&mut self, key: String, entry: DhtEntry) {
        self.records.insert(key, entry);
    }

    /// Retrieve a record from the DHT by key.
    pub fn get(&self, key: &str) -> Option<&DhtEntry> {
        self.records.get(key)
    }

    /// Number of peers/records in the table.
    pub fn peer_count(&self) -> usize {
        self.records.len()
    }

    /// Number of entries in a specific bucket.
    pub fn bucket_len(&self, idx: usize) -> usize {
        self.buckets.get(idx).map_or(0, |b| b.len())
    }
}

// ── Kademlia discovery backend ───────────────────────────────────

/// Kademlia DHT discovery backend.
///
/// Manages a local routing table and provides service/node registration
/// with governance-namespaced keys.
pub struct KademliaDiscovery {
    table: KademliaTable,
    governance_genesis: String,
    local_node_id: String,
    local_address: String,
    platform: String,
    pending: Vec<DiscoveredPeer>,
    active: bool,
}

impl KademliaDiscovery {
    /// Create a new Kademlia discovery backend.
    pub fn new(
        local_key: DhtKey,
        node_id: String,
        address: String,
        platform: String,
        governance_genesis: String,
    ) -> Self {
        Self {
            table: KademliaTable::new(local_key),
            governance_genesis,
            local_node_id: node_id,
            local_address: address,
            platform,
            pending: Vec::new(),
            active: false,
        }
    }

    /// Add a bootstrap peer to the routing table.
    pub fn add_bootstrap_peer(&mut self, peer_key: DhtKey, entry: DhtEntry) {
        self.table.add_peer(peer_key, entry);
    }

    /// Store a service in the DHT with governance namespacing.
    pub fn put_service(&mut self, service_name: &str, entry: DhtEntry) {
        let key = NamespacedDhtKey::service(&self.governance_genesis, service_name);
        self.table.put(key.to_key_string(), entry);
    }

    /// Resolve a service from the DHT.
    pub fn get_service(&self, service_name: &str) -> Option<&DhtEntry> {
        let key = NamespacedDhtKey::service(&self.governance_genesis, service_name);
        self.table.get(&key.to_key_string())
    }

    /// Store a node presence record in the DHT.
    pub fn put_node(&mut self, node_id: &str, entry: DhtEntry) {
        let key = NamespacedDhtKey::node(&self.governance_genesis, node_id);
        self.table.put(key.to_key_string(), entry);
    }

    /// Resolve a node from the DHT.
    pub fn get_node(&self, node_id: &str) -> Option<&DhtEntry> {
        let key = NamespacedDhtKey::node(&self.governance_genesis, node_id);
        self.table.get(&key.to_key_string())
    }

    /// Add a peer discovered externally (e.g. from mDNS) into the
    /// DHT and queue it for delivery via `poll`.
    pub fn inject_peer(&mut self, peer_key: DhtKey, entry: DhtEntry) {
        let peer = DiscoveredPeer {
            node_id: entry.node_id.clone(),
            address: entry.address.clone(),
            platform: entry.platform.clone(),
            source: DiscoverySource::Kademlia,
        };
        self.table.add_peer(peer_key, entry);
        self.pending.push(peer);
    }

    /// Access the routing table.
    pub fn table(&self) -> &KademliaTable {
        &self.table
    }

    /// Governance genesis hash.
    pub fn governance_genesis(&self) -> &str {
        &self.governance_genesis
    }
}

#[async_trait]
impl DiscoveryBackend for KademliaDiscovery {
    fn name(&self) -> &str {
        "kademlia"
    }

    async fn start(&mut self) -> Result<(), DiscoveryError> {
        self.active = true;

        // Register ourselves in the DHT under the governance namespace.
        let self_key = NamespacedDhtKey::node(&self.governance_genesis, &self.local_node_id);
        self.table.put(
            self_key.to_key_string(),
            DhtEntry {
                key: self_key.to_key_string(),
                node_id: self.local_node_id.clone(),
                address: self.local_address.clone(),
                platform: self.platform.clone(),
                last_seen: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
                governance_genesis_prefix: self.governance_genesis.chars().take(16).collect(),
            },
        );
        Ok(())
    }

    async fn poll(&mut self) -> Vec<DiscoveredPeer> {
        if !self.active {
            return vec![];
        }
        std::mem::take(&mut self.pending)
    }

    async fn stop(&mut self) -> Result<(), DiscoveryError> {
        self.active = false;
        self.pending.clear();
        Ok(())
    }
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── XOR distance ─────────────────────────────────────────────

    #[test]
    fn xor_distance_identity() {
        let key = [0xABu8; 32];
        let dist = xor_distance(&key, &key);
        assert_eq!(dist, [0u8; 32], "distance to self must be zero");
    }

    #[test]
    fn xor_distance_symmetric() {
        let a = [0x01u8; 32];
        let b = [0xFFu8; 32];
        assert_eq!(xor_distance(&a, &b), xor_distance(&b, &a));
    }

    #[test]
    fn xor_distance_known_value() {
        let mut a = [0u8; 32];
        let mut b = [0u8; 32];
        a[0] = 0b1010_0000;
        b[0] = 0b0110_0000;
        let dist = xor_distance(&a, &b);
        assert_eq!(dist[0], 0b1100_0000);
        assert_eq!(dist[1..], [0u8; 31]);
    }

    // ── Leading zeros ────────────────────────────────────────────

    #[test]
    fn leading_zeros_all_zero() {
        assert_eq!(leading_zeros(&[0u8; 32]), 256);
    }

    #[test]
    fn leading_zeros_first_bit_set() {
        let mut key = [0u8; 32];
        key[0] = 0x80; // 1000_0000
        assert_eq!(leading_zeros(&key), 0);
    }

    #[test]
    fn leading_zeros_last_bit_set() {
        let mut key = [0u8; 32];
        key[31] = 0x01;
        assert_eq!(leading_zeros(&key), 255);
    }

    #[test]
    fn leading_zeros_mid_byte() {
        let mut key = [0u8; 32];
        key[2] = 0x04; // 0000_0100 => 5 zeros in this byte, + 16 from first 2 bytes = 21
        assert_eq!(leading_zeros(&key), 21);
    }

    // ── Bucket index ─────────────────────────────────────────────

    #[test]
    fn bucket_index_zero_distance() {
        assert_eq!(bucket_index(&[0u8; 32]), 0);
    }

    #[test]
    fn bucket_index_max_distance() {
        let mut key = [0u8; 32];
        key[0] = 0x80; // MSB set => leading_zeros = 0 => bucket 255
        assert_eq!(bucket_index(&key), 255);
    }

    #[test]
    fn bucket_index_lsb_only() {
        let mut key = [0u8; 32];
        key[31] = 0x01; // leading_zeros = 255 => bucket 0
        assert_eq!(bucket_index(&key), 0);
    }

    // ── KademliaTable ────────────────────────────────────────────

    fn make_entry(node_id: &str, addr: &str) -> DhtEntry {
        DhtEntry {
            key: node_id.to_string(),
            node_id: node_id.to_string(),
            address: addr.to_string(),
            platform: "linux".into(),
            last_seen: 1000,
            governance_genesis_prefix: "abcdef0123456789".into(),
        }
    }

    #[test]
    fn table_add_and_get() {
        let local_key = [0u8; 32];
        let mut table = KademliaTable::new(local_key);

        let mut peer_key = [0u8; 32];
        peer_key[0] = 0x01;
        let entry = make_entry("peer-1", "10.0.0.1:9470");

        table.add_peer(peer_key, entry.clone());
        assert_eq!(table.peer_count(), 1);

        // Stored in records by node_id.
        let found = table.get("peer-1").unwrap();
        assert_eq!(found.address, "10.0.0.1:9470");
    }

    #[test]
    fn table_add_peer_updates_existing() {
        let local_key = [0u8; 32];
        let mut table = KademliaTable::new(local_key);

        let mut peer_key = [0u8; 32];
        peer_key[0] = 0x01;

        table.add_peer(peer_key, make_entry("peer-1", "old-addr"));
        table.add_peer(peer_key, make_entry("peer-1", "new-addr"));

        assert_eq!(table.peer_count(), 1);
        assert_eq!(table.get("peer-1").unwrap().address, "new-addr");
    }

    #[test]
    fn table_find_closest() {
        let local_key = [0u8; 32];
        let mut table = KademliaTable::new(local_key);

        for i in 0..5u8 {
            let mut pk = [0u8; 32];
            pk[0] = i + 1;
            table.add_peer(
                pk,
                make_entry(&format!("n-{i}"), &format!("10.0.0.{i}:9470")),
            );
        }

        let target = [0u8; 32]; // same as local
        let closest = table.find_closest(&target, 3);
        assert_eq!(closest.len(), 3);
    }

    #[test]
    fn table_put_get_by_string_key() {
        let mut table = KademliaTable::new([0u8; 32]);
        let entry = make_entry("some-node", "addr");
        table.put("custom-key".into(), entry);
        assert!(table.get("custom-key").is_some());
        assert!(table.get("nonexistent").is_none());
    }

    #[test]
    fn table_bucket_size_limit() {
        let local_key = [0u8; 32];
        let mut table = KademliaTable::new(local_key);

        // Insert K_BUCKET_SIZE + 5 peers that all land in the same bucket.
        for i in 0..(K_BUCKET_SIZE + 5) {
            let mut pk = [0u8; 32];
            pk[0] = 0x80; // all go to bucket 255
            pk[1] = i as u8; // different keys
            table.add_peer(
                pk,
                make_entry(&format!("peer-{i}"), &format!("10.0.0.{i}:9470")),
            );
        }

        // Bucket 255 should be capped at K_BUCKET_SIZE.
        assert_eq!(table.bucket_len(255), K_BUCKET_SIZE);
        // But all are in records.
        assert_eq!(table.peer_count(), K_BUCKET_SIZE + 5);
    }

    // ── NamespacedDhtKey ─────────────────────────────────────────

    #[test]
    fn namespaced_key_service() {
        let key = NamespacedDhtKey::service("abcdef0123456789extra", "my-svc");
        assert_eq!(key.to_key_string(), "svc:abcdef0123456789:my-svc");
    }

    #[test]
    fn namespaced_key_node() {
        let key = NamespacedDhtKey::node("deadbeefdeadbeefmore", "node-42");
        assert_eq!(key.to_key_string(), "node:deadbeefdeadbeef:node-42");
    }

    #[test]
    fn namespaced_key_agent() {
        let key = NamespacedDhtKey::agent("1234567890abcdef", "agent-x");
        assert_eq!(key.to_key_string(), "agent:1234567890abcdef:agent-x");
    }

    #[test]
    fn namespaced_key_roundtrip() {
        let original = NamespacedDhtKey::service("abcdef0123456789", "test-svc");
        let s = original.to_key_string();
        let parsed = NamespacedDhtKey::from_key_string(&s).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn namespaced_key_parse_invalid() {
        assert!(NamespacedDhtKey::from_key_string("nocolons").is_none());
        assert!(NamespacedDhtKey::from_key_string("one:two").is_none());
    }

    #[test]
    fn namespaced_key_matches_genesis() {
        let key = NamespacedDhtKey::service("abcdef0123456789extra", "svc");
        assert!(key.matches_genesis("abcdef0123456789extra"));
        assert!(key.matches_genesis("abcdef0123456789different_suffix"));
        assert!(!key.matches_genesis("0000000000000000"));
    }

    #[test]
    fn governance_prefix_isolation() {
        // Two different governance clusters should produce different keys.
        let key_a = NamespacedDhtKey::service("aaaa000000000000", "api-gateway");
        let key_b = NamespacedDhtKey::service("bbbb000000000000", "api-gateway");

        assert_ne!(key_a.to_key_string(), key_b.to_key_string());
        assert!(!key_a.matches_genesis("bbbb000000000000"));
        assert!(!key_b.matches_genesis("aaaa000000000000"));
    }

    // ── KademliaDiscovery lifecycle ──────────────────────────────

    #[tokio::test]
    async fn kademlia_discovery_lifecycle() {
        let mut disc = KademliaDiscovery::new(
            [0u8; 32],
            "kad-node-1".into(),
            "10.0.0.1:9470".into(),
            "linux".into(),
            "deadbeef01234567abcdef".into(),
        );

        assert_eq!(disc.name(), "kademlia");
        assert!(!disc.active);

        disc.start().await.unwrap();
        assert!(disc.active);

        // Self-registration should have created a record.
        let self_key = NamespacedDhtKey::node("deadbeef01234567abcdef", "kad-node-1");
        assert!(disc.table().get(&self_key.to_key_string()).is_some());

        disc.stop().await.unwrap();
        assert!(!disc.active);
    }

    #[tokio::test]
    async fn kademlia_put_get_service() {
        let mut disc = KademliaDiscovery::new(
            [0u8; 32],
            "node-1".into(),
            "10.0.0.1:9470".into(),
            "linux".into(),
            "aabbccdd00112233".into(),
        );
        disc.start().await.unwrap();

        let entry = DhtEntry {
            key: "api-gw".into(),
            node_id: "provider-1".into(),
            address: "10.0.0.5:8080".into(),
            platform: "linux".into(),
            last_seen: 2000,
            governance_genesis_prefix: "aabbccdd00112233".into(),
        };
        disc.put_service("api-gateway", entry.clone());

        let found = disc.get_service("api-gateway").unwrap();
        assert_eq!(found.node_id, "provider-1");
        assert_eq!(found.address, "10.0.0.5:8080");

        // Different service name should not match.
        assert!(disc.get_service("auth-service").is_none());
    }

    #[tokio::test]
    async fn kademlia_inject_peer_queued() {
        let mut disc = KademliaDiscovery::new(
            [0u8; 32],
            "local".into(),
            "127.0.0.1:9470".into(),
            "linux".into(),
            "0000".into(),
        );
        disc.start().await.unwrap();

        let mut pk = [0u8; 32];
        pk[0] = 0x42;
        disc.inject_peer(pk, make_entry("remote-peer", "10.0.0.9:9470"));

        let peers = disc.poll().await;
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].node_id, "remote-peer");
        assert_eq!(peers[0].source, DiscoverySource::Kademlia);

        // Second poll should be empty.
        let second = disc.poll().await;
        assert!(second.is_empty());
    }

    #[tokio::test]
    async fn kademlia_poll_inactive_empty() {
        let mut disc =
            KademliaDiscovery::new([0u8; 32], "n".into(), "a".into(), "p".into(), "g".into());
        // Not started.
        let peers = disc.poll().await;
        assert!(peers.is_empty());
    }

    #[test]
    fn dht_entry_serde_roundtrip() {
        let entry = make_entry("serde-node", "10.0.0.1:9470");
        let json = serde_json::to_string(&entry).unwrap();
        let back: DhtEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, back);
    }
}
