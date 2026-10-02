//! Trust root, rollback pin, revocation and history checks of a project
//! kernel's governance (ADR-103 D8, package E review round).

use chrono::{Duration, Utc};
use clawft_types::config::overlay::Limits;
use clawft_types::project::cert::{CertRequest, ProjectCert};
use ed25519_dalek::SigningKey;

use crate::chain::ChainManager;
use crate::governance_overlay::OverlayError;
use crate::governance_overlay_tests::{base_parent, parent_with, user_key};
use crate::overlay_runtime::prepare;
use crate::overlay_trust::{REVOKED_FILE, USER_PIN_FILE, VERSION_PIN_FILE, write_user_pin};
use crate::overlay_runtime_tests::{fixture, start, write_parent};
use crate::parent_policy::{
    VERSION_SLACK_SECS, export_rules, export_to, write_atomic_0600,
};

fn pin_path(f: &crate::overlay_runtime_tests::Fixture) -> std::path::PathBuf {
    f.paths.state_dir().unwrap().join(VERSION_PIN_FILE)
}

fn prepare_err(f: &crate::overlay_runtime_tests::Fixture) -> OverlayError {
    prepare(&f.paths).err().expect("must be refused")
}

#[test]
fn boot_writes_the_rollback_pin_and_an_unreadable_pin_refuses_boot() {
    let f = fixture(&parent_with(base_parent().rules, base_parent().limits, 9), None);
    assert!(!pin_path(&f).exists());
    let _r = start(&f);
    assert_eq!(std::fs::read_to_string(pin_path(&f)).unwrap().trim(), "9");

    // Garbage in the pin file is an error, not "no pin".
    std::fs::write(pin_path(&f), "nine").unwrap();
    assert!(matches!(prepare_err(&f), OverlayError::PinCorrupt { .. }));
    std::fs::write(pin_path(&f), b"\xff\xfe").unwrap();
    assert!(matches!(prepare_err(&f), OverlayError::PinCorrupt { .. }));
}

#[test]
fn a_missing_pin_after_an_applied_overlay_refuses_boot() {
    let f = fixture(&base_parent(), None);
    let r = start(&f);
    std::fs::remove_file(pin_path(&f)).unwrap();
    // Same chain (it records `governance.overlay.applied`), pin gone.
    let again = prepare(&f.paths).unwrap();
    assert_eq!(again.commit(&r.cm).unwrap_err(), OverlayError::PinMissing);
    // A fresh chain with no history is a first boot and writes the pin.
    again.commit(&ChainManager::new(0, 1000)).unwrap();
    assert!(pin_path(&f).exists());
}

#[test]
fn a_deleted_overlay_cannot_clear_a_non_empty_one_at_boot() {
    let f = fixture(
        &base_parent(),
        Some("[[deny]]\nid = \"d1\"\nactions = [\"a.b\"]\n"),
    );
    let r = start(&f);
    std::fs::remove_file(f.paths.overlay().unwrap()).unwrap();
    let again = prepare(&f.paths).unwrap();
    assert_eq!(again.commit(&r.cm).unwrap_err(), OverlayError::OverlayMissing);

    // An explicit empty file is the deliberate clear.
    std::fs::write(f.paths.overlay().unwrap(), "").unwrap();
    prepare(&f.paths).unwrap().commit(&r.cm).unwrap();

    // And a project that never had an overlay may keep having none.
    let g = fixture(&base_parent(), None);
    let r2 = start(&g);
    prepare(&g.paths).unwrap().commit(&r2.cm).unwrap();
}

#[test]
fn the_user_pin_outside_the_project_overrides_the_certificate() {
    let f = fixture(&base_parent(), None);
    let user_pk = user_key().verifying_key().to_bytes();

    // Matching pin: accepted.
    write_user_pin(f.paths.root(), &user_pk).unwrap();
    assert!(f.paths.root().join(USER_PIN_FILE).exists());
    prepare(&f.paths).unwrap();

    // A different pin: the certificate is refused even though it is
    // self-consistent.
    write_user_pin(f.paths.root(), &SigningKey::from_bytes(&[9u8; 32]).verifying_key().to_bytes())
        .unwrap();
    let e = prepare_err(&f);
    assert!(matches!(&e, OverlayError::Cert(m) if m.contains("user.pub")), "{e}");

    // An unreadable pin is refused, not ignored.
    std::fs::write(f.paths.root().join(USER_PIN_FILE), "not hex").unwrap();
    assert!(matches!(prepare_err(&f), OverlayError::Cert(_)));

    // Absent pin: development fallback to the certificate.
    std::fs::remove_file(f.paths.root().join(USER_PIN_FILE)).unwrap();
    prepare(&f.paths).unwrap();
}

