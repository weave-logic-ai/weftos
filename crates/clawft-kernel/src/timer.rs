//! Timer service: one-shot and repeating timers with message delivery.
//!
//! [`TimerService`] complements [`CronService`] (cron expressions, minute
//! granularity) with sub-second precision timers. Each timer delivers a
//! [`KernelMessage`] to the owner's PID when it fires.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::ipc::{KernelMessage, MessagePayload};
use crate::process::Pid;

/// Metadata about a timer (serializable snapshot).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimerInfo {
    /// Timer identifier.
    pub id: String,
    /// PID of the timer's owner.
    pub owner_pid: Pid,
    /// When the timer is scheduled to fire.
    pub fire_at: DateTime<Utc>,
    /// Repeat interval (None for one-shot).
    pub repeat_interval_ms: Option<u64>,
    /// Whether the timer has been cancelled.
    pub cancelled: bool,
}

/// A timer entry tracked by the service.
pub struct TimerEntry {
    /// Timer identifier.
    pub id: String,
    /// PID of the timer's owner.
    pub owner_pid: Pid,
    /// When the timer is scheduled to fire.
    pub fire_at: DateTime<Utc>,
    /// Repeat interval (None for one-shot timers).
    pub repeat_interval: Option<Duration>,
    /// Payload to deliver when the timer fires.
    pub payload: MessagePayload,
    /// Token used to cancel this timer.
    pub cancel_token: CancellationToken,
}

impl TimerEntry {
    /// Get a serializable info snapshot.
    pub fn info(&self) -> TimerInfo {
        TimerInfo {
            id: self.id.clone(),
            owner_pid: self.owner_pid,
            fire_at: self.fire_at,
            repeat_interval_ms: self.repeat_interval.map(|d| d.as_millis() as u64),
            cancelled: self.cancel_token.is_cancelled(),
        }
    }
}

/// Timer service: one-shot and repeating timers with message delivery.
///
/// Registered as a `SystemService` alongside `CronService`.
/// Timers deliver messages to the owner's PID via the A2A router
/// (or whatever delivery mechanism is wired at the kernel level).
pub struct TimerService {
    timers: DashMap<String, TimerEntry>,
    next_id: AtomicU64,
}

impl TimerService {
    /// Create a new timer service.
    pub fn new() -> Self {
        Self {
            timers: DashMap::new(),
            next_id: AtomicU64::new(1),
        }
    }

    /// Generate a unique timer ID.
    fn next_timer_id(&self) -> String {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        format!("timer-{id}")
    }

    /// Create a one-shot timer.
    ///
    /// Returns the timer ID and a cancellation token. The timer fires
    /// after `delay` and delivers a message with the given payload.
    pub fn create_oneshot(
        &self,
        owner_pid: Pid,
        delay: Duration,
        payload: MessagePayload,
    ) -> (String, CancellationToken) {
        let id = self.next_timer_id();
        let cancel_token = CancellationToken::new();
        let fire_at = Utc::now() + chrono::Duration::from_std(delay).unwrap_or_default();

        let entry = TimerEntry {
            id: id.clone(),
            owner_pid,
            fire_at,
            repeat_interval: None,
            payload,
            cancel_token: cancel_token.clone(),
        };

        self.timers.insert(id.clone(), entry);
        (id, cancel_token)
    }

    /// Create a repeating timer.
    ///
    /// Returns the timer ID and a cancellation token. The timer fires
    /// every `interval` starting after the first `interval` elapses.
    pub fn create_repeating(
        &self,
        owner_pid: Pid,
        interval: Duration,
        payload: MessagePayload,
    ) -> (String, CancellationToken) {
        let id = self.next_timer_id();
        let cancel_token = CancellationToken::new();
        let fire_at = Utc::now() + chrono::Duration::from_std(interval).unwrap_or_default();

        let entry = TimerEntry {
            id: id.clone(),
            owner_pid,
            fire_at,
            repeat_interval: Some(interval),
            payload,
            cancel_token: cancel_token.clone(),
        };

        self.timers.insert(id.clone(), entry);
        (id, cancel_token)
    }

    /// Cancel a timer by ID.
    ///
    /// Returns `true` if the timer was found and cancelled.
    pub fn cancel(&self, timer_id: &str) -> bool {
        if let Some(entry) = self.timers.get(timer_id) {
            entry.cancel_token.cancel();
            true
        } else {
            false
        }
    }

    /// Cancel all timers owned by a specific PID.
    ///
    /// Called when an agent exits to clean up its timers.
    /// Returns the number of timers cancelled.
    pub fn cancel_for_pid(&self, pid: Pid) -> usize {
        let mut cancelled = 0;
        for entry in self.timers.iter() {
            if entry.owner_pid == pid {
                entry.cancel_token.cancel();
                cancelled += 1;
            }
        }
        cancelled
    }

