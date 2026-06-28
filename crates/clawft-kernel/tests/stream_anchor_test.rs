//! Integration test: StreamWindowAnchor chain-appends per window.
//!
//! Boots a kernel with `exochain` enabled, starts a window anchor
//! on a test topic, publishes ~20 messages over ~3 seconds, and
//! asserts:
//! - at least one `stream.window_commit` event shows up on the chain
//! - the aggregated sample_count across windows is >= the publish
//!   count
//! - the blake3 hash is non-zero

#![cfg(all(feature = "native", feature = "exochain"))]

use std::sync::Arc;
use std::time::Duration;

use clawft_kernel::boot::Kernel;
use clawft_kernel::{KernelMessage, MessagePayload, MessageTarget, StreamWindowAnchor};
use clawft_platform::NativePlatform;
use clawft_types::config::{
    AgentDefaults, AgentsConfig, ChainConfig, Config, KernelConfig, ResourceTreeConfig,
};

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

fn kernel_config_with_chain() -> KernelConfig {
    KernelConfig {
        enabled: true,
        max_processes: 64,
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
        llm: None,
        agent: None,
    }
}

#[tokio::test]
async fn anchor_emits_window_commits_with_hash_and_count() {
    let platform = Arc::new(NativePlatform::new());
    let kernel = Kernel::boot(base_config(), kernel_config_with_chain(), platform)
        .await
        .unwrap();

    let a2a = kernel.a2a_router().clone();
    let chain = kernel
        .chain_manager()
        .cloned()
        .expect("chain manager required for this test");

    // Short window so the test stays quick.
    let anchor = StreamWindowAnchor::start_topic(
        Arc::clone(&a2a),
        Some(chain.clone()),
        "sensor.test".to_string(),
        Duration::from_millis(500),
    );

    // Publish 20 messages spread over ~1.5 seconds.
    for i in 0..20u32 {
        let msg = KernelMessage::new(
            0,
            MessageTarget::Topic("sensor.test".into()),
            MessagePayload::Json(serde_json::json!({"i": i})),
        );
        a2a.send(msg).await.unwrap();
        tokio::time::sleep(Duration::from_millis(75)).await;
    }

    // Give the last window a chance to flush.
    tokio::time::sleep(Duration::from_millis(700)).await;
    anchor.shutdown();

    // Drain + inspect chain.
    let events = chain.tail(200);
    let commits: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "stream.window_commit")
        .collect();
    assert!(
        !commits.is_empty(),
        "expected at least one stream.window_commit, got: {}",
        events
            .iter()
            .map(|e| e.kind.as_str())
            .collect::<Vec<_>>()
            .join(",")
    );

    let mut total_samples: u64 = 0;
    for commit in &commits {
        let payload = commit
            .payload
            .as_ref()
            .expect("stream.window_commit must carry a payload");
        let topic = payload["topic"].as_str().unwrap_or_default();
        assert_eq!(topic, "sensor.test");
        let n = payload["sample_count"].as_u64().unwrap_or_default();
        total_samples += n;
        let hash_hex = payload["blake3"].as_str().unwrap_or_default();
        assert_eq!(hash_hex.len(), 64, "blake3 hex should be 32 bytes");
        assert!(
            hash_hex.chars().any(|c| c != '0'),
            "blake3 hash must not be all zeroes"
        );
    }
    assert!(
        total_samples >= 20,
        "expected >= 20 samples anchored across windows, got {total_samples}"
    );
}
