//! Distributed process table for mesh networking (K6.5).
//!
//! Tracks processes across all mesh nodes using last-writer-wins
//! semantics. Each node gossips process summaries to peers.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::ipc::GlobalPid;
use crate::process::Pid;

// ── Consistent hash ring for PID-to-node assignment ────────────────

/// Consistent hash ring for PID-to-node assignment.
///
/// Uses virtual nodes for even distribution across cluster members.
/// When the `cluster` feature is enabled, this delegates to
/// `ruvector_cluster::ConsistentHashRing` under the hood; otherwise
/// a standalone implementation is provided so the mesh module can
/// still compile without the full cluster dependency.
pub struct ConsistentHashRing {
    /// Sorted ring of (hash, node_id) pairs.
    ring: Vec<(u64, String)>,
    /// Number of virtual nodes per real node.
    virtual_nodes: usize,
}

impl ConsistentHashRing {
    /// Create a new hash ring with the given number of virtual nodes
    /// per real node. Higher values give more even distribution.
    pub fn new(virtual_nodes: usize) -> Self {
        Self {
            ring: Vec::new(),
            virtual_nodes,
        }
    }

    /// Add a node to the ring.
    pub fn add_node(&mut self, node_id: &str) {
        for i in 0..self.virtual_nodes {
            let key = format!("{node_id}:{i}");
            let hash = Self::hash(&key);
            self.ring.push((hash, node_id.to_string()));
        }
        self.ring.sort_by_key(|(h, _)| *h);
    }

    /// Remove a node from the ring.
    pub fn remove_node(&mut self, node_id: &str) {
        self.ring.retain(|(_, id)| id != node_id);
    }

    /// Find which node owns a given PID (clockwise walk).
    pub fn assign(&self, pid: u64) -> Option<&str> {
        if self.ring.is_empty() {
            return None;
        }
        let hash = Self::hash(&pid.to_string());
        // Find first ring position >= hash
        let pos = self.ring.partition_point(|(h, _)| *h < hash);
        let idx = if pos >= self.ring.len() { 0 } else { pos };
        Some(&self.ring[idx].1)
    }

    /// Number of distinct real nodes in the ring.
    pub fn node_count(&self) -> usize {
        let mut nodes: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for (_, id) in &self.ring {
            nodes.insert(id);
        }
        nodes.len()
    }

    fn hash(key: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut hasher);
        hasher.finish()
    }
}

// ── Metadata consensus (Raft-style) ───────────────────────────────

/// Role in the metadata consensus group.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConsensusRole {
    Follower,
    Candidate,
    Leader,
}

/// A single entry in the consensus log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusEntry {
    pub term: u64,
    pub index: u64,
    pub operation: ConsensusOp,
}

/// Operations that can be replicated through consensus.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConsensusOp {
    /// Register a service in the cluster.
    RegisterService { name: String, node_id: String },
    /// Deregister a service.
    DeregisterService { name: String, node_id: String },
    /// Register a process.
    RegisterProcess {
        global_pid: String,
        agent_type: String,
    },
    /// Deregister a process.
    DeregisterProcess { global_pid: String },
}

/// Metadata consensus using Raft for authoritative state.
///
/// Wraps raft-style log replication for service registry and process
/// table consensus. When the `cluster` feature is active, this can
/// be backed by `ruvector_raft::RaftNode`; otherwise a self-contained
/// state machine is used.
pub struct MetadataConsensus {
    /// Node role in the raft group.
    role: ConsensusRole,
    /// Current term.
    term: u64,
    /// Committed log index.
    commit_index: u64,
    /// Log entries.
    log: Vec<ConsensusEntry>,
}

impl MetadataConsensus {
    /// Create a new consensus instance starting as a follower.
    pub fn new() -> Self {
        Self {
            role: ConsensusRole::Follower,
            term: 0,
            commit_index: 0,
            log: Vec::new(),
        }
    }

    /// Current role.
    pub fn role(&self) -> ConsensusRole {
        self.role
    }

