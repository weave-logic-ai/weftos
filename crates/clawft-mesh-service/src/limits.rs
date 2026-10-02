//! Per-principal connection and registration limits (plan 2 S): 64
//! connections and 5 registrations per minute per principal, plus a global
//! connection cap so many principals cannot exhaust file descriptors.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clawft_mesh_local::Principal;

/// Concurrent connections allowed per principal.
pub const MAX_CONNS_PER_PRINCIPAL: usize = 64;
/// Concurrent connections allowed in total.
pub const MAX_CONNS_TOTAL: usize = 512;
/// Register attempts allowed per principal per window.
pub const MAX_REGISTERS_PER_WINDOW: usize = 5;
/// The register-rate window.
pub const REGISTER_WINDOW: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy)]
pub struct LimitConfig {
    pub per_principal: usize,
    pub total: usize,
    pub registers: usize,
    pub window: Duration,
}

impl Default for LimitConfig {
    fn default() -> Self {
        Self {
            per_principal: MAX_CONNS_PER_PRINCIPAL,
            total: MAX_CONNS_TOTAL,
            registers: MAX_REGISTERS_PER_WINDOW,
            window: REGISTER_WINDOW,
        }
    }
}

#[derive(Default)]
struct Inner {
    conns: HashMap<Principal, usize>,
    total: usize,
    registers: HashMap<Principal, VecDeque<Instant>>,
}

/// Shared limiter state.
pub struct Limiter {
    cfg: LimitConfig,
    inner: Mutex<Inner>,
}

/// Releases a connection slot when dropped.
pub struct ConnGuard {
    limiter: Arc<Limiter>,
    principal: Principal,
}

impl Drop for ConnGuard {
    fn drop(&mut self) {
        let mut g = self.limiter.inner.lock().expect("limiter lock");
        g.total = g.total.saturating_sub(1);
        if let Some(n) = g.conns.get_mut(&self.principal) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                g.conns.remove(&self.principal);
            }
        }
    }
}

impl Limiter {
    pub fn new(cfg: LimitConfig) -> Arc<Self> {
        Arc::new(Self { cfg, inner: Mutex::new(Inner::default()) })
    }

    /// Take a connection slot, or `None` when the principal or the service is full.
    pub fn acquire_conn(self: &Arc<Self>, p: &Principal) -> Option<ConnGuard> {
        let mut g = self.inner.lock().expect("limiter lock");
        if g.total >= self.cfg.total || g.conns.get(p).copied().unwrap_or(0) >= self.cfg.per_principal {
            return None;
        }
        g.total += 1;
        *g.conns.entry(p.clone()).or_insert(0) += 1;
        drop(g);
        Some(ConnGuard { limiter: Arc::clone(self), principal: p.clone() })
    }

    /// Count a register attempt; false once the principal is over its rate.
    /// Failed attempts count, so a guessing client cannot probe indefinitely.
    pub fn allow_register(&self, p: &Principal) -> bool {
        self.allow_register_at(p, Instant::now())
    }

    pub(crate) fn allow_register_at(&self, p: &Principal, now: Instant) -> bool {
        let mut g = self.inner.lock().expect("limiter lock");
        let q = g.registers.entry(p.clone()).or_default();
        while q.front().is_some_and(|t| now.duration_since(*t) >= self.cfg.window) {
            q.pop_front();
        }
        if q.len() >= self.cfg.registers {
            return false;
        }
        q.push_back(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(n: u32) -> Principal {
        Principal::Uid(n)
    }

    #[test]
    fn connection_slots_are_per_principal_and_released() {
        let l = Limiter::new(LimitConfig { per_principal: 2, total: 10, ..Default::default() });
        let a = l.acquire_conn(&p(1)).unwrap();
        let _b = l.acquire_conn(&p(1)).unwrap();
        assert!(l.acquire_conn(&p(1)).is_none());
        assert!(l.acquire_conn(&p(2)).is_some());
        drop(a);
        assert!(l.acquire_conn(&p(1)).is_some());
    }

    #[test]
    fn total_cap_applies_across_principals() {
        let l = Limiter::new(LimitConfig { per_principal: 5, total: 2, ..Default::default() });
        let _a = l.acquire_conn(&p(1)).unwrap();
        let _b = l.acquire_conn(&p(2)).unwrap();
        assert!(l.acquire_conn(&p(3)).is_none());
    }

    #[test]
    fn five_registers_per_minute_then_window_slides() {
        let l = Limiter::new(LimitConfig::default());
        let t0 = Instant::now();
        for _ in 0..5 {
            assert!(l.allow_register_at(&p(1), t0));
        }
        assert!(!l.allow_register_at(&p(1), t0 + Duration::from_secs(59)));
        assert!(l.allow_register_at(&p(2), t0), "other principals are independent");
        assert!(l.allow_register_at(&p(1), t0 + Duration::from_secs(61)));
    }
}