#[test]
fn a_cert_and_policy_swapped_together_are_caught_by_the_pin() {
    // An attacker with the project dir writes their own key, cert and policy.
    let f = fixture(&base_parent(), None);
    write_user_pin(f.paths.root(), &user_key().verifying_key().to_bytes()).unwrap();
    let evil = SigningKey::from_bytes(&[9u8; 32]);
    let cert = ProjectCert::sign(
        &evil,
        &CertRequest {
            project_id: f.paths.child_id().unwrap().to_owned(),
            project_pubkey: SigningKey::from_bytes(&[3u8; 32]).verifying_key().to_bytes(),
            serial: 1,
            issued_at: Utc::now() - Duration::minutes(1),
            expires_at: None,
        },
    );
    std::fs::write(f.paths.project_cert().unwrap(), serde_json::to_vec(&cert).unwrap()).unwrap();
    let relaxed = export_rules(vec![], 0.99, false, &Limits::default(), &evil, 1, Utc::now()).unwrap();
    write_parent(&f.paths, &relaxed);
    assert!(matches!(prepare_err(&f), OverlayError::Cert(_)));
}

#[test]
fn a_revoked_project_stops_boot_reload_and_update() {
    let f = fixture(&base_parent(), None);
    let r = start(&f);
    std::fs::write(f.paths.root().join(REVOKED_FILE), "").unwrap();
    assert!(matches!(r.rt.reload().unwrap_err(), OverlayError::Revoked(_)));
    let newer = parent_with(base_parent().rules, base_parent().limits, 3);
    assert!(matches!(r.rt.apply_parent_update(newer).unwrap_err(), OverlayError::Revoked(_)));
    assert!(matches!(prepare_err(&f), OverlayError::Revoked(_)));
}

#[test]
fn an_expired_certificate_stops_reload_and_update() {
    let f = fixture(&base_parent(), None);
    let r = start(&f);
    let cert = ProjectCert::sign(
        &user_key(),
        &CertRequest {
            project_id: f.paths.child_id().unwrap().to_owned(),
            project_pubkey: SigningKey::from_bytes(&[3u8; 32]).verifying_key().to_bytes(),
            serial: 2,
            issued_at: Utc::now() - Duration::minutes(10),
            expires_at: Some(Utc::now() - Duration::minutes(1)),
        },
    );
    std::fs::write(f.paths.project_cert().unwrap(), serde_json::to_vec(&cert).unwrap()).unwrap();
    let e = r.rt.reload().unwrap_err();
    assert_eq!(e.key(), "project.cert.json");
    let newer = parent_with(base_parent().rules, base_parent().limits, 3);
    assert_eq!(r.rt.apply_parent_update(newer).unwrap_err().key(), "project.cert.json");
}

#[test]
fn export_only_trusts_a_previous_policy_this_key_signed_and_caps_the_version() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("parent-policy.json");
    let engine = crate::governance::GovernanceEngine::new(0.8, false);
    let now = Utc::now().timestamp() as u64;

    // Signed by someone else with a huge version: ignored.
    let other = SigningKey::from_bytes(&[9u8; 32]);
    let theirs = export_rules(vec![], 0.8, false, &Limits::default(), &other, u64::MAX / 2, Utc::now()).unwrap();
    write_atomic_0600(&path, &serde_json::to_vec(&theirs).unwrap()).unwrap();
    let a = export_to(&path, &engine, &Limits::default(), &user_key()).unwrap();
    assert!(a.version <= now + 5, "{}", a.version);

    // Ours, but far in the future: capped, not ratcheted.
    let ours = export_rules(vec![], 0.8, false, &Limits::default(), &user_key(), u64::MAX / 2, Utc::now()).unwrap();
    write_atomic_0600(&path, &serde_json::to_vec(&ours).unwrap()).unwrap();
    let b = export_to(&path, &engine, &Limits::default(), &user_key()).unwrap();
    assert!(b.version <= now + VERSION_SLACK_SECS + 5, "{}", b.version);

    // Garbage: the version starts from the clock.
    std::fs::write(&path, "{").unwrap();
    let c = export_to(&path, &engine, &Limits::default(), &user_key()).unwrap();
    assert!(c.version <= now + 5);
}

#[test]
fn a_lowered_pin_cannot_let_an_older_policy_in() {
    // The chain remembers version 7 was applied.
    let f = fixture(&parent_with(base_parent().rules, base_parent().limits, 5), None);
    let r = start(&f);
    r.rt.apply_parent_update(parent_with(vec![], Limits::default(), 7)).unwrap();

    // Pin zeroed and an older (validly signed) policy on disk: file below history.
    std::fs::write(pin_path(&f), "0").unwrap();
    let again = prepare(&f.paths).unwrap();
    let e = again.commit(&r.cm).unwrap_err();
    assert!(matches!(e, OverlayError::Parent(crate::parent_policy::ParentPolicyError::Rollback { have: 5, pinned: 7 })), "{e}");

    // Newest policy on disk but the pin lowered: the pin is below history.
    write_parent(&f.paths, &parent_with(vec![], Limits::default(), 7));
    let again = prepare(&f.paths).unwrap();
    let e = again.commit(&r.cm).unwrap_err();
    assert!(matches!(e, OverlayError::Parent(crate::parent_policy::ParentPolicyError::Rollback { have: 0, pinned: 7 })), "{e}");

    // With the pin intact it boots.
    std::fs::write(pin_path(&f), "7").unwrap();
    prepare(&f.paths).unwrap().commit(&r.cm).unwrap();
}