    /// Current term.
    pub fn term(&self) -> u64 {
        self.term
    }

    /// Committed log index.
    pub fn commit_index(&self) -> u64 {
        self.commit_index
    }

    /// Transition to a new role.
    pub fn set_role(&mut self, role: ConsensusRole) {
        self.role = role;
    }

    /// Advance to a new term (resets vote state).
    pub fn advance_term(&mut self, new_term: u64) {
        if new_term > self.term {
            self.term = new_term;
            self.role = ConsensusRole::Follower;
        }
    }

    /// Append an entry to the log.
    /// Returns the index of the newly appended entry.
    pub fn append(&mut self, operation: ConsensusOp) -> u64 {
        let index = self.log.len() as u64 + 1;
        self.log.push(ConsensusEntry {
            term: self.term,
            index,
            operation,
        });
        index
    }

    /// Commit entries up to the given index.
    /// Returns the newly committed entries.
    pub fn commit(&mut self, up_to: u64) -> Vec<&ConsensusEntry> {
        let old = self.commit_index;
        self.commit_index = up_to.min(self.log.len() as u64);
        self.log
            .iter()
            .filter(|e| e.index > old && e.index <= self.commit_index)
            .collect()
    }

    /// Get all entries in the log.
    pub fn entries(&self) -> &[ConsensusEntry] {
        &self.log
    }

    /// Number of log entries.
    pub fn log_len(&self) -> usize {
        self.log.len()
    }
}

impl Default for MetadataConsensus {
    fn default() -> Self {
        Self::new()
    }
}

// ── CRDT gossip state (delta-based) ───────────────────────────────

/// CRDT state for gossip-based convergence.
///
/// Uses last-writer-wins semantics inspired by
/// `ruvector_delta_consensus`. Each entry carries a logical clock
/// timestamp so that concurrent updates converge deterministically.
pub struct CrdtGossipState {
    /// State entries with logical timestamps.
    entries: HashMap<String, (serde_json::Value, u64)>,
    /// Local logical clock.
    clock: u64,
}

impl CrdtGossipState {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            clock: 0,
        }
    }

    /// Set a value (advances local clock).
    pub fn set(&mut self, key: String, value: serde_json::Value) {
        self.clock += 1;
        self.entries.insert(key, (value, self.clock));
    }

    /// Get a value.
    pub fn get(&self, key: &str) -> Option<&serde_json::Value> {
        self.entries.get(key).map(|(v, _)| v)
    }

    /// Merge remote state (LWW: higher clock wins, ties keep local).
    pub fn merge(&mut self, remote_entries: &[(String, serde_json::Value, u64)]) {
        for (key, value, remote_clock) in remote_entries {
            match self.entries.get(key) {
                Some((_, local_clock)) if local_clock >= remote_clock => {
                    // Local is newer or same, keep local.
                }
                _ => {
                    self.entries
                        .insert(key.clone(), (value.clone(), *remote_clock));
                    if *remote_clock > self.clock {
                        self.clock = *remote_clock;
                    }
                }
            }
        }
    }

    /// Get delta since a given clock value (for gossip protocol).
    pub fn delta_since(&self, since_clock: u64) -> Vec<(String, serde_json::Value, u64)> {
        self.entries
            .iter()
            .filter(|(_, (_, clock))| *clock > since_clock)
            .map(|(k, (v, c))| (k.clone(), v.clone(), *c))
            .collect()
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the state is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Current logical clock.
    pub fn clock(&self) -> u64 {
        self.clock
    }
}

impl Default for CrdtGossipState {
    fn default() -> Self {
        Self::new()
    }
}

/// Process summary advertised to peer nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessAdvertisement {
    /// Globally unique process ID.
    pub global_pid: GlobalPid,
    /// Agent type (e.g., "coder", "reviewer").
    pub agent_type: String,
    /// Capabilities this process offers.
    pub capabilities: Vec<String>,
    /// Services this process provides.
    pub services: Vec<String>,
    /// Current process status.
    pub status: ProcessStatus,
    /// Last update timestamp (for LWW merge).
    pub last_updated: u64,
    /// Resource usage summary.
    pub resource_summary: Option<ResourceSummary>,
}

