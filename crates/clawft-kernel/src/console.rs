//! Kernel console: boot event types and output formatting.
//!
//! Provides [`BootEvent`], [`BootPhase`], and [`LogLevel`] types for
//! recording and displaying kernel boot output. The interactive REPL
//! loop is stubbed (requires complex stdin handling); only the event
//! types and output formatting are implemented.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Phase of the boot sequence.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BootPhase {
    /// Pre-boot initialization.
    Init,
    /// Loading configuration.
    Config,
    /// Registering system services.
    Services,
    /// Loading resource tree from DAG.
    ResourceTree,
    /// Spawning service agents.
    Agents,
    /// Network service discovery.
    Network,
    /// ECC cognitive substrate initialization.
    Ecc,
    /// Boot complete, ready for commands.
    Ready,
}

impl BootPhase {
    /// Short tag string for console output (e.g. `[INIT]`).
    pub fn tag(&self) -> &'static str {
        match self {
            BootPhase::Init => "INIT",
            BootPhase::Config => "CONFIG",
            BootPhase::Services => "SERVICES",
            BootPhase::ResourceTree => "TREE",
            BootPhase::Agents => "AGENTS",
            BootPhase::Network => "NETWORK",
            BootPhase::Ecc => "ECC",
            BootPhase::Ready => "READY",
        }
    }
}

impl std::fmt::Display for BootPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.tag())
    }
}

/// Log level for boot events.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogLevel {
    /// Debug-level messages (not shown in normal boot output).
    Debug,
    /// Informational messages (standard boot output).
    Info,
    /// Warning messages.
    Warn,
    /// Error messages.
    Error,
}

/// A single boot event recorded during kernel startup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootEvent {
    /// When the event occurred.
    pub timestamp: DateTime<Utc>,
    /// Which boot phase generated the event.
    pub phase: BootPhase,
    /// Human-readable event message.
    pub message: String,
    /// Severity level.
    pub level: LogLevel,
}

impl BootEvent {
    /// Create a new info-level boot event.
    pub fn info(phase: BootPhase, message: impl Into<String>) -> Self {
        Self {
            timestamp: Utc::now(),
            phase,
            message: message.into(),
            level: LogLevel::Info,
        }
    }

    /// Create a new warning-level boot event.
    pub fn warn(phase: BootPhase, message: impl Into<String>) -> Self {
        Self {
            timestamp: Utc::now(),
            phase,
            message: message.into(),
            level: LogLevel::Warn,
        }
    }

    /// Create a new error-level boot event.
    pub fn error(phase: BootPhase, message: impl Into<String>) -> Self {
        Self {
            timestamp: Utc::now(),
            phase,
            message: message.into(),
            level: LogLevel::Error,
        }
    }

    /// Format this event for console display.
    ///
    /// Example: `  [INIT]      WeftOS v0.6.19 booting...` (version
    /// pulled from `CARGO_PKG_VERSION`).
    pub fn format_line(&self) -> String {
        let tag = self.phase.tag();
        format!("  [{tag:<10}] {}", self.message)
    }
}

/// Boot log: a recorded sequence of boot events.
///
/// Used to replay boot output when attaching to a running kernel
/// or for diagnostics.
#[derive(Debug, Clone, Default)]
pub struct BootLog {
    events: Vec<BootEvent>,
}

impl BootLog {
    /// Create an empty boot log.
    pub fn new() -> Self {
        Self { events: Vec::new() }
    }

    /// Record a boot event.
    pub fn push(&mut self, event: BootEvent) {
        self.events.push(event);
    }

    /// Get all recorded events.
    pub fn events(&self) -> &[BootEvent] {
        &self.events
    }

    /// Format all events for console display.
    pub fn format_all(&self) -> String {
        let mut output = String::new();
        for event in &self.events {
            if event.level == LogLevel::Debug {
                continue;
            }
            output.push_str(&event.format_line());
            output.push('\n');
        }
        output
    }

