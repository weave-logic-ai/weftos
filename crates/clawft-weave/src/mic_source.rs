//! Mic source node resolution for the whisper / classify pipeline.
//!
//! Node ids are derived from each node's key (ADR-103 D11), so no node id
//! can be baked in as a default. The source is either an explicit override
//! (`WHISPER_INPUT_NODE_ID`) or the registered node that publishes
//! `substrate/<id>/sensor/mic/...`, resolved at runtime and re-checked as
//! nodes register.

use std::sync::Arc;
use std::time::Duration;

use clawft_kernel::{Kernel, NodeRegistry, SubstrateService};
use clawft_platform::NativePlatform;
use tracing::{info, warn};

/// Env var that pins the mic node id, bypassing discovery.
pub const MIC_NODE_ENV: &str = "WHISPER_INPUT_NODE_ID";

const POLL: Duration = Duration::from_secs(5);
const WARN_EVERY: Duration = Duration::from_secs(60);

/// The first registered node (sorted by id) that has published anything
/// under `substrate/<id>/sensor/mic`.
///
/// The probe lists each candidate's own subtree *as that node*: the sensor
/// ACL admits the owning node, and this is a daemon-internal existence
/// check, not a read of the data.
pub fn resolve_mic_node(registry: &NodeRegistry, substrate: &SubstrateService) -> Option<String> {
    let mut ids: Vec<String> = registry.list().into_iter().map(|n| n.node_id).collect();
    ids.sort();
    ids.into_iter().find(|id| {
        let prefix = format!("substrate/{id}/sensor/mic");
        substrate
            .list(Some(id.as_str()), &prefix, 1)
            .map(|snap| !snap.children.is_empty())
            .unwrap_or(false)
    })
}

/// Return the pinned override if set and non-empty; otherwise wait until a
/// registered node publishes `sensor/mic`, warning loudly while none does.
pub async fn wait_for_mic_node(
    override_id: Option<String>,
    kernel: &Arc<tokio::sync::RwLock<Kernel<NativePlatform>>>,
) -> String {
    if let Some(id) = override_id.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        info!(node = %id, env = MIC_NODE_ENV, "mic source pinned by env override");
        return id;
    }
    let mut last_warn: Option<std::time::Instant> = None;
    loop {
        let found = {
            let k = kernel.read().await;
            resolve_mic_node(k.node_registry(), k.substrate_service())
        };
        if let Some(id) = found {
            info!(node = %id, "mic source resolved from node registry (sensor/mic publisher)");
            return id;
        }
        if last_warn.map_or(true, |t| t.elapsed() >= WARN_EVERY) {
            warn!(
                env = MIC_NODE_ENV,
                "no registered node is publishing sensor/mic; whisper and classify are NOT running. \
                 Waiting for a mic node to register, or set {MIC_NODE_ENV}=<node-id> to pin one"
            );
            last_warn = Some(std::time::Instant::now());
        }
        tokio::time::sleep(POLL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(reg: &NodeRegistry, seed: u8) -> String {
        reg.register([seed; 32], None).node_id
    }

    #[test]
    fn resolves_the_node_that_publishes_mic_not_a_hardcoded_id() {
        let reg = NodeRegistry::new();
        let sub = SubstrateService::new();
        let daemon = node(&reg, 1);
        let mic = node(&reg, 2);
        assert!(clawft_kernel::is_node_id(&mic));
        assert_eq!(resolve_mic_node(&reg, &sub), None, "nothing published yet");

        // A registered node with no mic path is ignored.
        sub.publish(Some(&daemon), &format!("substrate/{daemon}/health"), serde_json::json!({}));
        assert_eq!(resolve_mic_node(&reg, &sub), None);

        sub.publish(
            Some(&mic),
            &format!("substrate/{mic}/sensor/mic/pcm_chunk"),
            serde_json::json!({"n": 1}),
        );
        assert_eq!(resolve_mic_node(&reg, &sub), Some(mic));
    }

    #[test]
    fn unregistered_publisher_is_not_resolved() {
        let reg = NodeRegistry::new();
        let sub = SubstrateService::new();
        let daemon = node(&reg, 1);
        sub.publish(
            Some("stranger"),
            "substrate/00000000000000000000000000000000/sensor/mic/rms",
            serde_json::json!({}),
        );
        assert_eq!(resolve_mic_node(&reg, &sub), None);
    }
}