    /// Remove a timer from the registry (after it fires or is cancelled).
    pub fn remove(&self, timer_id: &str) -> Option<TimerEntry> {
        self.timers.remove(timer_id).map(|(_, e)| e)
    }

    /// Remove all cancelled timers from the registry.
    pub fn cleanup_cancelled(&self) -> usize {
        let cancelled: Vec<String> = self
            .timers
            .iter()
            .filter(|e| e.cancel_token.is_cancelled())
            .map(|e| e.key().clone())
            .collect();
        let count = cancelled.len();
        for id in cancelled {
            self.timers.remove(&id);
        }
        count
    }

    /// Get info about a specific timer.
    pub fn info(&self, timer_id: &str) -> Option<TimerInfo> {
        self.timers.get(timer_id).map(|e| e.info())
    }

    /// List all timer IDs.
    pub fn list(&self) -> Vec<String> {
        self.timers.iter().map(|e| e.key().clone()).collect()
    }

    /// List info about all timers.
    pub fn list_info(&self) -> Vec<TimerInfo> {
        self.timers.iter().map(|e| e.value().info()).collect()
    }

    /// List timers owned by a specific PID.
    pub fn list_for_pid(&self, pid: Pid) -> Vec<TimerInfo> {
        self.timers
            .iter()
            .filter(|e| e.owner_pid == pid)
            .map(|e| e.info())
            .collect()
    }

    /// Number of registered timers.
    pub fn len(&self) -> usize {
        self.timers.len()
    }

    /// Whether there are no timers.
    pub fn is_empty(&self) -> bool {
        self.timers.is_empty()
    }

    /// Check which timers are due to fire.
    ///
    /// Returns timer entries that have passed their fire_at time
    /// and have not been cancelled. One-shot timers are removed;
    /// repeating timers have their fire_at updated.
    pub fn collect_due(&self) -> Vec<(Pid, KernelMessage)> {
        let now = Utc::now();
        let mut due = Vec::new();
        let mut to_remove = Vec::new();
        let mut to_reschedule = Vec::new();

        for entry in self.timers.iter() {
            if entry.cancel_token.is_cancelled() {
                to_remove.push(entry.key().clone());
                continue;
            }

            if entry.fire_at <= now {
                let msg = KernelMessage::new(
                    0, // from kernel
                    crate::ipc::MessageTarget::Process(entry.owner_pid),
                    entry.payload.clone(),
                );
                due.push((entry.owner_pid, msg));

                if let Some(interval) = entry.repeat_interval {
                    to_reschedule.push((entry.key().clone(), interval));
                } else {
                    to_remove.push(entry.key().clone());
                }
            }
        }

        // Remove one-shot timers that fired
        for id in &to_remove {
            self.timers.remove(id);
        }

        // Reschedule repeating timers
        for (id, interval) in to_reschedule {
            if let Some(mut entry) = self.timers.get_mut(&id) {
                entry.fire_at =
                    Utc::now() + chrono::Duration::from_std(interval).unwrap_or_default();
            }
        }

        due
    }
}