    /// Get the number of recorded events.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Check whether the log is empty.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

/// Default capacity of the kernel event ring buffer.
const DEFAULT_EVENT_LOG_CAPACITY: usize = 1024;

/// Thread-safe ring buffer for kernel runtime events.
///
/// Captures boot events and any post-boot events (service starts/stops,
/// agent spawns, health checks, errors). Holds at most `capacity` events
/// — when full, the oldest event is evicted.
///
/// Used by the daemon to serve `kernel.logs` RPC requests.
pub struct KernelEventLog {
    events: std::sync::Mutex<std::collections::VecDeque<BootEvent>>,
    capacity: usize,
}

impl KernelEventLog {
    /// Create a new event log with the default capacity (1024).
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_EVENT_LOG_CAPACITY)
    }

    /// Create a new event log with a specific capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            events: std::sync::Mutex::new(std::collections::VecDeque::with_capacity(capacity)),
            capacity,
        }
    }

    /// Push an event into the ring buffer.
    pub fn push(&self, event: BootEvent) {
        let mut events = self.events.lock().unwrap();
        if events.len() >= self.capacity {
            events.pop_front();
        }
        events.push_back(event);
    }

    /// Push a simple info event with a source tag and message.
    pub fn info(&self, source: &str, message: impl Into<String>) {
        self.push(BootEvent {
            timestamp: Utc::now(),
            phase: BootPhase::Ready, // post-boot events use Ready phase
            message: format!("[{source}] {}", message.into()),
            level: LogLevel::Info,
        });
    }

    /// Push a warning event.
    pub fn warn(&self, source: &str, message: impl Into<String>) {
        self.push(BootEvent {
            timestamp: Utc::now(),
            phase: BootPhase::Ready,
            message: format!("[{source}] {}", message.into()),
            level: LogLevel::Warn,
        });
    }

    /// Push an error event.
    pub fn error(&self, source: &str, message: impl Into<String>) {
        self.push(BootEvent {
            timestamp: Utc::now(),
            phase: BootPhase::Ready,
            message: format!("[{source}] {}", message.into()),
            level: LogLevel::Error,
        });
    }

    /// Push an info event and optionally append to the local chain.
    #[cfg(feature = "exochain")]
    pub fn info_with_chain(
        &self,
        source: &str,
        message: impl Into<String>,
        chain: Option<&crate::chain::ChainManager>,
    ) {
        let msg = message.into();
        self.info(source, &msg);
        if let Some(cm) = chain {
            cm.append(
                source,
                "log.info",
                Some(serde_json::json!({ "message": msg })),
            );
        }
    }

    /// Ingest all events from a BootLog (used to seed boot events).
    pub fn ingest_boot_log(&self, boot_log: &BootLog) {
        for event in boot_log.events() {
            self.push(event.clone());
        }
    }

    /// Get the last `n` events (or all if `n` is 0 or exceeds count).
    pub fn tail(&self, n: usize) -> Vec<BootEvent> {
        let events = self.events.lock().unwrap();
        if n == 0 || n >= events.len() {
            events.iter().cloned().collect()
        } else {
            events.iter().skip(events.len() - n).cloned().collect()
        }
    }

    /// Get all events matching a minimum log level.
    pub fn filter_level(&self, min_level: &LogLevel, n: usize) -> Vec<BootEvent> {
        let events = self.events.lock().unwrap();
        let level_rank = |l: &LogLevel| -> u8 {
            match l {
                LogLevel::Debug => 0,
                LogLevel::Info => 1,
                LogLevel::Warn => 2,
                LogLevel::Error => 3,
            }
        };
        let min_rank = level_rank(min_level);
        let filtered: Vec<BootEvent> = events
            .iter()
            .filter(|e| level_rank(&e.level) >= min_rank)
            .cloned()
            .collect();
        if n == 0 || n >= filtered.len() {
            filtered
        } else {
            filtered[filtered.len() - n..].to_vec()
        }
    }

    /// Current number of events in the buffer.
    pub fn len(&self) -> usize {
        self.events.lock().unwrap().len()
    }

    /// Check if the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.events.lock().unwrap().is_empty()
    }
}

impl Default for KernelEventLog {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for KernelEventLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let len = self.len();
        f.debug_struct("KernelEventLog")
            .field("count", &len)
            .field("capacity", &self.capacity)
            .finish()
    }
}

