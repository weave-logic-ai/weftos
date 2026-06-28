//! Ephemeral causal impulse queue for inter-structure communication (ECC Phase K3c).
//!
//! Impulses are short-lived events that flow between the four ECC structures
//! (causal graph, spectral index, HNSW, cloud/edge bridge). The [`ImpulseQueue`]
//! provides a thread-safe, ordered buffer that producers [`emit`](ImpulseQueue::emit)
//! into and consumers [`drain_ready`](ImpulseQueue::drain_ready) from.
//!
//! Structure tags are represented as raw `u8` values to avoid cross-module
//! coupling. They correspond to `crossref::StructureTag::as_u8()`.

use std::fmt;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// ImpulseType
// ---------------------------------------------------------------------------

/// Discriminant for the kind of causal event being signalled.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImpulseType {
    /// causal -> hnsw (new embedding needed)
    BeliefUpdate,
    /// spectral -> causal (graph incoherent)
    CoherenceAlert,
    /// hnsw -> causal (new cluster found)
    NoveltyDetected,
    /// cloud -> edge (DEMOCRITUS validated edge)
    EdgeConfirmed,
    /// cloud -> edge (better embedding available)
    EmbeddingRefined,
    /// Extension point for user-defined impulse kinds.
    Custom(u8),
}

impl fmt::Display for ImpulseType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BeliefUpdate => write!(f, "BeliefUpdate"),
            Self::CoherenceAlert => write!(f, "CoherenceAlert"),
            Self::NoveltyDetected => write!(f, "NoveltyDetected"),
            Self::EdgeConfirmed => write!(f, "EdgeConfirmed"),
            Self::EmbeddingRefined => write!(f, "EmbeddingRefined"),
            Self::Custom(code) => write!(f, "Custom({code})"),
        }
    }
}

// ---------------------------------------------------------------------------
// Impulse
// ---------------------------------------------------------------------------

/// A single causal event travelling between ECC structures.
///
/// `source_structure` and `target_structure` are `u8` tags that correspond to
/// `crossref::StructureTag::as_u8()` values:
///   0 = CausalGraph, 1 = SpectralIndex, 2 = Hnsw, 3 = CloudBridge.
pub struct Impulse {
    /// Monotonically increasing identifier assigned by the queue.
    pub id: u64,
    /// Originating structure (see `StructureTag::as_u8()`).
    pub source_structure: u8,
    /// 32-byte universal node identifier from the source structure.
    pub source_node: [u8; 32],
    /// Destination structure (see `StructureTag::as_u8()`).
    pub target_structure: u8,
    /// The kind of impulse.
    pub impulse_type: ImpulseType,
    /// Arbitrary JSON payload carried by this impulse.
    pub payload: serde_json::Value,
    /// Hybrid-logical-clock timestamp for causal ordering.
    pub hlc_timestamp: u64,
    /// Set to `true` once the consumer has processed this impulse.
    pub acknowledged: AtomicBool,
}

// AtomicBool is not Clone, so we implement Clone manually.
impl Clone for Impulse {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            source_structure: self.source_structure,
            source_node: self.source_node,
            target_structure: self.target_structure,
            impulse_type: self.impulse_type.clone(),
            payload: self.payload.clone(),
            hlc_timestamp: self.hlc_timestamp,
            acknowledged: AtomicBool::new(self.acknowledged.load(Ordering::Acquire)),
        }
    }
}

impl fmt::Debug for Impulse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Impulse")
            .field("id", &self.id)
            .field("source_structure", &self.source_structure)
            .field("target_structure", &self.target_structure)
            .field("impulse_type", &self.impulse_type)
            .field("hlc_timestamp", &self.hlc_timestamp)
            .field("acknowledged", &self.acknowledged.load(Ordering::Relaxed))
            .finish()
    }
}

// ---------------------------------------------------------------------------
// ImpulseQueue
// ---------------------------------------------------------------------------

/// Thread-safe queue of [`Impulse`] events awaiting consumption.
pub struct ImpulseQueue {
    queue: Mutex<Vec<Impulse>>,
    next_id: AtomicU64,
}

impl ImpulseQueue {
    /// Create a new, empty impulse queue.
    pub fn new() -> Self {
        Self {
            queue: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(1),
        }
    }

    /// Enqueue a new impulse and return its assigned id.
    pub fn emit(
        &self,
        source_structure: u8,
        source_node: [u8; 32],
        target_structure: u8,
        impulse_type: ImpulseType,
        payload: serde_json::Value,
        hlc_timestamp: u64,
    ) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let impulse = Impulse {
            id,
            source_structure,
            source_node,
            target_structure,
            impulse_type,
            payload,
            hlc_timestamp,
            acknowledged: AtomicBool::new(false),
        };
        let mut q = self.queue.lock().expect("impulse queue poisoned");
        q.push(impulse);
        id
    }

    /// Drain all unacknowledged impulses, returning them sorted by
    /// `hlc_timestamp` (ascending). Acknowledged impulses are discarded.
    pub fn drain_ready(&self) -> Vec<Impulse> {
        let mut q = self.queue.lock().expect("impulse queue poisoned");
        let drained: Vec<Impulse> = q
            .drain(..)
            .filter(|imp| !imp.acknowledged.load(Ordering::Acquire))
            .collect();
        let mut sorted = drained;
        sorted.sort_by_key(|imp| imp.hlc_timestamp);
        sorted
    }

    /// Total number of impulses in the queue (acknowledged or not).
    pub fn len(&self) -> usize {
        self.queue.lock().expect("impulse queue poisoned").len()
    }

    /// Returns `true` if the queue contains no impulses.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Remove all impulses from the queue (e.g. during calibration).
    pub fn clear(&self) {
        self.queue.lock().expect("impulse queue poisoned").clear();
    }

    /// Count of impulses that have not yet been acknowledged.
    pub fn pending_count(&self) -> usize {
        self.queue
            .lock()
            .expect("impulse queue poisoned")
            .iter()
            .filter(|imp| !imp.acknowledged.load(Ordering::Acquire))
            .count()
    }
}

