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
use crate::overlay_runtime_tests::{Fixture, fixture, project_key};

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

/// Boot as the daemon does: a child boots with its project key as the node
/// key seed (what `pre_boot` loaded).
async fn boot(
    f: Option<&Fixture>,
    kc: KernelConfig,
) -> Result<Kernel<NativePlatform>, KernelError> {
    boot_seeded(f, kc, f.map(|_| project_key().to_bytes())).await
}

async fn boot_seeded(
    f: Option<&Fixture>,
    kc: KernelConfig,
    seed: Option<[u8; 32]>,
) -> Result<Kernel<NativePlatform>, KernelError> {
    TEST_CHILD_PATHS.with(|c| *c.borrow_mut() = f.map(|f| f.paths.clone()));
    let r = Kernel::boot_with_node_key(Config::default(), kc, Arc::new(NativePlatform::new()), seed).await;
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

#[tokio::test]
async fn a_project_chain_is_signed_by_the_project_key_and_nothing_else() {
    let f = fixture(&base_parent(), None);
    let t = tempfile::tempdir().unwrap();
    let k = boot(Some(&f), kernel_config(t.path(), Some(KernelProfile::Project)))
        .await
        .map_err(|e| e.to_string())
        .expect("boot");
    let cm = k.chain_manager().unwrap();
    assert_eq!(
        cm.verifying_key().map(|v| v.to_bytes()),
        Some(project_key().verifying_key().to_bytes())
    );
    assert!(!t.path().join("chain.key").exists(), "no chain.key beside the checkpoint");
}

#[tokio::test]
async fn a_project_kernel_without_its_key_or_with_a_swapped_key_is_refused() {
    let kc = |t: &tempfile::TempDir| kernel_config(t.path(), Some(KernelProfile::Project));
    // No node key seed: pre_boot's key never reached boot.
    let f = fixture(&base_parent(), None);
    let t = tempfile::tempdir().unwrap();
    let m = refusal(boot_seeded(Some(&f), kc(&t), None).await);
    assert!(m.contains("project key"), "{m}");
    assert!(!t.path().join("chain.key").exists());
    // project.key removed between pre_boot and boot: refused, not created.
    std::fs::remove_file(f.paths.project_key().unwrap()).unwrap();
    let t = tempfile::tempdir().unwrap();
    let m = refusal(boot(Some(&f), kc(&t)).await);
    assert!(m.contains("does not exist"), "{m}");
    assert!(!f.paths.project_key().unwrap().exists(), "boot must not create a key");
    // project.key swapped for another key: refused (the certificate check or
    // the chain-key check, whichever runs first).
    let f = fixture(&base_parent(), None);
    crate::parent_policy::write_atomic_0600(&f.paths.project_key().unwrap(), &[9u8; 32]).unwrap();
    let t = tempfile::tempdir().unwrap();
    refusal(boot(Some(&f), kc(&t)).await);
    // A seed that is not the certified key (file intact): refused.
    let f = fixture(&base_parent(), None);
    let t = tempfile::tempdir().unwrap();
    let m = refusal(boot_seeded(Some(&f), kc(&t), Some([9u8; 32])).await);
    assert!(m.contains("no longer holds"), "{m}");
}

#[tokio::test]
async fn an_overlay_human_flag_leaves_parent_denies_denied_on_the_tool_gate() {
    // Review M1, end to end on the child's gate (the agent tool path): a
    // Defer there is an approvable prompt, so a parent deny must stay Deny.
    let f = fixture(&base_parent(), Some("[limits]\nhuman_approval_required = true\n"));
    let t = tempfile::tempdir().unwrap();
    let k = boot(Some(&f), kernel_config(t.path(), Some(KernelProfile::Project)))
        .await
        .map_err(|e| e.to_string())
        .expect("boot");
    let gate = k.governance_gate().unwrap();
    for action in ["workload.place", "workload.start"] {
        let d = gate.check("agent-1", action, &json!({}));
        assert!(matches!(d, GateDecision::Deny { .. }), "{action}: {d:?}");
    }
    assert!(matches!(gate.check("agent-1", "tool.read_file", &json!({})), GateDecision::Permit { .. }));
}
