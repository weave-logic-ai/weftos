//! Card 03 facts as card 04 `PlacementFacts`.

use std::sync::Arc;

use clawft_types::placement::engine::{Liveness, PlacementFacts, TrustTier as EngineTier};
use clawft_types::placement::{CapabilityState, NodeFacts, TrustTier};
use ed25519_dalek::SigningKey;

use super::facts::{liveness_of, placement_view};
use super::test_support::{board_caps, cap};
use crate::cluster::{ClusterConfig, ClusterMembership, NodeState};
use crate::node_facts::NodeFactsCache;
use crate::node_facts_advert::sign_node_facts;
use crate::node_registry::node_id_from_pubkey;

fn signed(seed: u8, now: u64, busy: bool) -> (String, crate::node_facts_advert::SignedNodeFacts) {
    let key = SigningKey::from_bytes(&[seed; 32]);
    let id = node_id_from_pubkey(&key.verifying_key().to_bytes());
    let mut f = NodeFacts::new(id.clone(), now, 60, 1);
    f.capabilities = board_caps("pi5");
    let st = if busy {
        CapabilityState::Busy
    } else {
        CapabilityState::Available
    };
    f.capabilities.push(cap("accel.npu.hailo").with_state(st));
    (id, sign_node_facts(&f, &key).unwrap())
}

#[test]
fn cached_facts_feed_the_engine_with_tier_ttl_load_and_liveness() {
    let now = 1_800_000_000;
    let cache = NodeFactsCache::new();
    let (a, sa) = signed(1, now, true);
    let (b, sb) = signed(2, now, false);
    let (c, sc) = signed(3, now, false);
    cache.insert(sa, TrustTier::Paired, now).unwrap();
    cache.insert(sb, TrustTier::Pinned, now).unwrap();
    cache.insert(sc, TrustTier::Discovered, now).unwrap();

    let membership = Arc::new(ClusterMembership::new(ClusterConfig::default()));
    let contact = |id: &str| (id == b).then_some(Liveness::Alive);
    let view = placement_view(&cache, now, Some(&a), Some(&membership), &contact);
    assert_eq!(view.len(), 3);
    let get = |id: &str| view.iter().find(|v| v.node_id() == id).unwrap();

    assert_eq!(get(&a).liveness(), Liveness::Alive, "local node");
    assert_eq!(get(&b).liveness(), Liveness::Alive, "direct contact");
    assert_eq!(get(&c).liveness(), Liveness::Unknown, "never heard from");
    assert_eq!(get(&a).trust_tier(), EngineTier::Paired);
    assert_eq!(get(&b).trust_tier(), EngineTier::Pinned);
    assert_eq!(get(&c).trust_tier(), EngineTier::Discovered);
    assert_eq!(get(&a).facts_expire_at_ms(), Some((now + 60) * 1000));
    let busy = get(&a).load().unwrap();
    assert!(
        busy > 0.0 && busy < 1.0,
        "one busy capability counts: {busy}"
    );
    assert_eq!(get(&b).load(), Some(0.0));
    assert!(
        get(&a)
            .capabilities()
            .iter()
            .any(|c| c.id.as_str() == "os.linux")
    );

    // Expired facts leave the view.
    assert!(placement_view(&cache, now + 61, None, None, &|_| None).is_empty());
}

#[test]
fn membership_state_maps_to_liveness() {
    assert_eq!(liveness_of(&NodeState::Active), Liveness::Alive);
    assert_eq!(liveness_of(&NodeState::Suspect), Liveness::Suspect);
    assert_eq!(liveness_of(&NodeState::Unreachable), Liveness::Dead);
    assert_eq!(liveness_of(&NodeState::Left), Liveness::Dead);
    assert_eq!(liveness_of(&NodeState::Joining), Liveness::Unknown);
}
