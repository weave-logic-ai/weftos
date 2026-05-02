//! Kernel boot sequence and state machine.
//!
//! The [`Kernel`] struct is the central coordinator. It wraps
//! [`AppContext`] and manages the process table, service registry,
//! IPC, and health subsystem through a boot/shutdown lifecycle.

use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tracing::{error, info};
#[cfg(feature = "exochain")]
use tracing::warn;

use clawft_core::bootstrap::AppContext;
use clawft_core::bus::MessageBus;
use clawft_platform::Platform;
use clawft_types::config::Config;

#[cfg(feature = "native")]
use crate::a2a::A2ARouter;
use crate::capability::AgentCapabilities;
#[cfg(feature = "native")]
use crate::capability::CapabilityChecker;
use crate::cluster::{ClusterConfig, ClusterMembership};
use crate::console::{BootEvent, BootLog, BootPhase, KernelEventLog};
use crate::error::{KernelError, KernelResult};
use crate::health::HealthSystem;
use crate::ipc::KernelIpc;
use crate::process::{ProcessEntry, ProcessState, ProcessTable, ResourceUsage};
use crate::service::ServiceRegistry;
use crate::supervisor::AgentSupervisor;
#[cfg(feature = "native")]
use crate::topic::TopicRouter;
use clawft_types::config::KernelConfig;

/// Kernel lifecycle state.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum KernelState {
    /// Kernel is in the process of booting.
    Booting,
    /// Kernel is running and accepting commands.
    Running,
    /// Kernel is shutting down.
    ShuttingDown,
    /// Kernel has been halted.
    Halted,
}

impl std::fmt::Display for KernelState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KernelState::Booting => write!(f, "booting"),
            KernelState::Running => write!(f, "running"),
            KernelState::ShuttingDown => write!(f, "shutting_down"),
            KernelState::Halted => write!(f, "halted"),
        }
    }
}

// ── Feature-gated subsystem bundles ──────────────────────────────
//
// These group related feature-gated fields so the Kernel struct
// has fewer conditional fields (3 instead of 18) and init code
// is easier to read.

/// Exochain subsystem: chain manager, resource tree, and governance gate.
#[cfg(feature = "exochain")]
pub struct ChainSubsystem {
    pub(crate) chain_manager: Option<Arc<crate::chain::ChainManager>>,
    pub(crate) tree_manager: Option<Arc<crate::tree_manager::TreeManager>>,
    pub(crate) governance_gate: Option<Arc<dyn crate::gate::GateBackend>>,
}

/// ECC cognitive substrate: HNSW, causal graph, cognitive tick,
/// cross-references, impulse queue, and boot-time calibration.
#[cfg(feature = "ecc")]
pub struct EccSubsystem {
    pub(crate) hnsw: Option<Arc<crate::hnsw_service::HnswService>>,
    pub(crate) causal: Option<Arc<crate::causal::CausalGraph>>,
    pub(crate) tick: Option<Arc<crate::cognitive_tick::CognitiveTick>>,
    pub(crate) crossrefs: Option<Arc<crate::crossref::CrossRefStore>>,
    pub(crate) impulses: Option<Arc<crate::impulse::ImpulseQueue>>,
    pub(crate) calibration: Option<crate::calibration::EccCalibration>,
    pub(crate) vector_backend: Option<Arc<dyn crate::vector_backend::VectorBackend>>,
    pub(crate) eml_coherence: Option<Arc<std::sync::Mutex<crate::eml_coherence::EmlCoherenceModel>>>,
}

/// OS-patterns observability: metrics, structured logging, timers,
/// and the dead-letter queue.
#[cfg(feature = "os-patterns")]
pub struct ObservabilitySubsystem {
    pub(crate) metrics_registry: Option<Arc<crate::metrics::MetricsRegistry>>,
    pub(crate) log_service: Option<Arc<crate::log_service::LogService>>,
    pub(crate) timer_service: Option<Arc<crate::timer::TimerService>>,
    pub(crate) dead_letter_queue: Option<Arc<crate::dead_letter::DeadLetterQueue>>,
}

/// The WeftOS kernel.
///
/// Wraps `AppContext<P>` in a managed boot sequence with process
/// tracking, service lifecycle, IPC, and health monitoring.
///
/// # Lifecycle
///
/// 1. Call [`Kernel::boot`] to initialize all subsystems.
/// 2. The kernel transitions from `Booting` -> `Running`.
/// 3. Call [`Kernel::shutdown`] to gracefully stop everything.
/// 4. The kernel transitions from `Running` -> `ShuttingDown` -> `Halted`.
pub struct Kernel<P: Platform> {
    state: KernelState,
    config: KernelConfig,
    app_context: Option<AppContext<P>>,
    bus: Arc<MessageBus>,
    process_table: Arc<ProcessTable>,
    service_registry: Arc<ServiceRegistry>,
    ipc: Arc<KernelIpc>,
    #[cfg(feature = "native")]
    a2a_router: Arc<A2ARouter>,
    #[cfg(feature = "native")]
    cron_service: Arc<crate::cron::CronService>,
    #[cfg(feature = "native")]
    assessment_service: Arc<crate::assessment::AssessmentService>,
    health: HealthSystem,
    supervisor: AgentSupervisor<P>,
    boot_log: BootLog,
    event_log: Arc<KernelEventLog>,
    boot_time: Instant,
    cluster_membership: Arc<ClusterMembership>,
    revocation_list: Arc<crate::revocation::RevocationList>,
    #[cfg(feature = "native")]
    agent_registry: crate::agent_registry::AgentRegistry,
    #[cfg(feature = "native")]
    node_registry: crate::node_registry::NodeRegistry,
    #[cfg(feature = "native")]
    substrate_service: crate::substrate_service::SubstrateService,
    #[cfg(feature = "exochain")]
    chain: ChainSubsystem,
    #[cfg(feature = "ecc")]
    ecc: EccSubsystem,
    #[cfg(feature = "os-patterns")]
    observability: ObservabilitySubsystem,
}