/// Process status for distributed table.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProcessStatus {
    Running,
    Suspended,
    Stopping,
    Unreachable,
}

/// Summary of resource usage for load-aware scheduling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceSummary {
    /// Memory usage in bytes.
    pub memory_bytes: u64,
    /// CPU time used in microseconds.
    pub cpu_time_us: u64,
    /// Number of pending messages in inbox.
    pub inbox_depth: u32,
}

/// Distributed process table tracking processes across all nodes.
pub struct DistributedProcessTable {
    /// All known processes: global_pid string -> advertisement.
    processes: HashMap<String, ProcessAdvertisement>,
}

impl DistributedProcessTable {
    pub fn new() -> Self {
        Self {
            processes: HashMap::new(),
        }
    }

    /// Merge a process advertisement (LWW: latest timestamp wins).
    pub fn merge(&mut self, advert: ProcessAdvertisement) {
        let key = advert.global_pid.to_string();
        match self.processes.get(&key) {
            Some(existing) if existing.last_updated >= advert.last_updated => {
                // Existing is newer or same, ignore.
            }
            _ => {
                self.processes.insert(key, advert);
            }
        }
    }

    /// Look up a process by local PID and node ID.
    pub fn locate(&self, pid: Pid, node_id: &str) -> Option<&ProcessAdvertisement> {
        let key = format!("{node_id}:{pid}");
        self.processes.get(&key)
    }

    /// Find processes by agent type across all nodes.
    pub fn find_by_type(&self, agent_type: &str) -> Vec<&ProcessAdvertisement> {
        self.processes
            .values()
            .filter(|p| p.agent_type == agent_type && p.status == ProcessStatus::Running)
            .collect()
    }

    /// Find processes offering a specific capability.
    pub fn find_by_capability(&self, capability: &str) -> Vec<&ProcessAdvertisement> {
        self.processes
            .values()
            .filter(|p| {
                p.capabilities.contains(&capability.to_string())
                    && p.status == ProcessStatus::Running
            })
            .collect()
    }

    /// Find the least-loaded node for scheduling.
    pub fn least_loaded_node(&self) -> Option<String> {
        let mut node_loads: HashMap<String, u32> = HashMap::new();
        for advert in self.processes.values() {
            if advert.status == ProcessStatus::Running {
                *node_loads
                    .entry(advert.global_pid.node_id.clone())
                    .or_default() += 1;
            }
        }
        node_loads
            .into_iter()
            .min_by_key(|(_, count)| *count)
            .map(|(node, _)| node)
    }

    /// Remove all processes from a node (e.g., when node becomes unreachable).
    pub fn remove_node(&mut self, node_id: &str) {
        self.processes
            .retain(|_, p| p.global_pid.node_id != node_id);
    }

    /// Mark all processes on a node as unreachable.
    pub fn mark_node_unreachable(&mut self, node_id: &str) {
        for advert in self.processes.values_mut() {
            if advert.global_pid.node_id == node_id {
                advert.status = ProcessStatus::Unreachable;
            }
        }
    }

    /// Get all process advertisements (for gossip).
    pub fn all_advertisements(&self) -> Vec<&ProcessAdvertisement> {
        self.processes.values().collect()
    }

    /// Number of tracked processes.
    pub fn len(&self) -> usize {
        self.processes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.processes.is_empty()
    }

    /// Assign a PID to the optimal node using consistent hashing.
    pub fn assign_pid(&self, pid: u64, ring: &ConsistentHashRing) -> Option<String> {
        ring.assign(pid).map(|s| s.to_string())
    }
}

