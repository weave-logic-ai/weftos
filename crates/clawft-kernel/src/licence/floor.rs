//! The verifier's clock floor (ADR-106 section 4, "Clock").
//!
//! Per grant key: `floor = max(highest accepted issued_at, persisted local
//! now high-water mark)`. Expiry uses `max(now, floor)`, so a clock set back
//! cannot revive a grant. The mark never goes down. Its growth is capped at
//! 30 days past the newest accepted `issued_at`, so a forward clock jump
//! cannot push the floor arbitrarily far; undoing a jump that did land is the
//! Admin `reset_floor`.

use serde::{Deserialize, Serialize};

use super::FAR_FUTURE_CLAMP_SECS;

/// Floor state for one grant key.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FloorState {
    /// Highest `issued_at` among accepted grants (0 = none yet).
    pub max_issued: u64,
    /// Highest capped local `now` seen (the persisted high-water mark).
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

    /// Record the local clock: `hw = max(hw, min(now, max_issued + 30 d))`.
    /// Nothing is recorded while no grant has been accepted. Never lowers `hw`.
    pub fn observe(&mut self, now: u64) {
        if self.max_issued == 0 {
            return;
        }
        let cap = self.max_issued.saturating_add(FAR_FUTURE_CLAMP_SECS);
        self.hw = self.hw.max(now.min(cap));
    }

    /// Record an accepted grant's `issued_at`.
    pub fn note_issued(&mut self, issued_at: u64) {
        self.max_issued = self.max_issued.max(issued_at);
    }

    /// Operator reset: forget the high-water mark and restart it from `now`.
    pub fn reset(&mut self, now: u64) {
        self.hw = 0;
        self.observe(now);
    }
}
