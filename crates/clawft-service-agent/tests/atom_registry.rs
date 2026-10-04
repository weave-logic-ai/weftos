//! WEFT-641 / ADR-069: AtomRegistry mint at KernelTurnAnchor + locate/audit.
//!
//! Drives the exact daemon seam (`KernelTurnAnchor::anchor_turn`) with an
//! optional registry attached, proving:
//! - locate(ByChainSeq) and locate(ByUid) O(1) and agree
//! - substrate sibling binds chain_seq ↔ turn_id
//! - audit flags democritus-style chain_seq=0 projection rows
//! - absence of registry does not fail the turn (P3 removability)

use std::sync::Arc;

use clawft_core::agent::sink::Turn;
use clawft_kernel::chain::ChainManager;
use clawft_service_agent::{
    AtomKey, AtomRegistry, Disposition, KernelTurnAnchor, ProjectionAuditRow, TurnAnchor,
};

fn user_turn(id: &str, content: &str, ts_ms: u64) -> Turn {
    Turn {
        turn_id: id.into(),
        role: "user".into(),
        content: content.into(),
        tool_calls: None,
        tool_call_id: None,
        ts_ms,
        voice_analysis: None,
            audio: None,
    }
}

#[tokio::test]
async fn anchor_mints_locator_seq_and_uid_agree() {
    let chain = Arc::new(ChainManager::new(0, 1000));
    let registry = AtomRegistry::shared();
    let anchor = KernelTurnAnchor::new(Some(chain), None).with_atom_registry(registry.clone());

    let turn = user_turn("t-ulid-1", "hello panopticon", 100);
    anchor.anchor_turn("conv-641", "t-ulid-1", &turn).await;

    assert_eq!(registry.len(), 1, "one locator minted");
    // chain starts at 0 with checkpoint interval; first append is sequence 1 typically —
    // discover via sibling from turn_id.
    let seq = registry
        .chain_seq_for_turn("t-ulid-1")
        .expect("substrate sibling chain_seq↔turn_id");
    let by_seq = registry
        .locate(AtomKey::ByChainSeq(seq))
        .expect("locate by chain_seq");
    let by_uid = registry
        .locate(AtomKey::ByUid(by_seq.uid.clone()))
        .expect("locate by uid");
    assert_eq!(by_seq, by_uid, "ByChainSeq and ByUid must return the same locator");
    assert_eq!(by_seq.conv_id, "conv-641");
    assert_eq!(by_seq.turn_id, "t-ulid-1");
    assert_eq!(by_seq.role, "user");
    assert_eq!(by_seq.kind, "agent.chat.turn");
    assert_eq!(by_seq.disposition, Disposition::Committed);
    assert_eq!(registry.turn_id_for_seq(seq).as_deref(), Some("t-ulid-1"));
    // No turn text in the locator (references only).
    let json = serde_json::to_value(&by_seq).expect("serialize");
    assert!(json.get("content").is_none());
    assert!(json.get("text").is_none());
}

#[tokio::test]
async fn anchor_without_registry_still_succeeds() {
    // P3 removability: chain path works with no registry attached.
    let chain = Arc::new(ChainManager::new(0, 1000));
    let anchor = KernelTurnAnchor::new(Some(chain.clone()), None);
    let turn = user_turn("t-noop", "still lands", 1);
    anchor.anchor_turn("conv-x", "t-noop", &turn).await;
    assert!(
        !chain.is_empty(),
        "turn still lands on the chain without a registry"
    );
}

#[tokio::test]
async fn audit_flags_democritus_chain_seq_zero() {
    let registry = AtomRegistry::shared();
    // Healthy mint so internal maps are clean.
    registry.mint_and_record(
        "c",
        "t1",
        5,
        "user",
        "ok",
        "hash",
        "agent.chat.turn",
        1,
    );
    let clean = registry.audit();
    assert!(clean.ok, "internal maps must agree: {clean:?}");

    // Live defect class (WEFT-642 / democritus.rs:315): chain_seq hardcoded 0.
    let rows = [ProjectionAuditRow {
        projection: "ecc_brain_hnsw".into(),
        label: "node-293".into(),
        chain_seq: Some(0),
        uid: None,
    }];
    let report = registry.audit_projections(&rows);
    assert!(!report.ok);
    assert!(
        report.issues.iter().any(|i| i.code == "chain_seq_zero"),
        "expected chain_seq_zero: {report:?}"
    );
}
