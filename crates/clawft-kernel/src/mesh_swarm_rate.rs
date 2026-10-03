//! Bandwidth caps for the artifact swarm (ADR-099 section 6, card
//! mesh-placement-25).
//!
//! [`RateLimiter`] paces byte transfers to a fixed rate. It is a pacing
//! reservation, not a bursty bucket: each [`RateLimiter::acquire`] books the
//! next slice of time after the previous booking and waits until that
//! slice ends, so `n` bytes never complete faster than `n / rate`. It is
//! driven by `tokio::time`, so tests can run under paused time and measure
//! throughput deterministically.

use std::sync::Mutex;
use std::time::Duration;

use tokio::time::Instant;

/// Paces byte transfers to `bytes_per_sec`.
#[derive(Debug)]
pub struct RateLimiter {
    bytes_per_sec: u64,
    next_free: Mutex<Option<Instant>>,
}

impl RateLimiter {
    /// Limiter at `bytes_per_sec` (clamped to at least 1).
    pub fn new(bytes_per_sec: u64) -> Self {
        Self {
            bytes_per_sec: bytes_per_sec.max(1),
            next_free: Mutex::new(None),
        }
    }

    /// The configured rate.
    pub fn bytes_per_sec(&self) -> u64 {
        self.bytes_per_sec
    }

    /// Time `n` bytes take at the configured rate.
    pub fn cost(&self, n: u64) -> Duration {
        Duration::from_secs_f64(n as f64 / self.bytes_per_sec as f64)
    }

    /// Reserve `n` bytes and wait until their slice has elapsed.
    pub async fn acquire(&self, n: u64) {
        let until = {
            let mut next = self.next_free.lock().unwrap_or_else(|p| p.into_inner());
            let start = next.map_or_else(Instant::now, |t| t.max(Instant::now()));
            let end = start + self.cost(n);
            *next = Some(end);
            end
        };
        tokio::time::sleep_until(until).await;
    }
}

/// A node's upload and download caps (either may be unlimited).
#[derive(Debug, Default)]
pub struct Bandwidth {
    /// Cap on bytes sent to peers.
    pub up: Option<RateLimiter>,
    /// Cap on bytes received from peers.
    pub down: Option<RateLimiter>,
}

impl Bandwidth {
    /// Caps from bytes-per-second settings (`None` = unlimited).
    pub fn new(up: Option<u64>, down: Option<u64>) -> Self {
        Self {
            up: up.map(RateLimiter::new),
            down: down.map(RateLimiter::new),
        }
    }

    /// Wait for `n` bytes of upload budget.
    pub async fn upload(&self, n: usize) {
        if let Some(l) = &self.up {
            l.acquire(n as u64).await;
        }
    }

    /// Wait for `n` bytes of download budget.
    pub async fn download(&self, n: usize) {
        if let Some(l) = &self.down {
            l.acquire(n as u64).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn transfers_never_beat_the_rate_and_concurrent_callers_share_it() {
        let l = RateLimiter::new(1_000);
        let t0 = Instant::now();
        l.acquire(2_000).await;
        assert_eq!(t0.elapsed(), Duration::from_secs(2));
        // Two callers at once still share one 1000 B/s budget.
        let t1 = Instant::now();
        tokio::join!(l.acquire(500), l.acquire(500), l.acquire(1_000));
        assert_eq!(t1.elapsed(), Duration::from_secs(2));
    }

    #[tokio::test(start_paused = true)]
    async fn unlimited_does_not_wait_and_idle_time_is_not_banked() {
        let b = Bandwidth::new(None, Some(100));
        let t0 = Instant::now();
        b.upload(1 << 30).await;
        assert_eq!(t0.elapsed(), Duration::ZERO);
        tokio::time::sleep(Duration::from_secs(60)).await;
        let t1 = Instant::now();
        b.download(100).await;
        assert_eq!(t1.elapsed(), Duration::from_secs(1), "no burst credit from idling");
    }
}
