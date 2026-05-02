//! Feature composition integration tests.
//!
//! These tests verify that combining different feature flags produces
//! a bootable, functional kernel without service conflicts or panics.

use std::sync::Arc;

use clawft_kernel::boot::{Kernel, KernelState};
use clawft_platform::NativePlatform;
use clawft_types::config::{AgentDefaults, AgentsConfig, Config, KernelConfig};

fn base_config() -> Config {
    Config {
        agents: AgentsConfig {
            defaults: AgentDefaults {
                workspace: "~/.clawft/workspace".into(),
                model: "test/model".into(),
                max_tokens: 1024,
                temperature: 0.5,
                max_tool_iterations: 5,
                memory_window: 10,
            },
            ..AgentsConfig::default()
        },
        ..Config::default()
    }
}

fn minimal_kernel_config() -> KernelConfig {
    KernelConfig {
        enabled: true,
        max_processes: 16,
        health_check_interval_secs: 5,
        cluster: None,
        chain: None,
        resource_tree: None,
        vector: None,
        profiles: None,
        pairing: None,
        mesh: None,
        anchor: None,
        ipc_tcp: None,
    }
}

#[cfg(feature = "exochain")]
fn exochain_kernel_config() -> KernelConfig {
    use clawft_types::config::{ChainConfig, ResourceTreeConfig};
    KernelConfig {
        enabled: true,
        max_processes: 32,
        health_check_interval_secs: 5,
        cluster: None,
        chain: Some(ChainConfig {
            enabled: true,
            checkpoint_interval: 10_000,
            chain_id: 0,
            checkpoint_path: None,
        }),
        resource_tree: Some(ResourceTreeConfig {
            enabled: true,
            checkpoint_path: None,
        }),
        vector: None,
        profiles: None,
        pairing: None,
        mesh: None,
        anchor: None,
        ipc_tcp: None,
    }
}

// ── Test: native feature alone boots and runs ────────────────────

#[tokio::test]
async fn feature_comp_native_boots() {
    let platform = Arc::new(NativePlatform::new());
    let mut kernel = Kernel::boot(base_config(), minimal_kernel_config(), platform)
        .await
        .unwrap();
    assert_eq!(*kernel.state(), KernelState::Running);
    assert!(!kernel.process_table().is_empty(), "kernel process must exist");
    assert!(kernel.services().len() >= 2, "cron + containers at minimum");
    kernel.shutdown().await.unwrap();
    assert_eq!(*kernel.state(), KernelState::Halted);
}

// ── Test: native + exochain boots with chain + tree ──────────────

#[cfg(feature = "exochain")]
#[tokio::test]
async fn feature_comp_native_exochain_boots() {
    let platform = Arc::new(NativePlatform::new());
    let mut kernel = Kernel::boot(base_config(), exochain_kernel_config(), platform)
        .await
        .unwrap();
    assert_eq!(*kernel.state(), KernelState::Running);

    // Chain and tree should be present
    assert!(kernel.chain_manager().is_some(), "chain manager required with exochain");
    assert!(kernel.tree_manager().is_some(), "tree manager required with exochain");

    // Governance gate should be wired
    assert!(kernel.governance_gate().is_some(), "governance gate required with chain");

    kernel.shutdown().await.unwrap();
    assert_eq!(*kernel.state(), KernelState::Halted);
}

// ── Test: native + ecc boots with cognitive substrate ────────────

#[cfg(feature = "ecc")]
#[tokio::test]
async fn feature_comp_native_ecc_boots() {
    let platform = Arc::new(NativePlatform::new());
    let mut kernel = Kernel::boot(base_config(), minimal_kernel_config(), platform)
        .await
        .unwrap();
    assert_eq!(*kernel.state(), KernelState::Running);

    // ECC subsystems should be present
    assert!(kernel.ecc_hnsw().is_some(), "HNSW required with ecc");
    assert!(kernel.ecc_tick().is_some(), "cognitive tick required with ecc");
    assert!(kernel.ecc_causal().is_some(), "causal graph required with ecc");
    assert!(kernel.ecc_crossrefs().is_some(), "crossref store required with ecc");
    assert!(kernel.ecc_impulses().is_some(), "impulse queue required with ecc");
    assert!(kernel.ecc_calibration().is_some(), "calibration required with ecc");

    kernel.shutdown().await.unwrap();
}

// ── Test: native + exochain + ecc boots together ─────────────────

#[cfg(all(feature = "exochain", feature = "ecc"))]
#[tokio::test]
async fn feature_comp_exochain_ecc_boots() {
    let platform = Arc::new(NativePlatform::new());
    let mut kernel = Kernel::boot(base_config(), exochain_kernel_config(), platform)
        .await
        .unwrap();
    assert_eq!(*kernel.state(), KernelState::Running);

    // Both exochain and ECC subsystems present
    assert!(kernel.chain_manager().is_some());
    assert!(kernel.tree_manager().is_some());
    assert!(kernel.ecc_hnsw().is_some());
    assert!(kernel.ecc_tick().is_some());
    assert!(kernel.governance_gate().is_some());

    // ECC calibration logged to chain
    let chain = kernel.chain_manager().unwrap();
    let events = chain.tail(50);
    let ecc_events: Vec<_> = events.iter().filter(|e| e.kind.starts_with("ecc.")).collect();
    assert!(!ecc_events.is_empty(), "ECC calibration should be logged to chain");

    kernel.shutdown().await.unwrap();
}

