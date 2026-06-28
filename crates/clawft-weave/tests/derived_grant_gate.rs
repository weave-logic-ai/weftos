//! Integration test: mesh-canonical write gate (R3.6).
//!
//! Boots a daemon-equivalent kernel in-process, issues a `transcript`
//! grant for the daemon node, and verifies the substrate write gate
//! splits by tier:
//!
//! - With the grant in place, the daemon can publish at
//!   `substrate/_derived/transcript/<source>/mic`.
//! - Without the grant, the same publish is rejected with the
//!   `MissingDerivedGrant` error and a clean message.
//!
//! Lives in `clawft-weave/tests/` rather than `clawft-kernel` because
//! it exercises the daemon's grant-issuance flow (the kernel's lib
//! tests already cover the registry + gate in isolation).

use std::sync::Arc;

use clawft_kernel::boot::Kernel;
use clawft_kernel::{GateDenied, GrantScope};
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
        max_processes: 64,
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
        llm: None,
        agent: None,
    }
}

/// Boot a kernel in-process and register a synthetic daemon node.
/// Returns the kernel and the daemon node id so tests can drive the
/// gate the same way the production daemon does.
async fn boot_with_daemon_node() -> (Kernel<NativePlatform>, String) {
    let platform = NativePlatform::new();
    let kernel = Kernel::boot(base_config(), minimal_kernel_config(), Arc::new(platform))
        .await
        .expect("kernel boot");
    // Synthetic pubkey — we only need the registry entry; signature
    // verification is not on this path (in-process publish).
    let pubkey = [11u8; 32];
    let node = kernel
        .node_registry()
        .register(pubkey, Some("test-daemon".into()));
    (kernel, node.node_id)
}

#[tokio::test]
async fn derived_publish_succeeds_with_grant() {
    let (kernel, daemon_id) = boot_with_daemon_node().await;
    // Daemon issues itself a transcript grant — exact mirror of what
    // `clawft_weave::daemon` does at boot.
    kernel
        .node_registry()
        .issue_derived_grant(&daemon_id, "transcript", GrantScope::TopicPrefix)
        .expect("grant accepted");

    let path = "substrate/_derived/transcript/n-source/mic";
    let tick = kernel
        .substrate_service()
        .publish_gated_with_grants(
            Some(&daemon_id),
            path,
            serde_json::json!({"text": "hello mesh"}),
            kernel.node_registry(),
        )
        .expect("publish must succeed when grant is in place");
    // Defensive: tick may be > 1 if any future kernel-boot subsystem
    // bumps it before this test runs. We only care that the publish
    // landed.
    assert!(tick > 0);

    // Read back through the public surface — proves the value
    // actually landed at the canonical path.
    let snap = kernel
        .substrate_service()
        .read(None, path)
        .expect("read past egress");
    assert_eq!(snap.value, Some(serde_json::json!({"text": "hello mesh"})));
}

#[tokio::test]
async fn derived_publish_fails_without_grant() {
    let (kernel, daemon_id) = boot_with_daemon_node().await;
    // No grant issued.

    let path = "substrate/_derived/transcript/n-source/mic";
    let err = kernel
        .substrate_service()
        .publish_gated_with_grants(
            Some(&daemon_id),
            path,
            serde_json::json!({"text": "denied"}),
            kernel.node_registry(),
        )
        .expect_err("publish must reject without grant");
    let err_msg = err.to_string();
    match &err {
        GateDenied::MissingDerivedGrant {
            path: p,
            node_id: n,
        } => {
            assert_eq!(p, path);
            assert_eq!(n, &daemon_id);
            // Spec asks for a "clean error string" — verify the
            // Display impl actually mentions the relevant pieces so
            // operator log triage is unambiguous.
            assert!(
                err_msg.contains(&daemon_id)
                    && err_msg.contains(path)
                    && err_msg.contains("DerivedWriteGrant"),
                "error string must name the node, path, and grant type: {err_msg}"
            );
        }
        other => panic!("expected MissingDerivedGrant, got {other:?}"),
    }

    // The path must NOT have been written. A subsequent read returns
    // the empty snapshot (tick 0, no value).
    let snap = kernel
        .substrate_service()
        .read(None, path)
        .expect("read past egress");
    assert!(snap.value.is_none());
    assert_eq!(snap.tick, 0);
}

#[tokio::test]
async fn grant_for_one_topic_does_not_apply_to_another() {
    let (kernel, daemon_id) = boot_with_daemon_node().await;
    // Grant only `transcript`; attempt a `classify` write. R3.6
    // mandates path-bounded grants — compromising one pipeline
    // must not unlock another's namespace.
    kernel
        .node_registry()
        .issue_derived_grant(&daemon_id, "transcript", GrantScope::TopicPrefix)
        .expect("grant accepted");

    let err = kernel
        .substrate_service()
        .publish_gated_with_grants(
            Some(&daemon_id),
            "substrate/_derived/classify/n-source/mic",
            serde_json::json!({"label": "speech"}),
            kernel.node_registry(),
        )
        .expect_err("transcript grant must not unlock classify topic");
    assert!(matches!(err, GateDenied::MissingDerivedGrant { .. }));
}
