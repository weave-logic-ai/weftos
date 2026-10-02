//! `Kernel::boot` with `profile = "project"` (ADR-103 D8, package E): the
//! overlay merge, the process cap, the chain's rule hash and the refusals.

use std::sync::Arc;

use clawft_platform::NativePlatform;
use clawft_types::config::overlay::Limits;
use clawft_types::config::{ChainConfig, Config, KernelConfig, KernelProfile};
use serde_json::json;

use crate::boot::Kernel;
use crate::error::KernelError;
use crate::gate::GateDecision;
use crate::governance_overlay_tests::{base_parent, parent_with};
use crate::overlay_runtime::TEST_CHILD_PATHS;
use crate::overlay_runtime_tests::{Fixture, fixture};

fn kernel_config(dir: &std::path::Path, profile: Option<KernelProfile>) -> KernelConfig {
    KernelConfig {
        enabled: true,
        max_processes: 128,
        profile,
        chain: Some(ChainConfig {
            enabled: true,
            checkpoint_interval: 10_000,
            chain_id: 0,
            checkpoint_path: Some(dir.join("chain.json").to_string_lossy().into_owned()),
            external_anchor: None,
        }),
        ..KernelConfig::default()
    }
}

async fn boot(
    f: Option<&Fixture>,
    kc: KernelConfig,
) -> Result<Kernel<NativePlatform>, KernelError> {
    TEST_CHILD_PATHS.with(|c| *c.borrow_mut() = f.map(|f| f.paths.clone()));
    let r = Kernel::boot(Config::default(), kc, Arc::new(NativePlatform::new())).await;
    TEST_CHILD_PATHS.with(|c| *c.borrow_mut() = None);
    r
}

fn refusal(r: Result<Kernel<NativePlatform>, KernelError>) -> String {
    match r {
        Err(KernelError::BootRefused(m)) => m,
        Err(e) => panic!("expected BootRefused, got {e}"),
        Ok(_) => panic!("boot must be refused"),
    }
}

#[tokio::test]
async fn a_project_kernel_boots_with_the_effective_rules_limits_and_hash() {
    let parent = parent_with(
        base_parent().rules,
        Limits {
            max_processes: Some(64),
            spawn_budget: Some(8),
            ..Limits::default()
        },
        1,
    );
    let f = fixture(
        &parent,
        Some("[[deny]]\nid = \"d1\"\nactions = [\"tool.shell_exec\"]\n[limits]\nmax_processes = 32\n"),
    );
    let t = tempfile::tempdir().unwrap();
    let k = boot(Some(&f), kernel_config(t.path(), Some(KernelProfile::Project)))
        .await
        .map_err(|e| e.to_string())
        .expect("boot");

    // max_processes is enforced by the process table, from the merged limit.
    assert_eq!(k.process_table().max_processes(), 32);
    // spawn_budget (8 from the parent) caps concurrent agent spawns.
    let sub = &k.kernel_config().agent.as_ref().unwrap().subagents;
    assert_eq!(sub.max_per_conv, 5.min(8));

    let rt = k.governance_overlay().expect("overlay runtime");
    let hash = rt.rule_hash().expect("hash");
    assert_eq!(crate::overlay_runtime::prepare(&f.paths).ok().map(|p| p.applied().effective_hash),
        Some(rt.applied().effective_hash));

    // Every chain event carries the effective hash (genesis included? the
    // chain's own seq-0 event predates the provider by construction).
    let cm = k.chain_manager().unwrap();
    let evs = cm.tail(0);
    assert!(evs.len() > 5);
    for e in evs.iter().filter(|e| e.sequence > 0) {
        assert_eq!(e.rule_hash, Some(hash), "{} {}", e.sequence, e.kind);
    }
    // The genesis rules are the effective ones, not the default SOP set.
    let genesis = evs.iter().find(|e| e.kind == "governance.genesis").unwrap();
    let ids: Vec<String> = genesis.payload.as_ref().unwrap()["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_owned())
        .collect();
    assert!(ids.contains(&"d1".to_owned()), "{ids:?}");
    assert!(!ids.iter().any(|i| i.starts_with("SOP-")), "default rules must be replaced");

    // The overlay's deny is enforced and chained under the hash.
    let gate = k.governance_gate().unwrap();
    let d = gate.check("agent-1", "tool.shell_exec", &json!({}));
    assert!(matches!(d, GateDecision::Deny { .. }), "{d:?}");
    assert!(matches!(
        gate.check("agent-1", "tool.read_file", &json!({})),
        GateDecision::Permit { .. }
    ));
    let denied = cm.tail(0).into_iter().rfind(|e| e.kind == "governance.deny").unwrap();
    assert_eq!(denied.rule_hash, Some(hash));
}

#[tokio::test]
async fn a_broken_overlay_refuses_the_boot_with_the_offending_key() {
    let f = fixture(&base_parent(), Some("permit = [\"tool.shell_exec\"]\n"));
    let t = tempfile::tempdir().unwrap();
    let m = refusal(boot(Some(&f), kernel_config(t.path(), Some(KernelProfile::Project))).await);
    assert!(m.contains("`permit`") && m.contains("governance overlay refused"), "{m}");
}

#[tokio::test]
async fn a_tampered_parent_policy_refuses_the_boot() {
    let f = fixture(&base_parent(), None);
    let text = std::fs::read_to_string(f.paths.parent_policy()).unwrap();
    std::fs::write(
        f.paths.parent_policy(),
        text.replacen("\"active\": true", "\"active\": false", 1),
    )
    .unwrap();
    let t = tempfile::tempdir().unwrap();
    let m = refusal(boot(Some(&f), kernel_config(t.path(), Some(KernelProfile::Project))).await);
    assert!(m.contains("parent-policy.rule_hash"), "{m}");
}

#[tokio::test]
async fn the_project_profile_on_a_non_child_root_is_refused() {
    let t = tempfile::tempdir().unwrap();
    let m = refusal(boot(None, kernel_config(t.path(), Some(KernelProfile::Project))).await);
    assert!(m.contains("kernel.profile"), "{m}");
}

#[tokio::test]
async fn a_kernel_without_the_profile_is_unchanged() {
    let t = tempfile::tempdir().unwrap();
    let k = boot(None, kernel_config(t.path(), None)).await.map_err(|e| e.to_string()).expect("boot");
    assert!(k.governance_overlay().is_none());
    assert_eq!(k.process_table().max_processes(), 128);
    assert!(k.chain_manager().unwrap().tail(0).iter().all(|e| e.rule_hash.is_none()));
}