#[test]
fn the_applied_event_records_the_pin_and_a_later_boot_cannot_drop_it() {
    let f = fixture(&base_parent(), None);
    let r = start(&f);
    let applied = |cm: &crate::chain::ChainManager| {
        cm.tail(0)
            .into_iter()
            .rfind(|e| e.kind == "governance.overlay.applied")
            .unwrap()
            .payload
            .unwrap()
    };
    assert_eq!(applied(&r.cm)["user_pin"], false);
    assert_eq!(applied(&r.cm)["user_key_id"].as_str().unwrap().len(), 32);

    let g = fixture(&base_parent(), None);
    write_user_pin(g.paths.root(), &user_key().verifying_key().to_bytes()).unwrap();
    let r = start(&g);
    assert_eq!(applied(&r.cm)["user_pin"], true);
    // The pin goes away; the dev fallback would accept the cert, the history refuses.
    std::fs::remove_file(g.paths.root().join(USER_PIN_FILE)).unwrap();
    let again = prepare(&g.paths).unwrap();
    assert!(matches!(again.commit(&r.cm).unwrap_err(), OverlayError::Cert(_)));
    // A running kernel also refuses policy once its pin is gone.
    let newer = parent_with(base_parent().rules, base_parent().limits, 3);
    assert!(matches!(r.rt.apply_parent_update(newer).unwrap_err(), OverlayError::Cert(_)));
}

#[test]
fn a_swap_keeps_the_per_action_exemptions_of_the_running_gate() {
    use crate::gate::{GateBackend, GateDecision, GovernanceGate};
    use crate::governance::RuleSeverity;
    use crate::governance_overlay_tests::rule;
    let f = fixture(&base_parent(), None);
    let prepared = prepare(&f.paths).unwrap();
    let cm = std::sync::Arc::new(crate::chain::ChainManager::new(0, 1000));
    prepared.commit(&cm).unwrap();
    prepared.install_provider(&cm);
    let (t, h) = prepared.engine_params();
    let mut gate = GovernanceGate::new(t, h)
        .with_chain(cm.clone())
        .with_eval_rate_limit(crate::rate_limit::RateLimitConfig::unlimited())
        .exempt_action("net.fetch");
    for r in prepared.rules().iter().cloned() {
        gate = gate.add_rule(r);
    }
    let (gate, rt) = prepared.into_runtime(gate, cm);
    // Parent update adds a deny on net.fetch; the exemption still applies.
    let mut rules = base_parent().rules;
    rules.push(rule("NET", RuleSeverity::Blocking, Some("net.*"), true, true));
    rt.apply_parent_update(parent_with(rules, base_parent().limits, 2)).unwrap();
    let d = gate.check("a", "net.fetch", &serde_json::json!({}));
    assert!(matches!(d, GateDecision::Permit { .. }), "{d:?}");
    // Other net actions are denied by the new rule.
    let d = gate.check("a", "net.other", &serde_json::json!({}));
    assert!(matches!(d, GateDecision::Deny { .. }), "{d:?}");
}

#[test]
fn a_forged_applied_event_cannot_raise_the_rollback_floor() {
    // Review M3: a `chain.append` caller writes `governance.overlay.applied`
    // with a huge version under a source of its choosing.
    let f = fixture(&parent_with(base_parent().rules, base_parent().limits, 5), None);
    let r = start(&f);
    for source in ["x", "governance.x", "Governance"] {
        r.cm.append(
            source,
            "governance.overlay.applied",
            Some(serde_json::json!({"parent_version": u64::MAX, "user_pin": true, "overlay_hash": "ff"})),
        );
    }
    let h = crate::overlay_trust::chain_history(&r.cm);
    assert_eq!(h.max_parent_version, Some(5), "only the kernel's own record counts");
    assert!(!h.user_pin_used);
    // Boot, reload and a newer push all still work.
    prepare(&f.paths).unwrap().commit(&r.cm).unwrap();
    r.rt.reload().unwrap();
    r.rt.apply_parent_update(parent_with(base_parent().rules, base_parent().limits, 6)).unwrap();
}

#[test]
fn kernel_sources_are_reserved_against_callers_but_replicate() {
    use crate::chain::{is_caller_reserved_source, is_reserved_source};
    for s in ["governance", "project", "project.supervisor", " governance ", "user.projects", "project.anchor"] {
        assert!(is_caller_reserved_source(s), "{s}");
    }
    for s in ["x", "governance.x", "agent", "workload"] {
        assert!(!is_caller_reserved_source(s), "{s}");
    }
    // Replication still accepts governance events (every chain has them).
    assert!(!is_reserved_source("governance"));
}
