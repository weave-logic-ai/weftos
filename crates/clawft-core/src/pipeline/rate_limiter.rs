//! Sliding-window rate limiter with per-user and global limits.
//!
//! Implements a two-stage rate limiting strategy:
//! 1. **Global rate limit** -- caps total requests per window across all senders.
//! 2. **Per-user rate limit** -- caps requests per window for each `sender_id`.
//!
//! Thread-safe via `RwLock<HashMap>` for per-sender windows and `AtomicU64`
//! for the global counter. Designed for concurrent access from `tokio` tasks.
//!
//! # Algorithm
//!
//! Uses a sliding window counter: each request's `Instant` is recorded in a
//! per-sender `Vec<Instant>`. On each `check()`, expired timestamps (older
//! than `window_seconds`) are pruned. If the remaining count exceeds the
//! limit, the request is rejected.
//!
//! # LRU Eviction
//!
//! When the number of tracked senders exceeds `max_tracked_users`, the sender
//! with the oldest last-request timestamp is evicted. This bounds memory usage
//! to approximately `max_tracked_users * 1 KB` at peak.

use std::collections::HashMap;
use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::pipeline::traits::RateLimitable;

// ── SlidingWindow ────────────────────────────────────────────────────────

/// Per-sender sliding window state.
///
/// Stores timestamps of recent requests and the last access order for
/// LRU eviction. Timestamps are kept sorted (oldest first) by virtue
/// of being appended monotonically.
struct SlidingWindow {
    /// Timestamps of requests within the window, oldest first.
    timestamps: Vec<Instant>,
    /// Monotonic counter value at last access (for LRU eviction).
    last_access: u64,
}

// ── RateLimiter ──────────────────────────────────────────────────────────

/// A sliding-window rate limiter keyed by sender ID, with an optional
/// global rate limit that applies across all senders.
///
/// Thread-safe via `RwLock` (per-sender map) and `AtomicU64` (global counter).
/// Each sender has an independent sliding window of timestamps. When the
/// window is full, the request is rejected. A global rate limit, if
/// configured, is checked BEFORE per-user limits to prevent aggregate abuse
/// from many distinct sender_ids.
pub struct RateLimiter {
    /// Per-sender sliding window entries. Key is `sender_id`.
    windows: RwLock<HashMap<String, SlidingWindow>>,
    /// Window size in seconds. Stored for future config serialization
    /// when the rate limiter is persisted or exposed via admin API.
    /// TODO(Element-09): Expose via admin metrics endpoint (L2).
    #[allow(dead_code)]
    window_seconds: u32,
    /// Window size as a `Duration` (precomputed from `window_seconds`).
    window_duration: Duration,
    /// Global request counter for the current window.
    global_counter: AtomicU64,
    /// Global rate limit (requests per window). 0 = unlimited.
    global_rate_limit: u32,
    /// Start of the current global rate limit window.
    /// Protected by `RwLock` so that read-check-reset is atomic.
    global_reset: RwLock<Instant>,
    /// Maximum number of tracked users (LRU eviction threshold).
    max_tracked_users: usize,
    /// Monotonic counter for LRU access ordering.
    access_counter: AtomicU64,
}

impl RateLimiter {
    /// Create a new rate limiter.
    ///
    /// # Arguments
    /// - `window_seconds`: Duration of the sliding window. Use 60 for
    ///   requests-per-minute semantics.
    /// - `global_rate_limit`: Maximum requests per window across ALL senders.
    ///   0 = unlimited (no global cap).
    pub fn new(window_seconds: u32, global_rate_limit: u32) -> Self {
        Self {
            windows: RwLock::new(HashMap::new()),
            window_seconds,
            window_duration: Duration::from_secs(u64::from(window_seconds)),
            global_counter: AtomicU64::new(0),
            global_rate_limit,
            global_reset: RwLock::new(Instant::now()),
            max_tracked_users: 10_000,
            access_counter: AtomicU64::new(0),
        }
    }

    /// Builder method to set the maximum number of tracked users.
    ///
    /// When the number of tracked senders exceeds this threshold, the
    /// sender with the oldest last-request timestamp is evicted.
    /// Default: 10,000.
    pub fn with_max_tracked_users(mut self, max: usize) -> Self {
        self.max_tracked_users = max;
        self
    }

