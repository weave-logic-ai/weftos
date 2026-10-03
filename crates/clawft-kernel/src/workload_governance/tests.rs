//! Acceptance tests for workload governance (ADR-099 s4, s7).

use std::sync::Arc;

use serde_json::{Value, json};

use super::*;
use crate::chain::{ChainEvent, ChainManager};
use crate::gate::{GateBackend, GovernanceGate};
use crate::revocation::{RevocationKind, RevocationList};
use crate::rule_distribution::RuleDistribution;

const KEY: &str = "abababababababababababababababababababababababababababababababab";
const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// A well-behaved cog request: pinned signer, paired node, LAN, native.
fn cog_ctx() -> Value {
    json!({"workload": {
        "kind": "cog",
        "package_trust": "pinned_signer",
        "node_tier": "paired",
        "network": "lan",
        "secrets": false,
        "emulated": false,
        "resource_cost": 0.2,
        "package_id": "cog.fall-detect",
        "signer_keys": [KEY],
        "artifact_hashes": [HASH],
    }})
}

fn with(mut ctx: Value, key: &str, v: Value) -> Value {
    ctx["workload"][key] = v;
    ctx
}

fn chain() -> Arc<ChainManager> {
    Arc::new(ChainManager::new(0, 1000))
}

fn workload_events(cm: &ChainManager) -> Vec<ChainEvent> {
    cm.tail(0).into_iter().filter(|e| e.source == "workload").collect()
}

fn last_payload(cm: &ChainManager) -> Value {
    workload_events(cm).last().and_then(|e| e.payload.clone()).expect("workload event")
}

fn cog_place_permit() -> WorkloadPermitRule {
    WorkloadPermitRule::new("permit-cog-place", ["workload.place"], ["cog"])
}

#[test]
fn default_rules_deny_and_chain_every_governed_action() {
    let cm = chain();
    let gate = WorkloadGate::new(0.8, false).with_chain(cm.clone());
    for action in GOVERNED_ACTIONS {
        let d = gate.check("agent-1", action, &cog_ctx());
        assert!(d.is_deny(), "{action} should be denied by default");
        let ev = workload_events(&cm).pop().unwrap();
        assert_eq!(ev.kind, *action, "chain kind mirrors the action");
        let p = ev.payload.unwrap();
        assert_eq!(p["decision"], "deny");
        assert_eq!(p["action"], *action);
        assert!(p["reason"].as_str().unwrap().contains("default deny"));
        assert_eq!(p["evaluated_rules"], json!([DEFAULT_DENY_RULE_ID]));
        assert_eq!(p["workload"]["kind"], "cog");
    }
    assert_eq!(workload_events(&cm).len(), GOVERNED_ACTIONS.len());
}

#[test]
fn unknown_workload_action_is_refused_and_chained() {
    let cm = chain();
    let gate = WorkloadGate::new(0.8, false)
        .with_chain(cm.clone())
        .with_permit(WorkloadPermitRule::new("all", ["workload.*"], ["*"]))
        .unwrap();
    assert!(gate.check("a", "workload.teleport", &cog_ctx()).is_deny());
    let ev = workload_events(&cm).pop().unwrap();
    assert_eq!(ev.kind, "workload.refuse");
    assert_eq!(ev.payload.unwrap()["action"], "workload.teleport");
}

#[test]
fn non_workload_action_is_denied_and_not_chained() {
    let cm = chain();
    let gate = WorkloadGate::new(0.8, false).with_chain(cm.clone());
    assert!(gate.check("a", "tool.exec", &cog_ctx()).is_deny());
    assert!(workload_events(&cm).is_empty());
}

