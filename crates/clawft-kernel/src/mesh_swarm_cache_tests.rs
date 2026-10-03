//! Cache policy: LRU eviction, pins, budget, and what eviction frees.

use super::*;
use crate::artifact_store::ArtifactStore;
use crate::chain::{ChainManager, EVENT_KIND_ARTIFACT_EVICT};
use crate::mesh_artifact_types::ExchangeConfig;

fn exchange() -> (Arc<ArtifactExchange>, Arc<ChainManager>) {
    let chain = Arc::new(ChainManager::new(0, 1000));
    let cfg = ExchangeConfig {
        piece_size: 1024,
        ..Default::default()
    };
    let mut ex = ArtifactExchange::new("n1", Arc::new(ArtifactStore::new_memory()), cfg).unwrap();
    ex.set_chain_manager(chain.clone());
    (Arc::new(ex), chain)
}

/// `n` distinct 1000-byte artifacts.
fn blob(n: u8) -> Vec<u8> {
    (0..1000u32).map(|i| (i as u8).wrapping_add(n.wrapping_mul(37))).collect()
}

fn evictions(chain: &ChainManager) -> Vec<serde_json::Value> {
    chain
        .tail(chain.len())
        .into_iter()
        .filter(|e| e.kind == EVENT_KIND_ARTIFACT_EVICT)
        .map(|e| e.payload.unwrap())
        .collect()
}

#[test]
fn least_recently_used_unpinned_entries_go_first() {
    let (ex, chain) = exchange();
    let cache = ArtifactCache::new(ex.clone(), CacheConfig { max_bytes: 3000 });
    let ids: Vec<_> = (0..3).map(|n| ex.seed_bytes(&blob(n)).unwrap().id()).collect();
    assert_eq!(cache.used_bytes(), 3000);
    // Use the oldest, so the middle one is now least recently used.
    cache.touch(&ids[0]);
    let fourth = ex.seed_bytes(&blob(3)).unwrap().id();
    assert_eq!(cache.used_bytes(), 3000);
    assert!(ex.is_verified(&ids[0]) && ex.is_verified(&ids[2]) && ex.is_verified(&fourth));
    assert!(!ex.is_verified(&ids[1]), "LRU entry evicted");
    // The evicted artifact's bytes are gone from the store, the kept ones are not.
    assert!(ex.read_all(&ids[0]).is_ok());
    let ev = evictions(&chain);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0]["reason"], "lru");
    assert_eq!(ev[0]["artifact_id"], ids[1].to_string());
    assert!(ev[0]["bytes_freed"].as_u64().unwrap() >= 1000);
}

#[test]
fn a_pinned_entry_is_never_evicted_even_over_budget() {
    let (ex, chain) = exchange();
    let cache = ArtifactCache::new(ex.clone(), CacheConfig { max_bytes: 2000 });
    let a = ex.seed_bytes(&blob(0)).unwrap().id();
    assert!(cache.pin(&a));
    let b = ex.seed_bytes(&blob(1)).unwrap().id();
    assert!(cache.pin(&b));
    // Two pinned entries fill the budget; a third unpinned one cannot stay.
    let c = ex.seed_bytes(&blob(2)).unwrap().id();
    assert!(!ex.is_verified(&c), "unpinned newcomer evicted instead of a pin");
    assert!(ex.is_verified(&a) && ex.is_verified(&b));
    // Pin everything past the budget: nothing is evicted, the overage is reported.
    cache.pin_content(*blake3::hash(&blob(3)).as_bytes()); // pinned on arrival
    let d = ex.seed_bytes(&blob(3)).unwrap().id();
    assert!(cache.is_pinned(&d));
    assert_eq!(cache.over_budget(), 1000);
    assert!(ex.is_verified(&d));
    assert!(matches!(cache.remove(&a), Err(CacheError::Pinned(_))));
    assert!(cache.enforce().is_empty());
    // Unpinning makes the oldest evictable again, and the budget is enforced.
    assert!(cache.unpin(&a));
    assert!(!ex.is_verified(&a));
    assert_eq!(cache.over_budget(), 0);
    assert!(evictions(&chain).iter().all(|e| e["artifact_id"] != b.to_string()));
}

#[test]
fn an_unpinned_artifact_larger_than_the_budget_is_not_kept() {
    let (ex, _chain) = exchange();
    let cache = ArtifactCache::new(ex.clone(), CacheConfig { max_bytes: 500 });
    let d = ex.seed_bytes(&blob(0)).unwrap();
    assert!(!ex.is_verified(&d.id()));
    assert_eq!(cache.used_bytes(), 0);
    assert!(matches!(cache.admit(&d.id()), Err(CacheError::NotHeld(_))));
}

#[test]
fn eviction_keeps_pieces_another_artifact_still_lists() {
    let (ex, _chain) = exchange();
    let cache = ArtifactCache::new(ex.clone(), CacheConfig { max_bytes: 10_000 });
    // Two artifacts sharing their first 1024-byte piece.
    let mut one = vec![7u8; 1024];
    one.extend_from_slice(&[1u8; 100]);
    let mut two = vec![7u8; 1024];
    two.extend_from_slice(&[2u8; 100]);
    let a = ex.seed_bytes(&one).unwrap();
    let b = ex.seed_bytes(&two).unwrap();
    cache.remove(&a.id()).unwrap();
    assert!(ex.read_all(&b.id()).is_ok(), "the shared piece survives");
    assert!(ex.read_all(&a.id()).is_err());
}

#[test]
fn manual_removal_evicts_and_the_entry_list_is_lru_ordered() {
    let (ex, chain) = exchange();
    let cache = ArtifactCache::new(ex.clone(), CacheConfig { max_bytes: 100_000 });
    let ids: Vec<_> = (0..3).map(|n| ex.seed_bytes(&blob(n)).unwrap().id()).collect();
    cache.touch(&ids[0]);
    let order: Vec<_> = cache.entries().iter().map(|e| e.id).collect();
    assert_eq!(order, vec![ids[1], ids[2], ids[0]]);
    cache.remove(&ids[1]).unwrap();
    assert_eq!(evictions(&chain)[0]["reason"], "manual");
    assert!(matches!(cache.remove(&ids[1]), Err(CacheError::NotHeld(_))));
}
