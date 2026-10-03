//! The verifier's clock floor (ADR-106 section 4, "Clock").
//!
//! Per grant key: `floor = max(highest accepted issued_at, persisted local
//! now high-water mark)`. Expiry uses `max(now, floor)`, so a clock set back
//! cannot revive a grant. A floor more than 30 days ahead of the newest
//! accepted `issued_at` is far-future poisoning and is clamped.

use serde::{Deserialize, Serialize};

use super::FAR_FUTURE_CLAMP_SECS;

/// Floor state for one grant key.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FloorState {
    /// Highest `issued_at` among accepted grants (0 = none yet).
    pub max_issued: u64,
    /// Highest local `now` seen (the persisted high-water mark).
    pub hw: u64,
}

impl FloorState {
    /// The current floor.
    pub fn floor(&self) -> u64 {
        self.max_issued.max(self.hw)
    }

    /// The time to judge expiry by.
    pub fn effective_now(&self, now: u64) -> u64 {
        now.max(self.floor())
    }

    /// Record the local clock. Returns `Some((from, to))` when the stored
    /// high-water mark was far-future poisoning and was clamped: more than
    /// 30 days past the newest accepted `issued_at` *and* ahead of the clock
    /// now (a mark the running clock has caught up with is genuine).
    pub fn observe(&mut self, now: u64) -> Option<(u64, u64)> {
        let mut clamped = None;
        let limit = self.max_issued.saturating_add(FAR_FUTURE_CLAMP_SECS);
        if self.max_issued > 0 && self.hw > limit && self.hw > now {
            clamped = Some((self.hw, self.max_issued));
            self.hw = self.max_issued;
        }
        self.hw = self.hw.max(now);
        clamped
    }

    /// Record an accepted grant's `issued_at`.
    pub fn note_issued(&mut self, issued_at: u64) {
        self.max_issued = self.max_issued.max(issued_at);
    }

    /// Operator reset: forget the high-water mark and restart it at `now`.
    pub fn reset(&mut self, now: u64) {
        self.hw = now;
    }
}
