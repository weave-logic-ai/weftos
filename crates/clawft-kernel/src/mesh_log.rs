//! Cross-node log aggregation for mesh networking (K6-G2).
//!
//! [`LogAggregator`] collects log entries from mesh peers and provides
//! a unified query interface that merges logs by timestamp.

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// RemoteLogEntry
// ---------------------------------------------------------------------------

/// A log entry that includes the source node identifier.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteLogEntry {
    /// Source node identifier (local or remote).
    pub node_id: String,
    /// Log level.
    pub level: String,
    /// Log message.
    pub message: String,
    /// Originating module or service.
    pub source: String,
    /// When the log entry was created.
    pub timestamp: DateTime<Utc>,
}

impl RemoteLogEntry {
    /// Create a new log entry.
    pub fn new(
        node_id: &str,
        level: &str,
        message: &str,
        source: &str,
        timestamp: DateTime<Utc>,
    ) -> Self {
        Self {
            node_id: node_id.to_string(),
            level: level.to_string(),
            message: message.to_string(),
            source: source.to_string(),
            timestamp,
        }
    }
}

// ---------------------------------------------------------------------------
// LogQuery
// ---------------------------------------------------------------------------

/// Query parameters for filtering aggregated logs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LogQuery {
    /// Filter by source node (None = all nodes).
    pub node_id: Option<String>,
    /// Filter by log level.
    pub level: Option<String>,
    /// Filter by source module/service.
    pub source: Option<String>,
    /// Only entries after this time.
    pub since: Option<DateTime<Utc>>,
    /// Only entries before this time.
    pub until: Option<DateTime<Utc>>,
    /// Maximum number of entries to return.
    pub limit: Option<usize>,
}

// ---------------------------------------------------------------------------
// LogAggregator
// ---------------------------------------------------------------------------

/// Aggregates log entries from local and remote mesh nodes.
///
/// Stores entries per node and provides a merged, time-ordered query
/// interface.
pub struct LogAggregator {
    /// Local node identifier.
    local_node_id: String,
    /// Entries per node: node_id -> entries (append-only).
    entries: DashMap<String, Vec<RemoteLogEntry>>,
}

impl LogAggregator {
    /// Create a new log aggregator for the given local node.
    pub fn new(local_node_id: String) -> Self {
        Self {
            local_node_id,
            entries: DashMap::new(),
        }
    }

    /// Add a local log entry.
    pub fn log_local(&self, level: &str, message: &str, source: &str) {
        let entry = RemoteLogEntry::new(&self.local_node_id, level, message, source, Utc::now());
        self.entries
            .entry(self.local_node_id.clone())
            .or_default()
            .push(entry);
    }

    /// Add a remote log entry received from a mesh peer.
    pub fn add_remote(&self, entry: RemoteLogEntry) {
        self.entries
            .entry(entry.node_id.clone())
            .or_default()
            .push(entry);
    }

    /// Add a batch of remote log entries.
    pub fn add_remote_batch(&self, entries: Vec<RemoteLogEntry>) {
        for entry in entries {
            self.add_remote(entry);
        }
    }

    /// Query logs with optional filters, merged by timestamp.
    pub fn query(&self, query: &LogQuery) -> Vec<RemoteLogEntry> {
        let mut results: Vec<RemoteLogEntry> = Vec::new();

        for entry_ref in self.entries.iter() {
            let node_id = entry_ref.key();
            let entries = entry_ref.value();

            // Filter by node_id.
            if query.node_id.as_ref().is_some_and(|n| node_id != n) {
                continue;
            }

            for e in entries {
                // Filter by level.
                if query.level.as_ref().is_some_and(|l| e.level != *l) {
                    continue;
                }
                // Filter by source.
                if query.source.as_ref().is_some_and(|s| e.source != *s) {
                    continue;
                }
                // Filter by time range.
                if query.since.is_some_and(|since| e.timestamp < since) {
                    continue;
                }
                if query.until.is_some_and(|until| e.timestamp > until) {
                    continue;
                }

                results.push(e.clone());
            }
        }

        // Sort by timestamp (merge from multiple nodes).
        results.sort_by_key(|e| e.timestamp);

        // Apply limit.
        if let Some(limit) = query.limit {
            results.truncate(limit);
        }

        results
    }

    /// Get all entries for a specific node.
    pub fn entries_for_node(&self, node_id: &str) -> Vec<RemoteLogEntry> {
        self.entries
            .get(node_id)
            .map(|v| v.value().clone())
            .unwrap_or_default()
    }