impl Default for TimerService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::MessagePayload;

    fn text_payload(msg: &str) -> MessagePayload {
        MessagePayload::Text(msg.into())
    }

    #[test]
    fn create_oneshot_timer() {
        let service = TimerService::new();
        let (id, _token) =
            service.create_oneshot(1, Duration::from_secs(10), text_payload("fire!"));

        assert!(service.info(&id).is_some());
        assert_eq!(service.len(), 1);
        let info = service.info(&id).unwrap();
        assert_eq!(info.owner_pid, 1);
        assert!(info.repeat_interval_ms.is_none());
        assert!(!info.cancelled);
    }

    #[test]
    fn create_repeating_timer() {
        let service = TimerService::new();
        let (id, _token) =
            service.create_repeating(2, Duration::from_millis(500), text_payload("tick"));

        let info = service.info(&id).unwrap();
        assert_eq!(info.owner_pid, 2);
        assert_eq!(info.repeat_interval_ms, Some(500));
    }

    #[test]
    fn cancel_timer() {
        let service = TimerService::new();
        let (id, _token) = service.create_oneshot(1, Duration::from_secs(10), text_payload("x"));

        assert!(service.cancel(&id));
        let info = service.info(&id).unwrap();
        assert!(info.cancelled);
    }

    #[test]
    fn cancel_nonexistent() {
        let service = TimerService::new();
        assert!(!service.cancel("nope"));
    }

    #[test]
    fn cancel_for_pid() {
        let service = TimerService::new();
        service.create_oneshot(1, Duration::from_secs(10), text_payload("a"));
        service.create_oneshot(1, Duration::from_secs(20), text_payload("b"));
        service.create_oneshot(2, Duration::from_secs(10), text_payload("c"));

        let cancelled = service.cancel_for_pid(1);
        assert_eq!(cancelled, 2);

        // Timer for PID 2 should not be cancelled
        let pid2_timers = service.list_for_pid(2);
        assert_eq!(pid2_timers.len(), 1);
        assert!(!pid2_timers[0].cancelled);
    }

    #[test]
    fn cleanup_cancelled() {
        let service = TimerService::new();
        let (id, _) = service.create_oneshot(1, Duration::from_secs(10), text_payload("a"));
        service.create_oneshot(2, Duration::from_secs(10), text_payload("b"));

        service.cancel(&id);
        let cleaned = service.cleanup_cancelled();
        assert_eq!(cleaned, 1);
        assert_eq!(service.len(), 1);
    }

    #[test]
    fn collect_due_oneshot() {
        let service = TimerService::new();
        // Create a timer that fires immediately (0 delay)
        service.create_oneshot(5, Duration::from_millis(0), text_payload("now!"));

        // Small sleep to ensure fire_at is in the past
        std::thread::sleep(Duration::from_millis(10));

        let due = service.collect_due();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].0, 5); // owner_pid

        // One-shot should be removed
        assert!(service.is_empty());
    }

    #[test]
    fn collect_due_repeating_reschedules() {
        let service = TimerService::new();
        let (id, _) = service.create_repeating(3, Duration::from_millis(0), text_payload("repeat"));

        std::thread::sleep(Duration::from_millis(10));

        let due = service.collect_due();
        assert_eq!(due.len(), 1);

        // Repeating timer should still exist (rescheduled)
        assert!(service.info(&id).is_some());
        assert_eq!(service.len(), 1);
    }

    #[test]
    fn collect_due_skips_cancelled() {
        let service = TimerService::new();
        let (id, _) = service.create_oneshot(1, Duration::from_millis(0), text_payload("x"));
        service.cancel(&id);

        std::thread::sleep(Duration::from_millis(10));

        let due = service.collect_due();
        assert!(due.is_empty());
    }

    #[test]
    fn collect_due_skips_future() {
        let service = TimerService::new();
        service.create_oneshot(1, Duration::from_secs(3600), text_payload("far future"));

        let due = service.collect_due();
        assert!(due.is_empty());
        assert_eq!(service.len(), 1);
    }

    #[test]
    fn list_and_list_info() {
        let service = TimerService::new();
        service.create_oneshot(1, Duration::from_secs(10), text_payload("a"));
        service.create_oneshot(2, Duration::from_secs(20), text_payload("b"));

        assert_eq!(service.list().len(), 2);
        assert_eq!(service.list_info().len(), 2);
    }

    #[test]
    fn list_for_pid() {
        let service = TimerService::new();
        service.create_oneshot(1, Duration::from_secs(10), text_payload("a"));
        service.create_oneshot(1, Duration::from_secs(20), text_payload("b"));
        service.create_oneshot(2, Duration::from_secs(10), text_payload("c"));

        assert_eq!(service.list_for_pid(1).len(), 2);
        assert_eq!(service.list_for_pid(2).len(), 1);
        assert_eq!(service.list_for_pid(99).len(), 0);
    }

    #[test]
    fn remove_timer() {
        let service = TimerService::new();
        let (id, _) = service.create_oneshot(1, Duration::from_secs(10), text_payload("x"));
        assert_eq!(service.len(), 1);

        let entry = service.remove(&id).unwrap();
        assert_eq!(entry.owner_pid, 1);
        assert!(service.is_empty());
    }

    #[test]
    fn unique_ids() {
        let service = TimerService::new();
        let (id1, _) = service.create_oneshot(1, Duration::from_secs(10), text_payload("a"));
        let (id2, _) = service.create_oneshot(1, Duration::from_secs(10), text_payload("b"));
        assert_ne!(id1, id2);
    }

    #[test]
    fn timer_info_serde_roundtrip() {
        let info = TimerInfo {
            id: "timer-1".into(),
            owner_pid: 42,
            fire_at: Utc::now(),
            repeat_interval_ms: Some(1000),
            cancelled: false,
        };
        let json = serde_json::to_string(&info).unwrap();
        let restored: TimerInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.id, "timer-1");
        assert_eq!(restored.owner_pid, 42);
        assert_eq!(restored.repeat_interval_ms, Some(1000));
    }

    #[test]
    fn default_service() {
        let service = TimerService::default();
        assert!(service.is_empty());
    }
}
