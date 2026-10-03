//! Daemon boot glue for the machine mesh service (ADR-103 P3-U): decide the
//! mesh mode and node identity before the kernel boots, then start the link
//! once the kernel is up.

use std::path::Path;
use std::sync::Arc;

use clawft_kernel::boot::Kernel;
use clawft_platform::NativePlatform;
use clawft_types::config::{KernelConfig, MeshServicePolicy};
use serde_json::Value;
use tokio::sync::watch;

use crate::mesh_local_chain::{ChainQueue, ChainSink};
use crate::mesh_local_glue::{
    self, LinkDeps, LinkHandle, Resolved, ServiceLink, Timings, build_endpoint_async,
};
use crate::mesh_state;
use crate::node_identity::{self, DaemonIdentity};

const BUILD_SHA: &str = env!("BUILD_GIT_HASH");

/// The decision and the identity it implies.
pub struct MeshBoot {
    /// Who this daemon is on the mesh.
    pub identity: DaemonIdentity,
    /// The registered service link, in service mode.
    pub link: Option<Box<ServiceLink>>,
}

/// Decide the mesh mode and load the node identity.
///
/// Service mode never reads `<runtime>/node.key` and never generates a key:
/// the identity is the service's public identity. Every failure is a boot
/// failure with its reason; nothing falls back to a locally generated key.
pub async fn prepare(kernel_config: &KernelConfig, runtime_dir: &Path) -> anyhow::Result<MeshBoot> {
    let cfg = kernel_config.mesh.clone().unwrap_or_default();
    let local = || -> anyhow::Result<DaemonIdentity> {
        node_identity::load_or_generate(runtime_dir)
            .map_err(|e| anyhow::anyhow!("daemon identity bootstrap: {e}"))
    };
    if !crate::user_daemon::is_active() {
        // Service mode belongs to the user daemon; project and legacy daemons
        // keep their own mesh (a project kernel never talks to the service).
        if cfg.service == MeshServicePolicy::Required && cfg.enabled {
            anyhow::bail!(
                "kernel.mesh.service = \"required\" is only supported by the user daemon \
                 (--profile user); this daemon would run its own mesh listener"
            );
        }
        mesh_state::global().set(mesh_state::plain(if cfg.enabled { "collapsed" } else { "off" }));
        return Ok(MeshBoot { identity: local()?, link: None });
    }
    let home = crate::user_daemon::require_home()?;
    let endpoint = build_endpoint_async(&cfg, &home, BUILD_SHA).await;
    let resolved = mesh_local_glue::resolve(&cfg, endpoint)
        .await
        .map_err(|why| anyhow::anyhow!("mesh: {why}"))?;
    match resolved {
        Resolved::Service(link) => {
            // Roles and mesh mode in the handshake are right from here on.
            link.publish_state(mesh_state::global());
            let identity = link.identity().map_err(|e| anyhow::anyhow!("mesh service identity: {e}"))?;
            Ok(MeshBoot { identity, link: Some(link) })
        }
        other => {
            if matches!(other, Resolved::Collapsed) {
                let pinned = clawft_types::runtime_paths::user_weftos_dir(&home).join("mesh/machine.pub");
                refuse_fresh_identity(cfg.service, pinned.exists(), runtime_dir.join(clawft_kernel::NODE_KEY_FILE).exists())?;
            }
            mesh_local_glue::record_plain_mode(mesh_state::global(), &other);
            if matches!(other, Resolved::Collapsed) && cfg.service == MeshServicePolicy::Auto {
                // Booted without a service: keep watching, never switch live.
                mesh_local_glue::watch_for_service(
                    clawft_kernel::mesh_mode::service_socket(&cfg),
                    std::time::Duration::from_secs(30),
                );
            }
            Ok(MeshBoot { identity: local()?, link: None })
        }
    }
}

/// Review S2: under `auto`, a machine that has used the service (the machine
/// key is pinned) and no longer has a local `node.key` would get a freshly
/// generated node id when the service is down: an identity fork. Refuse; the
/// operator chooses `required` (wait for the service) or `off` (collapsed,
/// knowingly with a new key).
fn refuse_fresh_identity(policy: MeshServicePolicy, pinned: bool, has_node_key: bool) -> anyhow::Result<()> {
    if policy == MeshServicePolicy::Auto && pinned && !has_node_key {
        anyhow::bail!(
            "mesh: this machine has used the machine mesh service (its key is pinned in \
             ~/.weftos/mesh/machine.pub) but the service does not answer, and there is no local node.key. \
             Collapsing now would generate a NEW node id that peers do not know. Start the service \
             (weaver mesh status), or set kernel.mesh.service = \"required\" to wait for it; set \
             kernel.mesh.service = \"off\" only if you want a collapsed daemon with a new node id"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_never_mints_a_node_key_on_a_machine_that_used_the_service() {
        assert!(refuse_fresh_identity(MeshServicePolicy::Auto, true, false).is_err());
        assert!(refuse_fresh_identity(MeshServicePolicy::Auto, true, true).is_ok(), "rollback keeps its key");
        assert!(refuse_fresh_identity(MeshServicePolicy::Auto, false, false).is_ok(), "a new machine");
        assert!(refuse_fresh_identity(MeshServicePolicy::Off, true, false).is_ok(), "off is a deliberate choice");
    }
}

/// Chain sink used when the kernel has no chain: nothing can be appended, so
/// events stay queued (bounded) and the doctor/handshake show the link anyway.
struct NoChain;

impl ChainSink for NoChain {
    fn append(&self, _: &str, _: Value) -> Result<(), String> {
        Err("no chain manager".into())
    }
}

/// Start the link over an established [`ServiceLink`] once the kernel is up.
pub async fn start_link(
    link: Box<ServiceLink>,
    kernel: &Arc<tokio::sync::RwLock<Kernel<NativePlatform>>>,
) -> LinkHandle {
    let k = kernel.read().await;
    let delivery: Arc<dyn clawft_kernel::mesh_delivery::LocalDelivery> = k.a2a_router().clone();
    // The cog mesh takes its own topics from stamped deliveries; the rest go on.
    #[cfg(all(feature = "ecc", feature = "exochain"))]
    let delivery = crate::cog_swarm::wrap(delivery);
    #[cfg(feature = "exochain")]
    let chain = match k.chain_manager() {
        Some(cm) => ChainQueue::new(cm.clone()),
        None => ChainQueue::new(NoChain),
    };
    #[cfg(not(feature = "exochain"))]
    let chain = ChainQueue::new(NoChain);
    let handle = mesh_local_glue::spawn(
        link,
        LinkDeps {
            delivery,
            gate: k.governance_gate().cloned(),
            chain: Arc::new(chain),
            state: mesh_state::global().clone(),
            timings: Timings::default(),
        },
    );
    #[cfg(all(feature = "ecc", feature = "exochain"))]
    crate::cog_swarm::set_forwarder(handle.forwarder.clone());
    k.a2a_router().set_remote_forwarder(handle.forwarder.clone());
    handle
}

/// Stop `handle` when the daemon's shutdown signal fires.
pub fn stop_on_shutdown(handle: LinkHandle, mut rx: watch::Receiver<bool>) {
    tokio::spawn(async move {
        while !*rx.borrow() {
            if rx.changed().await.is_err() {
                break;
            }
        }
        handle.shutdown().await;
    });
}