    /// Check if a request from `sender_id` is allowed under the given `limit`.
    ///
    /// Performs a two-stage check:
    /// 1. **Global rate limit** -- checked first via [`check_global()`]. If the
    ///    global limit is exceeded, the request is rejected immediately.
    /// 2. **Per-user rate limit** -- checked second via the sliding window for
    ///    this `sender_id`.
    ///
    /// Returns `true` if the request is allowed, `false` if rate-limited.
    ///
    /// # Arguments
    /// - `sender_id`: Unique identifier for the sender. Empty string is valid
    ///   and treated as a distinct sender key.
    /// - `limit`: Maximum requests per window for this sender. `0` means
    ///   unlimited per-user (global limit still applies).
    pub fn check(&self, sender_id: &str, limit: u32) -> bool {
        // Stage 1: Check global rate limit BEFORE per-user limit.
        if !self.check_global() {
            return false;
        }

        // Stage 2: Per-user limit. 0 = unlimited per-user, always allow.
        if limit == 0 {
            return true;
        }

        let now = Instant::now();
        let order = self.access_counter.fetch_add(1, Ordering::Relaxed);

        // Acquire write lock to get-or-insert and mutate the entry.
        let mut windows = self.windows.write().unwrap();
        let entry = windows
            .entry(sender_id.to_string())
            .or_insert_with(|| SlidingWindow {
                timestamps: Vec::new(),
                last_access: order,
            });

        // Update LRU access order.
        entry.last_access = order;

        // Purge expired timestamps.
        // Because timestamps are appended monotonically, we can drain
        // from the front until we find one within the window.
        entry
            .timestamps
            .retain(|ts| now.duration_since(*ts) < self.window_duration);

        // Check if under the limit.
        if entry.timestamps.len() >= limit as usize {
            return false;
        }

        entry.timestamps.push(now);

        // Check if eviction is needed (after releasing write lock to avoid
        // holding it longer than necessary during the eviction scan).
        let needs_eviction = windows.len() > self.max_tracked_users;
        if needs_eviction {
            self.evict_oldest(&mut windows);
        }

        true
    }

    /// Check global rate limit only.
    ///
    /// Returns `true` if allowed, `false` if global limit exceeded.
    /// When `global_rate_limit` is 0, always returns `true` (unlimited).
    pub fn check_global(&self) -> bool {
        // 0 = no global limit.
        if self.global_rate_limit == 0 {
            return true;
        }

        let now = Instant::now();

        // Check if the current global window has expired; if so, reset.
        {
            let mut window_start = self.global_reset.write().unwrap();
            if now.duration_since(*window_start) >= self.window_duration {
                // Window expired: reset counter and start a new window.
                self.global_counter.store(0, Ordering::Relaxed);
                *window_start = now;
            }
        }

        // Atomically increment and check against limit.
        // `fetch_add` returns the previous value; if it was already at or
        // above the limit, the request is rejected.
        let prev = self.global_counter.fetch_add(1, Ordering::Relaxed);
        if prev >= u64::from(self.global_rate_limit) {
            // Over limit -- undo the increment so the counter does not
            // drift unboundedly when many requests are rejected.
            self.global_counter.fetch_sub(1, Ordering::Relaxed);
            return false;
        }

        true
    }

    /// Get current request count for a user in the window.
    ///
    /// Returns 0 if the sender has no tracked window. Expired timestamps
    /// are filtered (but not pruned) to give an accurate count without
    /// requiring a write lock.
    pub fn get_count(&self, sender_id: &str) -> u32 {
        let now = Instant::now();
        let windows = self.windows.read().unwrap();
        windows.get(sender_id).map_or(0, |entry| {
            entry
                .timestamps
                .iter()
                .filter(|ts| now.duration_since(**ts) < self.window_duration)
                .count() as u32
        })
    }

    /// Get the number of tracked senders.
    pub fn tracked_senders(&self) -> usize {
        self.windows.read().unwrap().len()
    }

    /// Get the current global request count within the active window.
    pub fn global_request_count(&self) -> u64 {
        self.global_counter.load(Ordering::Relaxed)
    }

    /// Get the configured global rate limit. 0 = unlimited.
    pub fn global_rate_limit(&self) -> u32 {
        self.global_rate_limit
    }

