//! NodeFacts: validation, time window, load and live-state deltas.

use super::capability::{Capability, CapabilityId, CapabilityState, Provenance};
use super::node_facts::{FactsDelta, FactsError, NodeFacts, StateChange};

fn cap(id: &str) -> Capability {
    Capability::new(CapabilityId::new(id).unwrap(), Provenance::Probed)
}

fn mac_facts() -> NodeFacts {
    let mut f = NodeFacts::new("n-abc123", 1_000, 600, 7);
    f.capabilities = vec![
        cap("cpu.arch.aarch64"),
        cap("mem.unified")
            .with_attr("total", 137_438_953_472i64)
            .with_attr("free", 60_000_000_000i64),
        cap("accel.gpu.metal").with_attr("unified", true),
    ];
    f
}

#[test]
fn valid_facts_pass_and_roundtrip() {
    let f = mac_facts();
    f.check_window(1_100, 60).unwrap();
    let json = serde_json::to_string(&f).unwrap();
    let back: NodeFacts = serde_json::from_str(&json).unwrap();
    assert_eq!(back, f);
    assert_eq!(back.node_id(), "n-abc123");
    assert_eq!(back.capabilities().len(), 3);
}

#[test]
fn expired_and_future_facts_are_rejected() {
    let f = mac_facts();
    assert!(matches!(
        f.check_window(1_600, 60),
        Err(FactsError::Expired {
            expired_at: 1_600,
            ..
        })
    ));
    assert!(f.check_window(1_599, 60).is_ok());
    assert!(matches!(
        f.check_window(900, 60),
        Err(FactsError::FromFuture { .. })
    ));
    assert!(f.check_window(940, 60).is_ok());
}

#[test]
fn self_asserted_trust_is_rejected() {
    let mut f = mac_facts();
    f.capabilities.push(cap("trust.tier.pinned"));
    assert!(matches!(f.validate(), Err(FactsError::Invalid(m)) if m.contains("trust")));
}

#[test]
fn bounds_are_enforced() {
    let mut f = mac_facts();
    f.ttl_secs = 5;
    assert!(f.validate().is_err());
    f.ttl_secs = 90_000;
    assert!(f.validate().is_err());
    let mut f = mac_facts();
    f.node_id = "bad id/../x".into();
    assert!(f.validate().is_err());
    let mut f = mac_facts();
    f.version = 2;
    assert!(f.validate().is_err());
}

#[test]
fn unknown_fields_are_rejected_at_the_boundary() {
    let mut v = serde_json::to_value(mac_facts()).unwrap();
    v["trust"] = serde_json::json!("pinned");
    assert!(serde_json::from_value::<NodeFacts>(v).is_err());
}

#[test]
fn load_counts_busy_and_reads_unified_free() {
    let mut f = mac_facts();
    f.capabilities[2].state = CapabilityState::Busy;
    let load = f.load();
    assert_eq!(load.busy, 1);
    assert_eq!(load.total, 3);
    assert_eq!(load.mem_free, Some(60_000_000_000));
}

fn delta(base_seq: u64) -> FactsDelta {
    FactsDelta {
        node_id: "n-abc123".into(),
        base_seq,
        seq: 1,
        issued_at: 1_010,
        changes: vec![StateChange {
            index: 2,
            id: CapabilityId::new("accel.gpu.metal").unwrap(),
            state: CapabilityState::Busy,
        }],
        mem_free: Some(10),
    }
}

#[test]
fn delta_updates_state_and_free_memory() {
    let mut f = mac_facts();
    f.apply_delta(&delta(7)).unwrap();
    assert_eq!(f.capabilities[2].state, CapabilityState::Busy);
    assert_eq!(f.load().mem_free, Some(10));
}

#[test]
fn delta_against_wrong_base_or_index_changes_nothing() {
    let mut f = mac_facts();
    let before = f.clone();
    assert!(matches!(
        f.apply_delta(&delta(6)),
        Err(FactsError::Delta(_))
    ));
    let mut d = delta(7);
    d.changes.push(StateChange {
        index: 0,
        id: CapabilityId::new("accel.gpu.metal").unwrap(),
        state: CapabilityState::Busy,
    });
    assert!(f.apply_delta(&d).is_err());
    let mut d = delta(7);
    d.node_id = "n-other".into();
    assert!(f.apply_delta(&d).is_err());
    assert_eq!(f, before, "failed deltas must not partially apply");
}
