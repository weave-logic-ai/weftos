//! Structured log service with ring buffer, query, and subscription support.
//!
//! [`LogService`] provides structured logging for all kernel subsystems.
//! Logs are stored in a bounded ring buffer and queryable by PID, service,
//! level, time range, and trace ID. Real-time subscribers receive entries
//! as they are ingested.

use std::collections::{HashMap, VecDeque};
use std::sync::RwLock;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::console::LogLevel;
use crate::process::Pid;

/// Default maximum number of log entries retained in the ring buffer.
pub const DEFAULT_LOG_CAPACITY: usize = 100_000;

/// Default subscriber channel capacity.
const SUBSCRIBER_CHANNEL_CAPACITY: usize = 1024;

/// A structured log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    /// When the log entry was created.
    pub timestamp: DateTime<Utc>,
    /// Log severity level.
    pub level: LogLevel,
    /// PID of the source agent (if applicable).
    pub source_pid: Option<Pid>,
    /// Name of the source service (if applicable).
    pub source_service: Option<String>,
    /// Human-readable log message.
    pub message: String,
    /// Structured fields (key-value metadata).
    pub fields: HashMap<String, serde_json::Value>,
    /// Distributed trace ID for correlated log queries.
    pub trace_id: Option<String>,
}

impl LogEntry {
    /// Create a new log entry with minimal fields.
    pub fn new(level: LogLevel, message: impl Into<String>) -> Self {
        Self {
            timestamp: Utc::now(),
            level,
            source_pid: None,
            source_service: None,
            message: message.into(),
            fields: HashMap::new(),
            trace_id: None,
        }
    }

    /// Set the source PID.
    pub fn with_pid(mut self, pid: Pid) -> Self {
        self.source_pid = Some(pid);
        self
    }

    /// Set the source service name.
    pub fn with_service(mut self, service: impl Into<String>) -> Self {
        self.source_service = Some(service.into());
        self
    }

    /// Set the trace ID.
    pub fn with_trace_id(mut self, trace_id: impl Into<String>) -> Self {
        self.trace_id = Some(trace_id.into());
        self
    }

    /// Add a structured field.
    pub fn with_field(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.fields.insert(key.into(), value);
        self
    }
}

/// Query parameters for filtering log entries.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LogQuery {
    /// Filter by source PID.
    pub pid: Option<Pid>,
    /// Filter by source service name.
    pub service: Option<String>,
    /// Minimum log level (inclusive).
    pub level_min: Option<LogLevel>,
    /// Start of time range (inclusive).
    pub since: Option<DateTime<Utc>>,
    /// End of time range (inclusive).
    pub until: Option<DateTime<Utc>>,
    /// Maximum number of entries to return.
    pub limit: usize,
    /// Filter by trace ID.
    pub trace_id: Option<String>,
}

impl LogQuery {
    /// Create a query with a limit.
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            ..Default::default()
        }
    }
}

/// Map LogLevel to a numeric severity for ordering.
fn level_severity(level: &LogLevel) -> u8 {
    match level {
        LogLevel::Debug => 0,
        LogLevel::Info => 1,
        LogLevel::Warn => 2,
        LogLevel::Error => 3,
    }
}

/// Structured logging service with ring buffer and query support.
///
/// Stores log entries in a bounded ring buffer (oldest evicted when full).
/// Supports query by PID, service, level, time range, and trace ID.
/// Real-time subscribers receive entries via `mpsc` channels.
pub struct LogService {
    entries: RwLock<VecDeque<LogEntry>>,
    max_entries: usize,
    subscribers: DashMap<String, mpsc::Sender<LogEntry>>,
}