    /// Get the configured window duration.
    pub fn window_duration(&self) -> Duration {
        self.window_duration
    }

    /// Remove all tracked entries and reset global counter.
    ///
    /// Used for testing and configuration reloads.
    pub fn clear(&self) {
        let mut windows = self.windows.write().unwrap();
        windows.clear();
        self.access_counter.store(0, Ordering::Relaxed);
        self.global_counter.store(0, Ordering::Relaxed);
        let mut reset = self.global_reset.write().unwrap();
        *reset = Instant::now();
    }

    /// Evict the oldest-accessed entry from the map.
    ///
    /// Called when `windows.len() > max_tracked_users`. Scans for the
    /// entry with the lowest `last_access` counter and removes it.
    fn evict_oldest(&self, windows: &mut HashMap<String, SlidingWindow>) {
        if windows.len() <= self.max_tracked_users {
            return;
        }

        let mut oldest_key: Option<String> = None;
        let mut oldest_access = u64::MAX;

        for (key, entry) in windows.iter() {
            if entry.last_access < oldest_access {
                oldest_access = entry.last_access;
                oldest_key = Some(key.clone());
            }
        }

        if let Some(key) = oldest_key {
            windows.remove(&key);
        }
    }

    /// Evict oldest entries when max_tracked_users is exceeded.
    ///
    /// This is the public-facing eviction trigger, called internally
    /// but also available for external eviction triggers.
    /// TODO(Element-09): Used by admin maintenance endpoint (L2).
    #[allow(dead_code)]
    fn evict_if_needed(&self) {
        let mut windows = self.windows.write().unwrap();
        self.evict_oldest(&mut windows);
    }
}

// ── RateLimitable Trait Implementation ───────────────────────────────────

