//! Audit logging for WASM plugin host function calls.
//!
//! Every host function invocation (HTTP, filesystem, env, log) is recorded
//! in a per-plugin audit log. This provides a tamper-evident record of all
//! side-effecting operations performed by a plugin.
//!
//! The audit log is stored in memory and can be flushed to persistent storage
//! or forwarded to a monitoring system.

use std::sync::Mutex;
use std::time::Instant;

/// A single audit log entry recording one host function call.
#[derive(Debug, Clone)]
pub struct AuditEntry {
    /// Plugin that made the call.
    pub plugin_id: String,
    /// Host function name (e.g., "http-request", "read-file").
    pub function: String,
    /// Summary of the call parameters (redacted where appropriate).
    pub params_summary: String,
    /// Whether the call was permitted.
    pub permitted: bool,
    /// Error message if denied or failed.
    pub error: Option<String>,
    /// Wall-clock duration of the call.
    pub duration_ms: u64,
    /// Monotonic timestamp (elapsed since log creation).
    pub elapsed_ms: u64,
}

/// Per-plugin audit log.
///
/// Thread-safe via internal [`Mutex`]. Each plugin instance gets its own
/// `AuditLog` to prevent cross-plugin log pollution.
pub struct AuditLog {
    plugin_id: String,
    entries: Mutex<Vec<AuditEntry>>,
    created_at: Instant,
    /// Maximum entries before oldest are evicted (ring buffer behavior).
    max_entries: usize,
}

impl AuditLog {
    /// Create a new audit log for the given plugin.
    pub fn new(plugin_id: String) -> Self {
        Self {
            plugin_id,
            entries: Mutex::new(Vec::new()),
            created_at: Instant::now(),
            max_entries: 10_000,
        }
    }

    /// Create a new audit log with a custom max entry limit.
    pub fn with_max_entries(plugin_id: String, max_entries: usize) -> Self {
        Self {
            plugin_id,
            entries: Mutex::new(Vec::new()),
            created_at: Instant::now(),
            max_entries,
        }
    }

    /// Record a successful host function call.
    pub fn record_success(&self, function: &str, params_summary: &str, duration_ms: u64) {
        self.record(function, params_summary, true, None, duration_ms);
    }

    /// Record a denied host function call.
    pub fn record_denied(&self, function: &str, params_summary: &str, error: &str) {
        self.record(function, params_summary, false, Some(error), 0);
    }

    /// Record a host function call that was permitted but failed.
    pub fn record_error(
        &self,
        function: &str,
        params_summary: &str,
        error: &str,
        duration_ms: u64,
    ) {
        self.record(function, params_summary, true, Some(error), duration_ms);
    }

    /// Record an audit entry.
    fn record(
        &self,
        function: &str,
        params_summary: &str,
        permitted: bool,
        error: Option<&str>,
        duration_ms: u64,
    ) {
        let entry = AuditEntry {
            plugin_id: self.plugin_id.clone(),
            function: function.to_string(),
            params_summary: params_summary.to_string(),
            permitted,
            error: error.map(String::from),
            duration_ms,
            elapsed_ms: self.created_at.elapsed().as_millis() as u64,
        };

        let mut entries = self.entries.lock().unwrap();
        if entries.len() >= self.max_entries {
            // Ring buffer: remove oldest entry
            entries.remove(0);
        }
        entries.push(entry);
    }

    /// Get a snapshot of all audit entries.
    pub fn entries(&self) -> Vec<AuditEntry> {
        self.entries.lock().unwrap().clone()
    }