impl Default for ImpulseQueue {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn impulse_type_display() {
        assert_eq!(ImpulseType::BeliefUpdate.to_string(), "BeliefUpdate");
        assert_eq!(ImpulseType::CoherenceAlert.to_string(), "CoherenceAlert");
        assert_eq!(ImpulseType::NoveltyDetected.to_string(), "NoveltyDetected");
        assert_eq!(ImpulseType::EdgeConfirmed.to_string(), "EdgeConfirmed");
        assert_eq!(
            ImpulseType::EmbeddingRefined.to_string(),
            "EmbeddingRefined"
        );
        assert_eq!(ImpulseType::Custom(42).to_string(), "Custom(42)");
    }

    #[test]
    fn impulse_queue_new_empty() {
        let q = ImpulseQueue::new();
        assert!(q.is_empty());
        assert_eq!(q.len(), 0);
        assert_eq!(q.pending_count(), 0);
    }

    #[test]
    fn impulse_queue_emit_assigns_id() {
        let q = ImpulseQueue::new();
        let node = [0u8; 32];
        let id1 = q.emit(
            0,
            node,
            2,
            ImpulseType::BeliefUpdate,
            serde_json::json!({}),
            100,
        );
        let id2 = q.emit(
            1,
            node,
            0,
            ImpulseType::CoherenceAlert,
            serde_json::json!({}),
            200,
        );
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(q.len(), 2);
    }

    #[test]
    fn impulse_queue_drain_sorted_by_hlc() {
        let q = ImpulseQueue::new();
        let node = [0u8; 32];
        // Emit in reverse timestamp order.
        q.emit(
            0,
            node,
            2,
            ImpulseType::BeliefUpdate,
            serde_json::json!({}),
            300,
        );
        q.emit(
            1,
            node,
            0,
            ImpulseType::CoherenceAlert,
            serde_json::json!({}),
            100,
        );
        q.emit(
            2,
            node,
            0,
            ImpulseType::NoveltyDetected,
            serde_json::json!({}),
            200,
        );

        let drained = q.drain_ready();
        assert_eq!(drained.len(), 3);
        assert_eq!(drained[0].hlc_timestamp, 100);
        assert_eq!(drained[1].hlc_timestamp, 200);
        assert_eq!(drained[2].hlc_timestamp, 300);
    }

    #[test]
    fn impulse_queue_drain_removes_items() {
        let q = ImpulseQueue::new();
        let node = [0u8; 32];
        q.emit(
            0,
            node,
            2,
            ImpulseType::BeliefUpdate,
            serde_json::json!({}),
            1,
        );
        q.emit(
            0,
            node,
            2,
            ImpulseType::EdgeConfirmed,
            serde_json::json!({}),
            2,
        );
        assert_eq!(q.len(), 2);

        let drained = q.drain_ready();
        assert_eq!(drained.len(), 2);
        assert!(q.is_empty());
    }

    #[test]
    fn impulse_queue_clear() {
        let q = ImpulseQueue::new();
        let node = [0u8; 32];
        q.emit(
            0,
            node,
            1,
            ImpulseType::EmbeddingRefined,
            serde_json::json!({}),
            10,
        );
        q.emit(
            0,
            node,
            1,
            ImpulseType::Custom(7),
            serde_json::json!({}),
            20,
        );
        assert_eq!(q.len(), 2);

        q.clear();
        assert!(q.is_empty());
        assert_eq!(q.len(), 0);
    }

    #[test]
    fn impulse_queue_pending_count() {
        let q = ImpulseQueue::new();
        let node = [0u8; 32];
        q.emit(
            0,
            node,
            2,
            ImpulseType::BeliefUpdate,
            serde_json::json!({}),
            1,
        );
        q.emit(
            1,
            node,
            0,
            ImpulseType::CoherenceAlert,
            serde_json::json!({}),
            2,
        );
        assert_eq!(q.pending_count(), 2);

        // Acknowledge one via the internal queue.
        {
            let guard = q.queue.lock().unwrap();
            guard[0].acknowledged.store(true, Ordering::Release);
        }
        assert_eq!(q.pending_count(), 1);
    }

    #[test]
    fn impulse_emit_and_acknowledge() {
        let q = ImpulseQueue::new();
        let node = [1u8; 32];
        q.emit(
            0,
            node,
            2,
            ImpulseType::NoveltyDetected,
            serde_json::json!({"k": "v"}),
            50,
        );
        q.emit(
            0,
            node,
            3,
            ImpulseType::EdgeConfirmed,
            serde_json::json!(null),
            60,
        );

        let drained = q.drain_ready();
        assert_eq!(drained.len(), 2);
        // Queue is now empty after drain.
        assert!(q.is_empty());

        // Mark drained impulses as acknowledged.
        for imp in &drained {
            imp.acknowledged.store(true, Ordering::Release);
        }

        // Verify acknowledgement persists on the drained copies.
        for imp in &drained {
            assert!(imp.acknowledged.load(Ordering::Acquire));
        }
    }
}