/// Implement the Phase C trait so that `TieredRouter` can use `RateLimiter`
/// as `Arc<dyn RateLimitable + Send + Sync>`.
///
/// Delegates directly to [`RateLimiter::check()`], which performs both
/// the global rate limit check and the per-user sliding window check.
impl RateLimitable for RateLimiter {
    fn check(&self, sender_id: &str, limit: u32) -> bool {
        RateLimiter::check(self, sender_id, limit)
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    // --- Test 1: Basic allow under limit ---
    #[test]
    fn test_allows_under_limit() {
        let limiter = RateLimiter::new(60, 0);
        // 10 requests with limit of 10 should all pass.
        for _ in 0..10 {
            assert!(limiter.check("user_1", 10));
        }
    }

    // --- Test 2: Reject at limit ---
    #[test]
    fn test_rejects_at_limit() {
        let limiter = RateLimiter::new(60, 0);
        // Fill the window.
        for _ in 0..5 {
            assert!(limiter.check("user_1", 5));
        }
        // 6th request should be rejected.
        assert!(!limiter.check("user_1", 5));
    }

    // --- Test 3: Window expiry allows new requests ---
    #[test]
    fn test_window_expiry() {
        // Use a 1-second window for fast testing.
        let limiter = RateLimiter::new(1, 0);
        // Fill the window.
        for _ in 0..3 {
            assert!(limiter.check("user_1", 3));
        }
        assert!(!limiter.check("user_1", 3));

        // Wait for window to expire.
        thread::sleep(Duration::from_millis(1100));

        // Should be allowed again.
        assert!(limiter.check("user_1", 3));
    }

    // --- Test 4: Global rate limit blocks ---
    #[test]
    fn test_global_rate_limit_rejects() {
        // Global limit of 5 requests per window, no per-user limit.
        let limiter = RateLimiter::new(60, 5);
        // 5 requests from different users should all pass.
        for i in 0..5 {
            let sender = format!("user_{}", i);
            assert!(limiter.check(&sender, 0), "request {} should be allowed", i);
        }
        // 6th request from a new user should be rejected (global limit hit).
        assert!(!limiter.check("user_new", 0));
        assert_eq!(limiter.global_request_count(), 5);
    }

    // --- Test 5: Global rate limit resets after window ---
    #[test]
    fn test_global_window_resets() {
        // 1-second window, global limit of 2.
        let limiter = RateLimiter::new(1, 2);
        assert!(limiter.check("user_1", 0));
        assert!(limiter.check("user_2", 0));
        assert!(!limiter.check("user_3", 0)); // global limit hit

        // Wait for window to expire.
        thread::sleep(Duration::from_millis(1100));

        // Global counter should reset; new requests should be allowed.
        assert!(limiter.check("user_4", 0));
        assert_eq!(limiter.global_request_count(), 1);
    }

    // --- Test 6: Zero per-user limit means unlimited ---
    #[test]
    fn test_unlimited_per_user() {
        let limiter = RateLimiter::new(60, 0);
        for _ in 0..1000 {
            assert!(limiter.check("user_1", 0));
        }
        // Should not even create an entry (limit=0 short-circuits before map access).
        assert_eq!(limiter.tracked_senders(), 0);
    }

    // --- Test 7: Zero global limit means no global cap ---
    #[test]
    fn test_global_limit_zero_is_unlimited() {
        let limiter = RateLimiter::new(60, 0);
        // Should never hit a global limit.
        for i in 0..1000 {
            let sender = format!("user_{}", i);
            assert!(limiter.check(&sender, 0));
        }
    }

    // --- Test 8: Multiple users have independent limits ---
    #[test]
    fn test_independent_senders() {
        let limiter = RateLimiter::new(60, 0);
        // Fill user_1's window.
        for _ in 0..5 {
            limiter.check("user_1", 5);
        }
        assert!(!limiter.check("user_1", 5));
        // user_2 should be unaffected.
        assert!(limiter.check("user_2", 5));
    }

    // --- Test 9: LRU eviction works ---
    #[test]
    fn test_lru_eviction() {
        let limiter = RateLimiter::new(60, 0).with_max_tracked_users(3);
        // Add 4 senders to a limiter with max_tracked_users=3.
        limiter.check("user_a", 10);
        limiter.check("user_b", 10);
        limiter.check("user_c", 10);
        limiter.check("user_d", 10); // should trigger eviction of user_a

        // user_a was evicted (LRU).
        assert!(limiter.tracked_senders() <= 3);
    }

    // --- Test 10: get_count accuracy ---
    #[test]
    fn test_get_count() {
        let limiter = RateLimiter::new(60, 0);
        assert_eq!(limiter.get_count("user_1"), 0);

        limiter.check("user_1", 10);
        limiter.check("user_1", 10);
        limiter.check("user_1", 10);

        assert_eq!(limiter.get_count("user_1"), 3);
    }

    // --- Test 11: Concurrent access safety ---
    #[test]
    fn test_concurrent_access() {
        let limiter = Arc::new(RateLimiter::new(60, 0));
        let mut handles = vec![];

        for i in 0..10 {
            let limiter = Arc::clone(&limiter);
            handles.push(thread::spawn(move || {
                let sender = format!("user_{}", i);
                for _ in 0..100 {
                    limiter.check(&sender, 200);
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // All 10 senders should be tracked.
        assert_eq!(limiter.tracked_senders(), 10);
    }

    // --- Test 12: Empty sender_id handling ---
    #[test]
    fn test_empty_sender_id() {
        let limiter = RateLimiter::new(60, 0);
        assert!(limiter.check("", 2));
        assert!(limiter.check("", 2));
        assert!(!limiter.check("", 2));
    }

    // --- Test 13: Global limit checked before per-user limit ---
    #[test]
    fn test_global_limit_before_per_user() {
        // Global limit of 3, per-user limit of 100.
        let limiter = RateLimiter::new(60, 3);
        assert!(limiter.check("user_1", 100));
        assert!(limiter.check("user_2", 100));
        assert!(limiter.check("user_3", 100));
        // 4th request: user_4 has plenty of per-user budget but global is exhausted.
        assert!(!limiter.check("user_4", 100));
    }

    // --- Test 14: Clear resets all state ---
    #[test]
    fn test_clear() {
        let limiter = RateLimiter::new(60, 100);
        for _ in 0..5 {
            limiter.check("user_1", 5);
        }
        assert!(!limiter.check("user_1", 5));
        assert!(limiter.global_request_count() > 0);

        limiter.clear();

        // After clear, per-user and global state should be reset.
        assert!(limiter.check("user_1", 5));
        assert_eq!(limiter.tracked_senders(), 1);
        // Global counter was reset by clear(); the single check above set it to 1.
        assert_eq!(limiter.global_request_count(), 1);
    }

    // --- Test 15: RateLimitable trait delegation ---
    #[test]
    fn test_rate_limitable_trait_impl() {
        let limiter = RateLimiter::new(60, 0);
        // Call through the trait interface.
        let trait_obj: &dyn RateLimitable = &limiter;
        assert!(trait_obj.check("user_1", 2));
        assert!(trait_obj.check("user_1", 2));
        assert!(!trait_obj.check("user_1", 2));
    }

    // --- Test 16: Different limits for same sender across calls ---
    #[test]
    fn test_different_limits_per_call() {
        let limiter = RateLimiter::new(60, 0);
        // First call with limit=5: allowed.
        assert!(limiter.check("user_1", 5));
        assert!(limiter.check("user_1", 5));
        // Now 2 requests in window. Check with limit=2: should reject.
        assert!(!limiter.check("user_1", 2));
        // Check with limit=5: should still allow (only 2 in window, rejected
        // check did not add a timestamp).
        assert!(limiter.check("user_1", 5));
    }

    // --- Test 17: Global limit with concurrent threads ---
    #[test]
    fn test_global_limit_concurrent() {
        // Global limit of 50, many threads sending requests.
        let limiter = Arc::new(RateLimiter::new(60, 50));
        let mut handles = vec![];

        for i in 0..10 {
            let limiter = Arc::clone(&limiter);
            handles.push(thread::spawn(move || {
                let sender = format!("user_{}", i);
                let mut allowed = 0u32;
                for _ in 0..20 {
                    if limiter.check(&sender, 100) {
                        allowed += 1;
                    }
                }
                allowed
            }));
        }

        let total_allowed: u32 = handles.into_iter().map(|h| h.join().unwrap()).sum();

        // Total allowed should not exceed global limit of 50.
        assert!(
            total_allowed <= 50,
            "total_allowed={} exceeded global limit 50",
            total_allowed
        );
    }

    // --- Test 18: LRU evicts correct (oldest) entry ---
    #[test]
    fn test_lru_evicts_oldest() {
        let limiter = RateLimiter::new(60, 0).with_max_tracked_users(2);

        // user_a is accessed first (oldest).
        limiter.check("user_a", 10);
        // user_b is accessed second.
        limiter.check("user_b", 10);
        // user_c triggers eviction -- user_a should be evicted.
        limiter.check("user_c", 10);

        assert_eq!(limiter.tracked_senders(), 2);
        // user_a should have been evicted, so get_count returns 0.
        assert_eq!(limiter.get_count("user_a"), 0);
        // user_b and user_c should still be tracked.
        assert_eq!(limiter.get_count("user_b"), 1);
        assert_eq!(limiter.get_count("user_c"), 1);
    }

    // --- Test 19: window_seconds=0 means all timestamps expire immediately ---
    #[test]
    fn test_zero_window_allows_all() {
        let limiter = RateLimiter::new(0, 0);
        // With a zero-duration window, all previous timestamps are expired
        // by the time retain() runs (unless they are the exact same Instant,
        // which is possible). Regardless, the practical effect is that a
        // limit of 1 allows nearly all requests.
        for _ in 0..100 {
            // limit=1 with zero window: the previous timestamp is either
            // expired or at the same Instant. In practice this allows all.
            limiter.check("user_1", 1);
        }
        // No assertion on exact count; just verify no panics.
    }

    // --- Test 20: with_max_tracked_users builder ---
    #[test]
    fn test_builder_max_tracked_users() {
        let limiter = RateLimiter::new(60, 0).with_max_tracked_users(5);
        for i in 0..10 {
            let sender = format!("user_{}", i);
            limiter.check(&sender, 10);
        }
        assert!(limiter.tracked_senders() <= 5);
    }

    // --- Test 21: Rejected request does not add timestamp ---
    #[test]
    fn test_rejected_request_no_timestamp() {
        let limiter = RateLimiter::new(60, 0);
        // Fill to limit.
        assert!(limiter.check("user_1", 2));
        assert!(limiter.check("user_1", 2));
        assert_eq!(limiter.get_count("user_1"), 2);

        // Rejected -- should NOT add a timestamp.
        assert!(!limiter.check("user_1", 2));
        assert_eq!(limiter.get_count("user_1"), 2);
    }
}
