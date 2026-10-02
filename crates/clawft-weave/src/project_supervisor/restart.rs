//! Restart policy: OTP `one_for_one` with `Transient` children (ADR-103 A6).
//!
//! Only a crashed child is restarted (non-zero exit, a signal, or a vanished
//! process); a clean exit (idle stop, explicit stop) is never restarted. The
//! delay doubles from `base` (1 s) to `cap` (30 s). At most `max` restarts
//! are granted inside `window`; the next crash gives up and the project goes
//! `failed` until an operator runs `project.restart`.
//!
//! Pure over explicit `Instant`s so the schedule is testable without
//! sleeping.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// What to do about a crash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Start again after `after`.
    Restart {
        /// Backoff before the new start.
        after: Duration,
    },
    /// The budget is spent: mark the project failed.
    GiveUp {
        /// Restarts already granted inside the window.
        restarts_in_window: usize,
    },
}

/// Per-project restart bookkeeping.
#[derive(Debug, Clone)]
pub struct RestartTracker {
    max: u32,
    window: Duration,
    base: Duration,
    cap: Duration,
    recent: VecDeque<Instant>,
    consecutive: u32,
    last_start: Option<Instant>,
}

impl RestartTracker {
    /// A tracker granting `max` restarts per `window`, backing off from
    /// `base` to `cap`.
    pub fn new(max: u32, window: Duration, base: Duration, cap: Duration) -> Self {
        Self {
            max,
            window,
            base,
            cap,
            recent: VecDeque::new(),
            consecutive: 0,
            last_start: None,
        }
    }

    /// Adopt new limits (the manifest may have changed) keeping history.
    pub fn reconfigure(&mut self, max: u32, window: Duration) {
        self.max = max;
        self.window = window;
    }

    /// The child was (re)started at `now`.
    pub fn on_started(&mut self, now: Instant) {
        self.last_start = Some(now);
    }

    /// Forget all history (explicit `project.restart`).
    pub fn reset(&mut self) {
        self.recent.clear();
        self.consecutive = 0;
        self.last_start = None;
    }

    /// Restarts granted inside the current window.
    pub fn restarts_in_window(&self, now: Instant) -> usize {
        self.recent
            .iter()
            .filter(|t| now.saturating_duration_since(**t) < self.window)
            .count()
    }

    /// The child crashed at `now`.
    pub fn on_crash(&mut self, now: Instant) -> Decision {
        // A run that outlived the window was healthy: start the backoff over.
        if self
            .last_start
            .is_some_and(|s| now.saturating_duration_since(s) >= self.window)
        {
            self.consecutive = 0;
        }
        while self
            .recent
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) >= self.window)
        {
            self.recent.pop_front();
        }
        if self.recent.len() as u32 >= self.max {
            return Decision::GiveUp {
                restarts_in_window: self.recent.len(),
            };
        }
        self.recent.push_back(now);
        self.consecutive = self.consecutive.saturating_add(1);
        let shift = (self.consecutive - 1).min(16);
        let delay = self
            .base
            .checked_mul(1u32 << shift)
            .unwrap_or(self.cap)
            .min(self.cap);
        Decision::Restart { after: delay }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: Duration = Duration::from_secs(1);

    fn tracker(max: u32) -> RestartTracker {
        RestartTracker::new(max, Duration::from_secs(60), S, Duration::from_secs(30))
    }

    #[test]
    fn backoff_doubles_to_the_cap() {
        let mut t = tracker(100);
        let t0 = Instant::now();
        let secs: Vec<u64> = (0..8)
            .map(|i| match t.on_crash(t0 + S * i) {
                Decision::Restart { after } => after.as_secs(),
                d => panic!("{d:?}"),
            })
            .collect();
        assert_eq!(secs, [1, 2, 4, 8, 16, 30, 30, 30]);
    }

    #[test]
    fn budget_then_give_up() {
        let mut t = tracker(2);
        let t0 = Instant::now();
        assert!(matches!(t.on_crash(t0), Decision::Restart { .. }));
        assert!(matches!(t.on_crash(t0 + S), Decision::Restart { .. }));
        assert_eq!(
            t.on_crash(t0 + S * 2),
            Decision::GiveUp { restarts_in_window: 2 }
        );
    }

    #[test]
    fn window_expiry_refills_the_budget() {
        let mut t = tracker(1);
        let t0 = Instant::now();
        assert!(matches!(t.on_crash(t0), Decision::Restart { .. }));
        assert!(matches!(t.on_crash(t0 + S), Decision::GiveUp { .. }));
        assert!(matches!(t.on_crash(t0 + S * 61), Decision::Restart { .. }));
    }

    #[test]
    fn a_long_healthy_run_resets_the_backoff() {
        let mut t = tracker(100);
        let t0 = Instant::now();
        for i in 0..4 {
            t.on_crash(t0 + S * i);
        }
        t.on_started(t0 + S * 4);
        assert_eq!(
            t.on_crash(t0 + S * 70),
            Decision::Restart { after: S },
            "backoff restarts at the base after a run longer than the window"
        );
    }

    #[test]
    fn reset_forgets_everything() {
        let mut t = tracker(1);
        let t0 = Instant::now();
        t.on_crash(t0);
        assert!(matches!(t.on_crash(t0 + S), Decision::GiveUp { .. }));
        t.reset();
        assert!(matches!(t.on_crash(t0 + S * 2), Decision::Restart { .. }));
    }
}
