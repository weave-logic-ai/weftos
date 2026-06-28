//! Process links and monitors (K1-G2).
//!
//! Provides Erlang-inspired bidirectional crash notification (links)
//! and unidirectional process monitoring. When a process exits, all
//! linked processes receive a `LinkExit` signal and all monitors
//! receive a `ProcessDown` notification.
//!
//! Gated behind `cfg(feature = "os-patterns")`.

use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::DashMap;
use serde::{Deserialize, Serialize};

use crate::process::Pid;

/// Why a process exited.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExitReason {
    /// Normal completion (exit code 0).
    Normal,
    /// Crashed with error message.
    Crash(String),
    /// Killed by supervisor or operator.
    Killed,
    /// Timed out (resource limit).
    Timeout,
}

impl std::fmt::Display for ExitReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExitReason::Normal => write!(f, "normal"),
            ExitReason::Crash(msg) => write!(f, "crash: {msg}"),
            ExitReason::Killed => write!(f, "killed"),
            ExitReason::Timeout => write!(f, "timeout"),
        }
    }
}

/// Bidirectional crash notification link between two processes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessLink {
    pub pid_a: Pid,
    pub pid_b: Pid,
}

/// Unidirectional process monitor.
#[derive(Debug, Clone)]
pub struct ProcessMonitor {
    pub watcher: Pid,
    pub target: Pid,
    pub ref_id: String,
}

/// Notification sent when a monitored process exits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessDown {
    pub pid: Pid,
    pub reason: ExitReason,
    pub ref_id: String,
}

/// Registry for process links and monitors.
///
/// Thread-safe via `DashMap`. Delivers exit signals when processes
/// terminate.
pub struct MonitorRegistry {
    /// Links indexed by PID -> set of linked PIDs
    links: DashMap<Pid, Vec<Pid>>,
    /// Monitors indexed by target PID -> list of monitors watching it
    monitors: DashMap<Pid, Vec<ProcessMonitor>>,
    /// Counter for generating unique monitor ref IDs
    next_ref: AtomicU64,
}

impl MonitorRegistry {
    /// Create a new, empty monitor registry.
    pub fn new() -> Self {
        Self {
            links: DashMap::new(),
            monitors: DashMap::new(),
            next_ref: AtomicU64::new(1),
        }
    }

    /// Create a bidirectional link between two processes.
    ///
    /// If the link already exists, this is a no-op.
    pub fn link(&self, pid_a: Pid, pid_b: Pid) {
        // Add pid_b to pid_a's link set
        self.links
            .entry(pid_a)
            .or_default()
            .value_mut()
            .retain(|&p| p != pid_b); // dedup
        self.links.entry(pid_a).or_default().push(pid_b);

        // Add pid_a to pid_b's link set
        self.links
            .entry(pid_b)
            .or_default()
            .value_mut()
            .retain(|&p| p != pid_a);
        self.links.entry(pid_b).or_default().push(pid_a);
    }

    /// Remove a bidirectional link between two processes.
    pub fn unlink(&self, pid_a: Pid, pid_b: Pid) {
        if let Some(mut links) = self.links.get_mut(&pid_a) {
            links.retain(|&p| p != pid_b);
        }
        if let Some(mut links) = self.links.get_mut(&pid_b) {
            links.retain(|&p| p != pid_a);
        }
    }

    /// Create a unidirectional monitor: `watcher` monitors `target`.
    ///
    /// Returns a unique reference ID that can be used to demonitor.
    pub fn monitor(&self, watcher: Pid, target: Pid) -> String {
        let ref_id = format!("mon-{}", self.next_ref.fetch_add(1, Ordering::Relaxed));
        let monitor = ProcessMonitor {
            watcher,
            target,
            ref_id: ref_id.clone(),
        };
        self.monitors.entry(target).or_default().push(monitor);
        ref_id
    }

    /// Remove a monitor by its reference ID.
    pub fn demonitor(&self, ref_id: &str) {
        // We need to search all targets; in practice monitors are
        // few, so linear scan is fine.
        for mut entry in self.monitors.iter_mut() {
            entry.value_mut().retain(|m| m.ref_id != ref_id);
        }
    }

    /// Called when a process exits. Returns the set of signals to deliver:
    /// - `(linked_pid, ExitReason)` for each linked process
    /// - `ProcessDown` for each monitoring process
    pub fn process_exited(
        &self,
        pid: Pid,
        reason: &ExitReason,
    ) -> (Vec<(Pid, ExitReason)>, Vec<ProcessDown>) {
        let mut link_signals = Vec::new();
        let mut down_signals = Vec::new();

        // Deliver link exit signals
        if let Some((_, linked_pids)) = self.links.remove(&pid) {
            for linked_pid in &linked_pids {
                link_signals.push((*linked_pid, reason.clone()));
                // Remove the reverse link
                if let Some(mut reverse) = self.links.get_mut(linked_pid) {
                    reverse.retain(|&p| p != pid);
                }
            }
        }

        // Deliver monitor down signals
        if let Some((_, monitors)) = self.monitors.remove(&pid) {
            for mon in monitors {
                down_signals.push(ProcessDown {
                    pid,
                    reason: reason.clone(),
                    ref_id: mon.ref_id,
                });
            }
        }

        (link_signals, down_signals)
    }

    /// Get all PIDs linked to the given PID.
    pub fn get_links(&self, pid: Pid) -> Vec<Pid> {
        self.links
            .get(&pid)
            .map(|v| v.value().clone())
            .unwrap_or_default()
    }