/// Format the boot banner header.
pub fn boot_banner() -> String {
    let mut output = String::new();
    output.push_str("\n  WeftOS v");
    output.push_str(env!("CARGO_PKG_VERSION"));
    output.push('\n');
    output.push_str("  ");
    output.push_str(&"-".repeat(45));
    output.push('\n');
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_phase_tags() {
        assert_eq!(BootPhase::Init.tag(), "INIT");
        assert_eq!(BootPhase::Config.tag(), "CONFIG");
        assert_eq!(BootPhase::Services.tag(), "SERVICES");
        assert_eq!(BootPhase::ResourceTree.tag(), "TREE");
        assert_eq!(BootPhase::Agents.tag(), "AGENTS");
        assert_eq!(BootPhase::Network.tag(), "NETWORK");
        assert_eq!(BootPhase::Ready.tag(), "READY");
    }

    #[test]
    fn boot_event_info() {
        let msg = format!("WeftOS v{} booting...", env!("CARGO_PKG_VERSION"));
        let event = BootEvent::info(BootPhase::Init, msg.clone());
        assert_eq!(event.phase, BootPhase::Init);
        assert_eq!(event.level, LogLevel::Info);
        assert_eq!(event.message, msg);
    }

    #[test]
    fn boot_event_format_line() {
        let event = BootEvent::info(BootPhase::Init, "PID 0 (kernel)");
        let line = event.format_line();
        assert!(line.contains("[INIT"));
        assert!(line.contains("PID 0 (kernel)"));
    }

    #[test]
    fn boot_log_push_and_format() {
        let mut log = BootLog::new();
        log.push(BootEvent::info(BootPhase::Init, "booting..."));
        log.push(BootEvent::info(BootPhase::Config, "config loaded"));
        log.push(BootEvent::info(BootPhase::Ready, "ready"));

        assert_eq!(log.len(), 3);
        let formatted = log.format_all();
        assert!(formatted.contains("booting..."));
        assert!(formatted.contains("config loaded"));
        assert!(formatted.contains("ready"));
    }

    #[test]
    fn boot_log_skips_debug() {
        let mut log = BootLog::new();
        log.push(BootEvent {
            timestamp: Utc::now(),
            phase: BootPhase::Init,
            message: "debug msg".into(),
            level: LogLevel::Debug,
        });
        log.push(BootEvent::info(BootPhase::Init, "info msg"));

        let formatted = log.format_all();
        assert!(!formatted.contains("debug msg"));
        assert!(formatted.contains("info msg"));
    }

    #[test]
    fn boot_log_empty() {
        let log = BootLog::new();
        assert!(log.is_empty());
        assert_eq!(log.len(), 0);
        assert!(log.format_all().is_empty());
    }

    #[test]
    fn boot_banner_format() {
        let banner = boot_banner();
        assert!(banner.contains("WeftOS v"));
        assert!(banner.contains(env!("CARGO_PKG_VERSION")));
        assert!(banner.contains("---"));
    }

    #[test]
    fn boot_event_serde() {
        let event = BootEvent::info(BootPhase::Services, "[OK] message-bus");
        let json = serde_json::to_string(&event).unwrap();
        let restored: BootEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.phase, BootPhase::Services);
        assert_eq!(restored.message, "[OK] message-bus");
    }

    #[test]
    fn boot_event_warn_and_error() {
        let warn_event = BootEvent::warn(BootPhase::Services, "slow start");
        assert_eq!(warn_event.level, LogLevel::Warn);

        let err_event = BootEvent::error(BootPhase::Services, "failed");
        assert_eq!(err_event.level, LogLevel::Error);
    }

    // ── KernelEventLog tests ──────────────────────────────────

    #[test]
    fn event_log_push_and_tail() {
        let log = KernelEventLog::new();
        log.info("test", "first");
        log.info("test", "second");
        log.warn("test", "third");

        assert_eq!(log.len(), 3);

        let all = log.tail(0);
        assert_eq!(all.len(), 3);
        assert!(all[0].message.contains("first"));

        let last_two = log.tail(2);
        assert_eq!(last_two.len(), 2);
        assert!(last_two[0].message.contains("second"));
    }

    #[test]
    fn event_log_ring_buffer_evicts() {
        let log = KernelEventLog::with_capacity(3);
        log.info("a", "1");
        log.info("b", "2");
        log.info("c", "3");
        log.info("d", "4"); // evicts "1"

        assert_eq!(log.len(), 3);
        let all = log.tail(0);
        assert!(all[0].message.contains("[b] 2"));
        assert!(all[2].message.contains("[d] 4"));
    }

    #[test]
    fn event_log_filter_level() {
        let log = KernelEventLog::new();
        log.info("test", "info msg");
        log.warn("test", "warn msg");
        log.error("test", "error msg");

        let warns_and_above = log.filter_level(&LogLevel::Warn, 0);
        assert_eq!(warns_and_above.len(), 2);

        let errors_only = log.filter_level(&LogLevel::Error, 0);
        assert_eq!(errors_only.len(), 1);
    }

    #[test]
    fn event_log_ingest_boot_log() {
        let mut boot_log = BootLog::new();
        boot_log.push(BootEvent::info(BootPhase::Init, "booting"));
        boot_log.push(BootEvent::info(BootPhase::Ready, "ready"));

        let event_log = KernelEventLog::new();
        event_log.ingest_boot_log(&boot_log);

        assert_eq!(event_log.len(), 2);
        let events = event_log.tail(0);
        assert!(events[0].message.contains("booting"));
    }

    #[test]
    fn event_log_empty() {
        let log = KernelEventLog::new();
        assert!(log.is_empty());
        assert_eq!(log.len(), 0);
        assert!(log.tail(10).is_empty());
    }
}
