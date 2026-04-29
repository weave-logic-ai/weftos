//! Cross-target `Instant` helpers that survive a fresh wasm time
//! origin.
//!
//! On `wasm32-unknown-unknown` (`web_time::Instant` is `performance.now()`)
//! a freshly-mounted page can hand back an `Instant::now()` whose
//! underlying high-res value is `< Duration` — so `now - some_duration`
//! panics with `overflow when subtracting duration from instant`.
//!
//! Native `std::time::Instant` doesn't have the same issue in
//! practice, but the same call site is built on both targets, so we
//! standardise on `web_time::Instant` and the `checked_sub` fallback
//! everywhere.
//!
//! Use [`epoch_minus`] when you want "an instant far enough in the
//! past that the next interval-elapsed check fires immediately, but
//! that doesn't crash on a cold-load wasm clock". Tracker:
//! WEFT-247.

use std::time::Duration;

use web_time::Instant;

/// Return `Instant::now() - dt` with an underflow-safe fallback.
///
/// On wasm a `performance.now()` reading just past page-load can be
/// numerically smaller than `dt`, which would panic in the unchecked
/// `Sub` impl. The fallback is the bare `Instant::now()` — meaning
/// the first interval check fires `dt` later than ideal, instead of
/// immediately. That's an acceptable trade for not crashing on cold
/// load.
///
/// # Examples
///
/// ```ignore
/// # use std::time::Duration;
/// # use clawft_gui_egui::wasm_time::epoch_minus;
/// // "an instant in the past so the first poll runs now":
/// let last_poll = epoch_minus(Duration::from_millis(800));
/// ```
#[inline]
pub fn epoch_minus(dt: Duration) -> Instant {
    let now = Instant::now();
    now.checked_sub(dt).unwrap_or(now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_minus_returns_instant_at_or_before_now() {
        // Native std `Instant` always supports the subtraction, so
        // `epoch_minus(D)` here lands `D` in the past.
        let dt = Duration::from_secs(1);
        let before = Instant::now();
        let got = epoch_minus(dt);
        let after = Instant::now();
        assert!(got <= after);
        // Either we successfully subtracted (got < before-ish) OR we
        // hit the fallback (got >= before). Both are acceptable —
        // the contract is "doesn't panic, doesn't outrun now".
        let _ = before;
    }

    #[test]
    fn epoch_minus_zero_is_now_ish() {
        let before = Instant::now();
        let got = epoch_minus(Duration::ZERO);
        let after = Instant::now();
        assert!(got >= before);
        assert!(got <= after);
    }

    #[test]
    fn epoch_minus_huge_duration_falls_back_safely() {
        // Saturating against a duration larger than any realistic
        // monotonic clock origin. Must not panic.
        let dt = Duration::from_secs(60 * 60 * 24 * 365 * 100); // 100 years
        let _ = epoch_minus(dt);
    }
}