impl LogService {
    /// Create a new log service with the given capacity.
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: RwLock::new(VecDeque::with_capacity(max_entries.min(8192))),
            max_entries,
            subscribers: DashMap::new(),
        }
    }

    /// Create a log service with default capacity (100,000).
    pub fn with_default_capacity() -> Self {
        Self::new(DEFAULT_LOG_CAPACITY)
    }

    /// Ingest a log entry.
    ///
    /// Stores in the ring buffer and fans out to all subscribers.
    /// Write lock is released before subscriber notification to
    /// prevent blocking on slow consumers.
    pub fn ingest(&self, entry: LogEntry) {
        // Store in ring buffer
        {
            let mut entries = self.entries.write().expect("log write lock poisoned");
            while entries.len() >= self.max_entries {
                entries.pop_front();
            }
            entries.push_back(entry.clone());
        }

        // Fan out to subscribers (non-blocking)
        let mut dead_subs = Vec::new();
        for sub in self.subscribers.iter() {
            if sub.value().try_send(entry.clone()).is_err() {
                dead_subs.push(sub.key().clone());
            }
        }
        for key in dead_subs {
            self.subscribers.remove(&key);
        }
    }

    /// Log a convenience method: ingest with level, message, and optional PID.
    pub fn log(&self, level: LogLevel, message: impl Into<String>, pid: Option<Pid>) {
        let mut entry = LogEntry::new(level, message);
        entry.source_pid = pid;
        self.ingest(entry);
    }

    /// Query log entries matching the given criteria.
    pub fn query(&self, query: &LogQuery) -> Vec<LogEntry> {
        let entries = self.entries.read().expect("log read lock poisoned");
        entries
            .iter()
            .filter(|e| query.pid.is_none_or(|pid| e.source_pid == Some(pid)))
            .filter(|e| {
                query
                    .service
                    .as_ref()
                    .is_none_or(|s| e.source_service.as_ref() == Some(s))
            })
            .filter(|e| {
                query
                    .level_min
                    .as_ref()
                    .is_none_or(|min| level_severity(&e.level) >= level_severity(min))
            })
            .filter(|e| query.since.is_none_or(|since| e.timestamp >= since))
            .filter(|e| query.until.is_none_or(|until| e.timestamp <= until))
            .filter(|e| {
                query
                    .trace_id
                    .as_ref()
                    .is_none_or(|tid| e.trace_id.as_ref() == Some(tid))
            })
            .take(query.limit)
            .cloned()
            .collect()
    }

    /// Subscribe to real-time log entries.
    ///
    /// Returns a receiver that will receive all new log entries.
    /// The subscription ID can be used to unsubscribe.
    pub fn subscribe(&self) -> (String, mpsc::Receiver<LogEntry>) {
        let id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = mpsc::channel(SUBSCRIBER_CHANNEL_CAPACITY);
        self.subscribers.insert(id.clone(), tx);
        (id, rx)
    }

    /// Unsubscribe from real-time log entries.
    pub fn unsubscribe(&self, id: &str) {
        self.subscribers.remove(id);
    }

    /// Number of log entries currently stored.
    pub fn len(&self) -> usize {
        self.entries.read().expect("log read lock poisoned").len()
    }

    /// Whether the log buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Maximum capacity of the ring buffer.
    pub fn capacity(&self) -> usize {
        self.max_entries
    }

    /// Number of active subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.subscribers.len()
    }

    /// Clear all log entries.
    pub fn clear(&self) {
        self.entries
            .write()
            .expect("log write lock poisoned")
            .clear();
    }
}