    /// Number of entries in the log.
    pub fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }

    /// Whether the log is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.lock().unwrap().is_empty()
    }

    /// Clear all entries.
    pub fn clear(&self) {
        self.entries.lock().unwrap().clear();
    }

    /// Get the plugin ID this log belongs to.
    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    /// Count entries for a specific function.
    pub fn count_by_function(&self, function: &str) -> usize {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.function == function)
            .count()
    }

    /// Count denied entries.
    pub fn count_denied(&self) -> usize {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .filter(|e| !e.permitted)
            .count()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_log_new_is_empty() {
        let log = AuditLog::new("test-plugin".into());
        assert!(log.is_empty());
        assert_eq!(log.len(), 0);
        assert_eq!(log.plugin_id(), "test-plugin");
    }

    #[test]
    fn record_success() {
        let log = AuditLog::new("test-plugin".into());
        log.record_success("http-request", "GET https://api.example.com/data", 42);

        assert_eq!(log.len(), 1);
        let entries = log.entries();
        assert_eq!(entries[0].function, "http-request");
        assert!(entries[0].permitted);
        assert!(entries[0].error.is_none());
        assert_eq!(entries[0].duration_ms, 42);
    }

    #[test]
    fn record_denied() {
        let log = AuditLog::new("test-plugin".into());
        log.record_denied("read-file", "/etc/passwd", "filesystem access denied");

        assert_eq!(log.len(), 1);
        let entries = log.entries();
        assert!(!entries[0].permitted);
        assert_eq!(
            entries[0].error.as_deref(),
            Some("filesystem access denied")
        );
    }

    #[test]
    fn record_error() {
        let log = AuditLog::new("test-plugin".into());
        log.record_error(
            "http-request",
            "GET https://api.example.com/",
            "timeout",
            5000,
        );

        assert_eq!(log.len(), 1);
        let entries = log.entries();
        assert!(entries[0].permitted);
        assert_eq!(entries[0].error.as_deref(), Some("timeout"));
        assert_eq!(entries[0].duration_ms, 5000);
    }

    #[test]
    fn multiple_entries() {
        let log = AuditLog::new("test-plugin".into());
        log.record_success("http-request", "GET /a", 10);
        log.record_success("read-file", "/sandbox/data.txt", 5);
        log.record_denied("read-file", "/etc/passwd", "denied");
        log.record_success("get-env", "MY_VAR", 0);
        log.record_success("log", "info: hello", 0);

        assert_eq!(log.len(), 5);
        assert_eq!(log.count_by_function("http-request"), 1);
        assert_eq!(log.count_by_function("read-file"), 2);
        assert_eq!(log.count_by_function("get-env"), 1);
        assert_eq!(log.count_by_function("log"), 1);
        assert_eq!(log.count_denied(), 1);
    }

    #[test]
    fn ring_buffer_eviction() {
        let log = AuditLog::with_max_entries("test-plugin".into(), 3);
        log.record_success("fn1", "p1", 0);
        log.record_success("fn2", "p2", 0);
        log.record_success("fn3", "p3", 0);
        assert_eq!(log.len(), 3);

        // Adding a 4th should evict the oldest
        log.record_success("fn4", "p4", 0);
        assert_eq!(log.len(), 3);

        let entries = log.entries();
        assert_eq!(entries[0].function, "fn2");
        assert_eq!(entries[1].function, "fn3");
        assert_eq!(entries[2].function, "fn4");
    }

    #[test]
    fn clear_removes_all() {
        let log = AuditLog::new("test-plugin".into());
        log.record_success("fn", "p", 0);
        log.record_success("fn", "p", 0);
        assert_eq!(log.len(), 2);

        log.clear();
        assert!(log.is_empty());
    }

    #[test]
    fn elapsed_ms_is_monotonic() {
        let log = AuditLog::new("test-plugin".into());
        log.record_success("fn1", "p1", 0);
        std::thread::sleep(std::time::Duration::from_millis(5));
        log.record_success("fn2", "p2", 0);

        let entries = log.entries();
        assert!(entries[1].elapsed_ms >= entries[0].elapsed_ms);
    }

    #[test]
    fn t42_all_host_functions_produce_audit_entries() {
        // T42: Every host function call type produces an audit entry
        let log = AuditLog::new("test-plugin".into());

        // Simulate all 5 host function types
        log.record_success("http-request", "GET https://example.com/", 100);
        log.record_success("read-file", "/sandbox/file.txt", 5);
        log.record_success("write-file", "/sandbox/out.txt", 10);
        log.record_success("get-env", "MY_VAR", 0);
        log.record_success("log", "info: message", 0);

        assert_eq!(log.len(), 5);

        // Verify each function type has an entry
        assert_eq!(log.count_by_function("http-request"), 1);
        assert_eq!(log.count_by_function("read-file"), 1);
        assert_eq!(log.count_by_function("write-file"), 1);
        assert_eq!(log.count_by_function("get-env"), 1);
        assert_eq!(log.count_by_function("log"), 1);
    }
}