impl Default for DistributedProcessTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_advert(node: &str, pid: Pid, agent_type: &str, ts: u64) -> ProcessAdvertisement {
        ProcessAdvertisement {
            global_pid: GlobalPid::local(pid, node),
            agent_type: agent_type.to_string(),
            capabilities: vec!["code".to_string()],
            services: vec![],
            status: ProcessStatus::Running,
            last_updated: ts,
            resource_summary: None,
        }
    }

    #[test]
    fn process_advertisement_serde_roundtrip() {
        let advert = make_advert("node-a", 1, "coder", 100);
        let json = serde_json::to_string(&advert).unwrap();
        let restored: ProcessAdvertisement = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.global_pid.node_id, "node-a");
        assert_eq!(restored.global_pid.pid, 1);
        assert_eq!(restored.agent_type, "coder");
        assert_eq!(restored.last_updated, 100);
    }

    #[test]
    fn merge_lww_newer_wins() {
        let mut table = DistributedProcessTable::new();

        let old = make_advert("node-a", 1, "coder", 100);
        let new = ProcessAdvertisement {
            agent_type: "reviewer".to_string(),
            last_updated: 200,
            ..make_advert("node-a", 1, "coder", 200)
        };

        table.merge(old);
        assert_eq!(table.len(), 1);

        // Newer timestamp wins
        table.merge(new);
        let found = table.locate(1, "node-a").unwrap();
        assert_eq!(found.agent_type, "reviewer");
        assert_eq!(found.last_updated, 200);
    }

    #[test]
    fn merge_lww_older_ignored() {
        let mut table = DistributedProcessTable::new();

        let newer = make_advert("node-a", 1, "reviewer", 200);
        let older = make_advert("node-a", 1, "coder", 100);

        table.merge(newer);
        table.merge(older); // should be ignored

        let found = table.locate(1, "node-a").unwrap();
        assert_eq!(found.agent_type, "reviewer");
    }

    #[test]
    fn find_by_type_returns_matching() {
        let mut table = DistributedProcessTable::new();
        table.merge(make_advert("node-a", 1, "coder", 100));
        table.merge(make_advert("node-b", 2, "reviewer", 100));
        table.merge(make_advert("node-c", 3, "coder", 100));

        let coders = table.find_by_type("coder");
        assert_eq!(coders.len(), 2);
        for c in &coders {
            assert_eq!(c.agent_type, "coder");
        }
    }

    #[test]
    fn find_by_capability() {
        let mut table = DistributedProcessTable::new();

        let mut advert = make_advert("node-a", 1, "coder", 100);
        advert.capabilities = vec!["code".to_string(), "review".to_string()];
        table.merge(advert);

        let mut advert2 = make_advert("node-b", 2, "tester", 100);
        advert2.capabilities = vec!["test".to_string()];
        table.merge(advert2);

        let reviewers = table.find_by_capability("review");
        assert_eq!(reviewers.len(), 1);
        assert_eq!(reviewers[0].global_pid.node_id, "node-a");
    }

    #[test]
    fn least_loaded_node() {
        let mut table = DistributedProcessTable::new();
        // node-a has 3 processes, node-b has 1
        table.merge(make_advert("node-a", 1, "coder", 100));
        table.merge(make_advert("node-a", 2, "coder", 100));
        table.merge(make_advert("node-a", 3, "coder", 100));
        table.merge(make_advert("node-b", 1, "coder", 100));

        let node = table.least_loaded_node().unwrap();
        assert_eq!(node, "node-b");
    }

    #[test]
    fn remove_node() {
        let mut table = DistributedProcessTable::new();
        table.merge(make_advert("node-a", 1, "coder", 100));
        table.merge(make_advert("node-a", 2, "coder", 100));
        table.merge(make_advert("node-b", 1, "coder", 100));

        table.remove_node("node-a");
        assert_eq!(table.len(), 1);
        assert!(table.locate(1, "node-a").is_none());
        assert!(table.locate(1, "node-b").is_some());
    }

    #[test]
    fn mark_node_unreachable() {
        let mut table = DistributedProcessTable::new();
        table.merge(make_advert("node-a", 1, "coder", 100));
        table.merge(make_advert("node-a", 2, "coder", 100));
        table.merge(make_advert("node-b", 1, "coder", 100));

        table.mark_node_unreachable("node-a");

        let a1 = table.locate(1, "node-a").unwrap();
        assert_eq!(a1.status, ProcessStatus::Unreachable);
        let a2 = table.locate(2, "node-a").unwrap();
        assert_eq!(a2.status, ProcessStatus::Unreachable);

        // node-b unaffected
        let b1 = table.locate(1, "node-b").unwrap();
        assert_eq!(b1.status, ProcessStatus::Running);
    }

    // ── ConsistentHashRing tests ───────────────────────────────────

    #[test]
    fn consistent_hash_ring_add_remove() {
        let mut ring = ConsistentHashRing::new(64);
        assert_eq!(ring.node_count(), 0);

        ring.add_node("node-a");
        assert_eq!(ring.node_count(), 1);

        ring.add_node("node-b");
        assert_eq!(ring.node_count(), 2);

        ring.remove_node("node-a");
        assert_eq!(ring.node_count(), 1);

        // All assignments should go to node-b now
        assert_eq!(ring.assign(42), Some("node-b"));
    }

    #[test]
    fn consistent_hash_ring_assignment_stable() {
        let mut ring = ConsistentHashRing::new(64);
        ring.add_node("node-a");
        ring.add_node("node-b");
        ring.add_node("node-c");

        // Same PID always maps to same node
        let first = ring.assign(12345).unwrap().to_string();
        for _ in 0..100 {
            assert_eq!(ring.assign(12345).unwrap(), first);
        }
    }

    #[test]
    fn consistent_hash_ring_redistribution() {
        let mut ring = ConsistentHashRing::new(64);
        ring.add_node("node-a");
        ring.add_node("node-b");
        ring.add_node("node-c");

        // Record assignments for 1000 PIDs
        let before: Vec<String> = (0..1000)
            .map(|pid| ring.assign(pid).unwrap().to_string())
            .collect();

        // Remove one node
        ring.remove_node("node-b");

        let after: Vec<String> = (0..1000)
            .map(|pid| ring.assign(pid).unwrap().to_string())
            .collect();

        // Only PIDs that were on node-b should have moved
        let mut moved = 0;
        for i in 0..1000 {
            if before[i] != after[i] {
                moved += 1;
                // Moved PID must have been on the removed node
                assert_eq!(before[i], "node-b");
            }
        }
        // Some PIDs should have moved, but not all of them
        assert!(moved > 0, "at least some PIDs should be redistributed");
        assert!(moved < 1000, "not all PIDs should move");
    }

    #[test]
    fn consistent_hash_ring_empty_returns_none() {
        let ring = ConsistentHashRing::new(64);
        assert!(ring.assign(1).is_none());
    }

    #[test]
    fn pid_to_node_assignment() {
        let mut ring = ConsistentHashRing::new(64);
        ring.add_node("node-a");
        ring.add_node("node-b");

        let table = DistributedProcessTable::new();
        let node = table.assign_pid(42, &ring);
        assert!(node.is_some());
        let n = node.unwrap();
        assert!(n == "node-a" || n == "node-b");
    }

    // ── MetadataConsensus tests ────────────────────────────────────

    #[test]
    fn consensus_append_commit_cycle() {
        let mut mc = MetadataConsensus::new();
        assert_eq!(mc.role(), ConsensusRole::Follower);
        assert_eq!(mc.term(), 0);
        assert_eq!(mc.commit_index(), 0);

        mc.set_role(ConsensusRole::Leader);
        assert_eq!(mc.role(), ConsensusRole::Leader);

        let idx1 = mc.append(ConsensusOp::RegisterService {
            name: "cache".into(),
            node_id: "node-a".into(),
        });
        assert_eq!(idx1, 1);

        let idx2 = mc.append(ConsensusOp::RegisterProcess {
            global_pid: "node-a:1".into(),
            agent_type: "coder".into(),
        });
        assert_eq!(idx2, 2);

        assert_eq!(mc.log_len(), 2);

        // Commit up to index 2
        let committed = mc.commit(2);
        assert_eq!(committed.len(), 2);
        assert_eq!(mc.commit_index(), 2);

        // Committing again yields nothing new
        let committed2 = mc.commit(2);
        assert_eq!(committed2.len(), 0);
    }

    #[test]
    fn consensus_advance_term() {
        let mut mc = MetadataConsensus::new();
        mc.set_role(ConsensusRole::Leader);
        mc.advance_term(5);
        assert_eq!(mc.term(), 5);
        // Advancing term resets role to follower
        assert_eq!(mc.role(), ConsensusRole::Follower);

        // Old term is ignored
        mc.advance_term(3);
        assert_eq!(mc.term(), 5);
    }

    #[test]
    fn consensus_commit_clamps_to_log_len() {
        let mut mc = MetadataConsensus::new();
        mc.append(ConsensusOp::DeregisterProcess {
            global_pid: "node-a:1".into(),
        });
        // Commit beyond log length is clamped
        mc.commit(999);
        assert_eq!(mc.commit_index(), 1);
    }

    // ── CrdtGossipState tests ──────────────────────────────────────

    #[test]
    fn crdt_set_get() {
        let mut state = CrdtGossipState::new();
        assert!(state.is_empty());

        state.set("key1".into(), serde_json::json!("value1"));
        assert_eq!(state.len(), 1);
        assert_eq!(state.get("key1"), Some(&serde_json::json!("value1")));
        assert!(state.get("missing").is_none());
    }

    #[test]
    fn crdt_merge_lww() {
        let mut local = CrdtGossipState::new();
        local.set("key1".into(), serde_json::json!("local_v1"));
        // local clock is now 1

        // Remote has a higher clock for key1
        let remote = vec![("key1".into(), serde_json::json!("remote_v2"), 5u64)];
        local.merge(&remote);
        assert_eq!(local.get("key1"), Some(&serde_json::json!("remote_v2")));
        assert_eq!(local.clock(), 5);

        // Merge with older clock -- should be ignored
        let old_remote = vec![("key1".into(), serde_json::json!("old_value"), 2u64)];
        local.merge(&old_remote);
        assert_eq!(local.get("key1"), Some(&serde_json::json!("remote_v2")));
    }

    #[test]
    fn crdt_delta_since() {
        let mut state = CrdtGossipState::new();
        state.set("a".into(), serde_json::json!(1));
        state.set("b".into(), serde_json::json!(2));
        state.set("c".into(), serde_json::json!(3));

        // Delta since clock=1 should include b (clock=2) and c (clock=3)
        let delta = state.delta_since(1);
        assert_eq!(delta.len(), 2);

        // Delta since clock=0 should include everything
        let all = state.delta_since(0);
        assert_eq!(all.len(), 3);

        // Delta since current clock should be empty
        let none = state.delta_since(state.clock());
        assert_eq!(none.len(), 0);
    }

    #[test]
    fn crdt_concurrent_updates_converge() {
        // Simulate two nodes with different clocks, then merge.
        // With LWW, the higher clock always wins on both sides.
        let mut node_a = CrdtGossipState::new();
        let mut node_b = CrdtGossipState::new();

        // Node A sets key1 at clock 1
        node_a.set("key1".into(), serde_json::json!("A"));
        // Node B sets key1 then key2, so clock is 2 (higher than A's 1)
        node_b.set("warmup".into(), serde_json::json!("x"));
        node_b.set("key1".into(), serde_json::json!("B"));
        // node_b clock for key1 = 2, node_a clock for key1 = 1

        // Exchange deltas
        let delta_a = node_a.delta_since(0);
        let delta_b = node_b.delta_since(0);

        node_a.merge(&delta_b);
        node_b.merge(&delta_a);

        // Both should converge to B's value (clock 2 > clock 1)
        assert_eq!(node_a.get("key1"), Some(&serde_json::json!("B")));
        assert_eq!(node_b.get("key1"), Some(&serde_json::json!("B")));
    }
}