    /// Get all monitors watching the given PID.
    pub fn get_monitors(&self, pid: Pid) -> Vec<ProcessMonitor> {
        self.monitors
            .get(&pid)
            .map(|v| v.value().clone())
            .unwrap_or_default()
    }

    /// Check if two PIDs are linked.
    pub fn is_linked(&self, pid_a: Pid, pid_b: Pid) -> bool {
        self.links
            .get(&pid_a)
            .map(|v| v.contains(&pid_b))
            .unwrap_or(false)
    }
}

impl Default for MonitorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_creates_bidirectional() {
        let reg = MonitorRegistry::new();
        reg.link(1, 2);
        assert!(reg.is_linked(1, 2));
        assert!(reg.is_linked(2, 1));
    }

    #[test]
    fn link_idempotent() {
        let reg = MonitorRegistry::new();
        reg.link(1, 2);
        reg.link(1, 2);
        // Should only have one entry, not duplicates
        assert_eq!(reg.get_links(1).len(), 1);
        assert_eq!(reg.get_links(2).len(), 1);
    }

    #[test]
    fn unlink_removes_bidirectional() {
        let reg = MonitorRegistry::new();
        reg.link(1, 2);
        reg.unlink(1, 2);
        assert!(!reg.is_linked(1, 2));
        assert!(!reg.is_linked(2, 1));
    }

    #[test]
    fn monitor_returns_unique_ref() {
        let reg = MonitorRegistry::new();
        let ref1 = reg.monitor(10, 20);
        let ref2 = reg.monitor(10, 20);
        assert_ne!(ref1, ref2);
    }

    #[test]
    fn demonitor_removes_monitor() {
        let reg = MonitorRegistry::new();
        let ref_id = reg.monitor(10, 20);
        assert_eq!(reg.get_monitors(20).len(), 1);
        reg.demonitor(&ref_id);
        assert_eq!(reg.get_monitors(20).len(), 0);
    }

    #[test]
    fn process_exited_delivers_link_signals() {
        let reg = MonitorRegistry::new();
        reg.link(1, 2);
        reg.link(1, 3);

        let (links, downs) = reg.process_exited(1, &ExitReason::Crash("panic".into()));

        assert_eq!(links.len(), 2);
        assert!(downs.is_empty());

        // Verify linked PIDs received signals
        let pids: Vec<Pid> = links.iter().map(|(p, _)| *p).collect();
        assert!(pids.contains(&2));
        assert!(pids.contains(&3));

        // Reverse links should be cleaned up
        assert!(!reg.is_linked(2, 1));
        assert!(!reg.is_linked(3, 1));
    }

    #[test]
    fn process_exited_delivers_down_signals() {
        let reg = MonitorRegistry::new();
        let ref1 = reg.monitor(10, 1);
        let ref2 = reg.monitor(20, 1);

        let (links, downs) = reg.process_exited(1, &ExitReason::Normal);

        assert!(links.is_empty());
        assert_eq!(downs.len(), 2);

        let refs: Vec<&str> = downs.iter().map(|d| d.ref_id.as_str()).collect();
        assert!(refs.contains(&ref1.as_str()));
        assert!(refs.contains(&ref2.as_str()));
    }

    #[test]
    fn process_exited_delivers_both_links_and_monitors() {
        let reg = MonitorRegistry::new();
        reg.link(1, 2);
        reg.monitor(10, 1);

        let (links, downs) = reg.process_exited(1, &ExitReason::Killed);

        assert_eq!(links.len(), 1);
        assert_eq!(downs.len(), 1);
        assert_eq!(links[0].0, 2);
        assert_eq!(downs[0].reason, ExitReason::Killed);
    }

    #[test]
    fn normal_exit_delivers_normal_reason() {
        let reg = MonitorRegistry::new();
        reg.link(1, 2);
        let (links, _) = reg.process_exited(1, &ExitReason::Normal);
        assert_eq!(links[0].1, ExitReason::Normal);
    }

    #[test]
    fn multiple_monitors_on_same_target() {
        let reg = MonitorRegistry::new();
        reg.monitor(10, 1);
        reg.monitor(20, 1);
        reg.monitor(30, 1);

        let (_, downs) = reg.process_exited(1, &ExitReason::Timeout);
        assert_eq!(downs.len(), 3);
    }

    #[test]
    fn exit_reason_display() {
        assert_eq!(ExitReason::Normal.to_string(), "normal");
        assert_eq!(ExitReason::Crash("oom".into()).to_string(), "crash: oom");
        assert_eq!(ExitReason::Killed.to_string(), "killed");
        assert_eq!(ExitReason::Timeout.to_string(), "timeout");
    }

    #[test]
    fn exit_reason_serde_roundtrip() {
        let reasons = vec![
            ExitReason::Normal,
            ExitReason::Crash("test".into()),
            ExitReason::Killed,
            ExitReason::Timeout,
        ];
        for reason in reasons {
            let json = serde_json::to_string(&reason).unwrap();
            let restored: ExitReason = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, reason);
        }
    }

    #[test]
    fn process_down_serde_roundtrip() {
        let down = ProcessDown {
            pid: 42,
            reason: ExitReason::Crash("segfault".into()),
            ref_id: "mon-1".into(),
        };
        let json = serde_json::to_string(&down).unwrap();
        let restored: ProcessDown = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.pid, 42);
        assert_eq!(restored.ref_id, "mon-1");
    }

    #[test]
    fn get_links_empty() {
        let reg = MonitorRegistry::new();
        assert!(reg.get_links(999).is_empty());
    }

    #[test]
    fn get_monitors_empty() {
        let reg = MonitorRegistry::new();
        assert!(reg.get_monitors(999).is_empty());
    }
}
