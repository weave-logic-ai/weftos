//! Lock-free holder for the current rule hash (ADR-103 A7).
//!
//! The chain calls its `rule_hash` provider while holding the chain lock,
//! so the provider must never block. [`RuleHashCell`] is the intended
//! source: writers publish with [`RuleHashCell::set`], the provider reads
//! with [`RuleHashCell::get`], and a read never takes a lock (a seqlock
//! over four atomic words; a reader only retries while a write is in
//! flight, which is a few instructions).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering, fence};

/// The current rule hash, readable without locking.
#[derive(Debug, Default)]
pub struct RuleHashCell {
    /// Odd while a write is in flight.
    seq: AtomicU64,
    present: AtomicBool,
    words: [AtomicU64; 4],
    /// Serializes writers only; readers never touch it.
    writers: Mutex<()>,
}

impl RuleHashCell {
    /// An empty cell (`get` returns `None`).
    pub fn new() -> Self {
        Self::default()
    }

    /// Publish a new value (or clear it).
    pub fn set(&self, hash: Option<[u8; 32]>) {
        let _w = self.writers.lock().unwrap_or_else(|e| e.into_inner());
        self.seq.fetch_add(1, Ordering::AcqRel); // odd: write in flight
        let h = hash.unwrap_or([0; 32]);
        for (i, w) in self.words.iter().enumerate() {
            let mut b = [0u8; 8];
            b.copy_from_slice(&h[i * 8..i * 8 + 8]);
            w.store(u64::from_le_bytes(b), Ordering::Relaxed);
        }
        self.present.store(hash.is_some(), Ordering::Relaxed);
        self.seq.fetch_add(1, Ordering::Release); // even: stable
    }

    /// The current value. Never blocks on a lock.
    pub fn get(&self) -> Option<[u8; 32]> {
        loop {
            let s1 = self.seq.load(Ordering::Acquire);
            if s1 & 1 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let mut out = [0u8; 32];
            for (i, w) in self.words.iter().enumerate() {
                out[i * 8..i * 8 + 8].copy_from_slice(&w.load(Ordering::Relaxed).to_le_bytes());
            }
            let present = self.present.load(Ordering::Relaxed);
            fence(Ordering::Acquire);
            if self.seq.load(Ordering::Relaxed) == s1 {
                return present.then_some(out);
            }
        }
    }

    /// A provider closure for `ChainManager::set_rule_hash_provider`.
    pub fn provider(self: &std::sync::Arc<Self>) -> crate::chain::RuleHashProvider {
        let cell = std::sync::Arc::clone(self);
        std::sync::Arc::new(move || cell.get())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_clear() {
        let c = RuleHashCell::new();
        assert_eq!(c.get(), None);
        c.set(Some([7; 32]));
        assert_eq!(c.get(), Some([7; 32]));
        c.set(None);
        assert_eq!(c.get(), None);
    }

    #[test]
    fn readers_never_see_a_torn_value() {
        let c = std::sync::Arc::new(RuleHashCell::new());
        let w = {
            let c = c.clone();
            std::thread::spawn(move || {
                for i in 0..20_000u32 {
                    c.set(Some([(i % 251) as u8; 32]));
                }
            })
        };
        for _ in 0..20_000 {
            if let Some(h) = c.get() {
                assert!(h.iter().all(|b| *b == h[0]), "torn read {h:?}");
            }
        }
        w.join().unwrap();
    }
}