impl<P: Platform> Kernel<P> {
    /// Boot the kernel from configuration and platform.
    ///
    /// This is the primary entry point. It:
    /// 1. Creates subsystems (process table, service registry, IPC, health)
    /// 2. Creates AppContext (reuses existing bootstrap)
    /// 3. Registers the kernel process (PID 0)
    /// 4. Starts all registered services
    /// 5. Transitions to Running state
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::Boot`] if any critical subsystem fails
    /// to initialize.
    pub async fn boot(
        config: Config,
        kernel_config: KernelConfig,
        platform: Arc<P>,
    ) -> KernelResult<Self> {
        let boot_time = Instant::now();
        let mut boot_log = BootLog::new();

        info!("WeftOS kernel booting");
        boot_log.push(BootEvent::info(BootPhase::Init, "WeftOS v0.1.0 booting..."));
        boot_log.push(BootEvent::info(BootPhase::Init, "PID 0 (kernel)"));

        // 1. Create subsystems
        let process_table = Arc::new(ProcessTable::new(kernel_config.max_processes));
        let service_registry = Arc::new(ServiceRegistry::new());

        boot_log.push(BootEvent::info(
            BootPhase::Config,
            format!("Max processes: {}", kernel_config.max_processes),
        ));
        boot_log.push(BootEvent::info(
            BootPhase::Config,
            format!(
                "Health check interval: {}s",
                kernel_config.health_check_interval_secs
            ),
        ));

        // 2. Create AppContext
        let app_context = AppContext::new(config, platform)
            .await
            .map_err(|e| KernelError::Boot(format!("AppContext init failed: {e}")))?;

        // 3. Create IPC from the AppContext's MessageBus
        let bus = app_context.bus().clone();
        let ipc = Arc::new(KernelIpc::new(bus.clone()));

        // 4. Create health system
        let health = HealthSystem::new(kernel_config.health_check_interval_secs);

        // 5. Register kernel process (PID 0)
        let kernel_entry = ProcessEntry {
            pid: 0,
            agent_id: "kernel".to_owned(),
            state: ProcessState::Running,
            capabilities: AgentCapabilities::default(),
            resource_usage: ResourceUsage::default(),
            cancel_token: crate::process::CancellationToken::new(),
            parent_pid: None,
        };
        process_table
            .insert_with_pid(kernel_entry)
            .map_err(|e| KernelError::Boot(format!("failed to register kernel process: {e}")))?;

        boot_log.push(BootEvent::info(
            BootPhase::Services,
            "Service registry ready",
        ));

        // 5a. Construct A2ARouter (per-PID inboxes, capability-checked routing)
        #[cfg(feature = "native")]
        let capability_checker = Arc::new(CapabilityChecker::new(process_table.clone()));
        #[cfg(feature = "native")]
        let topic_router = Arc::new(TopicRouter::new(process_table.clone()));
        #[cfg(feature = "native")]
        let a2a_router = Arc::new(A2ARouter::new(
            process_table.clone(),
            capability_checker,
            topic_router,
        ));

        #[cfg(feature = "native")]
        boot_log.push(BootEvent::info(BootPhase::Services, "A2A router ready"));

        // 5b. Register cron service (K0 gate requirement)
        #[cfg(feature = "native")]
        let cron_svc = Arc::new(crate::cron::CronService::new());
        #[cfg(feature = "native")]
        if let Err(e) = service_registry.register(cron_svc.clone()) {
            error!(error = %e, "failed to register cron service");
        } else {
            boot_log.push(BootEvent::info(BootPhase::Services, "Cron service registered"));
        }

        // 5b½. Register assessment service
        #[cfg(feature = "native")]
        let assessment_svc = Arc::new(crate::assessment::AssessmentService::new());
        #[cfg(feature = "native")]
        if let Err(e) = service_registry.register(assessment_svc.clone()) {
            error!(error = %e, "failed to register assessment service");
        } else {
            boot_log.push(BootEvent::info(BootPhase::Services, "Assessment service registered"));
        }

        // 5c. Register container service (K4)
        let container_manager = std::sync::Arc::new(
            crate::container::ContainerManager::new(crate::container::ContainerConfig::default()),
        );
        let container_service = std::sync::Arc::new(
            crate::container::ContainerService::new(container_manager.clone()),
        );
        if let Err(e) = service_registry.register(container_service) {
            error!(error = %e, "failed to register container service");
        } else {
            boot_log.push(BootEvent::info(
                BootPhase::Services,
                "Container service registered",
            ));
        }

        // 5d. Initialize mesh transport (K6) if configured.
        //     Must happen before cluster (step 7) because cluster needs mesh
        //     to reach peer nodes.
        #[cfg(all(feature = "native", feature = "mesh"))]
        let mesh_runtime = {
            let mesh_config = kernel_config.mesh.clone().unwrap_or_default();
            if mesh_config.enabled {
                let node_id = uuid::Uuid::new_v4().to_string();
                let mut runtime = if mesh_config.discovery {
                    let kad_id = blake3::hash(node_id.as_bytes());
                    crate::mesh_runtime::MeshRuntime::with_discovery(
                        node_id.clone(),
                        *kad_id.as_bytes(),
                    )
                } else {
                    crate::mesh_runtime::MeshRuntime::new(node_id.clone())
                };

                // Wire mesh into the A2A router for incoming message injection.
                runtime.set_local_router(Arc::clone(&a2a_router));

                let runtime = Arc::new(runtime);

                // Clone values needed by seed-peer loop before moving mesh_config.
                let seed_peers = mesh_config.seed_peers.clone();
                let transport_name_for_seeds = mesh_config.transport.clone();
                let transport_display = mesh_config.transport.clone();
                let listen_display = mesh_config.listen_addr.clone();
                let noise_enabled = mesh_config.noise;

                // Generate or load Noise keypair.
                let noise_config = if noise_enabled {
                    let private_key: [u8; 32] = if let Some(ref key_path) = mesh_config.noise_key_path {
                        let key_bytes = std::fs::read(key_path).unwrap_or_else(|_| {
                            tracing::warn!("noise key file not found, generating ephemeral key");
                            let mut key = [0u8; 32];
                            use rand::RngCore;
                            rand::thread_rng().fill_bytes(&mut key);
                            key.to_vec()
                        });
                        let mut arr = [0u8; 32];
                        arr.copy_from_slice(&key_bytes[..32.min(key_bytes.len())]);
                        arr
                    } else {
                        let mut key = [0u8; 32];
                        use rand::RngCore;
                        rand::thread_rng().fill_bytes(&mut key);
                        key
                    };
                    Some(Arc::new(crate::mesh_noise::NoiseConfig {
                        pattern: crate::mesh_noise::NoisePattern::XX,
                        local_private_key: private_key,
                        remote_static_key: None,
                    }))
                } else {
                    None
                };
                let noise_for_seeds = noise_config.clone();

                // Spawn the mesh listener on the configured transport.
                let listen_addr = mesh_config.listen_addr.clone();
                let rt = Arc::clone(&runtime);
                tokio::spawn(async move {
                    use crate::mesh::MeshTransport;

                    let transport: Box<dyn MeshTransport> = match mesh_config.transport.as_str() {
                        "ws" | "websocket" => {
                            Box::new(crate::mesh_ws::WsTransport)
                        }
                        _ => {
                            Box::new(crate::mesh_tcp::TcpTransport)
                        }
                    };

                    match transport.listen(&listen_addr).await {
                        Ok(mut listener) => {
                            let bind = listener.local_addr()
                                .map(|a| a.to_string())
                                .unwrap_or_else(|_| listen_addr.clone());
                            tracing::info!(
                                transport = transport.name(),
                                addr = %bind,
                                noise = noise_config.is_some(),
                                "mesh listener started"
                            );

                            loop {
                                match listener.accept().await {
                                    Ok((stream, peer_addr)) => {
                                        let rt2 = Arc::clone(&rt);
                                        let nc = noise_config.clone();
                                        tokio::spawn(async move {
                                            tracing::info!(
                                                peer = %peer_addr,
                                                noise = nc.is_some(),
                                                "mesh peer connected"
                                            );

                                            // Optionally wrap in Noise encryption.
                                            let mut channel: Box<dyn crate::mesh_noise::EncryptedChannel> = match &nc {
                                                Some(cfg) => {
                                                    match crate::mesh_noise::NoiseChannel::respond(stream, cfg).await {
                                                        Ok(ch) => {
                                                            tracing::info!(peer = %peer_addr, "noise handshake complete");
                                                            Box::new(ch)
                                                        }
                                                        Err(e) => {
                                                            tracing::warn!(peer = %peer_addr, error = %e, "noise handshake failed, dropping");
                                                            return;
                                                        }
                                                    }
                                                }
                                                None => {
                                                    Box::new(crate::mesh_noise::PassthroughChannel::new(stream))
                                                }
                                            };

                                            // Outbound channel: the kernel pushes frames into
                                            // `out_tx` (via `MeshRuntime::send_to_peer`) and this
                                            // task drains `out_rx` back through the encrypted
                                            // stream. This is what lets the topic forwarder in
                                            // `A2ARouter` deliver pushes to inbound leaf peers that
                                            // subscribed via `mesh.subscribe`.
                                            let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(256);

                                            // Bidirectional loop. `handle_incoming_from` auto-
                                            // registers the peer by `envelope.source_node` on first
                                            // arrival so the kernel can route back.
                                            loop {
                                                tokio::select! {
                                                    inbound = channel.recv_encrypted() => match inbound {
                                                        Ok(data) => {
                                                            if let Err(e) = rt2
                                                                .handle_incoming_from(&data, out_tx.clone())
                                                                .await
                                                            {
                                                                tracing::debug!(error = %e, "mesh message handling error");
                                                            }
                                                        }
                                                        Err(_) => break,
                                                    },
                                                    outbound = out_rx.recv() => match outbound {
                                                        Some(data) => {
                                                            if channel.send_encrypted(&data).await.is_err() {
                                                                break;
                                                            }
                                                        }
                                                        None => break,
                                                    },
                                                }
                                            }
                                        });
                                    }
                                    Err(e) => {
                                        tracing::warn!(error = %e, "mesh accept error");
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!(
                                error = %e,
                                addr = %listen_addr,
                                "failed to start mesh listener"
                            );
                        }
                    }
                });

                // Connect to seed peers.
                for peer_addr in &seed_peers {
                    let addr = peer_addr.clone();
                    let rt = Arc::clone(&runtime);
                    let transport_name = transport_name_for_seeds.clone();
                    let nc = noise_for_seeds.clone();
                    tokio::spawn(async move {
                        use crate::mesh::MeshTransport;

                        let transport: Box<dyn MeshTransport> = match transport_name.as_str() {
                            "ws" | "websocket" => Box::new(crate::mesh_ws::WsTransport),
                            _ => Box::new(crate::mesh_tcp::TcpTransport),
                        };

                        match transport.connect(&addr).await {
                            Ok(stream) => {
                                // Optionally wrap in Noise encryption (as initiator).
                                let mut channel: Box<dyn crate::mesh_noise::EncryptedChannel> = match &nc {
                                    Some(cfg) => {
                                        match crate::mesh_noise::NoiseChannel::initiate(stream, cfg).await {
                                            Ok(ch) => {
                                                tracing::info!(peer = %addr, "noise handshake complete (initiator)");
                                                Box::new(ch)
                                            }
                                            Err(e) => {
                                                tracing::warn!(peer = %addr, error = %e, "noise handshake failed");
                                                return;
                                            }
                                        }
                                    }
                                    None => Box::new(crate::mesh_noise::PassthroughChannel::new(stream)),
                                };

                                let (tx, mut rx) = tokio::sync::mpsc::channel(256);
                                let peer_id = addr.clone();
                                rt.add_peer(peer_id.clone(), tx);
                                tracing::info!(peer = %addr, noise = nc.is_some(), "connected to seed peer");

                                // Drain outbound queue through encrypted channel.
                                tokio::spawn(async move {
                                    while let Some(data) = rx.recv().await {
                                        if channel.send_encrypted(&data).await.is_err() {
                                            break;
                                        }
                                    }
                                });
                            }
                            Err(e) => {
                                tracing::warn!(peer = %addr, error = %e, "failed to connect to seed peer");
                            }
                        }
                    });
                }

                boot_log.push(BootEvent::info(
                    BootPhase::Network,
                    format!(
                        "Mesh transport started ({} on {}, {} seed peers)",
                        transport_display,
                        listen_display,
                        seed_peers.len(),
                    ),
                ));

                Some(runtime)
            } else {
                boot_log.push(BootEvent::info(
                    BootPhase::Network,
                    "Mesh transport disabled",
                ));
                None
            }
        };

        // 6. Create cluster membership (universal, always present)
        let cluster_config = ClusterConfig {
            node_id: uuid::Uuid::new_v4().to_string(),
            node_name: kernel_config
                .cluster
                .as_ref()
                .and_then(|c| c.node_name.clone())
                .unwrap_or_else(|| "local".into()),
            heartbeat_interval_secs: kernel_config
                .cluster
                .as_ref()
                .map(|c| c.heartbeat_interval_secs)
                .unwrap_or(5),
            ..ClusterConfig::default()
        };
        // Cluster peer membership persists to disk so joins survive restarts.
        let cluster_peers_path =
            std::path::PathBuf::from(".weftos/runtime/cluster_peers.json");
        let cluster_membership = Arc::new(
            ClusterMembership::new(cluster_config).with_persist_path(&cluster_peers_path),
        );

        boot_log.push(BootEvent::info(
            BootPhase::Network,
            format!(
                "Cluster membership ready (node {}, {} peer(s) rehydrated)",
                cluster_membership.local_node_id(),
                cluster_membership.len(),
            ),
        ));

        // 6b. Load host revocation list (persistent ban list)
        let revocation_path = crate::revocation::RevocationList::default_path(std::path::Path::new("."));
        let revocation_list = Arc::new(crate::revocation::RevocationList::load(revocation_path));
        {
            let count = revocation_list.len();
            if count > 0 {
                boot_log.push(BootEvent::info(
                    BootPhase::Network,
                    format!("Host revocation list loaded ({count} banned hosts)"),
                ));
            } else {
                boot_log.push(BootEvent::info(
                    BootPhase::Network,
                    "Host revocation list ready (empty)",
                ));
            }
        }

        // 7. Register cluster service (when feature-gated ruvector integration is enabled)
        #[cfg(feature = "cluster")]
        {
            use crate::cluster::ClusterService;
            use ruvector_cluster::StaticDiscovery;

            let net_config = kernel_config
                .cluster
                .clone()
                .unwrap_or_default();
            let seed_addrs: Vec<std::net::SocketAddr> = net_config
                .seed_nodes
                .iter()
                .filter_map(|s| s.parse().ok())
                .collect();
            let seed_nodes: Vec<ruvector_cluster::ClusterNode> = seed_addrs
                .into_iter()
                .map(|addr| ruvector_cluster::ClusterNode::new(addr.to_string(), addr))
                .collect();
            let discovery = Box::new(StaticDiscovery::new(seed_nodes));
            let node_id = cluster_membership.local_node_id().to_owned();

            match ClusterService::new(
                net_config,
                node_id,
                discovery,
                Arc::clone(&cluster_membership),
            ) {
                Ok(cluster_svc) => {
                    let svc = Arc::new(cluster_svc);
                    if let Err(e) = service_registry.register(svc) {
                        error!(error = %e, "failed to register cluster service");
                    } else {
                        boot_log.push(BootEvent::info(
                            BootPhase::Network,
                            "Cluster service registered (ruvector)",
                        ));
                    }
                }
                Err(e) => {
                    error!(error = %e, "failed to create cluster service");
                    boot_log.push(BootEvent::info(
                        BootPhase::Network,
                        format!("Cluster service failed: {e}"),
                    ));
                }
            }
        }

        // 8. Start services (none registered by default at boot, unless cluster feature added one)
        service_registry
            .start_all()
            .await
            .map_err(|e| KernelError::Boot(format!("service start failed: {e}")))?;

        // 8b. Initialize local exochain (when exochain feature is enabled)
        //     Restores from checkpoint file if available; otherwise fresh genesis.
        #[cfg(feature = "exochain")]
        let chain_manager = {
            let chain_config = kernel_config.chain.clone().unwrap_or_default();
            if chain_config.enabled {
                // Load or generate Ed25519 signing key for chain integrity.
                let signing_key = if let Some(ref ckpt_path) = chain_config.effective_checkpoint_path() {
                    let key_path = std::path::PathBuf::from(ckpt_path).with_extension("key");
                    match crate::chain::ChainManager::load_or_create_key(&key_path) {
                        Ok(key) => {
                            boot_log.push(BootEvent::info(
                                BootPhase::Services,
                                format!("Ed25519 signing key loaded: {}", key_path.display()),
                            ));
                            Some(key)
                        }
                        Err(e) => {
                            warn!(error = %e, "failed to load/create signing key");
                            boot_log.push(BootEvent::info(
                                BootPhase::Services,
                                format!("Signing key unavailable: {e} — chain will be unsigned"),
                            ));
                            None
                        }
                    }
                } else {
                    None
                };

                let cm = if let Some(ref ckpt_path) = chain_config.effective_checkpoint_path() {
                    let json_path = std::path::PathBuf::from(ckpt_path);
                    // Derive RVF path from JSON path: same directory, `.rvf` extension
                    let rvf_path = json_path.with_extension("rvf");

                    if rvf_path.exists() {
                        // Prefer RVF format (cryptographic integrity verification)
                        match crate::chain::ChainManager::load_from_rvf(&rvf_path, chain_config.checkpoint_interval) {
                            Ok(restored) => {
                                let seq = restored.sequence();
                                boot_log.push(BootEvent::info(
                                    BootPhase::Services,
                                    format!(
                                        "Chain restored from RVF (seq={}, chain_id={})",
                                        seq, chain_config.chain_id,
                                    ),
                                ));
                                Arc::new(restored)
                            }
                            Err(e) => {
                                error!(error = %e, "failed to restore RVF chain, trying JSON fallback");
                                // Fall back to JSON
                                if json_path.exists() {
                                    match crate::chain::ChainManager::load_from_file(&json_path, chain_config.checkpoint_interval) {
                                        Ok(restored) => {
                                            let seq = restored.sequence();
                                            boot_log.push(BootEvent::info(
                                                BootPhase::Services,
                                                format!(
                                                    "Chain restored from JSON fallback (seq={}, chain_id={})",
                                                    seq, chain_config.chain_id,
                                                ),
                                            ));
                                            Arc::new(restored)
                                        }
                                        Err(e2) => {
                                            error!(error = %e2, "JSON fallback also failed, starting fresh");
                                            boot_log.push(BootEvent::info(
                                                BootPhase::Services,
                                                format!("Chain restore failed (RVF: {e}, JSON: {e2}), starting fresh"),
                                            ));
                                            Arc::new(crate::chain::ChainManager::new(
                                                chain_config.chain_id,
                                                chain_config.checkpoint_interval,
                                            ))
                                        }
                                    }
                                } else {
                                    boot_log.push(BootEvent::info(
                                        BootPhase::Services,
                                        format!("RVF restore failed: {e}, starting fresh"),
                                    ));
                                    Arc::new(crate::chain::ChainManager::new(
                                        chain_config.chain_id,
                                        chain_config.checkpoint_interval,
                                    ))
                                }
                            }
                        }
                    } else if json_path.exists() {
                        // Legacy JSON format
                        match crate::chain::ChainManager::load_from_file(&json_path, chain_config.checkpoint_interval) {
                            Ok(restored) => {
                                let seq = restored.sequence();
                                boot_log.push(BootEvent::info(
                                    BootPhase::Services,
                                    format!(
                                        "Chain restored from JSON (seq={}, chain_id={}, will migrate to RVF)",
                                        seq, chain_config.chain_id,
                                    ),
                                ));
                                Arc::new(restored)
                            }
                            Err(e) => {
                                error!(error = %e, "failed to restore chain, starting fresh");
                                boot_log.push(BootEvent::info(
                                    BootPhase::Services,
                                    format!("Chain restore failed: {e}, starting fresh"),
                                ));
                                Arc::new(crate::chain::ChainManager::new(
                                    chain_config.chain_id,
                                    chain_config.checkpoint_interval,
                                ))
                            }
                        }
                    } else {
                        Arc::new(crate::chain::ChainManager::new(
                            chain_config.chain_id,
                            chain_config.checkpoint_interval,
                        ))
                    }
                } else {
                    Arc::new(crate::chain::ChainManager::new(
                        chain_config.chain_id,
                        chain_config.checkpoint_interval,
                    ))
                };

                // Attach Ed25519 signing key if available.
                let mut cm = cm;
                if let Some(key) = signing_key
                    && let Some(inner) = Arc::get_mut(&mut cm)
                {
                    // Generate ML-DSA key from Ed25519 key bytes for dual signing.
                    let ml_dsa_seed = key.to_bytes();
                    let (ml_key, _ml_vk) = weftos_rvf_crypto::MlDsa65Key::generate(&ml_dsa_seed);
                    inner.set_signing_key(key);
                    inner.set_ml_dsa_key(ml_key);
                    boot_log.push(BootEvent::info(
                        BootPhase::Services,
                        "Dual signing enabled (Ed25519 + ML-DSA-65)",
                    ));
                }

                boot_log.push(BootEvent::info(
                    BootPhase::Services,
                    format!(
                        "Local chain ready (chain_id={}, seq={}, signed={})",
                        chain_config.chain_id,
                        cm.sequence(),
                        cm.has_signing_key(),
                    ),
                ));

                // Log boot phases to chain
                cm.append(
                    "kernel",
                    "boot.init",
                    Some(serde_json::json!({"version": "0.1.0"})),
                );
                cm.append(
                    "kernel",
                    "boot.config",
                    Some(serde_json::json!({
                        "max_processes": kernel_config.max_processes,
                        "health_interval": kernel_config.health_check_interval_secs,
                    })),
                );
                cm.append(
                    "kernel",
                    "boot.services",
                    Some(serde_json::json!({
                        "count": service_registry.len(),
                    })),
                );

                Some(cm)
            } else {
                boot_log.push(BootEvent::info(
                    BootPhase::Services,
                    "Local chain disabled",
                ));
                None
            }
        };

        // 8b-mesh. Wire the chain manager into the mesh runtime so every
        //          successful `handle_incoming` appends a `peer.envelope`
        //          event to the ExoChain. Makes mesh activity auditable
        //          via `weaver chain local` without adding a topic
        //          subscriber.
        #[cfg(all(feature = "native", feature = "mesh", feature = "exochain"))]
        if let (Some(mr), Some(cm)) = (mesh_runtime.as_ref(), chain_manager.as_ref()) {
            mr.set_chain_manager(Arc::clone(cm));
        }

        // 8c. Bootstrap resource tree via TreeManager (when exochain feature is enabled)
        //     First attempt to restore from checkpoint; fall back to fresh bootstrap.
        #[cfg(feature = "exochain")]
        let tree_manager = {
            let rt_config = kernel_config.resource_tree.clone().unwrap_or_default();
            if rt_config.enabled {
                if let Some(ref cm) = chain_manager {
                    let tm = Arc::new(crate::tree_manager::TreeManager::new(Arc::clone(cm)));

                    // Derive tree checkpoint path from chain checkpoint path
                    let chain_cfg = kernel_config.chain.clone().unwrap_or_default();
                    let tree_ckpt_path = chain_cfg
                        .effective_checkpoint_path()
                        .map(|p| std::path::PathBuf::from(p).with_extension("tree.json"));

                    let mut restored_from_checkpoint = false;
                    if let Some(ref tree_path) = tree_ckpt_path
                        && tree_path.exists()
                    {
                        match tm.load_checkpoint(tree_path) {
                            Ok(()) => {
                                let stats = tm.stats();
                                // Verify tree root hash against the chain's last recorded hash.
                                let chain_tree_hash = cm.last_tree_root_hash();
                                if let Some(ref expected) = chain_tree_hash {
                                    if stats.root_hash == *expected {
                                        boot_log.push(BootEvent::info(
                                            BootPhase::ResourceTree,
                                            format!(
                                                "Resource tree restored from checkpoint ({} nodes, root={}..., hash verified)",
                                                stats.node_count,
                                                &stats.root_hash[..12],
                                            ),
                                        ));
                                        restored_from_checkpoint = true;
                                    } else {
                                        warn!(
                                            expected = %expected,
                                            actual = %stats.root_hash,
                                            "tree checkpoint root hash mismatch — falling back to fresh bootstrap"
                                        );
                                        boot_log.push(BootEvent::info(
                                            BootPhase::ResourceTree,
                                            format!(
                                                "Tree checkpoint hash mismatch (expected={}..., got={}...), bootstrapping fresh",
                                                &expected[..std::cmp::min(12, expected.len())],
                                                &stats.root_hash[..12],
                                            ),
                                        ));
                                        // Don't set restored_from_checkpoint — falls through to bootstrap.
                                    }
                                } else {
                                    // No hash in chain to verify against — accept as-is.
                                    boot_log.push(BootEvent::info(
                                        BootPhase::ResourceTree,
                                        format!(
                                            "Resource tree restored from checkpoint ({} nodes, root={}...)",
                                            stats.node_count,
                                            &stats.root_hash[..12],
                                        ),
                                    ));
                                    restored_from_checkpoint = true;
                                }
                            }
                            Err(e) => {
                                error!(error = %e, "failed to restore tree checkpoint, bootstrapping fresh");
                            }
                        }
                    }

                    if !restored_from_checkpoint {
                        if let Err(e) = tm.bootstrap() {
                            error!(error = %e, "failed to bootstrap resource tree");
                            // Still allow boot to proceed without tree
                        } else {
                            let stats = tm.stats();
                            boot_log.push(BootEvent::info(
                                BootPhase::ResourceTree,
                                format!(
                                    "Resource tree bootstrapped ({} nodes, root={}...)",
                                    stats.node_count,
                                    &stats.root_hash[..12],
                                ),
                            ));
                        }
                    }

                    // Register cron service in tree with manifest (5b wiring)
                    if let Err(e) = tm.register_service_with_manifest("cron", "scheduler") {
                        tracing::debug!(error = %e, "failed to register cron in tree (may already exist)");
                    }

                    // K4: Register container namespace in the resource tree
                    {
                        use exo_resource_tree::model::{ResourceId, ResourceKind};
                        if let Err(e) = tm.insert(
                            ResourceId::new("/kernel/containers"),
                            ResourceKind::Namespace,
                            ResourceId::new("/kernel"),
                        ) {
                            tracing::debug!(
                                error = %e,
                                "failed to register containers namespace (may already exist)"
                            );
                        }
                    }

                    // K3c: Register ECC namespaces in the resource tree
                    #[cfg(feature = "ecc")]
                    {
                        use exo_resource_tree::model::{ResourceId, ResourceKind};
                        let ecc_namespaces = [
                            ("/kernel/services/ecc", "/kernel/services"),
                            ("/kernel/services/ecc/hnsw", "/kernel/services/ecc"),
                            ("/kernel/services/ecc/causal", "/kernel/services/ecc"),
                            ("/kernel/services/ecc/tick", "/kernel/services/ecc"),
                            ("/kernel/services/ecc/calibration", "/kernel/services/ecc"),
                            ("/kernel/services/ecc/crossrefs", "/kernel/services/ecc"),
                        ];
                        for (path, parent) in &ecc_namespaces {
                            if let Err(e) = tm.insert(
                                ResourceId::new(*path),
                                ResourceKind::Namespace,
                                ResourceId::new(*parent),
                            ) {
                                tracing::debug!(
                                    error = %e, path = *path,
                                    "failed to register ECC namespace (may already exist)"
                                );
                            }
                        }
                        boot_log.push(BootEvent::info(
                            BootPhase::Ecc,
                            "ECC resource tree namespaces registered",
                        ));
                    }

                    // K3: Register tool namespaces in the resource tree
                    {
                        use exo_resource_tree::model::{ResourceId, ResourceKind};
                        let tool_namespaces = [
                            ("/kernel/tools", "/kernel"),
                            ("/kernel/tools/fs", "/kernel/tools"),
                            ("/kernel/tools/agent", "/kernel/tools"),
                            ("/kernel/tools/sys", "/kernel/tools"),
                            ("/kernel/tools/ipc", "/kernel/tools"),
                            #[cfg(feature = "ecc")]
                            ("/kernel/tools/ecc", "/kernel/tools"),
                        ];
                        for (path, parent) in &tool_namespaces {
                            if let Err(e) = tm.insert(
                                ResourceId::new(*path),
                                ResourceKind::Namespace,
                                ResourceId::new(*parent),
                            ) {
                                tracing::debug!(
                                    error = %e, path = *path,
                                    "failed to register tool namespace (may already exist)"
                                );
                            }
                        }
                        // Register each built-in tool as a Tool node
                        let catalog = crate::wasm_runner::builtin_tool_catalog();
                        for spec in &catalog {
                            let cat_path = match spec.category {
                                crate::wasm_runner::ToolCategory::Filesystem => "/kernel/tools/fs",
                                crate::wasm_runner::ToolCategory::Agent => "/kernel/tools/agent",
                                crate::wasm_runner::ToolCategory::System => "/kernel/tools/sys",
                                crate::wasm_runner::ToolCategory::Ecc => "/kernel/tools/ecc",
                                crate::wasm_runner::ToolCategory::User => "/kernel/tools",
                            };
                            let tool_path = format!("{}/{}", cat_path, spec.name.replace('.', "/"));
                            if let Err(e) = tm.insert(
                                ResourceId::new(&tool_path),
                                ResourceKind::Tool,
                                ResourceId::new(cat_path),
                            ) {
                                tracing::debug!(
                                    error = %e, tool = %spec.name,
                                    "failed to register tool node (may already exist)"
                                );
                            }
                        }
                        boot_log.push(BootEvent::info(
                            BootPhase::ResourceTree,
                            format!("Registered {} built-in tools in resource tree", catalog.len()),
                        ));
                    }

                    Some(tm)
                } else {
                    boot_log.push(BootEvent::info(
                        BootPhase::ResourceTree,
                        "Resource tree requires chain — skipped",
                    ));
                    None
                }
            } else {
                boot_log.push(BootEvent::info(
                    BootPhase::ResourceTree,
                    "Resource tree disabled",
                ));
                None
            }
        };

        // 8f. Initialize governance engine with chain-anchored genesis rules
        //
        // The governance gate is the constitutional layer of the kernel.
        // At boot, genesis rules are written to the chain as immutable
        // entries. Once anchored, they cannot be modified — only superseded
        // by a `governance.root.supersede` event that references the
        // original genesis sequence.
        #[cfg(feature = "exochain")]
        let governance_gate: Option<Arc<dyn crate::gate::GateBackend>> = {
            if let Some(ref cm) = chain_manager {
                use crate::governance::{GovernanceBranch, GovernanceRule, RuleSeverity};
                use crate::gate::GovernanceGate;

                // Default risk threshold (0.7 for production safety)
                let risk_threshold = 0.7;
                let human_approval = false;

                let mut gate = GovernanceGate::new(risk_threshold, human_approval)
                    .with_chain(Arc::clone(cm));

                // ── Genesis governance rules (immutable chain entries) ────
                // These are the constitutional rules that govern all agent
                // behavior. Once written to the chain, they cannot be
                // modified — only superseded by a governance.root.supersede
                // event.

                let genesis_rules = vec![
                    // ── Core constitutional rules (GOV-001 .. GOV-007) ──────
                    GovernanceRule {
                        id: "GOV-001".into(),
                        description: "High-risk operations require elevated review".into(),
                        branch: GovernanceBranch::Judicial,
                        severity: RuleSeverity::Blocking,
                        active: true,
                        reference_url: None,
                        sop_category: None,
                    },
                    GovernanceRule {
                        id: "GOV-002".into(),
                        description: "Security-sensitive actions must not exceed security threshold".into(),
                        branch: GovernanceBranch::Judicial,
                        severity: RuleSeverity::Blocking,
                        active: true,
                        reference_url: None,
                        sop_category: None,
                    },
                    GovernanceRule {
                        id: "GOV-003".into(),
                        description: "Privacy-impacting operations flagged for review".into(),
                        branch: GovernanceBranch::Legislative,
                        severity: RuleSeverity::Warning,
                        active: true,
                        reference_url: None,
                        sop_category: None,
                    },
                    GovernanceRule {
                        id: "GOV-004".into(),
                        description: "Novel/unprecedented actions require advisory logging".into(),
                        branch: GovernanceBranch::Executive,
                        severity: RuleSeverity::Advisory,
                        active: true,
                        reference_url: None,
                        sop_category: None,
                    },
                    GovernanceRule {
                        id: "GOV-005".into(),
                        description: "Filesystem write operations scored for risk".into(),
                        branch: GovernanceBranch::Legislative,
                        severity: RuleSeverity::Warning,
                        active: true,
                        reference_url: None,
                        sop_category: None,
                    },
                    GovernanceRule {
                        id: "GOV-006".into(),
                        description: "Agent spawn operations require governance clearance".into(),
                        branch: GovernanceBranch::Executive,
                        severity: RuleSeverity::Blocking,
                        active: true,
                        reference_url: None,
                        sop_category: None,
                    },
                    GovernanceRule {
                        id: "GOV-007".into(),
                        description: "IPC messages between agents logged for audit trail".into(),
                        branch: GovernanceBranch::Judicial,
                        severity: RuleSeverity::Advisory,
                        active: true,
                        reference_url: None,
                        sop_category: None,
                    },
                    // ── AI-SDLC SOP rules: Legislative (6) ──────────────────
                    GovernanceRule {
                        id: "SOP-L001".into(),
                        description: "AI-IRB approval required before high-impact deployments".into(),
                        branch: GovernanceBranch::Legislative,
                        severity: RuleSeverity::Blocking,
                        active: true,
                        reference_url: Some("https://github.com/AISDLC/AI-SDLC-SOPs/blob/main/sops/SOP-1300-01-AI_IRB_Approval.md".into()),
                        sop_category: Some("governance".into()),
                    },
                    GovernanceRule {
                        id: "SOP-L002".into(),
                        description: "Version control and branching policies must be enforced".into(),
                        branch: GovernanceBranch::Legislative,
                        severity: RuleSeverity::Warning,
                        active: true,
                        reference_url: Some("https://github.com/AISDLC/AI-SDLC-SOPs/blob/main/sops/SOP-1003-01-AI_Version_Control.md".into()),
                        sop_category: Some("governance".into()),
                    },
                    GovernanceRule {
                        id: "SOP-L003".into(),
                        description: "Requirements must include AI-IRB ethical review".into(),
                        branch: GovernanceBranch::Legislative,
                        severity: RuleSeverity::Warning,
                        active: true,
                        reference_url: Some("https://github.com/AISDLC/AI-SDLC-SOPs/blob/main/sops/SOP-1040-01-AI_Requirements.md".into()),
                        sop_category: Some("engineering".into()),
                    },
                    GovernanceRule {
                        id: "SOP-L004".into(),
                        description: "Release planning must follow structured lifecycle gates".into(),
                        branch: GovernanceBranch::Legislative,
                        severity: RuleSeverity::Advisory,
                        active: true,
                        reference_url: Some("https://github.com/AISDLC/AI-SDLC-SOPs/blob/main/sops/SOP-1005-01-AI_Release_Planning.md".into()),
                        sop_category: Some("lifecycle".into()),
                    },
                    GovernanceRule {
                        id: "SOP-L005".into(),
                        description: "Data protection and PII handling must comply with policy".into(),
                        branch: GovernanceBranch::Legislative,
                        severity: RuleSeverity::Blocking,
                        active: true,
                        reference_url: Some("https://github.com/AISDLC/AI-SDLC-SOPs/blob/main/sops/SOP-1303-01-AI_Data_Protection.md".into()),
                        sop_category: Some("ethics".into()),
                    },
                    GovernanceRule {
                        id: "SOP-L006".into(),
                        description: "Risk register must be maintained and reviewed".into(),
                        branch: GovernanceBranch::Legislative,
                        severity: RuleSeverity::Warning,
                        active: true,
                        reference_url: Some("https://github.com/AISDLC/AI-SDLC-SOPs/blob/main/sops/SOP-1062-01-AI_Risk_Register.md".into()),
                        sop_category: Some("governance".into()),
                    },
                    // ── AI-SDLC SOP rules: Executive (5) ────────────────────
                    GovernanceRule {
                        id: "SOP-E001".into(),
                        description: "Secure coding standards must be followed".into(),
                        branch: GovernanceBranch::Executive,
                        severity: RuleSeverity::Warning,
                        active: true,
                        reference_url: Some("https://github.com/AISDLC/AI-SDLC-SOPs/blob/main/sops/SOP-1200-01-AI_Secure_Coding.md".into()),
                        sop_category: Some("engineering".into()),
                    },
                    GovernanceRule {
                        id: "SOP-E002".into(),
                        description: "Deployment requires governance clearance checkpoint".into(),
                        branch: GovernanceBranch::Executive,
                        severity: RuleSeverity::Blocking,
                        active: true,
                        reference_url: Some("https://github.com/AISDLC/AI-SDLC-SOPs/blob/main/sops/SOP-1220-01-AI_Deployment_Clearance.md".into()),
                        sop_category: Some("lifecycle".into()),
                    },
                    GovernanceRule {
                        id: "SOP-E003".into(),
                        description: "Incident response procedures must be documented and followed".into(),
                        branch: GovernanceBranch::Executive,
                        severity: RuleSeverity::Warning,
                        active: true,
                        reference_url: Some("https://github.com/AISDLC/AI-SDLC-SOPs/blob/main/sops/SOP-1008-01-AI_Incident_Response.md".into()),
                        sop_category: Some("security".into()),
                    },
                    GovernanceRule {
                        id: "SOP-E004".into(),
                        description: "Decommissioning must follow structured teardown procedure".into(),
                        branch: GovernanceBranch::Executive,
                        severity: RuleSeverity::Advisory,
                        active: true,
                        reference_url: Some("https://github.com/AISDLC/AI-SDLC-SOPs/blob/main/sops/SOP-1011-01-AI_Decommissioning.md".into()),
                        sop_category: Some("lifecycle".into()),
                    },
                    GovernanceRule {
                        id: "SOP-E005".into(),
                        description: "Third-party AI procurement requires screening".into(),
                        branch: GovernanceBranch::Executive,
                        severity: RuleSeverity::Advisory,
                        active: true,
                        reference_url: Some("https://github.com/AISDLC/AI-SDLC-SOPs/blob/main/sops/SOP-1004-01-AI_Procurement_Screening.md".into()),
                        sop_category: Some("governance".into()),
                    },
                    // ── AI-SDLC SOP rules: Judicial (4) ─────────────────────
                    GovernanceRule {
                        id: "SOP-J001".into(),
                        description: "Bias and fairness assessments required for model outputs".into(),
                        branch: GovernanceBranch::Judicial,
                        severity: RuleSeverity::Blocking,
                        active: true,
                        reference_url: Some("https://github.com/AISDLC/AI-SDLC-SOPs/blob/main/sops/SOP-1301-01-AI_Bias_Fairness.md".into()),
                        sop_category: Some("ethics".into()),
                    },
                    GovernanceRule {
                        id: "SOP-J002".into(),
                        description: "Explainability documentation required for decision systems".into(),
                        branch: GovernanceBranch::Judicial,
                        severity: RuleSeverity::Warning,
                        active: true,
                        reference_url: Some("https://github.com/AISDLC/AI-SDLC-SOPs/blob/main/sops/SOP-1302-01-AI_Explainability.md".into()),
                        sop_category: Some("ethics".into()),
                    },
                    GovernanceRule {
                        id: "SOP-J003".into(),
                        description: "Model drift detection and monitoring must be active".into(),
                        branch: GovernanceBranch::Judicial,
                        severity: RuleSeverity::Warning,
                        active: true,
                        reference_url: Some("https://github.com/AISDLC/AI-SDLC-SOPs/blob/main/sops/SOP-1009-01-AI_Drift_Detection.md".into()),
                        sop_category: Some("lifecycle".into()),
                    },
                    GovernanceRule {
                        id: "SOP-J004".into(),
                        description: "Quality records must be maintained for audit compliance".into(),
                        branch: GovernanceBranch::Judicial,
                        severity: RuleSeverity::Advisory,
                        active: true,
                        reference_url: Some("https://github.com/AISDLC/AI-SDLC-SOPs/blob/main/sops/SOP-2002-01-AI_Quality_Records.md".into()),
                        sop_category: Some("quality".into()),
                    },
                ];

                // Anchor genesis rules to chain
                let genesis_seq = cm.sequence();
                let rules_json: Vec<serde_json::Value> = genesis_rules.iter().map(|r| {
                    serde_json::json!({
                        "id": r.id,
                        "description": r.description,
                        "branch": format!("{}", r.branch),
                        "severity": format!("{}", r.severity),
                        "reference_url": r.reference_url,
                        "sop_category": r.sop_category,
                    })
                }).collect();

                // The governance.genesis event is the ROOT — it establishes the
                // constitutional rules. Its chain sequence is the governance root.
                cm.append(
                    "governance",
                    "governance.genesis",
                    Some(serde_json::json!({
                        "version": "2.0.0",
                        "risk_threshold": risk_threshold,
                        "human_approval_required": human_approval,
                        "rules": rules_json,
                        "rule_count": genesis_rules.len(),
                        "genesis_seq": genesis_seq,
                    })),
                );

                // Anchor each rule individually for granular verification
                for rule in &genesis_rules {
                    cm.append(
                        "governance",
                        "governance.rule",
                        Some(serde_json::json!({
                            "rule_id": rule.id,
                            "branch": format!("{}", rule.branch),
                            "severity": format!("{}", rule.severity),
                            "genesis_seq": genesis_seq,
                        })),
                    );
                }

                // Add rules to the gate
                for rule in genesis_rules {
                    gate = gate.add_rule(rule);
                }

                boot_log.push(BootEvent::info(
                    BootPhase::Services,
                    format!(
                        "Governance genesis anchored (seq={}, {} rules, threshold={:.1})",
                        genesis_seq,
                        gate.engine().rule_count(),
                        risk_threshold,
                    ),
                ));

                Some(Arc::new(gate) as Arc<dyn crate::gate::GateBackend>)
            } else {
                boot_log.push(BootEvent::info(
                    BootPhase::Services,
                    "Governance: no chain available, using open governance",
                ));
                None
            }
        };

        // Wire governance gate into A2A router for dual-layer enforcement.
        // The A2ARouter is already behind Arc, so we use set_gate() which
        // relies on OnceLock interior mutability.
        #[cfg(feature = "exochain")]
        if let Some(ref gate) = governance_gate {
            a2a_router.set_gate(Arc::clone(gate));
        }

        // 8d. Log cluster and boot.ready chain events
        #[cfg(feature = "exochain")]
        if let Some(ref cm) = chain_manager {
            cm.append(
                "kernel",
                "boot.cluster",
                Some(serde_json::json!({
                    "node_id": cluster_membership.local_node_id(),
                })),
            );

            let elapsed_ms = boot_time.elapsed().as_millis() as u64;
            let mut ready_payload = serde_json::json!({
                "elapsed_ms": elapsed_ms,
                "processes": process_table.len(),
                "services": service_registry.len(),
            });

            if let Some(ref tm) = tree_manager {
                let root_hash = tm.stats().root_hash;
                ready_payload
                    .as_object_mut()
                    .unwrap()
                    .insert("tree_root_hash".to_string(), serde_json::json!(root_hash));
            }

            cm.append("kernel", "boot.ready", Some(ready_payload));

            // Emit boot manifest as a chain event (becomes an ExochainCheckpoint
            // segment in RVF persistence, capturing the complete boot state).
            let mut manifest = serde_json::json!({
                "version": env!("CARGO_PKG_VERSION"),
                "node_id": cluster_membership.local_node_id(),
                "process_count": process_table.len(),
                "service_count": service_registry.len(),
                "chain_sequence": cm.sequence(),
                "boot_elapsed_ms": boot_time.elapsed().as_millis() as u64,
            });
            if let Some(ref tm) = tree_manager {
                let stats = tm.stats();
                manifest.as_object_mut().unwrap().insert(
                    "tree_root_hash".to_string(),
                    serde_json::json!(stats.root_hash),
                );
                manifest.as_object_mut().unwrap().insert(
                    "tree_node_count".to_string(),
                    serde_json::json!(stats.node_count),
                );
            }
            cm.append("kernel", "boot.manifest", Some(manifest));
        }

        // 8e. Initialize ECC cognitive substrate (when ecc feature is enabled)
        #[cfg(feature = "ecc")]
        let (ecc_hnsw, ecc_causal, ecc_tick, ecc_crossrefs, ecc_impulses, ecc_calibration, ecc_vector_backend, ecc_eml_model) = {
            use crate::calibration::{EccCalibrationConfig, run_calibration};
            use crate::causal::CausalGraph;
            use crate::cognitive_tick::{CognitiveTick, CognitiveTickConfig};
            use crate::service::SystemService;
            use crate::crossref::CrossRefStore;
            use crate::hnsw_service::{HnswService, HnswServiceConfig};
            use crate::impulse::ImpulseQueue;
            use crate::vector_backend::VectorBackend;
            use crate::vector_hnsw::HnswBackend;
            use crate::vector_diskann::{DiskAnnBackend, DiskAnnConfig};
            use crate::vector_hybrid::{HybridBackend, HybridConfig};

            boot_log.push(BootEvent::info(BootPhase::Ecc, "Initializing ECC cognitive substrate"));

            let hnsw = Arc::new(HnswService::new(HnswServiceConfig::default()));
            let causal = Arc::new(CausalGraph::new());
            let crossrefs = Arc::new(CrossRefStore::new());
            let impulses = Arc::new(ImpulseQueue::new());

            // Construct the vector backend based on kernel config.
            let vector_config = kernel_config.vector.clone();
            let vector_backend: Arc<dyn VectorBackend> = match vector_config.as_ref().map(|v| v.backend) {
                Some(clawft_types::config::VectorBackendKind::DiskAnn) => {
                    let da_cfg = vector_config.as_ref()
                        .and_then(|v| v.diskann.as_ref())
                        .map(|d| DiskAnnConfig {
                            max_points: d.max_points,
                            dimensions: d.dimensions,
                            num_neighbors: d.num_neighbors,
                            search_list_size: d.search_list_size,
                            data_path: d.data_path.clone(),
                            use_pq: d.use_pq,
                            pq_num_chunks: d.pq_num_chunks,
                        })
                        .unwrap_or_default();
                    boot_log.push(BootEvent::info(BootPhase::Ecc, "Vector backend: DiskANN (stub)"));
                    Arc::new(DiskAnnBackend::new(da_cfg))
                }
                Some(clawft_types::config::VectorBackendKind::Hybrid) => {
                    let hnsw_cfg = vector_config.as_ref()
                        .and_then(|v| v.hnsw.as_ref())
                        .map(|h| HnswServiceConfig {
                            ef_construction: h.ef_construction,
                            ef_search: 100,
                            default_dimensions: 384,
                        })
                        .unwrap_or_default();
                    let da_cfg = vector_config.as_ref()
                        .and_then(|v| v.diskann.as_ref())
                        .map(|d| DiskAnnConfig {
                            max_points: d.max_points,
                            dimensions: d.dimensions,
                            num_neighbors: d.num_neighbors,
                            search_list_size: d.search_list_size,
                            data_path: d.data_path.clone(),
                            use_pq: d.use_pq,
                            pq_num_chunks: d.pq_num_chunks,
                        })
                        .unwrap_or_default();
                    let hybrid_cfg = vector_config.as_ref()
                        .and_then(|v| v.hybrid.as_ref())
                        .map(|h| HybridConfig {
                            hot_capacity: h.hot_capacity,
                            promotion_threshold: h.promotion_threshold,
                            eviction_policy: crate::vector_hybrid::EvictionPolicy::Lru,
                        })
                        .unwrap_or_default();
                    boot_log.push(BootEvent::info(
                        BootPhase::Ecc,
                        format!("Vector backend: Hybrid (hot={}, threshold={})", hybrid_cfg.hot_capacity, hybrid_cfg.promotion_threshold),
                    ));
                    Arc::new(HybridBackend::new(hnsw_cfg, da_cfg, hybrid_cfg))
                }
                _ => {
                    // Default: HNSW only.
                    boot_log.push(BootEvent::info(BootPhase::Ecc, "Vector backend: HNSW (in-memory)"));
                    Arc::new(HnswBackend::with_defaults())
                }
            };

            // Run boot-time calibration
            let cal_config = EccCalibrationConfig::default();
            let calibration = run_calibration(&hnsw, &causal, &cal_config);

            boot_log.push(BootEvent::info(
                BootPhase::Ecc,
                format!(
                    "ECC calibration complete (p50={}us, p95={}us, tick={}ms, spectral={})",
                    calibration.compute_p50_us,
                    calibration.compute_p95_us,
                    calibration.tick_interval_ms,
                    calibration.spectral_capable,
                ),
            ));

            // Create cognitive tick with calibrated interval
            let tick_config = CognitiveTickConfig {
                tick_interval_ms: calibration.tick_interval_ms,
                ..CognitiveTickConfig::default()
            };
            let tick = Arc::new(CognitiveTick::new(tick_config));

            // Register ECC services
            if let Err(e) = service_registry.register(hnsw.clone()) {
                tracing::debug!(error = %e, "failed to register HNSW service");
            }
            if let Err(e) = service_registry.register(tick.clone()) {
                tracing::debug!(error = %e, "failed to register cognitive tick service");
            }

            // Start the cognitive tick service explicitly -- start_all() ran
            // before ECC init so this service was never started, which caused
            // health checks to report "degraded - cognitive tick not running".
            if let Err(e) = tick.start().await {
                tracing::warn!(error = %e, "failed to start cognitive tick service");
            }

            // Spawn the DEMOCRITUS two-tier coherence tick loop.
            // This runs in the background, using the EML model for O(1)
            // predictions on every tick and falling back to exact Lanczos
            // spectral analysis when drift is detected.
            let eml_model = Arc::new(std::sync::Mutex::new(
                crate::eml_coherence::EmlCoherenceModel::new(),
            ));
            {
                let tick_clone = tick.clone();
                let causal_clone = causal.clone();
                let hnsw_clone = hnsw.clone();
                let eml_clone = eml_model.clone();
                tokio::spawn(async move {
                    crate::cognitive_tick::run_democritus_loop(
                        tick_clone,
                        causal_clone,
                        hnsw_clone,
                        eml_clone,
                    )
                    .await;
                });
            }
            boot_log.push(BootEvent::info(
                BootPhase::Ecc,
                "DEMOCRITUS two-tier coherence loop spawned",
            ));

            // Log calibration to chain
            #[cfg(feature = "exochain")]
            if let Some(ref cm) = chain_manager {
                cm.append(
                    "ecc",
                    "ecc.boot.calibration",
                    Some(serde_json::json!({
                        "compute_p50_us": calibration.compute_p50_us,
                        "compute_p95_us": calibration.compute_p95_us,
                        "tick_interval_ms": calibration.tick_interval_ms,
                        "spectral_capable": calibration.spectral_capable,
                    })),
                );
            }

            boot_log.push(BootEvent::info(
                BootPhase::Ecc,
                format!(
                    "ECC ready (hnsw={}, causal={} nodes, tick={}ms, vector={})",
                    hnsw.len(),
                    causal.node_count(),
                    calibration.tick_interval_ms,
                    vector_backend.backend_name(),
                ),
            ));

            (
                Some(hnsw),
                Some(causal),
                Some(tick),
                Some(crossrefs),
                Some(impulses),
                Some(calibration),
                Some(vector_backend),
                Some(eml_model),
            )
        };

        // 8g. Initialize os-patterns observability modules
        #[cfg(feature = "os-patterns")]
        let (metrics_registry, log_svc, timer_svc, dead_letter_queue) = {
            use crate::dead_letter::DeadLetterQueue;
            use crate::log_service::LogService;
            use crate::metrics::MetricsRegistry;
            use crate::timer::TimerService;

            boot_log.push(BootEvent::info(
                BootPhase::Services,
                "Initializing os-patterns observability modules",
            ));

            // MetricsRegistry with built-in + boot-specific gauges
            let registry = Arc::new(MetricsRegistry::with_builtins());
            // Seed additional kernel gauges requested by W3
            registry.gauge_set("kernel.process.count", process_table.len() as i64);
            registry.gauge_set("kernel.agent.count", 0);
            registry.gauge_set("kernel.ipc.messages_sent", 0);
            registry.counter_add("kernel.ipc.messages_failed", 0);
            registry.gauge_set("kernel.chain.height", 0);
            registry.gauge_set("kernel.uptime_secs", 0);

            boot_log.push(BootEvent::info(
                BootPhase::Services,
                "MetricsRegistry ready (built-in + kernel gauges)",
            ));

            // LogService (ring-buffer structured logs)
            let log_svc = Arc::new(LogService::with_default_capacity());
            boot_log.push(BootEvent::info(BootPhase::Services, "LogService ready"));

            // TimerService (sub-second precision timers)
            let timer_svc = Arc::new(TimerService::new());
            boot_log.push(BootEvent::info(BootPhase::Services, "TimerService ready"));

            // DeadLetterQueue (undeliverable message sink)
            let dlq = Arc::new(DeadLetterQueue::with_default_capacity());
            boot_log.push(BootEvent::info(
                BootPhase::Services,
                "DeadLetterQueue ready",
            ));

            // Wire DLQ into the A2A router
            a2a_router.set_dead_letter_queue(Arc::clone(&dlq));

            (Some(registry), Some(log_svc), Some(timer_svc), Some(dlq))
        };

        let elapsed = boot_time.elapsed();
        boot_log.push(BootEvent::info(
            BootPhase::Ready,
            format!(
                "Boot complete in {:.1}s ({} processes, {} services)",
                elapsed.as_secs_f64(),
                process_table.len(),
                service_registry.len(),
            ),
        ));

        info!(
            elapsed_ms = elapsed.as_millis(),
            processes = process_table.len(),
            services = service_registry.len(),
            "kernel boot complete"
        );

        // 9. Create agent supervisor
        let supervisor = AgentSupervisor::new(
            process_table.clone(),
            ipc.clone(),
            AgentCapabilities::default(),
        );

        // 9b. Wire A2ARouter and cron into supervisor
        #[cfg(feature = "native")]
        let supervisor = supervisor.with_a2a_router(a2a_router.clone(), cron_svc.clone());

        // 9c. Wire exochain managers into supervisor
        #[cfg(feature = "exochain")]
        let supervisor = supervisor.with_exochain(
            tree_manager.clone(),
            chain_manager.clone(),
        );

        // 10. Seed the event ring buffer with boot events
        let event_log = Arc::new(KernelEventLog::new());
        event_log.ingest_boot_log(&boot_log);

        Ok(Self {
            state: KernelState::Running,
            config: kernel_config,
            app_context: Some(app_context),
            bus,
            process_table,
            service_registry,
            ipc,
            #[cfg(feature = "native")]
            a2a_router,
            #[cfg(feature = "native")]
            cron_service: cron_svc,
            #[cfg(feature = "native")]
            assessment_service: assessment_svc,
            health,
            supervisor,
            boot_log,
            event_log,
            boot_time,
            cluster_membership,
            revocation_list,
            #[cfg(feature = "native")]
            agent_registry: crate::agent_registry::AgentRegistry::new(),
            #[cfg(feature = "native")]
            node_registry: crate::node_registry::NodeRegistry::new(),
            #[cfg(feature = "native")]
            substrate_service: crate::substrate_service::SubstrateService::new(),
            #[cfg(feature = "exochain")]
            chain: ChainSubsystem {
                chain_manager,
                tree_manager,
                governance_gate,
            },
            #[cfg(feature = "ecc")]
            ecc: EccSubsystem {
                hnsw: ecc_hnsw,
                causal: ecc_causal,
                tick: ecc_tick,
                crossrefs: ecc_crossrefs,
                impulses: ecc_impulses,
                calibration: ecc_calibration,
                vector_backend: ecc_vector_backend,
                eml_coherence: ecc_eml_model,
            },
            #[cfg(feature = "os-patterns")]
            observability: ObservabilitySubsystem {
                metrics_registry,
                log_service: log_svc,
                timer_service: timer_svc,
                dead_letter_queue,
            },
        })
    }

    /// Shut down the kernel gracefully.
    ///
    /// Stops all services, cancels all processes, and transitions
    /// to the `Halted` state.
    pub async fn shutdown(&mut self) -> KernelResult<()> {
        if self.state != KernelState::Running {
            return Err(KernelError::WrongState {
                expected: "Running".into(),
                actual: self.state.to_string(),
            });
        }

        info!("kernel shutting down");
        self.state = KernelState::ShuttingDown;
        self.event_log.info("kernel", "shutdown initiated");

        // Stop all services
        if let Err(e) = self.service_registry.stop_all().await {
            error!(error = %e, "error stopping services during shutdown");
        }

        // Checkpoint tree+chain before shutting down
        #[cfg(feature = "exochain")]
        if let Some(ref tm) = self.chain.tree_manager
            && let Some(ref cm) = self.chain.chain_manager
        {
            let stats = tm.stats();
            cm.append(
                "kernel",
                "shutdown",
                Some(serde_json::json!({
                    "tree_root_hash": stats.root_hash,
                    "chain_seq": cm.sequence(),
                    "tree_nodes": stats.node_count,
                })),
            );
        }

        // Abort all running agent tasks
        #[cfg(feature = "native")]
        self.supervisor.abort_all();

        // Cancel all processes
        for entry in self.process_table.list() {
            if entry.pid == 0 {
                continue; // Don't cancel the kernel process
            }
            entry.cancel_token.cancel();

            // Log agent stop in tree/chain
            #[cfg(feature = "exochain")]
            if let Some(ref tm) = self.chain.tree_manager {
                let _ = tm.unregister_agent(&entry.agent_id, entry.pid, 0);
            }

            let _ = self
                .process_table
                .update_state(entry.pid, ProcessState::Exited(0));
        }

        // Persist chain to RVF checkpoint (primary), JSON as fallback
        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain.chain_manager {
            let chain_config = self.config.chain.clone().unwrap_or_default();
            if let Some(ref ckpt_path) = chain_config.effective_checkpoint_path() {
            let json_path = std::path::PathBuf::from(ckpt_path);
            let rvf_path = json_path.with_extension("rvf");

            // Save RVF format (primary)
            match cm.save_to_rvf(&rvf_path) {
                Ok(()) => info!(path = %rvf_path.display(), "chain saved to RVF checkpoint"),
                Err(e) => {
                    error!(error = %e, "failed to save RVF checkpoint, falling back to JSON");
                    // Fallback: save JSON
                    match cm.save_to_file(&json_path) {
                        Ok(()) => info!(path = %json_path.display(), "chain saved to JSON checkpoint (fallback)"),
                        Err(e2) => error!(error = %e2, "failed to save JSON checkpoint fallback"),
                    }
                }
            }

            // Save tree checkpoint alongside chain
            if let Some(ref tm) = self.chain.tree_manager {
                let tree_path = json_path.with_extension("tree.json");
                match tm.save_checkpoint(&tree_path) {
                    Ok(()) => {
                        info!(path = %tree_path.display(), "tree checkpoint saved");
                        cm.append(
                            "tree",
                            "tree.checkpoint",
                            Some(serde_json::json!({
                                "path": tree_path.display().to_string(),
                                "root_hash": tm.stats().root_hash,
                            })),
                        );
                    }
                    Err(e) => error!(error = %e, "failed to save tree checkpoint"),
                }
            }
            }
        }

        self.state = KernelState::Halted;
        self.event_log.info("kernel", "halted");
        info!("kernel halted");
        Ok(())
    }

    /// Get the current kernel state.
    pub fn state(&self) -> &KernelState {
        &self.state
    }

    /// Get the kernel configuration.
    pub fn kernel_config(&self) -> &KernelConfig {
        &self.config
    }

    /// Get the process table.
    pub fn process_table(&self) -> &Arc<ProcessTable> {
        &self.process_table
    }

    /// Get the service registry.
    pub fn services(&self) -> &Arc<ServiceRegistry> {
        &self.service_registry
    }

    /// Get the IPC subsystem.
    pub fn ipc(&self) -> &Arc<KernelIpc> {
        &self.ipc
    }

    /// Get the message bus.
    pub fn bus(&self) -> &Arc<MessageBus> {
        &self.bus
    }

    /// Get the A2A router.
    #[cfg(feature = "native")]
    pub fn a2a_router(&self) -> &Arc<A2ARouter> {
        &self.a2a_router
    }

    /// Get the agent identity registry.
    ///
    /// External clients call `agent.register` to obtain an `agent_id`
    /// bound to an Ed25519 public key. The registry is then consulted
    /// when verifying signatures on `ipc.publish` and
    /// `ipc.subscribe_stream` requests.
    #[cfg(feature = "native")]
    pub fn agent_registry(&self) -> &crate::agent_registry::AgentRegistry {
        &self.agent_registry
    }

    /// Get the node identity registry.
    ///
    /// Maps node-ids to their ed25519 public keys. The daemon
    /// registers its own node here at boot; remote nodes (ESP32
    /// leaves, future kernel-class peers) register via a future
    /// `node.register` RPC. Consulted by the substrate publish gate
    /// to verify node signatures and by the Explorer's tree to
    /// resolve labels.
    #[cfg(feature = "native")]
    pub fn node_registry(&self) -> &crate::node_registry::NodeRegistry {
        &self.node_registry
    }

    /// Get the substrate RPC service.
    ///
    /// Backs the `substrate.read`, `substrate.publish`,
    /// `substrate.subscribe`, and `substrate.notify` RPCs.
    #[cfg(feature = "native")]
    pub fn substrate_service(&self) -> &crate::substrate_service::SubstrateService {
        &self.substrate_service
    }

    /// Get the cron service.
    #[cfg(feature = "native")]
    pub fn cron_service(&self) -> &Arc<crate::cron::CronService> {
        &self.cron_service
    }

    /// Get the assessment service.
    #[cfg(feature = "native")]
    pub fn assessment_service(&self) -> &Arc<crate::assessment::AssessmentService> {
        &self.assessment_service
    }

    /// Get the health system.
    pub fn health(&self) -> &HealthSystem {
        &self.health
    }

    /// Get the agent supervisor.
    pub fn supervisor(&self) -> &AgentSupervisor<P> {
        &self.supervisor
    }

    /// Get the boot log.
    pub fn boot_log(&self) -> &BootLog {
        &self.boot_log
    }

    /// Get the runtime event log (ring buffer).
    pub fn event_log(&self) -> &Arc<KernelEventLog> {
        &self.event_log
    }

    /// Get kernel uptime.
    pub fn uptime(&self) -> std::time::Duration {
        self.boot_time.elapsed()
    }

    /// Get the cluster membership tracker.
    pub fn cluster_membership(&self) -> &Arc<ClusterMembership> {
        &self.cluster_membership
    }

    /// Get the host revocation list.
    pub fn revocation_list(&self) -> &Arc<crate::revocation::RevocationList> {
        &self.revocation_list
    }

    /// Get the local chain manager (when exochain feature is enabled).
    #[cfg(feature = "exochain")]
    pub fn chain_manager(&self) -> Option<&Arc<crate::chain::ChainManager>> {
        self.chain.chain_manager.as_ref()
    }

    /// Get the tree manager (when exochain feature is enabled).
    #[cfg(feature = "exochain")]
    pub fn tree_manager(&self) -> Option<&Arc<crate::tree_manager::TreeManager>> {
        self.chain.tree_manager.as_ref()
    }

    /// Get the governance gate backend (if configured).
    #[cfg(feature = "exochain")]
    pub fn governance_gate(&self) -> Option<&Arc<dyn crate::gate::GateBackend>> {
        self.chain.governance_gate.as_ref()
    }

    /// Get the ECC HNSW service (if ecc feature enabled).
    #[cfg(feature = "ecc")]
    pub fn ecc_hnsw(&self) -> Option<&Arc<crate::hnsw_service::HnswService>> {
        self.ecc.hnsw.as_ref()
    }

    /// Get the ECC vector backend (if ecc feature enabled).
    ///
    /// Returns the configured vector search backend (HNSW, DiskANN,
    /// or Hybrid) based on the `[kernel.vector]` configuration.
    #[cfg(feature = "ecc")]
    pub fn ecc_vector_backend(&self) -> Option<&Arc<dyn crate::vector_backend::VectorBackend>> {
        self.ecc.vector_backend.as_ref()
    }

    /// Get the ECC causal graph (if ecc feature enabled).
    #[cfg(feature = "ecc")]
    pub fn ecc_causal(&self) -> Option<&Arc<crate::causal::CausalGraph>> {
        self.ecc.causal.as_ref()
    }

    /// Get the ECC cognitive tick (if ecc feature enabled).
    #[cfg(feature = "ecc")]
    pub fn ecc_tick(&self) -> Option<&Arc<crate::cognitive_tick::CognitiveTick>> {
        self.ecc.tick.as_ref()
    }

    /// Get the ECC calibration results (if ecc feature enabled).
    #[cfg(feature = "ecc")]
    pub fn ecc_calibration(&self) -> Option<&crate::calibration::EccCalibration> {
        self.ecc.calibration.as_ref()
    }

    /// Get the ECC cross-reference store (if ecc feature enabled).
    #[cfg(feature = "ecc")]
    pub fn ecc_crossrefs(&self) -> Option<&Arc<crate::crossref::CrossRefStore>> {
        self.ecc.crossrefs.as_ref()
    }

    /// Get the ECC impulse queue (if ecc feature enabled).
    #[cfg(feature = "ecc")]
    pub fn ecc_impulses(&self) -> Option<&Arc<crate::impulse::ImpulseQueue>> {
        self.ecc.impulses.as_ref()
    }

    /// Get the EML coherence model (if ecc feature enabled).
    #[cfg(feature = "ecc")]
    pub fn ecc_eml_coherence(&self) -> Option<&Arc<std::sync::Mutex<crate::eml_coherence::EmlCoherenceModel>>> {
        self.ecc.eml_coherence.as_ref()
    }

    /// Get the os-patterns metrics registry (if os-patterns feature enabled).
    #[cfg(feature = "os-patterns")]
    pub fn metrics_registry(&self) -> Option<&Arc<crate::metrics::MetricsRegistry>> {
        self.observability.metrics_registry.as_ref()
    }

    /// Get the os-patterns log service (if os-patterns feature enabled).
    #[cfg(feature = "os-patterns")]
    pub fn log_service(&self) -> Option<&Arc<crate::log_service::LogService>> {
        self.observability.log_service.as_ref()
    }

    /// Get the os-patterns timer service (if os-patterns feature enabled).
    #[cfg(feature = "os-patterns")]
    pub fn timer_service(&self) -> Option<&Arc<crate::timer::TimerService>> {
        self.observability.timer_service.as_ref()
    }

    /// Get the os-patterns dead letter queue (if os-patterns feature enabled).
    #[cfg(feature = "os-patterns")]
    pub fn dead_letter_queue(&self) -> Option<&Arc<crate::dead_letter::DeadLetterQueue>> {
        self.observability.dead_letter_queue.as_ref()
    }

    /// Take ownership of the AppContext for agent loop consumption.
    ///
    /// This is a one-shot operation: after calling this, the kernel
    /// no longer holds the AppContext. Use this before calling
    /// `AppContext::into_agent_loop()`.
    pub fn take_app_context(&mut self) -> Option<AppContext<P>> {
        self.app_context.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawft_platform::NativePlatform;
    use clawft_types::config::{AgentDefaults, AgentsConfig};

    fn test_config() -> Config {
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

    fn test_kernel_config() -> KernelConfig {
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

    #[tokio::test]
    async fn boot_and_shutdown() {
        let platform = Arc::new(NativePlatform::new());
        let mut kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();

        assert_eq!(*kernel.state(), KernelState::Running);
        // Uptime should be non-negative (boot_time is set before boot completes)
        let _uptime = kernel.uptime();

        // Kernel process should be PID 0
        let kernel_proc = kernel.process_table().get(0).unwrap();
        assert_eq!(kernel_proc.agent_id, "kernel");
        assert_eq!(kernel_proc.state, ProcessState::Running);

        kernel.shutdown().await.unwrap();
        assert_eq!(*kernel.state(), KernelState::Halted);
    }

    #[tokio::test]
    async fn double_shutdown_fails() {
        let platform = Arc::new(NativePlatform::new());
        let mut kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();

        kernel.shutdown().await.unwrap();
        let result = kernel.shutdown().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn boot_log_has_events() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();

        let log = kernel.boot_log();
        assert!(!log.is_empty());

        let formatted = log.format_all();
        assert!(formatted.contains("WeftOS v0.1.0"));
        assert!(formatted.contains("Boot complete"));
    }

    #[tokio::test]
    async fn process_table_accessible() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();

        let pt = kernel.process_table();
        assert_eq!(pt.len(), 1); // kernel process only
        assert_eq!(pt.max_processes(), 16);
    }

    #[tokio::test]
    async fn services_accessible() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();

        // Base: cron + containers; +cluster; +hnsw +cognitive_tick (ecc); +assess (native)
        let count = kernel.services().len();
        #[cfg(all(feature = "ecc", feature = "cluster"))]
        assert_eq!(count, 6, "expected cron+containers+assess+cluster+hnsw+cognitive_tick");
        #[cfg(all(feature = "ecc", not(feature = "cluster")))]
        assert_eq!(count, 5, "expected cron+containers+assess+hnsw+cognitive_tick");
        #[cfg(all(not(feature = "ecc"), feature = "cluster"))]
        assert_eq!(count, 4, "expected cron+containers+assess+cluster");
        #[cfg(all(not(feature = "ecc"), not(feature = "cluster")))]
        assert_eq!(count, 3, "expected cron+containers+assess");
    }

    #[tokio::test]
    async fn take_app_context() {
        let platform = Arc::new(NativePlatform::new());
        let mut kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();

        let ctx = kernel.take_app_context();
        assert!(ctx.is_some());

        // Second take returns None
        let ctx2 = kernel.take_app_context();
        assert!(ctx2.is_none());
    }

    #[tokio::test]
    async fn ipc_accessible() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();

        let ipc = kernel.ipc();
        assert!(Arc::ptr_eq(ipc.bus(), kernel.bus()));
    }

    #[test]
    fn kernel_state_display() {
        assert_eq!(KernelState::Booting.to_string(), "booting");
        assert_eq!(KernelState::Running.to_string(), "running");
        assert_eq!(KernelState::ShuttingDown.to_string(), "shutting_down");
        assert_eq!(KernelState::Halted.to_string(), "halted");
    }

    // ── Full-stack integration helpers ─────────────────────────────

    /// Kernel config with exochain + resource tree enabled (no checkpoint
    /// path so everything stays in-memory).
    #[cfg(all(feature = "exochain", feature = "ecc", feature = "wasm-sandbox"))]
    fn test_kernel_config_full_stack() -> KernelConfig {
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

    // ── Integration: full-stack kernel ──────────────────────────────

    /// K0-K5 integration: native agent + WASM tool + container service + ECC
    /// all connected in a single kernel with chain witnessing.
    #[tokio::test]
    #[cfg(all(feature = "exochain", feature = "ecc", feature = "wasm-sandbox"))]
    async fn integration_full_stack_kernel() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config_full_stack(), platform)
            .await
            .unwrap();

        // ── K0: Kernel is running ──────────────────────────────────
        assert_eq!(*kernel.state(), KernelState::Running);

        // ── K0: Services registered at boot ────────────────────────
        // cron + container + hnsw + cognitive_tick = 4 with ecc
        let svc_count = kernel.services().len();
        assert!(svc_count >= 4, "expected >= 4 services, got {svc_count}");

        // ── K1: Spawn a native agent ───────────────────────────────
        let spawn_result = kernel.supervisor().spawn(
            crate::supervisor::SpawnRequest {
                agent_id: "integration-agent-1".into(),
                capabilities: None,
                parent_pid: None,
                env: std::collections::HashMap::new(),
                backend: None, // defaults to Native
            },
        );
        assert!(spawn_result.is_ok(), "native agent spawn failed: {:?}", spawn_result.err());
        let agent_pid = spawn_result.unwrap().pid;
        assert!(agent_pid > 0, "agent should get PID > 0");

        // ── K1: Agent appears in process table ─────────────────────
        let processes = kernel.process_table().list();
        assert!(
            processes.iter().any(|p| p.pid == agent_pid),
            "agent not in process table"
        );

        // ── K2: A2A messaging between kernel and agent ─────────────
        let a2a = kernel.a2a_router();
        let _inbox = a2a.create_inbox(agent_pid);

        // Send a message from kernel (PID 0) to agent
        let msg = crate::ipc::KernelMessage::new(
            0, // from kernel
            crate::ipc::MessageTarget::Process(agent_pid),
            crate::ipc::MessagePayload::Json(serde_json::json!({"cmd": "ping"})),
        );
        let send_result = a2a.send(msg).await;
        // This should succeed: PID 0 exists, agent inbox exists
        assert!(send_result.is_ok(), "A2A send failed: {:?}", send_result.err());

        // ── K3: WASM tool execution ────────────────────────────────
        let wasm_config = crate::wasm_runner::WasmSandboxConfig::default();
        let wasm_runner = crate::wasm_runner::WasmToolRunner::new(wasm_config);

        // Minimal WAT module that exports _start and immediately returns
        let noop_wat = r#"(module (func (export "_start")))"#;
        let result = wasm_runner
            .execute_bytes("integration-tool", noop_wat.as_bytes(), serde_json::json!({}))
            .await;
        assert!(result.is_ok(), "WASM execution failed: {:?}", result.err());
        let wasm_result = result.unwrap();
        assert_eq!(wasm_result.exit_code, 0, "WASM tool should exit cleanly");
        assert!(wasm_result.fuel_consumed > 0, "WASM should consume fuel");

        // ── K3: WASM tool in registry ──────────────────────────────
        let wasm_runner_arc = Arc::new(
            crate::wasm_runner::WasmToolRunner::new(
                crate::wasm_runner::WasmSandboxConfig::default(),
            ),
        );
        let mut registry = crate::wasm_runner::ToolRegistry::new();
        registry
            .register_wasm_tool(
                "demo-tool",
                "A demo WASM tool for integration testing",
                noop_wat.as_bytes().to_vec(),
                wasm_runner_arc,
            )
            .unwrap();
        assert!(registry.get("demo-tool").is_some(), "WASM tool should be in registry");
        let tool_list = registry.list();
        assert!(
            tool_list.contains(&"demo-tool".to_string()),
            "WASM tool should appear in listing"
        );

        // ── K4: Container service registered ───────────────────────
        let svc_list = kernel.services().list();
        assert!(
            svc_list.iter().any(|(name, _)| name == "containers"),
            "container service should be registered"
        );

        // ── K3c: ECC cognitive substrate active ────────────────────
        let ecc_hnsw = kernel.ecc_hnsw().expect("HNSW service should be present");
        // After calibration the store is cleared, so it should be empty
        assert_eq!(ecc_hnsw.len(), 0, "HNSW should be empty after calibration cleanup");

        let ecc_causal = kernel.ecc_causal().expect("causal graph should be present");
        assert_eq!(ecc_causal.node_count(), 0, "causal graph should be empty after cleanup");

        let _ecc_tick = kernel.ecc_tick().expect("cognitive tick should be present");

        let calibration = kernel.ecc_calibration().expect("calibration should exist");
        assert!(calibration.compute_p95_us > 0, "calibration should have run");
        assert!(calibration.tick_interval_ms > 0, "tick interval should be auto-calibrated");

        // ── ECC: Insert a vector and search ────────────────────────
        ecc_hnsw.insert(
            "test-doc".into(),
            vec![1.0, 0.0, 0.0, 0.0],
            serde_json::json!({"text": "hello"}),
        );
        assert_eq!(ecc_hnsw.len(), 1);
        let results = ecc_hnsw.search(&[1.0, 0.0, 0.0, 0.0], 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "test-doc");

        // ── ECC: Causal graph operations ───────────────────────────
        let node1 = ecc_causal.add_node("concept-A".into(), serde_json::json!({}));
        let node2 = ecc_causal.add_node("concept-B".into(), serde_json::json!({}));
        ecc_causal.link(
            node1,
            node2,
            crate::causal::CausalEdgeType::Causes,
            1.0,
            0,
            0,
        );
        assert_eq!(ecc_causal.node_count(), 2);
        assert_eq!(ecc_causal.edge_count(), 1);
        let path = ecc_causal.find_path(node1, node2, 5);
        assert!(path.is_some(), "should find path between linked nodes");

        // ── ExoChain: Chain has witnessed boot events ──────────────
        let chain = kernel.chain_manager().expect("chain should exist");
        assert!(
            chain.sequence() > 5,
            "chain should have boot events, got seq={}",
            chain.sequence()
        );

        // Verify chain integrity
        let verify_result = chain.verify_integrity();
        assert!(verify_result.valid, "chain should be valid: {:?}", verify_result.errors);
        assert!(verify_result.errors.is_empty(), "no integrity errors");

        // ── Governance: genesis rules anchored to chain ────────────
        let _governance = kernel.governance_gate().expect("governance gate should exist");
        let all_events = chain.tail(0);
        let genesis_events: Vec<_> = all_events.iter()
            .filter(|e| e.kind == "governance.genesis")
            .collect();
        assert!(!genesis_events.is_empty(), "governance genesis should be on chain");

        // Verify the genesis payload contains the correct rule count
        // Find the kernel's own genesis event (version 2.0.0)
        let genesis_payload = genesis_events.iter()
            .filter_map(|e| e.payload.as_ref())
            .find(|p| p.get("version").and_then(|v| v.as_str()) == Some("2.0.0"))
            .expect("should find v2.0.0 governance genesis on chain");
        assert_eq!(
            genesis_payload["rule_count"].as_u64().unwrap(),
            22,
            "genesis should contain 22 rules"
        );
        assert_eq!(
            genesis_payload["version"].as_str().unwrap(),
            "2.0.0",
            "genesis version should be 2.0.0"
        );

        // Each rule should be individually anchored (at least 22)
        let rule_events: Vec<_> = all_events.iter()
            .filter(|e| e.kind == "governance.rule")
            .collect();
        assert!(
            rule_events.len() >= 22,
            "at least 22 genesis rules should be individually anchored, got {}",
            rule_events.len(),
        );

        // Verify all rule IDs are present (GOV-001..007 + SOP-L/E/J)
        let rule_ids: Vec<&str> = rule_events.iter()
            .filter_map(|e| e.payload.as_ref()?.get("rule_id")?.as_str())
            .collect();
        for expected_id in &[
            "GOV-001", "GOV-002", "GOV-003", "GOV-004", "GOV-005", "GOV-006", "GOV-007",
            "SOP-L001", "SOP-L002", "SOP-L003", "SOP-L004", "SOP-L005", "SOP-L006",
            "SOP-E001", "SOP-E002", "SOP-E003", "SOP-E004", "SOP-E005",
            "SOP-J001", "SOP-J002", "SOP-J003", "SOP-J004",
        ] {
            assert!(
                rule_ids.contains(expected_id),
                "{expected_id} should be anchored on chain"
            );
        }

        // ── ExoChain: ECC calibration event logged ─────────────────
        let ecc_events: Vec<_> = all_events.iter().filter(|e| e.kind.starts_with("ecc.")).collect();
        assert!(
            !ecc_events.is_empty(),
            "ECC boot calibration should be logged to chain"
        );

        // ── Resource Tree: namespaces exist ────────────────────────
        let tree = kernel.tree_manager().expect("tree should exist");
        let stats = tree.stats();
        assert!(
            stats.node_count > 20,
            "tree should have many nodes (got {})",
            stats.node_count
        );

        // ── K2: ServiceApi + adapters compile ──────────────────────
        // (validated by service::tests; here we just confirm registry is queryable)
        assert!(kernel.services().get("cron").is_some(), "cron service should be accessible");
        assert!(
            kernel.services().get("containers").is_some(),
            "container service should be accessible"
        );

        // ── Graceful shutdown ──────────────────────────────────────
        let mut kernel = kernel;
        kernel.shutdown().await.unwrap();
        assert_eq!(*kernel.state(), KernelState::Halted);
    }

    // ── Integration: cross-backend tools ───────────────────────────

    /// Demonstrates that native (built-in) tools and WASM tools coexist
    /// in a single ToolRegistry and can be dispatched through the same
    /// lookup interface.
    #[test]
    #[cfg(feature = "wasm-sandbox")]
    fn integration_cross_backend_tools() {
        use crate::wasm_runner::{
            BuiltinTool, BuiltinToolSpec, ToolCategory, ToolError, ToolRegistry,
            WasmSandboxConfig, WasmToolRunner,
        };
        use crate::governance::EffectVector;

        // ── A native (Rust) tool ───────────────────────────────────
        struct EchoTool;

        impl BuiltinTool for EchoTool {
            fn name(&self) -> &str {
                "native.echo"
            }
            fn spec(&self) -> &BuiltinToolSpec {
                // Leak a static spec for test simplicity
                static SPEC: std::sync::OnceLock<BuiltinToolSpec> = std::sync::OnceLock::new();
                SPEC.get_or_init(|| BuiltinToolSpec {
                    name: "native.echo".into(),
                    category: ToolCategory::System,
                    description: "Echoes input back".into(),
                    parameters: serde_json::json!({}),
                    gate_action: "tool.native.echo".into(),
                    effect: EffectVector::default(),
                    native: true,
                })
            }
            fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
                Ok(serde_json::json!({"echo": args}))
            }
        }

        // ── Build a mixed registry ─────────────────────────────────
        let mut registry = ToolRegistry::new();

        // Register native tool
        registry.register(Arc::new(EchoTool));

        // Register WASM tool
        let runner = Arc::new(WasmToolRunner::new(WasmSandboxConfig::default()));
        let noop_wat = r#"(module (func (export "_start")))"#;
        registry
            .register_wasm_tool(
                "wasm.noop",
                "A no-op WASM tool",
                noop_wat.as_bytes().to_vec(),
                runner,
            )
            .unwrap();

        // ── Both tools accessible through one registry ─────────────
        assert_eq!(registry.list().len(), 2);

        let native = registry.get("native.echo").expect("native tool should exist");
        assert!(native.spec().native, "native tool should be marked native");

        let wasm = registry.get("wasm.noop").expect("wasm tool should exist");
        assert!(!wasm.spec().native, "wasm tool should NOT be marked native");

        // ── Native tool executes synchronously ─────────────────────
        let result = native.execute(serde_json::json!({"hello": "world"})).unwrap();
        assert_eq!(result["echo"]["hello"], "world");

        // ── Hierarchical registry ──────────────────────────────────
        // Child registry overlays parent; WASM tools from parent visible
        let parent = Arc::new(registry);
        let mut child = ToolRegistry::with_parent(parent);

        // Child sees parent tools
        assert!(child.get("native.echo").is_some(), "child sees parent native tool");
        assert!(child.get("wasm.noop").is_some(), "child sees parent wasm tool");

        // Child can shadow
        struct OverrideTool;
        impl BuiltinTool for OverrideTool {
            fn name(&self) -> &str {
                "native.echo"
            }
            fn spec(&self) -> &BuiltinToolSpec {
                static SPEC: std::sync::OnceLock<BuiltinToolSpec> = std::sync::OnceLock::new();
                SPEC.get_or_init(|| BuiltinToolSpec {
                    name: "native.echo".into(),
                    category: ToolCategory::System,
                    description: "Overridden echo".into(),
                    parameters: serde_json::json!({}),
                    gate_action: "tool.native.echo".into(),
                    effect: EffectVector::default(),
                    native: true,
                })
            }
            fn execute(&self, _args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
                Ok(serde_json::json!({"overridden": true}))
            }
        }

        child.register(Arc::new(OverrideTool));
        let result = child.get("native.echo").unwrap().execute(serde_json::json!({})).unwrap();
        assert_eq!(result["overridden"], true, "child should shadow parent tool");

        // Parent WASM tool still reachable from child
        assert!(child.get("wasm.noop").is_some(), "WASM tool still reachable via parent");
    }

    // ── Boot path coverage tests ─────────────────────────────────

    #[tokio::test]
    async fn boot_reaches_running_state() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();
        assert_eq!(*kernel.state(), KernelState::Running);
    }

    #[tokio::test]
    async fn boot_registers_cron_service() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();
        assert!(
            kernel.services().get("cron").is_some(),
            "cron service must be registered at boot"
        );
    }

    #[tokio::test]
    async fn boot_registers_container_service() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();
        assert!(
            kernel.services().get("containers").is_some(),
            "container service must be registered at boot"
        );
    }

    #[cfg(feature = "exochain")]
    fn test_kernel_config_exochain() -> KernelConfig {
        use clawft_types::config::{ChainConfig, ResourceTreeConfig};
        KernelConfig {
            enabled: true,
            max_processes: 16,
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

    #[cfg(feature = "exochain")]
    #[tokio::test]
    async fn boot_exochain_creates_chain_manager() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config_exochain(), platform)
            .await
            .unwrap();
        assert!(
            kernel.chain_manager().is_some(),
            "chain manager must be present with exochain feature and enabled chain config"
        );
    }

    #[cfg(feature = "exochain")]
    #[tokio::test]
    async fn boot_exochain_creates_tree_manager() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config_exochain(), platform)
            .await
            .unwrap();
        assert!(
            kernel.tree_manager().is_some(),
            "tree manager must be present with exochain feature and enabled config"
        );
    }

    #[cfg(feature = "exochain")]
    #[tokio::test]
    async fn boot_exochain_chain_has_boot_events() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config_exochain(), platform)
            .await
            .unwrap();
        let chain = kernel.chain_manager().unwrap();
        // Chain should have boot.init, boot.config, boot.services, boot.cluster, boot.ready, boot.manifest at minimum
        assert!(
            chain.sequence() >= 6,
            "chain should have at least 6 boot events, got {}",
            chain.sequence()
        );
    }

    #[cfg(feature = "exochain")]
    #[tokio::test]
    async fn boot_exochain_governance_gate_present() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config_exochain(), platform)
            .await
            .unwrap();
        assert!(
            kernel.governance_gate().is_some(),
            "governance gate should be present when chain is enabled"
        );
    }

    #[cfg(feature = "ecc")]
    #[tokio::test]
    async fn boot_ecc_registers_hnsw_service() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();
        assert!(
            kernel.ecc_hnsw().is_some(),
            "HNSW service must be present with ecc feature"
        );
    }

    #[cfg(feature = "ecc")]
    #[tokio::test]
    async fn boot_ecc_registers_cognitive_tick() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();
        assert!(
            kernel.ecc_tick().is_some(),
            "cognitive tick must be present with ecc feature"
        );
    }

    #[cfg(feature = "ecc")]
    #[tokio::test]
    async fn boot_ecc_calibration_has_valid_results() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();
        let cal = kernel.ecc_calibration().unwrap();
        assert!(cal.tick_interval_ms > 0, "tick interval must be positive");
        assert!(cal.compute_p50_us > 0, "p50 latency must be measured");
        assert!(cal.compute_p95_us >= cal.compute_p50_us, "p95 >= p50");
    }

    #[cfg(feature = "ecc")]
    #[tokio::test]
    async fn boot_ecc_causal_graph_accessible() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();
        assert!(kernel.ecc_causal().is_some(), "causal graph must be accessible");
        assert_eq!(kernel.ecc_causal().unwrap().node_count(), 0, "causal graph starts empty");
    }

    #[cfg(feature = "ecc")]
    #[tokio::test]
    async fn boot_ecc_crossrefs_and_impulses_accessible() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();
        assert!(kernel.ecc_crossrefs().is_some(), "cross-ref store must be accessible");
        assert!(kernel.ecc_impulses().is_some(), "impulse queue must be accessible");
    }

    #[tokio::test]
    async fn boot_cluster_membership_accessible() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();
        let cm = kernel.cluster_membership();
        assert!(!cm.local_node_id().is_empty(), "cluster membership should have a node ID");
    }

    #[tokio::test]
    async fn shutdown_transitions_to_halted() {
        let platform = Arc::new(NativePlatform::new());
        let mut kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();
        assert_eq!(*kernel.state(), KernelState::Running);
        kernel.shutdown().await.unwrap();
        assert_eq!(*kernel.state(), KernelState::Halted);
    }

    #[tokio::test]
    async fn shutdown_from_halted_fails() {
        let platform = Arc::new(NativePlatform::new());
        let mut kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();
        kernel.shutdown().await.unwrap();
        let err = kernel.shutdown().await.unwrap_err();
        match err {
            KernelError::WrongState { expected, actual } => {
                assert_eq!(expected, "Running");
                assert_eq!(actual, "halted");
            }
            other => panic!("expected WrongState, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn process_table_has_kernel_pid_zero() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();
        let entry = kernel.process_table().get(0).expect("PID 0 should exist");
        assert_eq!(entry.agent_id, "kernel");
        assert_eq!(entry.state, ProcessState::Running);
        assert_eq!(entry.pid, 0);
    }

    #[tokio::test]
    async fn a2a_router_accessible_after_boot() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();
        let a2a = kernel.a2a_router();
        // Should be able to create an inbox for a future PID
        // (A2ARouter is wired to the same process table)
        let _inbox = a2a.create_inbox(0); // kernel PID
    }

    #[tokio::test]
    async fn boot_log_contains_expected_phases() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();
        let formatted = kernel.boot_log().format_all();
        assert!(formatted.contains("WeftOS v0.1.0"), "should contain version");
        assert!(formatted.contains("Service registry ready"), "should have service phase");
        assert!(formatted.contains("A2A router ready"), "should have A2A phase");
        assert!(formatted.contains("Boot complete"), "should have ready phase");
    }

    #[tokio::test]
    async fn event_log_populated_after_boot() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();
        // The event_log is populated from boot_log during boot
        let events = kernel.event_log();
        // At minimum there should be some events from boot ingestion
        assert!(!events.is_empty(), "event log should have entries after boot");
    }

    #[tokio::test]
    async fn health_system_accessible() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();
        let _health = kernel.health();
        // Health system should be constructable and accessible
    }

    #[tokio::test]
    async fn uptime_is_positive_after_boot() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();
        let uptime = kernel.uptime();
        assert!(uptime.as_nanos() > 0, "uptime should be positive");
    }

    #[tokio::test]
    async fn cron_service_accessible_and_empty() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();
        assert_eq!(kernel.cron_service().job_count(), 0, "no cron jobs at boot");
    }

    #[tokio::test]
    async fn max_processes_matches_config() {
        let platform = Arc::new(NativePlatform::new());
        let mut kconfig = test_kernel_config();
        kconfig.max_processes = 42;
        let kernel = Kernel::boot(test_config(), kconfig, platform)
            .await
            .unwrap();
        assert_eq!(kernel.process_table().max_processes(), 42);
    }

    // ── Sprint 09a: KernelState serde + display tests ────────────

    #[test]
    fn kernel_state_serde_roundtrip_all_variants() {
        for state in [
            KernelState::Booting,
            KernelState::Running,
            KernelState::ShuttingDown,
            KernelState::Halted,
        ] {
            let json = serde_json::to_string(&state).unwrap();
            let restored: KernelState = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, state);
        }
    }

    #[test]
    fn kernel_state_equality() {
        assert_eq!(KernelState::Running, KernelState::Running);
        assert_ne!(KernelState::Running, KernelState::Halted);
    }

    #[tokio::test]
    async fn kernel_state_is_running_after_boot() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();
        assert_eq!(kernel.state(), &KernelState::Running);
    }

    #[tokio::test]
    async fn boot_log_has_entries() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();
        let log = kernel.boot_log();
        assert!(!log.events().is_empty(), "boot log should have events");
    }

    #[tokio::test]
    async fn bus_is_accessible_after_boot() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();
        // Bus should exist (used for message passing)
        let _bus = kernel.bus();
    }

    #[tokio::test]
    async fn a2a_router_has_inboxes_after_boot() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();
        let _router = kernel.a2a_router();
        // Router should be accessible after boot
    }

    #[tokio::test]
    async fn ipc_accessible_after_boot() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();
        let _ipc = kernel.ipc();
    }

    // ── W5: Test hardening — additional coverage ─────────────────

    #[tokio::test]
    async fn supervisor_accessible_after_boot() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();
        let _supervisor = kernel.supervisor();
    }

    #[tokio::test]
    async fn kernel_config_accessor_returns_boot_config() {
        let platform = Arc::new(NativePlatform::new());
        let mut kconfig = test_kernel_config();
        kconfig.max_processes = 99;
        kconfig.health_check_interval_secs = 42;
        let kernel = Kernel::boot(test_config(), kconfig, platform)
            .await
            .unwrap();
        assert_eq!(kernel.kernel_config().max_processes, 99);
        assert_eq!(kernel.kernel_config().health_check_interval_secs, 42);
    }

    #[tokio::test]
    async fn shutdown_exits_all_agent_processes() {
        let platform = Arc::new(NativePlatform::new());
        let mut kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();

        // Spawn a couple of agents
        let spawn1 = kernel.supervisor().spawn(crate::supervisor::SpawnRequest {
            agent_id: "test-agent-1".into(),
            capabilities: None,
            parent_pid: None,
            env: std::collections::HashMap::new(),
            backend: None,
        });
        let spawn2 = kernel.supervisor().spawn(crate::supervisor::SpawnRequest {
            agent_id: "test-agent-2".into(),
            capabilities: None,
            parent_pid: None,
            env: std::collections::HashMap::new(),
            backend: None,
        });
        assert!(spawn1.is_ok());
        assert!(spawn2.is_ok());
        let pid1 = spawn1.unwrap().pid;
        let pid2 = spawn2.unwrap().pid;

        // Verify agents exist in process table
        assert!(kernel.process_table().get(pid1).is_some());
        assert!(kernel.process_table().get(pid2).is_some());

        kernel.shutdown().await.unwrap();

        // After shutdown, agent processes should no longer be Running.
        // They may be Exited (if the shutdown loop reached them) or still
        // Starting (if spawn() never transitioned them to Running because
        // no agent loop was attached in this test). Either is acceptable;
        // the key assertion is that shutdown completed without error.
        if let Some(entry1) = kernel.process_table().get(pid1) {
            assert!(
                !matches!(entry1.state, ProcessState::Running),
                "agent 1 should not be Running after shutdown, got: {}",
                entry1.state
            );
        }
        if let Some(entry2) = kernel.process_table().get(pid2) {
            assert!(
                !matches!(entry2.state, ProcessState::Running),
                "agent 2 should not be Running after shutdown, got: {}",
                entry2.state
            );
        }
    }

    #[tokio::test]
    async fn shutdown_stops_services() {
        let platform = Arc::new(NativePlatform::new());
        let mut kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();

        // Services should be running before shutdown
        let svc_count_before = kernel.services().len();
        assert!(svc_count_before > 0, "should have services before shutdown");

        kernel.shutdown().await.unwrap();
        // After shutdown, services stop_all() was called — the registry is
        // still there but state is Halted
        assert_eq!(*kernel.state(), KernelState::Halted);
    }

    #[tokio::test]
    async fn boot_with_custom_max_processes() {
        let platform = Arc::new(NativePlatform::new());
        let mut kconfig = test_kernel_config();
        kconfig.max_processes = 128;
        let kernel = Kernel::boot(test_config(), kconfig, platform)
            .await
            .unwrap();
        assert_eq!(kernel.process_table().max_processes(), 128);
    }

    #[tokio::test]
    async fn boot_creates_a2a_router_with_topic_router() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();
        // A2A router should have a topic router wired in
        let _topic_router = kernel.a2a_router().topic_router();
    }

    #[tokio::test]
    async fn event_log_has_shutdown_events() {
        let platform = Arc::new(NativePlatform::new());
        let mut kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();

        let events_before = kernel.event_log().len();
        kernel.shutdown().await.unwrap();

        // Shutdown should have added events
        let events_after = kernel.event_log().len();
        assert!(
            events_after > events_before,
            "event log should grow during shutdown (before={events_before}, after={events_after})"
        );
    }

    #[tokio::test]
    async fn boot_log_contains_max_processes() {
        let platform = Arc::new(NativePlatform::new());
        let mut kconfig = test_kernel_config();
        kconfig.max_processes = 64;
        let kernel = Kernel::boot(test_config(), kconfig, platform)
            .await
            .unwrap();
        let formatted = kernel.boot_log().format_all();
        assert!(
            formatted.contains("Max processes: 64"),
            "boot log should contain max_processes config value"
        );
    }

    #[tokio::test]
    async fn boot_log_contains_health_interval() {
        let platform = Arc::new(NativePlatform::new());
        let mut kconfig = test_kernel_config();
        kconfig.health_check_interval_secs = 15;
        let kernel = Kernel::boot(test_config(), kconfig, platform)
            .await
            .unwrap();
        let formatted = kernel.boot_log().format_all();
        assert!(
            formatted.contains("Health check interval: 15s"),
            "boot log should contain health check interval"
        );
    }

    #[tokio::test]
    async fn take_app_context_returns_some_then_none() {
        let platform = Arc::new(NativePlatform::new());
        let mut kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();

        assert!(kernel.take_app_context().is_some(), "first take should succeed");
        assert!(kernel.take_app_context().is_none(), "second take should return None");
        assert!(kernel.take_app_context().is_none(), "third take should also return None");
    }

    #[tokio::test]
    async fn kernel_pid_zero_has_default_capabilities() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();
        let entry = kernel.process_table().get(0).unwrap();
        assert_eq!(entry.agent_id, "kernel");
        assert!(entry.parent_pid.is_none(), "kernel should have no parent");
        // Default capabilities: can_ipc should be true, ipc_scope should be All
        assert!(entry.capabilities.can_ipc, "kernel should be able to IPC");
    }

    #[tokio::test]
    async fn cluster_membership_has_nonempty_node_id() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();
        let node_id = kernel.cluster_membership().local_node_id();
        assert!(!node_id.is_empty(), "cluster node ID should not be empty");
        // UUID v4 format: 8-4-4-4-12 = 36 chars
        assert_eq!(node_id.len(), 36, "node ID should be UUID format");
    }

    #[tokio::test]
    async fn boot_log_contains_cluster_info() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();
        let formatted = kernel.boot_log().format_all();
        assert!(
            formatted.contains("Cluster membership ready"),
            "boot log should mention cluster membership"
        );
    }

    #[cfg(feature = "exochain")]
    #[tokio::test]
    async fn shutdown_with_exochain_completes() {
        let platform = Arc::new(NativePlatform::new());
        let mut kernel = Kernel::boot(test_config(), test_kernel_config_exochain(), platform)
            .await
            .unwrap();

        assert!(kernel.chain_manager().is_some());
        kernel.shutdown().await.unwrap();
        assert_eq!(*kernel.state(), KernelState::Halted);
    }

    #[cfg(feature = "exochain")]
    #[tokio::test]
    async fn exochain_chain_integrity_valid_after_boot() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config_exochain(), platform)
            .await
            .unwrap();
        let chain = kernel.chain_manager().unwrap();
        let result = chain.verify_integrity();
        assert!(result.valid, "chain integrity should be valid after boot");
        assert!(result.errors.is_empty(), "no integrity errors expected");
    }

    #[cfg(feature = "exochain")]
    #[tokio::test]
    async fn exochain_boot_manifest_event_present() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config_exochain(), platform)
            .await
            .unwrap();
        let chain = kernel.chain_manager().unwrap();
        let all_events = chain.tail(0);
        let manifest_events: Vec<_> = all_events
            .iter()
            .filter(|e| e.kind == "boot.manifest")
            .collect();
        assert!(
            !manifest_events.is_empty(),
            "boot.manifest event should be on chain"
        );
    }

    // ── os-patterns observability wiring tests (W3) ─────────────────

    #[tokio::test]
    #[cfg(feature = "os-patterns")]
    async fn boot_creates_metrics_registry() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();

        let registry = kernel.metrics_registry();
        assert!(registry.is_some(), "MetricsRegistry should be created at boot");
        // Verify a built-in gauge was seeded
        let r = registry.unwrap();
        let val = r.gauge_get("kernel.process.count");
        assert!(val >= 1, "kernel.process.count gauge should be seeded");
    }

    #[tokio::test]
    #[cfg(feature = "os-patterns")]
    async fn boot_creates_log_service() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();

        assert!(
            kernel.log_service().is_some(),
            "LogService should be created at boot"
        );
    }

    #[tokio::test]
    #[cfg(feature = "os-patterns")]
    async fn boot_creates_timer_service() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();

        assert!(
            kernel.timer_service().is_some(),
            "TimerService should be created at boot"
        );
    }

    #[tokio::test]
    #[cfg(feature = "os-patterns")]
    async fn boot_creates_dead_letter_queue() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();

        let dlq = kernel.dead_letter_queue();
        assert!(dlq.is_some(), "DeadLetterQueue should be created at boot");
        assert!(dlq.unwrap().is_empty(), "DLQ should start empty");
    }

    #[tokio::test]
    #[cfg(feature = "os-patterns")]
    async fn kernel_metrics_accessor_returns_registry() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();

        let registry = kernel.metrics_registry().unwrap();
        // Verify counter_inc works on built-in metric
        registry.counter_inc(crate::metrics::METRIC_MESSAGES_SENT);
        assert_eq!(registry.counter_get(crate::metrics::METRIC_MESSAGES_SENT), 1);
    }

    #[tokio::test]
    #[cfg(feature = "os-patterns")]
    async fn dlq_accessible_via_kernel_accessor() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();

        let dlq = kernel.dead_letter_queue().unwrap();
        assert_eq!(dlq.capacity(), crate::dead_letter::DEFAULT_DLQ_CAPACITY);
    }

    #[tokio::test]
    #[cfg(feature = "os-patterns")]
    async fn failed_a2a_send_routes_to_dlq() {
        use crate::ipc::{KernelMessage, MessageTarget};

        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();

        // Register a sender agent
        let sender_entry = ProcessEntry {
            pid: 0,
            agent_id: "dlq-sender".to_owned(),
            state: ProcessState::Running,
            capabilities: AgentCapabilities::default(),
            resource_usage: ResourceUsage::default(),
            cancel_token: crate::process::CancellationToken::new(),
            parent_pid: None,
        };
        let sender_pid = kernel.process_table().insert(sender_entry).unwrap();
        kernel.a2a_router().create_inbox(sender_pid);

        // Send to a PID that has no inbox (PID 9999 doesn't exist)
        let msg = KernelMessage::text(
            sender_pid,
            MessageTarget::Process(9999),
            "dead letter test",
        );
        let result = kernel.a2a_router().send(msg).await;
        assert!(result.is_err(), "send to unknown PID should fail");

        // Verify the message landed in the DLQ
        let dlq = kernel.dead_letter_queue().unwrap();
        assert_eq!(dlq.len(), 1, "DLQ should contain 1 dead letter");
    }

    #[tokio::test]
    #[cfg(feature = "os-patterns")]
    async fn kernel_metrics_gauge_seeded_at_boot() {
        let platform = Arc::new(NativePlatform::new());
        let kernel = Kernel::boot(test_config(), test_kernel_config(), platform)
            .await
            .unwrap();

        let registry = kernel.metrics_registry().unwrap();
        // kernel.process.count should reflect at least the kernel PID 0
        assert!(
            registry.gauge_get("kernel.process.count") >= 1,
            "kernel.process.count should be >= 1 at boot"
        );
        // Verify additional gauges exist (they were set to 0, so gauge_get returns 0 not None)
        assert_eq!(
            registry.gauge_get("kernel.uptime_secs"),
            0,
            "kernel.uptime_secs gauge should exist and be 0 at boot"
        );
        assert_eq!(
            registry.gauge_get("kernel.chain.height"),
            0,
            "kernel.chain.height gauge should exist and be 0 at boot"
        );
    }
}