impl Default for LogService {
    fn default() -> Self {
        Self::with_default_capacity()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info_entry(msg: &str) -> LogEntry {
        LogEntry::new(LogLevel::Info, msg)
    }

    #[test]
    fn ingest_and_len() {
        let service = LogService::new(100);
        assert!(service.is_empty());

        service.ingest(info_entry("hello"));
        assert_eq!(service.len(), 1);
    }

    #[test]
    fn ring_buffer_eviction() {
        let service = LogService::new(3);

        for i in 0..5 {
            service.ingest(info_entry(&format!("msg-{i}")));
        }

        assert_eq!(service.len(), 3);
        let results = service.query(&LogQuery::new(10));
        assert_eq!(results[0].message, "msg-2");
        assert_eq!(results[2].message, "msg-4");
    }

    #[test]
    fn query_by_pid() {
        let service = LogService::new(100);
        service.ingest(info_entry("from-1").with_pid(1));
        service.ingest(info_entry("from-2").with_pid(2));
        service.ingest(info_entry("from-1-again").with_pid(1));

        let results = service.query(&LogQuery {
            pid: Some(1),
            limit: 10,
            ..Default::default()
        });
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|e| e.source_pid == Some(1)));
    }

    #[test]
    fn query_by_service() {
        let service = LogService::new(100);
        service.ingest(info_entry("a").with_service("auth"));
        service.ingest(info_entry("b").with_service("db"));
        service.ingest(info_entry("c").with_service("auth"));

        let results = service.query(&LogQuery {
            service: Some("auth".into()),
            limit: 10,
            ..Default::default()
        });
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn query_by_level() {
        let service = LogService::new(100);
        service.ingest(LogEntry::new(LogLevel::Debug, "debug-msg"));
        service.ingest(LogEntry::new(LogLevel::Info, "info-msg"));
        service.ingest(LogEntry::new(LogLevel::Warn, "warn-msg"));
        service.ingest(LogEntry::new(LogLevel::Error, "error-msg"));

        let results = service.query(&LogQuery {
            level_min: Some(LogLevel::Warn),
            limit: 10,
            ..Default::default()
        });
        assert_eq!(results.len(), 2);
        assert!(
            results
                .iter()
                .all(|e| matches!(e.level, LogLevel::Warn | LogLevel::Error))
        );
    }

    #[test]
    fn query_by_time_range() {
        let service = LogService::new(100);
        let before = Utc::now();
        service.ingest(info_entry("in-range"));
        let after = Utc::now();

        let results = service.query(&LogQuery {
            since: Some(before),
            until: Some(after),
            limit: 10,
            ..Default::default()
        });
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn query_by_trace_id() {
        let service = LogService::new(100);
        service.ingest(info_entry("traced").with_trace_id("trace-abc"));
        service.ingest(info_entry("untraced"));
        service.ingest(info_entry("other-trace").with_trace_id("trace-def"));

        let results = service.query(&LogQuery {
            trace_id: Some("trace-abc".into()),
            limit: 10,
            ..Default::default()
        });
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].message, "traced");
    }

    #[test]
    fn query_limit() {
        let service = LogService::new(100);
        for i in 0..10 {
            service.ingest(info_entry(&format!("msg-{i}")));
        }

        let results = service.query(&LogQuery::new(3));
        assert_eq!(results.len(), 3);
    }

    #[tokio::test]
    async fn subscriber_receives_entries() {
        let service = LogService::new(100);
        let (_id, mut rx) = service.subscribe();

        service.ingest(info_entry("sub-msg-1"));
        service.ingest(info_entry("sub-msg-2"));

        let m1 = rx.recv().await.unwrap();
        let m2 = rx.recv().await.unwrap();
        assert_eq!(m1.message, "sub-msg-1");
        assert_eq!(m2.message, "sub-msg-2");
    }

    #[test]
    fn unsubscribe() {
        let service = LogService::new(100);
        let (id, _rx) = service.subscribe();
        assert_eq!(service.subscriber_count(), 1);

        service.unsubscribe(&id);
        assert_eq!(service.subscriber_count(), 0);
    }

    #[test]
    fn dead_subscriber_removed() {
        let service = LogService::new(100);
        let (_id, rx) = service.subscribe();
        assert_eq!(service.subscriber_count(), 1);

        // Drop the receiver
        drop(rx);

        // Ingesting should clean up dead subscribers
        service.ingest(info_entry("after-drop"));
        assert_eq!(service.subscriber_count(), 0);
    }

    #[test]
    fn clear() {
        let service = LogService::new(100);
        service.ingest(info_entry("a"));
        service.ingest(info_entry("b"));
        assert_eq!(service.len(), 2);

        service.clear();
        assert!(service.is_empty());
    }

    #[test]
    fn log_convenience() {
        let service = LogService::new(100);
        service.log(LogLevel::Warn, "warning!", Some(42));

        let results = service.query(&LogQuery {
            pid: Some(42),
            limit: 10,
            ..Default::default()
        });
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].message, "warning!");
        assert!(matches!(results[0].level, LogLevel::Warn));
    }

    #[test]
    fn log_entry_builder() {
        let entry = LogEntry::new(LogLevel::Info, "test")
            .with_pid(5)
            .with_service("auth")
            .with_trace_id("t-123")
            .with_field("key", serde_json::json!("value"));

        assert_eq!(entry.source_pid, Some(5));
        assert_eq!(entry.source_service.as_deref(), Some("auth"));
        assert_eq!(entry.trace_id.as_deref(), Some("t-123"));
        assert_eq!(
            entry.fields.get("key").unwrap(),
            &serde_json::json!("value")
        );
    }

    #[test]
    fn log_entry_serde_roundtrip() {
        let entry = LogEntry::new(LogLevel::Error, "boom")
            .with_pid(7)
            .with_service("db")
            .with_trace_id("trace-999")
            .with_field("code", serde_json::json!(500));

        let json = serde_json::to_string(&entry).unwrap();
        let restored: LogEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.message, "boom");
        assert_eq!(restored.source_pid, Some(7));
        assert_eq!(restored.fields["code"], serde_json::json!(500));
    }

    #[test]
    fn default_service() {
        let service = LogService::default();
        assert_eq!(service.capacity(), DEFAULT_LOG_CAPACITY);
    }

    #[test]
    fn log_query_default() {
        let query = LogQuery::default();
        assert!(query.pid.is_none());
        assert_eq!(query.limit, 0);
    }
}