#[test]
fn permit_rule_allows_and_chains() {
    let cm = chain();
    let gate = WorkloadGate::new(0.8, false)
        .with_chain(cm.clone())
        .with_permit(cog_place_permit())
        .unwrap();
    assert!(gate.check("agent-1", "workload.place", &cog_ctx()).is_permit());
    let ev = workload_events(&cm).pop().unwrap();
    assert_eq!(ev.kind, "workload.place");
    let p = ev.payload.unwrap();
    assert_eq!(p["decision"], "permit");
    assert_eq!(p["permit_rule"], "permit-cog-place");
    assert_eq!(p["threshold_exceeded"], false);
    assert!(p["effect"]["risk"].as_f64().unwrap() > 0.0);
    assert!(!p["evaluated_rules"].as_array().unwrap().iter().any(|r| r == DEFAULT_DENY_RULE_ID));

    // The permit is scoped: another action and another kind stay denied.
    assert!(gate.check("agent-1", "workload.start", &cog_ctx()).is_deny());
    let inference = with(cog_ctx(), "kind", json!("inference"));
    assert!(gate.check("agent-1", "workload.place", &inference).is_deny());
    assert_eq!(last_payload(&cm)["decision"], "deny");
}

#[test]
fn permit_rule_conditions_are_all_enforced() {
    let gate = WorkloadGate::new(0.8, false).with_permit(cog_place_permit()).unwrap();
    let cases = [
        ("package_trust", json!("signed_unpinned")),
        ("node_tier", json!("discovered")),
        ("network", json!("egress")),
        ("emulated", json!(true)),
        ("accelerator", json!("accel.gpu.metal")),
    ];
    for (field, value) in cases {
        let ctx = with(cog_ctx(), field, value.clone());
        assert!(gate.check("a", "workload.place", &ctx).is_deny(), "{field}={value}");
    }
    let mut cost_cap = cog_place_permit();
    cost_cap.max_resource_cost = 0.1;
    let capped = WorkloadGate::new(0.8, false).with_permit(cost_cap).unwrap();
    assert!(capped.check("a", "workload.place", &cog_ctx()).is_deny());

    // Opt-ins lift the matching condition only.
    let mut opt = cog_place_permit();
    opt.allow_emulated = true;
    opt.accelerators = vec!["accel.gpu.*".into()];
    let gate = WorkloadGate::new(0.8, false).with_permit(opt).unwrap();
    let emulated = with(cog_ctx(), "emulated", json!(true));
    assert!(gate.check("a", "workload.place", &emulated).is_permit());
    let metal = with(cog_ctx(), "accelerator", json!("accel.gpu.metal"));
    assert!(gate.check("a", "workload.place", &metal).is_permit());
    let npu = with(cog_ctx(), "accelerator", json!("accel.npu.ane"));
    assert!(gate.check("a", "workload.place", &npu).is_deny());
}

/// A permit rule that allows everything still cannot lift the ceiling.
fn permissive() -> WorkloadPermitRule {
    let mut p = WorkloadPermitRule::new("permissive", ["workload.*"], ["*"]);
    p.min_package_trust = PackageTrust::Unsigned;
    p.min_node_tier = NodeTrustTier::Discovered;
    p.max_network = NetworkPolicy::Egress;
    p
}

fn risky_ctx() -> Value {
    let ctx = with(cog_ctx(), "package_trust", json!("unsigned"));
    let ctx = with(ctx, "node_tier", json!("discovered"));
    with(ctx, "network", json!("egress"))
}

#[test]
fn effect_ceiling_denies_high_effect_even_with_permit() {
    let cm = chain();
    let gate = WorkloadGate::new(0.8, false)
        .with_chain(cm.clone())
        .with_permit(permissive())
        .unwrap();
    assert!(gate.check("a", "workload.install", &cog_ctx()).is_permit());
    // Caller-supplied `effect` is ignored; the vector is derived.
    let mut ctx = risky_ctx();
    ctx["effect"] = json!({"risk": 0.0, "security": 0.0});
    assert!(gate.check("a", "workload.install", &ctx).is_deny());
    let p = last_payload(&cm);
    assert_eq!(p["threshold_exceeded"], true);
    assert_eq!(p["permit_rule"], "permissive");
    assert!(p["evaluated_rules"].as_array().unwrap().iter().any(|r| r == EFFECT_CEILING_RULE_ID));
}

#[test]
fn human_approval_turns_ceiling_deny_into_defer() {
    let cm = chain();
    let gate = WorkloadGate::new(0.8, true)
        .with_chain(cm.clone())
        .with_permit(permissive())
        .unwrap();
    let d = gate.check("a", "workload.install", &risky_ctx());
    assert!(matches!(d, crate::gate::GateDecision::Defer { .. }));
    assert_eq!(last_payload(&cm)["decision"], "defer");
}

