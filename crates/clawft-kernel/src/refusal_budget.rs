//! Bound on chain writes caused by unauthenticated requests.
//!
//! A request that fails verification (bad signature, unknown controller,
//! replayed nonce, expired) comes from someone this node has not
//! authenticated. Chaining every one would let any peer that can reach the
//! listener grow the node's chain without limit, so at most `per_window`
//! such refusals are chained per window. The rest are counted, and the
//! count rides on the next refusal that is chained (`suppressed`).
//! Refusals of authenticated requests are always chained.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Default: chained verify refusals per window.
pub const DEFAULT_PER_WINDOW: u32 = 16;
/// Default window.
pub const DEFAULT_WINDOW: Duration = Duration::from_secs(60);

#[derive(Debug)]
struct State {
    start: Instant,
    used: u32,
    suppressed: u64,
}

/// Fixed-window budget.
#[derive(Debug)]
pub struct RefusalBudget {
    per_window: u32,
    window: Duration,
    state: Mutex<State>,
}

impl Default for RefusalBudget {
    fn default() -> Self {
        Self::new(DEFAULT_PER_WINDOW, DEFAULT_WINDOW)
    }
}

impl RefusalBudget {
    /// `per_window` chain writes per `window`.
    pub fn new(per_window: u32, window: Duration) -> Self {
        Self {
            per_window,
            window,
            state: Mutex::new(State {
                start: Instant::now(),
                used: 0,
                suppressed: 0,
            }),
        }
    }

    /// `Some(suppressed since the last chained one)` if this refusal may be
    /// chained now, else `None` (counted as suppressed).
    pub fn take(&self) -> Option<u64> {
        let mut s = self.state.lock().ok()?;
        if s.start.elapsed() >= self.window {
            s.start = Instant::now();
            s.used = 0;
        }
        if s.used >= self.per_window {
            if s.suppressed == 0 {
                tracing::warn!("unauthenticated workload.ctl refusals over budget; not chaining");
            }
            s.suppressed = s.suppressed.saturating_add(1);
            return None;
        }
        s.used += 1;
        Some(std::mem::take(&mut s.suppressed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_per_window_and_the_suppressed_count_is_carried_forward() {
        let b = RefusalBudget::new(2, Duration::from_millis(60));
        assert_eq!(b.take(), Some(0));
        assert_eq!(b.take(), Some(0));
        for _ in 0..5 {
            assert_eq!(b.take(), None);
        }
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(b.take(), Some(5), "the next chained refusal reports 5");
        assert_eq!(b.take(), Some(0));
        assert_eq!(b.take(), None);
    }
}