// ── Test: services from different features don't conflict ────────

#[tokio::test]
async fn feature_comp_no_service_name_conflicts() {
    let platform = Arc::new(NativePlatform::new());
    let kernel = Kernel::boot(base_config(), minimal_kernel_config(), platform)
        .await
        .unwrap();

    let services = kernel.services().list();
    let mut names: Vec<&str> = services.iter().map(|(n, _)| n.as_str()).collect();
    let total = names.len();
    names.sort();
    names.dedup();
    assert_eq!(
        names.len(),
        total,
        "service names must be unique -- duplicate detected"
    );
}

// ── Test: IPC works after boot (message routing) ─────────────────

#[tokio::test]
async fn feature_comp_ipc_message_routing() {
    use clawft_kernel::ipc::{KernelMessage, MessagePayload, MessageTarget};

    let platform = Arc::new(NativePlatform::new());
    let kernel = Kernel::boot(base_config(), minimal_kernel_config(), platform)
        .await
        .unwrap();

    let a2a = kernel.a2a_router();
    // Create inbox for kernel PID 0
    let mut inbox = a2a.create_inbox(0);

    // Spawn a test agent via process table + inbox
    use clawft_kernel::process::{ProcessEntry, ProcessState, ResourceUsage};
    use clawft_kernel::capability::AgentCapabilities;

    let agent_entry = ProcessEntry {
        pid: 0, // auto-assigned
        agent_id: "ipc-test-agent".into(),
        state: ProcessState::Running,
        capabilities: AgentCapabilities::default(),
        resource_usage: ResourceUsage::default(),
        cancel_token: tokio_util::sync::CancellationToken::new(),
        parent_pid: None,
    };
    let agent_pid = kernel.process_table().insert(agent_entry).unwrap();
    let _agent_inbox = a2a.create_inbox(agent_pid);

    // Send message from agent to kernel
    let msg = KernelMessage::new(
        agent_pid,
        MessageTarget::Process(0),
        MessagePayload::Json(serde_json::json!({"test": "hello"})),
    );
    a2a.send(msg).await.unwrap();

    // Kernel inbox should receive the message
    let received = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        inbox.recv(),
    )
    .await
    .unwrap()
    .unwrap();

    if let MessagePayload::Json(v) = &received.payload {
        assert_eq!(v["test"], "hello");
    } else {
        panic!("expected JSON payload");
    }
}

// ── Test: process table correctly isolated per boot ──────────────

#[tokio::test]
async fn feature_comp_process_table_isolated() {
    let platform = Arc::new(NativePlatform::new());
    let kernel = Kernel::boot(base_config(), minimal_kernel_config(), platform)
        .await
        .unwrap();

    // Only kernel process at PID 0
    let processes = kernel.process_table().list();
    assert_eq!(processes.len(), 1);
    assert_eq!(processes[0].pid, 0);
    assert_eq!(processes[0].agent_id, "kernel");
}

// ── Test: cluster membership initialized regardless of feature ───

#[tokio::test]
async fn feature_comp_cluster_membership_initialized() {
    let platform = Arc::new(NativePlatform::new());
    let kernel = Kernel::boot(base_config(), minimal_kernel_config(), platform)
        .await
        .unwrap();

    let cm = kernel.cluster_membership();
    assert!(!cm.local_node_id().is_empty(), "node ID must be set");
}

// ── Test: exochain + ecc + mesh resource tree has ECC namespaces ─

#[cfg(all(feature = "exochain", feature = "ecc"))]
#[tokio::test]
async fn feature_comp_ecc_namespaces_in_tree() {
    let platform = Arc::new(NativePlatform::new());
    let kernel = Kernel::boot(base_config(), exochain_kernel_config(), platform)
        .await
        .unwrap();

    let tree = kernel.tree_manager().unwrap();
    let stats = tree.stats();
    // With ECC namespaces registered, we should have a reasonable node count
    assert!(
        stats.node_count > 10,
        "tree should have >10 nodes with ECC namespaces, got {}",
        stats.node_count
    );
}

// ── Test: shutdown after spawn cleans up ──────────────────────────

#[tokio::test]
async fn feature_comp_shutdown_cleans_spawned_agents() {
    let platform = Arc::new(NativePlatform::new());
    let mut kernel = Kernel::boot(base_config(), minimal_kernel_config(), platform)
        .await
        .unwrap();

    // Spawn an agent via supervisor
    let spawn_result = kernel.supervisor().spawn(
        clawft_kernel::supervisor::SpawnRequest {
            agent_id: "cleanup-test-agent".into(),
            capabilities: None,
            parent_pid: None,
            env: std::collections::HashMap::new(),
            backend: None,
        },
    );
    assert!(spawn_result.is_ok());

    kernel.shutdown().await.unwrap();
    assert_eq!(*kernel.state(), KernelState::Halted);

    // After shutdown, all non-kernel processes should be Exited
    let processes = kernel.process_table().list();
    for p in &processes {
        if p.pid != 0 {
            assert!(
                matches!(p.state, clawft_kernel::ProcessState::Exited(_)),
                "PID {} should be Exited after shutdown, got {:?}",
                p.pid,
                p.state
            );
        }
    }
}