#[test]
fn secrets_require_pinned_node_even_when_permitted() {
    let mut p = cog_place_permit();
    p.allow_secrets = true;
    let gate = WorkloadGate::new(0.8, false).with_permit(p).unwrap();
    let secret = with(cog_ctx(), "secrets", json!(true));
    assert!(gate.check("a", "workload.place", &secret).is_deny());
    let pinned = with(secret, "node_tier", json!("pinned"));
    assert!(gate.check("a", "workload.place", &pinned).is_permit());
}

#[test]
fn invalid_context_is_denied_and_chained() {
    let cm = chain();
    let gate = WorkloadGate::new(0.8, false)
        .with_chain(cm.clone())
        .with_permit(permissive())
        .unwrap();
    let bad = [
        json!({}),
        json!({"workload": "cog"}),
        with(cog_ctx(), "kind", json!("Cog!")),
        with(cog_ctx(), "resource_cost", json!(2.0)),
        with(cog_ctx(), "package_trust", json!("trusted")),
        with(cog_ctx(), "accelerator", json!("GPU")),
        with(cog_ctx(), "signer_keys", json!(["not-hex"])),
    ];
    for ctx in &bad {
        assert!(gate.check("a", "workload.place", ctx).is_deny(), "{ctx}");
        let p = last_payload(&cm);
        assert_eq!(p["decision"], "deny");
        assert!(p["reason"].as_str().unwrap().contains("invalid workload context"), "{p}");
    }
    assert_eq!(workload_events(&cm).len(), bad.len());
}

#[test]
fn revoked_package_signer_or_artifact_is_denied_and_chained() {
    let dir = tempfile::tempdir().unwrap();
    let list = Arc::new(RevocationList::new(dir.path().join("revoked_hosts.json")));
    let cm = chain();
    let gate = WorkloadGate::new(0.8, false)
        .with_chain(cm.clone())
        .with_revocations(list.clone())
        .with_permit(cog_place_permit())
        .unwrap();
    assert!(gate.check("a", "workload.place", &cog_ctx()).is_permit());

    for (kind, id) in [
        (RevocationKind::Package, "cog.fall-detect"),
        (RevocationKind::SignerKey, KEY),
        (RevocationKind::ArtifactHash, HASH),
    ] {
        assert!(revoke_and_record(&list, Some(&cm), kind, id, "test", "operator").unwrap());
        let rev = workload_events(&cm).pop().unwrap();
        assert_eq!(rev.kind, "workload.revoke");
        assert_eq!(rev.payload.as_ref().unwrap()["subject_id"], id);

        assert!(gate.check("a", "workload.place", &cog_ctx()).is_deny(), "{kind}");
        let p = last_payload(&cm);
        assert_eq!(p["revoked"]["kind"], json!(kind));
        list.unrevoke_subject(kind, id).unwrap();
    }
    // A repeat revocation is not re-chained.
    let before = workload_events(&cm).len();
    revoke_and_record(&list, Some(&cm), RevocationKind::Package, "x.y", "r", "op").unwrap();
    assert!(!revoke_and_record(&list, Some(&cm), RevocationKind::Package, "x.y", "r", "op").unwrap());
    assert_eq!(workload_events(&cm).len(), before + 1);
}

#[test]
fn unreadable_revocation_list_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(crate::revocation::SUBJECTS_FILE_NAME), "[{").unwrap();
    let list = Arc::new(RevocationList::load(dir.path().join("revoked_hosts.json")));
    let gate = WorkloadGate::new(0.8, false)
        .with_revocations(list)
        .with_permit(cog_place_permit())
        .unwrap();
    assert!(gate.check("a", "workload.place", &cog_ctx()).is_deny());
}

#[test]
fn default_deny_rule_blocks_workload_in_plain_governance_gate() {
    let mut g = GovernanceGate::new(0.8, false);
    for r in default_rules() {
        g = g.add_rule(r);
    }
    let zero = json!({"effect": {"risk": 0.0}});
    assert!(g.check("a", "workload.place", &zero).is_deny());
    assert!(g.check("a", "tool.read_file", &zero).is_permit(), "scoped to workload.*");
}

