//! Artifact cache with pinned and evictable entries (ADR-099 section 6,
//! card mesh-placement-25).
//!
//! The cache accounts for verified artifacts held by an
//! [`ArtifactExchange`] against a byte budget. When the unpinned entries
//! push the total over budget, the **least recently used unpinned** entries
//! are evicted (their bytes freed, `artifact.evict` chained) until it fits.
//! A **pinned** entry is never evicted by the policy: that is how a node
//! keeps the packages and model weights it is running. Pins can push the
//! cache over budget; [`ArtifactCache::over_budget`] reports it.
//!
//! "Used" means admitted, fetched, or served to a peer, so artifacts the
//! swarm keeps asking this node for stay.

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex};

use clawft_types::placement::NodeFacts;

use crate::mesh_artifact::ArtifactExchange;
use crate::mesh_artifact_types::ArtifactId;
use crate::mesh_swarm_governance::set_held_capabilities;

/// What an entry is, for advertisement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ArtifactKind {
    /// Any artifact (package file, manifest).
    #[default]
    Generic,
    /// A model shard: also advertised as `model.present`.
    Model,
}

/// Cache budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheConfig {
    /// Bytes of verified artifacts to keep (pinned entries may exceed it).
    pub max_bytes: u64,
}

/// Why the cache refused something.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CacheError {
    /// Unknown or unverified artifact.
    #[error("artifact {0} is not held and verified")]
    NotHeld(String),
    /// Larger than the whole budget and not pinned: not kept.
    #[error("artifact {id} ({size} bytes) exceeds the cache budget of {budget} bytes")]
    TooLarge {
        /// Artifact.
        id: String,
        /// Its size.
        size: u64,
        /// The budget.
        budget: u64,
    },
    /// Pinned entries cannot be removed until unpinned.
    #[error("artifact {0} is pinned")]
    Pinned(String),
}

/// One cache entry as reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheEntryInfo {
    /// Artifact.
    pub id: ArtifactId,
    /// Whole-content hash.
    pub content_hash: [u8; 32],
    /// Size in bytes.
    pub size: u64,
    /// Protected from eviction.
    pub pinned: bool,
    /// Kind, for advertisement.
    pub kind: ArtifactKind,
}

struct Entry {
    content_hash: [u8; 32],
    size: u64,
    pinned: bool,
    kind: ArtifactKind,
    last_used: u64,
}

#[derive(Default)]
struct Inner {
    entries: BTreeMap<ArtifactId, Entry>,
    tick: u64,
    /// Content hashes to pin the moment they are admitted.
    pin_on_arrival: HashSet<[u8; 32]>,
}

impl Inner {
    fn next_tick(&mut self) -> u64 {
        self.tick += 1;
        self.tick
    }
    fn used(&self) -> u64 {
        self.entries.values().map(|e| e.size).sum()
    }
}

fn package_hashes(pkg: &crate::mesh_artifact_pkg::ExchangedPackage) -> Vec<[u8; 32]> {
    use crate::workload_pkg::codec::hex_decode_exact;
    std::iter::once(pkg.manifest_hash.as_str())
        .chain(pkg.verified.body.files().map(|f| f.blake3.as_str()))
        .filter_map(hex_decode_exact::<32>)
        .collect()
}

/// Budgeted, pin-aware view of the verified artifacts of one exchange.
pub struct ArtifactCache {
    ex: Arc<ArtifactExchange>,
    cfg: CacheConfig,
    inner: Mutex<Inner>,
}