    /// Total number of log entries across all nodes.
    pub fn total_entries(&self) -> usize {
        self.entries.iter().map(|e| e.value().len()).sum()
    }

    /// List all known node IDs.
    pub fn known_nodes(&self) -> Vec<String> {
        self.entries.iter().map(|e| e.key().clone()).collect()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn make_entry(node: &str, level: &str, msg: &str, offset_secs: i64) -> RemoteLogEntry {
        RemoteLogEntry::new(
            node,
            level,
            msg,
            "test-module",
            Utc::now() + Duration::seconds(offset_secs),
        )
    }

    #[test]
    fn log_local_entry() {
        let agg = LogAggregator::new("node-1".into());
        agg.log_local("info", "hello", "boot");
        assert_eq!(agg.total_entries(), 1);
        let entries = agg.entries_for_node("node-1");
        assert_eq!(entries[0].message, "hello");
        assert_eq!(entries[0].node_id, "node-1");
    }

    #[test]
    fn add_remote_entry() {
        let agg = LogAggregator::new("node-1".into());
        let entry = make_entry("node-2", "warn", "remote warning", 0);
        agg.add_remote(entry);
        assert_eq!(agg.total_entries(), 1);
        let entries = agg.entries_for_node("node-2");
        assert_eq!(entries[0].node_id, "node-2");
    }

    #[test]
    fn query_all_merged_by_timestamp() {
        let agg = LogAggregator::new("node-1".into());
        agg.add_remote(make_entry("node-2", "info", "second", 2));
        agg.add_remote(make_entry("node-3", "info", "first", 1));
        agg.add_remote(make_entry("node-1", "info", "third", 3));

        let results = agg.query(&LogQuery::default());
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].message, "first");
        assert_eq!(results[1].message, "second");
        assert_eq!(results[2].message, "third");
    }

    #[test]
    fn query_filter_by_node() {
        let agg = LogAggregator::new("node-1".into());
        agg.add_remote(make_entry("node-1", "info", "local", 0));
        agg.add_remote(make_entry("node-2", "info", "remote", 0));

        let results = agg.query(&LogQuery {
            node_id: Some("node-2".into()),
            ..Default::default()
        });
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].node_id, "node-2");
    }

    #[test]
    fn query_filter_by_level() {
        let agg = LogAggregator::new("node-1".into());
        agg.add_remote(make_entry("node-1", "info", "info msg", 0));
        agg.add_remote(make_entry("node-1", "error", "error msg", 0));

        let results = agg.query(&LogQuery {
            level: Some("error".into()),
            ..Default::default()
        });
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].level, "error");
    }

    #[test]
    fn query_with_limit() {
        let agg = LogAggregator::new("node-1".into());
        for i in 0..10 {
            agg.add_remote(make_entry("node-1", "info", &format!("msg-{i}"), i));
        }

        let results = agg.query(&LogQuery {
            limit: Some(3),
            ..Default::default()
        });
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn query_filter_by_source() {
        let agg = LogAggregator::new("n1".into());
        agg.add_remote(RemoteLogEntry::new("n1", "info", "a", "boot", Utc::now()));
        agg.add_remote(RemoteLogEntry::new("n1", "info", "b", "mesh", Utc::now()));

        let results = agg.query(&LogQuery {
            source: Some("mesh".into()),
            ..Default::default()
        });
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source, "mesh");
    }

    #[test]
    fn add_remote_batch() {
        let agg = LogAggregator::new("n1".into());
        let batch = vec![
            make_entry("n2", "info", "a", 0),
            make_entry("n3", "info", "b", 0),
        ];
        agg.add_remote_batch(batch);
        assert_eq!(agg.total_entries(), 2);
    }

    #[test]
    fn known_nodes() {
        let agg = LogAggregator::new("n1".into());
        agg.log_local("info", "x", "y");
        agg.add_remote(make_entry("n2", "info", "z", 0));
        let mut nodes = agg.known_nodes();
        nodes.sort();
        assert_eq!(nodes, vec!["n1", "n2"]);
    }

    #[test]
    fn empty_aggregator() {
        let agg = LogAggregator::new("n1".into());
        assert_eq!(agg.total_entries(), 0);
        assert!(agg.query(&LogQuery::default()).is_empty());
        assert!(agg.known_nodes().is_empty());
    }

    #[test]
    fn remote_log_entry_serialization() {
        let entry = RemoteLogEntry::new("n1", "info", "msg", "src", Utc::now());
        let json = serde_json::to_string(&entry).unwrap();
        let deser: RemoteLogEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.node_id, "n1");
        assert_eq!(deser.message, "msg");
    }
}
