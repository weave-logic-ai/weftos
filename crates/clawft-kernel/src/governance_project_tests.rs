use super::*;
use crate::governance::{GatePrincipal, GovernanceEngine, GovernanceRule, RuleSeverity};

const P1: &str = "01JB8Z3Q0V6X9KQ4M2N7T5R1WD";
const INST: &str = "6a3803d5f059902a1c6dafbc9ba47292";

fn att() -> ProjectAttestation {
    ProjectAttestation::from_verified(P1, AttestSource::BoundKernel)
}

#[test]
fn only_a_bound_kernel_attestation_can_be_the_instance_project() {
    // Never set in this test binary except through this refusal path: the
    // other sources are rejected before the cell is touched.
    for src in [AttestSource::TokenScope, AttestSource::UserForward] {
        assert!(!set_instance_project(ProjectAttestation::from_verified(P1, src), INST));
    }
}

#[test]
fn attributed_with_stamps_principal_and_context_from_the_attestation() {
    let r = GovernanceRequest::new("a", "act").attributed_with(Some(&att()), Some(INST));
    let p = r.base_principal();
    assert_eq!(p.project_id(), Some(P1));
    assert_eq!(p.instance_id(), Some(INST));
    assert_eq!(r.context.get("project_id").map(String::as_str), Some(P1));
}

#[test]
fn caller_supplied_project_context_is_dropped_never_trusted() {
    // Through the builder.
    let r = GovernanceRequest::new("a", "act")
        .with_context_entry("project_id", "01JB8Z3Q0V6X9KQ4M2N7T5R1WE")
        .with_context_entry("instance_id", "evil");
    assert!(!r.context.contains_key("project_id"));
    assert!(!r.context.contains_key("instance_id"));
    // Through a literal map, as `GovernanceGate` and `http_api` build them.
    let mut forged = GovernanceRequest::new("a", "act");
    forged.context.insert("project_id".into(), "01JB8Z3Q0V6X9KQ4M2N7T5R1WE".into());
    let unattested = forged.clone().attributed_with(None, None);
    assert!(!unattested.context.contains_key("project_id"));
    let attested = forged.attributed_with(Some(&att()), None);
    assert_eq!(attested.context["project_id"], P1);
}

#[test]
fn an_event_audited_under_a_child_carries_principal_project_id() {
    let engine = GovernanceEngine::new(0.9, false);
    let req = GovernanceRequest::new("a", "act").attributed_with(Some(&att()), Some(INST));
    let result = engine.evaluate(&req);
    let p = result.principal.expect("principal attributed");
    assert_eq!(p.project_id(), Some(P1));
    let ev = crate::chain::GovernanceDecisionEvent {
        agent_id: "a".into(),
        action: "act".into(),
        decision: "Permit".into(),
        effect_magnitude: 0.0,
        threshold_exceeded: false,
        evaluated_rules: vec![],
        timestamp: chrono::Utc::now(),
        principal: Some(p),
    };
    use crate::chain::ChainLoggable;
    let payload = ev.chain_event_payload();
    assert_eq!(payload["principal"]["project_id"], P1);
    assert_eq!(payload["principal"]["instance_id"], INST);
}

#[test]
fn an_unattested_kernel_leaves_principals_without_a_project() {
    let r = GovernanceRequest::new("a", "act").attributed_with(None, None);
    assert_eq!(r.base_principal(), GatePrincipal::agent("a"));
    let _ = (GovernanceRule::browser_policy("x", "y"), RuleSeverity::Advisory);
}