impl ArtifactCache {
    /// Cache over `ex`. The exchange reports every artifact that becomes
    /// verified (seeded or fetched) to the cache, and the cache enforces
    /// the budget as they arrive. Artifacts already verified are admitted.
    pub fn new(ex: Arc<ArtifactExchange>, cfg: CacheConfig) -> Arc<Self> {
        let cache = Arc::new(Self {
            ex: ex.clone(),
            cfg,
            inner: Mutex::new(Inner::default()),
        });
        ex.swarm.attach_cache(&cache);
        for d in ex.descriptors.iter().map(|d| d.clone()).collect::<Vec<_>>() {
            let _ = cache.admit(&d.id());
        }
        cache
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The exchange this cache accounts.
    pub fn exchange(&self) -> &Arc<ArtifactExchange> {
        &self.ex
    }

    /// Account for verified artifact `id` and enforce the budget. Returns
    /// the artifacts evicted to make room. An unpinned artifact larger than
    /// the whole budget is evicted again and reported as an error.
    pub fn admit(&self, id: &ArtifactId) -> Result<Vec<ArtifactId>, CacheError> {
        let d = self
            .ex
            .descriptor(id)
            .ok_or_else(|| CacheError::NotHeld(id.to_string()))?;
        {
            let mut g = self.lock();
            let tick = g.next_tick();
            let pinned = g.pin_on_arrival.contains(&d.content_hash);
            g.entries
                .entry(*id)
                .and_modify(|e| e.last_used = tick)
                .or_insert(Entry {
                    content_hash: d.content_hash,
                    size: d.total_size,
                    pinned,
                    kind: ArtifactKind::Generic,
                    last_used: tick,
                });
        }
        let too_large = d.total_size > self.cfg.max_bytes && !self.is_pinned(id);
        if too_large {
            self.evict(id, "lru");
            return Err(CacheError::TooLarge {
                id: id.to_string(),
                size: d.total_size,
                budget: self.cfg.max_bytes,
            });
        }
        Ok(self.enforce_keeping(Some(id)))
    }

    /// Hook from the exchange: an artifact became verified.
    pub(crate) fn on_verified(&self, id: &ArtifactId) {
        let _ = self.admit(id);
    }

    /// Hook from the exchange: a download began. Its bytes count against the
    /// budget, so room is made now rather than after it lands.
    pub(crate) fn on_pending(&self) {
        self.enforce();
    }

    /// Hook from the exchange: an artifact was forgotten (evicted or revoked).
    pub(crate) fn on_forgotten(&self, id: &ArtifactId) {
        self.lock().entries.remove(id);
    }

    /// Mark `id` as just used.
    pub fn touch(&self, id: &ArtifactId) {
        let mut g = self.lock();
        let tick = g.next_tick();
        if let Some(e) = g.entries.get_mut(id) {
            e.last_used = tick;
        }
    }

    /// Protect `id` from eviction. False if it is not in the cache.
    pub fn pin(&self, id: &ArtifactId) -> bool {
        self.set_pinned(id, true)
    }

    /// Make `id` evictable again, then enforce the budget (it may go now).
    pub fn unpin(&self, id: &ArtifactId) -> bool {
        let known = self.set_pinned(id, false);
        if known {
            self.enforce();
        }
        known
    }

    fn set_pinned(&self, id: &ArtifactId, pinned: bool) -> bool {
        let mut g = self.lock();
        let tick = g.next_tick();
        match g.entries.get_mut(id) {
            Some(e) => {
                e.pinned = pinned;
                e.last_used = tick;
                true
            }
            None => false,
        }
    }

    /// Pin whatever has this content hash, now or the moment it arrives. A
    /// fetch admits an artifact as soon as it verifies, so a caller about to
    /// run a package pins its hashes first, or an over-budget cache could
    /// evict the artifact before the caller can pin it.
    pub fn pin_content(&self, content_hash: [u8; 32]) {
        let mut g = self.lock();
        g.pin_on_arrival.insert(content_hash);
        for e in g.entries.values_mut().filter(|e| e.content_hash == content_hash) {
            e.pinned = true;
        }
    }

    /// Undo [`Self::pin_content`] and make matching entries evictable again.
    pub fn unpin_content(&self, content_hash: &[u8; 32]) {
        {
            let mut g = self.lock();
            g.pin_on_arrival.remove(content_hash);
            for e in g.entries.values_mut().filter(|e| e.content_hash == *content_hash) {
                e.pinned = false;
            }
        }
        self.enforce();
    }

    /// Pin a package's manifest and every file it lists.
    pub fn pin_package(&self, pkg: &crate::mesh_artifact_pkg::ExchangedPackage) {
        for h in package_hashes(pkg) {
            self.pin_content(h);
        }
    }

    /// Make a package's manifest and files evictable again (it was removed).
    pub fn unpin_package(&self, pkg: &crate::mesh_artifact_pkg::ExchangedPackage) {
        for h in package_hashes(pkg) {
            self.unpin_content(&h);
        }
    }

    /// True if `id` is pinned.
    pub fn is_pinned(&self, id: &ArtifactId) -> bool {
        self.lock().entries.get(id).is_some_and(|e| e.pinned)
    }

    /// Set what `id` is (for `model.present`).
    pub fn set_kind(&self, id: &ArtifactId, kind: ArtifactKind) -> bool {
        match self.lock().entries.get_mut(id) {
            Some(e) => {
                e.kind = kind;
                true
            }
            None => false,
        }
    }

    /// Evict `id` now. Pinned entries are refused.
    pub fn remove(&self, id: &ArtifactId) -> Result<(), CacheError> {
        if self.is_pinned(id) {
            return Err(CacheError::Pinned(id.to_string()));
        }
        if !self.lock().entries.contains_key(id) {
            return Err(CacheError::NotHeld(id.to_string()));
        }
        self.evict(id, "manual");
        Ok(())
    }

    /// Evict least-recently-used unpinned entries until the total fits the
    /// budget. Returns the evicted ids, oldest first.
    pub fn enforce(&self) -> Vec<ArtifactId> {
        self.enforce_keeping(None)
    }

    /// As [`Self::enforce`], but `keep` (just admitted) is evicted only as a
    /// last resort, when nothing else unpinned is left to free.
    fn enforce_keeping(&self, keep: Option<&ArtifactId>) -> Vec<ArtifactId> {
        let mut evicted = Vec::new();
        loop {
            let victim = {
                let g = self.lock();
                if g.used() + self.ex.pending_bytes() <= self.cfg.max_bytes {
                    break;
                }
                let oldest = |skip: Option<&ArtifactId>| {
                    g.entries
                        .iter()
                        .filter(|(id, e)| !e.pinned && Some(*id) != skip)
                        .min_by_key(|(_, e)| e.last_used)
                        .map(|(id, _)| *id)
                };
                oldest(keep).or_else(|| keep.and_then(|k| oldest(None).filter(|v| v == k)))
            };
            let Some(id) = victim else { break };
            self.evict(&id, "lru");
            evicted.push(id);
        }
        evicted
    }

    fn evict(&self, id: &ArtifactId, reason: &str) {
        self.lock().entries.remove(id);
        self.ex.forget(id, reason);
    }

    /// Bytes accounted: verified entries plus downloads in progress.
    pub fn used_bytes(&self) -> u64 {
        self.lock().used() + self.ex.pending_bytes()
    }

    /// Bytes over the budget (only pins and downloads in progress can cause this).
    pub fn over_budget(&self) -> u64 {
        self.used_bytes().saturating_sub(self.cfg.max_bytes)
    }

    /// Entries, least recently used first.
    pub fn entries(&self) -> Vec<CacheEntryInfo> {
        let g = self.lock();
        let mut v: Vec<_> = g.entries.iter().collect();
        v.sort_by_key(|(_, e)| e.last_used);
        v.into_iter()
            .map(|(id, e)| CacheEntryInfo {
                id: *id,
                content_hash: e.content_hash,
                size: e.size,
                pinned: e.pinned,
                kind: e.kind,
            })
            .collect()
    }

    /// Merge what this node can serve into `facts` as capabilities
    /// (`store.artifact.*`, `model.present`), replacing any older ones.
    pub fn advertise(&self, facts: &mut NodeFacts) {
        let shards: HashSet<[u8; 32]> = self
            .lock()
            .entries
            .values()
            .filter(|e| e.kind == ArtifactKind::Model)
            .map(|e| e.content_hash)
            .collect();
        set_held_capabilities(facts, self.ex.held_capabilities(&shards));
    }
}

#[cfg(test)]
#[path = "mesh_swarm_cache_tests.rs"]
mod tests;