#[test]
fn default_rules_distribute_to_peers_per_adr_092() {
    let mut origin = RuleDistribution::new("node-a");
    install_default_rules(&mut origin, 100);
    let mut peer = RuleDistribution::new("node-b");
    assert_eq!(peer.merge(&origin.gossip_full()), 2);

    let rules = peer.active_rules();
    assert!(rules.iter().any(|r| r.id == DEFAULT_DENY_RULE_ID));
    let mut g = GovernanceGate::new(0.8, false);
    for r in rules.clone() {
        g = g.add_rule(r);
    }
    assert!(g.check("a", "workload.install", &json!({})).is_deny());
    let wg = WorkloadGate::with_rules(0.8, false, rules);
    assert!(wg.check("a", "workload.install", &cog_ctx()).is_deny());
}

#[test]
fn gate_fails_closed_without_default_deny_rule() {
    let gate = WorkloadGate::with_rules(0.8, false, Vec::new());
    assert!(gate.check("a", "workload.place", &cog_ctx()).is_deny());
    let gate = gate.with_permit(cog_place_permit()).unwrap();
    assert!(gate.check("a", "workload.place", &cog_ctx()).is_permit());
}

#[test]
fn permit_rules_are_validated() {
    let bad = [
        WorkloadPermitRule::new("", ["workload.place"], ["cog"]),
        WorkloadPermitRule::new("r", Vec::<String>::new(), ["cog"]),
        WorkloadPermitRule::new("r", ["workload.place"], Vec::<String>::new()),
        WorkloadPermitRule::new("r", ["*"], ["cog"]),
        WorkloadPermitRule::new("r", ["tool.*"], ["cog"]),
        WorkloadPermitRule::new("r", ["workload.teleport"], ["cog"]),
    ];
    for rule in bad {
        assert!(WorkloadGate::new(0.8, false).with_permit(rule.clone()).is_err(), "{rule:?}");
    }
    let dup = WorkloadGate::new(0.8, false)
        .with_permit(cog_place_permit())
        .unwrap()
        .with_permit(cog_place_permit());
    assert!(dup.is_err());
}

#[test]
fn scripted_permit_and_deny_events_survive_chain_save_and_load() {
    let dir = tempfile::tempdir().unwrap();
    let cm = chain();
    let gate = WorkloadGate::new(0.8, false)
        .with_chain(cm.clone())
        .with_permit(cog_place_permit())
        .unwrap();
    gate.check("operator", "workload.place", &cog_ctx());
    gate.check("operator", "workload.start", &cog_ctx());
    let path = dir.path().join("chain.json");
    cm.save_to_file(&path).unwrap();

    let loaded = ChainManager::load_from_file(&path, 1000).unwrap();
    let got: Vec<(String, Value)> = workload_events(&loaded)
        .into_iter()
        .map(|e| (e.kind, e.payload.unwrap()["decision"].clone()))
        .collect();
    assert_eq!(
        got,
        vec![
            ("workload.place".to_owned(), json!("permit")),
            ("workload.start".to_owned(), json!("deny")),
        ]
    );
}

// ── ADR-103 A6 (Phase 2 package I): permits match the verified project ──

mod project_permits {
    use super::*;
    use crate::governance::{AttestSource, ProjectAttestation};

    const P1: &str = "01JB8Z3Q0V6X9KQ4M2N7T5R1WD";
    const P2: &str = "01JB8Z3Q0V6X9KQ4M2N7T5R1WE";

    fn gate_for(project: Option<&str>) -> WorkloadGate {
        let mut permit = cog_place_permit();
        permit.projects = vec![P1.into()];
        let g = WorkloadGate::new(0.8, false).with_permit(permit).unwrap();
        match project {
            Some(p) => g.with_attestation(ProjectAttestation::from_verified(p, AttestSource::BoundKernel)),
            None => g,
        }
    }

    #[test]
    fn a_project_scoped_permit_matches_only_the_verified_project() {
        assert!(!gate_for(Some(P1)).check("a", "workload.place", &cog_ctx()).is_deny());
        assert!(gate_for(Some(P2)).check("a", "workload.place", &cog_ctx()).is_deny());
        assert!(gate_for(None).check("a", "workload.place", &cog_ctx()).is_deny());
    }

