//! TTL cache: only verified, fresh, newer facts are held; deltas must come
//! from the same key and move forward.

use super::*;
use crate::node_facts_advert::{sign_facts_delta, sign_node_facts};
use crate::node_registry::node_id_from_pubkey;
use clawft_types::placement::{CapabilityId, CapabilityState, FactsDelta, Provenance, StateChange};
use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;

fn key() -> SigningKey {
    SigningKey::generate(&mut OsRng)
}

fn facts(k: &SigningKey, seq: u64, issued_at: u64) -> NodeFacts {
    let id = node_id_from_pubkey(&k.verifying_key().to_bytes());
    let mut f = NodeFacts::new(id, issued_at, 600, seq);
    f.capabilities = vec![
        Capability::new(
            CapabilityId::new("accel.tpu.coral").unwrap(),
            Provenance::Probed,
        )
        .exclusive(),
        Capability::new(CapabilityId::new("mem.system").unwrap(), Provenance::Probed)
            .with_attr("total", 8_000i64)
            .with_attr("free", 4_000i64),
    ];
    f
}

fn signed(k: &SigningKey, seq: u64, issued_at: u64) -> SignedNodeFacts {
    sign_node_facts(&facts(k, seq, issued_at), k).unwrap()
}

fn delta(k: &SigningKey, base_seq: u64, seq: u64, state: CapabilityState) -> SignedFactsDelta {
    let d = FactsDelta {
        node_id: node_id_from_pubkey(&k.verifying_key().to_bytes()),
        base_seq,
        seq,
        issued_at: 1_010,
        changes: vec![StateChange {
            index: 0,
            id: CapabilityId::new("accel.tpu.coral").unwrap(),
            state,
        }],
        mem_free: Some(1_000),
    };
    sign_facts_delta(&d, k).unwrap()
}

#[test]
fn verified_facts_are_served_until_ttl_then_hidden_and_evicted() {
    let (c, k) = (NodeFactsCache::new(), key());
    assert_eq!(
        c.insert(signed(&k, 1, 1_000), TrustTier::Paired, 1_000),
        Ok(InsertOutcome::Added)
    );
    let id = node_id_from_pubkey(&k.verifying_key().to_bytes());
    let got = c.get(&id, 1_599).unwrap();
    assert_eq!(got.trust_tier(), TrustTier::Paired);
    assert_eq!(got.capabilities().len(), 2);
    assert!(
        c.get(&id, 1_600).is_none(),
        "expired facts must not be served"
    );
    assert!(c.list(1_600).is_empty());
    assert_eq!(c.evict_expired(1_600), 1);
    assert!(c.is_empty());
}

#[test]
fn tampered_or_expired_facts_never_enter() {
    let (c, k) = (NodeFactsCache::new(), key());
    let mut s = signed(&k, 1, 1_000);
    s.payload = s.payload.replace("8000", "9000");
    assert!(matches!(
        c.insert(s, TrustTier::Paired, 1_000),
        Err(CacheError::Verify(NodeFactsAdvertError::BadSignature))
    ));
    assert!(matches!(
        c.insert(signed(&k, 1, 1_000), TrustTier::Paired, 2_000),
        Err(CacheError::Verify(NodeFactsAdvertError::Facts(
            FactsError::Expired { .. }
        )))
    ));
    assert!(c.is_empty());
}

#[test]
fn older_seq_cannot_replace_newer_facts() {
    let (c, k) = (NodeFactsCache::new(), key());
    c.insert(signed(&k, 5, 1_000), TrustTier::Paired, 1_000)
        .unwrap();
    assert_eq!(
        c.insert(signed(&k, 5, 1_000), TrustTier::Paired, 1_001),
        Ok(InsertOutcome::Unchanged)
    );
    assert!(matches!(
        c.insert(signed(&k, 4, 1_050), TrustTier::Paired, 1_060),
        Err(CacheError::Stale {
            got: 4,
            held: 5,
            ..
        })
    ));
    assert_eq!(
        c.insert(signed(&k, 6, 1_050), TrustTier::Paired, 1_060),
        Ok(InsertOutcome::Replaced)
    );
}

#[test]
fn deltas_update_state_in_order_and_only_from_the_node_key() {
    let (c, k, other) = (NodeFactsCache::new(), key(), key());
    c.insert(signed(&k, 3, 1_000), TrustTier::Paired, 1_000)
        .unwrap();
    let id = node_id_from_pubkey(&k.verifying_key().to_bytes());

    c.apply_delta(&delta(&k, 3, 1, CapabilityState::Busy), 1_020)
        .unwrap();
    let got = c.get(&id, 1_020).unwrap();
    assert_eq!(got.capabilities()[0].state, CapabilityState::Busy);
    assert_eq!(got.load().mem_free, Some(1_000));
    assert_eq!(got.load().busy, 1);

    // Replay of the same delta seq is refused.
    assert!(
        c.apply_delta(&delta(&k, 3, 1, CapabilityState::Available), 1_021)
            .is_err()
    );
    // Wrong base seq is refused.
    assert!(
        c.apply_delta(&delta(&k, 2, 2, CapabilityState::Available), 1_021)
            .is_err()
    );
    // Another node's key cannot update this node (its delta names its own id).
    assert!(matches!(
        c.apply_delta(&delta(&other, 3, 9, CapabilityState::Available), 1_021),
        Err(CacheError::UnknownNode(_))
    ));
    assert_eq!(
        c.get(&id, 1_021).unwrap().capabilities()[0].state,
        CapabilityState::Busy
    );

    // After expiry a delta has no base.
    assert!(matches!(
        c.apply_delta(&delta(&k, 3, 2, CapabilityState::Available), 1_700),
        Err(CacheError::UnknownNode(_))
    ));
}
