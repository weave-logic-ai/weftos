//! Rate windows, the in-flight permit and the byte-rate throttle.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

/// A sliding 60 s window of event times (unix seconds).
#[derive(Debug, Default)]
pub struct RateWindow {
    events: VecDeque<u64>,
}

impl RateWindow {
    /// Count one event at `now` if fewer than `max` happened in the last 60 s.
    pub fn try_acquire(&mut self, now: u64, max: u32) -> bool {
        while self.events.front().is_some_and(|t| t.saturating_add(60) <= now) {
            self.events.pop_front();
        }
        // A clock set back must not freeze the window.
        while self.events.back().is_some_and(|t| *t > now.saturating_add(60)) {
            self.events.pop_back();
        }
        if self.events.len() as u32 >= max {
            return false;
        }
        self.events.push_back(now);
        true
    }
}

/// A counting permit: at most `max` holders at once.
#[derive(Debug, Clone)]
pub struct Permits {
    used: Arc<AtomicU32>,
    max: u32,
}

/// Held while a checkout or a transfer is in flight; released on drop.
#[derive(Debug)]
pub struct Permit(Arc<AtomicU32>);

impl Permits {
    /// A pool of `max` permits.
    pub fn new(max: u32) -> Self {
        Self { used: Arc::new(AtomicU32::new(0)), max }
    }

    /// Take a permit, or `None` when all are held.
    pub fn try_acquire(&self) -> Option<Permit> {
        let mut cur = self.used.load(Ordering::Acquire);
        loop {
            if cur >= self.max {
                return None;
            }
            match self.used.compare_exchange(cur, cur + 1, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return Some(Permit(self.used.clone())),
                Err(v) => cur = v,
            }
        }
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// How long to sleep so that `sent` bytes since `start_elapsed_ms` stay
/// within `rate` bytes per second. Zero when already within budget.
pub fn throttle_sleep_ms(sent: u64, elapsed_ms: u64, rate: u64) -> u64 {
    if rate == 0 {
        return 0;
    }
    let due_ms = sent.saturating_mul(1000) / rate;
    due_ms.saturating_sub(elapsed_ms)
}