    #[test]
    fn a_project_named_in_the_request_context_is_not_a_source() {
        let mut ctx = cog_ctx();
        ctx["project_id"] = json!(P1);
        ctx["workload"]["project_id"] = json!(P1);
        assert!(gate_for(Some(P2)).check("a", "workload.place", &ctx).is_deny());
        assert!(gate_for(None).check("a", "workload.place", &ctx).is_deny());
    }

    #[test]
    fn permit_rules_without_projects_serialise_as_before() {
        let v = serde_json::to_value(cog_place_permit()).unwrap();
        assert!(v.get("projects").is_none());
        let mut bad = cog_place_permit();
        bad.projects = vec![String::new()];
        assert!(bad.validate().is_err());
    }
}

fn stop_permit() -> WorkloadPermitRule {
    let mut p = WorkloadPermitRule::new("permit-cog-stop", ["workload.stop"], ["cog"]);
    p.min_node_tier = NodeTrustTier::Paired;
    p
}

#[test]
fn teardown_waives_a_node_tier_denial_and_chains_the_waiver() {
    let cm = chain();
    let gate = WorkloadGate::new(0.8, false)
        .with_chain(cm.clone())
        .with_permit(stop_permit())
        .unwrap();
    let demoted = with(cog_ctx(), "node_tier", json!("discovered"));
    assert!(gate.check("a", "workload.stop", &demoted).is_deny());
    assert!(
        gate.check_teardown("a", "workload.stop", &demoted)
            .is_permit()
    );
    assert_eq!(last_payload(&cm)["teardown_node_tier_waived"], true);
}

#[test]
fn teardown_never_waives_a_revocation_or_a_default_deny() {
    let dir = tempfile::tempdir().unwrap();
    let list = Arc::new(RevocationList::new(dir.path().join("revoked_hosts.json")));
    list.revoke_subject(RevocationKind::Package, "cog.fall-detect", "bad")
        .unwrap();
    let gate = WorkloadGate::new(0.8, false)
        .with_permit(stop_permit())
        .unwrap()
        .with_revocations(list);
    let demoted = with(cog_ctx(), "node_tier", json!("discovered"));
    let paired = cog_ctx();
    // Revoked: denied on a paired node and on a demoted one.
    assert!(gate.check_teardown("a", "workload.stop", &paired).is_deny());
    assert!(
        gate.check_teardown("a", "workload.stop", &demoted)
            .is_deny()
    );
    // No permit for the action at all: a default deny stays a deny.
    let bare = WorkloadGate::new(0.8, false);
    assert!(
        bare.check_teardown("a", "workload.stop", &demoted)
            .is_deny()
    );
    // A permit for another kind of workload does not cover this one either.
    let other = WorkloadGate::new(0.8, false)
        .with_permit(WorkloadPermitRule::new(
            "other",
            ["workload.stop"],
            ["inference"],
        ))
        .unwrap();
    assert!(
        other
            .check_teardown("a", "workload.stop", &demoted)
            .is_deny()
    );
}

#[test]
fn check_teardown_refuses_any_action_that_is_not_a_teardown() {
    // The guard itself: start, place and install are not teardown actions.
    for a in ["workload.start", "workload.place", "workload.install", "workload.migrate"] {
        assert!(WorkloadGate::not_teardown(a).unwrap().is_deny(), "{a}");
    }
    for a in ["workload.stop", "workload.unload", "workload.load"] {
        assert!(WorkloadGate::not_teardown(a).is_none(), "{a}");
    }
    // Through the trait method: denied in release builds; in debug builds
    // the debug_assert fires first.
    let r = std::panic::catch_unwind(|| {
        let g = WorkloadGate::new(0.8, false)
            .with_permit(WorkloadPermitRule::new("all", ["workload.*"], ["cog"]))
            .unwrap();
        g.check_teardown("a", "workload.start", &cog_ctx())
    });
    match r {
        Ok(d) => assert!(d.is_deny()),
        Err(_) => assert!(cfg!(debug_assertions)),
    }
}
