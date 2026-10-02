//! Parent-policy file handling and the gate swap under concurrency
//! (ADR-103 D8, package E).

use std::path::PathBuf;
use std::sync::Arc;

use clawft_types::config::overlay::Limits;

use crate::gate::{GateBackend, GateDecision};
use crate::governance::RuleSeverity;
use crate::governance_overlay_tests::{base_parent, parent_with, rule, user_key};
use crate::overlay_runtime_tests::{fixture, start};
use crate::parent_policy::write_atomic_0600;
use serde_json::json;

fn check(g: &Arc<dyn GateBackend>, action: &str) -> GateDecision {
    g.check("agent-1", action, &json!({}))
}

#[test]
fn write_atomic_is_private_and_leaves_no_temp_file() {
    let t = tempfile::tempdir().unwrap();
    let p: PathBuf = t.path().join("sub").join("f.json");
    write_atomic_0600(&p, b"one").unwrap();
    write_atomic_0600(&p, b"two").unwrap();
    assert_eq!(std::fs::read(&p).unwrap(), b"two");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let names: Vec<_> = std::fs::read_dir(p.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, ["f.json"]);
}

#[test]
fn export_to_continues_the_version_and_never_goes_backwards() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("run").join("parent-policy.json");
    let engine = crate::governance::GovernanceEngine::new(0.8, false);
    let a = crate::parent_policy::export_to(&path, &engine, &Limits::default(), &user_key()).unwrap();
    let b = crate::parent_policy::export_to(&path, &engine, &Limits::default(), &user_key()).unwrap();
    assert!(b.version > a.version);
    let loaded = crate::parent_policy::load_parent_policy(&path).unwrap();
    assert_eq!(loaded.version, b.version);
    crate::parent_policy::verify_parent_policy(&loaded, &user_key().verifying_key().to_bytes())
        .unwrap();
}

// ── a decision is never chained with the other generation's hash ─────────

#[test]
fn concurrent_checks_and_swaps_never_pair_a_decision_with_the_wrong_hash() {
    // Policy A denies net.fetch; policy B permits it. Whatever interleaving,
    // a `governance.deny` event must carry A's hash and a `governance.permit`
    // event B's.
    let a_rules = {
        let mut r = base_parent().rules;
        r.push(rule("NET", RuleSeverity::Blocking, Some("net.*"), true, true));
        r
    };
    let policy_a = |v| parent_with(a_rules.clone(), base_parent().limits, v);
    let policy_b = |v| parent_with(base_parent().rules, base_parent().limits, v);
    let f = fixture(&policy_a(1), None);
    let r = start(&f);
    let hash_a = r.rt.rule_hash().unwrap();
    r.rt.apply_parent_update(policy_b(2)).unwrap();
    let hash_b = r.rt.rule_hash().unwrap();
    assert_ne!(hash_a, hash_b);

    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let checkers: Vec<_> = (0..3)
        .map(|_| {
            let (g, stop) = (Arc::clone(&r.gate), Arc::clone(&stop));
            std::thread::spawn(move || {
                let mut n = 0u32;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) && n < 40_000 {
                    check(&g, "net.fetch");
                    n += 1;
                }
            })
        })
        .collect();
    for v in 3..60u64 {
        let p = if v % 2 == 1 { policy_a(v) } else { policy_b(v) };
        r.rt.apply_parent_update(p).unwrap();
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    for c in checkers {
        c.join().unwrap();
    }

    let (mut denies, mut permits) = (0, 0);
    for e in r.cm.tail(0) {
        match e.kind.as_str() {
            "governance.deny" => {
                denies += 1;
                assert_eq!(e.rule_hash, Some(hash_a), "deny chained with the wrong hash");
            }
            "governance.permit" => {
                permits += 1;
                assert_eq!(e.rule_hash, Some(hash_b), "permit chained with the wrong hash");
            }
            _ => {}
        }
    }
    assert!(denies > 0 && permits > 0, "both generations must have been exercised");
}
