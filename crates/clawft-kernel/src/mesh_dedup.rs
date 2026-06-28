//! Message deduplication for mesh IPC (K6.3).
//!
//! Prevents duplicate delivery of messages that may arrive via
//! multiple paths in the mesh network. Uses a time-bounded set
//! of recently seen message/envelope IDs.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Time-bounded deduplication filter.
///
/// Tracks recently seen IDs and automatically expires entries
/// older than the configured TTL.
pub struct DedupFilter {
    /// Seen IDs with their insertion time.
    seen: HashMap<String, Instant>,
    /// How long to remember IDs.
    ttl: Duration,
    /// Maximum number of entries before forced eviction.
    max_entries: usize,
}

impl DedupFilter {
    /// Create a new dedup filter with the given TTL and max capacity.
    pub fn new(ttl: Duration, max_entries: usize) -> Self {
        Self {
            seen: HashMap::new(),
            ttl,
            max_entries,
        }
    }

    /// Create a dedup filter with default settings (60s TTL, 10000 entries).
    pub fn default_mesh() -> Self {
        Self::new(Duration::from_secs(60), 10_000)
    }

    /// Check if an ID has been seen before. If not, mark it as seen.
    /// Returns `true` if this is a **new** (not duplicate) message.
    pub fn check_and_insert(&mut self, id: &str) -> bool {
        self.evict_expired();

        if self.seen.contains_key(id) {
            return false; // duplicate
        }

        // Force eviction if at capacity
        if self.seen.len() >= self.max_entries {
            self.evict_oldest();
        }

        self.seen.insert(id.to_string(), Instant::now());
        true // new message
    }

    /// Check if an ID has been seen (without inserting).
    pub fn is_duplicate(&self, id: &str) -> bool {
        if let Some(inserted) = self.seen.get(id) {
            inserted.elapsed() < self.ttl
        } else {
            false
        }
    }

    /// Number of tracked IDs.
    pub fn len(&self) -> usize {
        self.seen.len()
    }

    /// Whether the filter is empty.
    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }

    /// Remove expired entries.
    fn evict_expired(&mut self) {
        self.seen
            .retain(|_, inserted| inserted.elapsed() < self.ttl);
    }

    /// Remove the oldest entry to make room for new ones.
    fn evict_oldest(&mut self) {
        if let Some(oldest_key) = self
            .seen
            .iter()
            .min_by_key(|(_, t)| **t)
            .map(|(k, _)| k.clone())
        {
            self.seen.remove(&oldest_key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_message_returns_true() {
        let mut f = DedupFilter::new(Duration::from_secs(10), 100);
        assert!(f.check_and_insert("msg-1"));
    }

    #[test]
    fn duplicate_returns_false() {
        let mut f = DedupFilter::new(Duration::from_secs(10), 100);
        assert!(f.check_and_insert("msg-1"));
        assert!(!f.check_and_insert("msg-1"));
    }

    #[test]
    fn different_ids_both_new() {
        let mut f = DedupFilter::new(Duration::from_secs(10), 100);
        assert!(f.check_and_insert("a"));
        assert!(f.check_and_insert("b"));
        assert_eq!(f.len(), 2);
    }

    #[test]
    fn is_duplicate_without_inserting() {
        let mut f = DedupFilter::new(Duration::from_secs(10), 100);
        assert!(!f.is_duplicate("x"));
        f.check_and_insert("x");
        assert!(f.is_duplicate("x"));
    }

    #[test]
    fn expired_entries_evicted() {
        // Use a zero TTL so entries expire immediately.
        let mut f = DedupFilter::new(Duration::from_nanos(0), 100);
        f.seen
            .insert("old".to_string(), Instant::now() - Duration::from_secs(1));
        // check_and_insert evicts expired, so "old" should be gone
        assert!(f.check_and_insert("old"));
    }

    #[test]
    fn max_capacity_evicts_oldest() {
        let mut f = DedupFilter::new(Duration::from_secs(60), 2);
        f.check_and_insert("a");
        // Backdate "a" so it is oldest
        *f.seen.get_mut("a").unwrap() = Instant::now() - Duration::from_secs(30);
        f.check_and_insert("b");
        // At capacity (2), inserting "c" should evict "a"
        f.check_and_insert("c");
        assert_eq!(f.len(), 2);
        assert!(!f.is_duplicate("a"));
        assert!(f.is_duplicate("b"));
        assert!(f.is_duplicate("c"));
    }

    #[test]
    fn empty_filter() {
        let f = DedupFilter::default_mesh();
        assert!(f.is_empty());
        assert_eq!(f.len(), 0);
    }

    #[test]
    fn default_mesh_settings() {
        let f = DedupFilter::default_mesh();
        assert_eq!(f.max_entries, 10_000);
        assert_eq!(f.ttl, Duration::from_secs(60));
    }
}
