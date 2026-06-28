//! WeftOS kernel layer for clawft.
//!
//! This crate provides the kernel abstraction layer that sits between
//! the CLI/API surface and `clawft-core`. It introduces:
//!
//! - **Boot sequence** ([`boot::Kernel`]) -- lifecycle management
//!   wrapping `AppContext` with structured startup/shutdown.
//! - **Process table** ([`process::ProcessTable`]) -- PID-based
//!   agent tracking with state machine transitions.
//! - **Service registry** ([`service::ServiceRegistry`]) -- named
//!   service lifecycle with health checks.
//! - **IPC** ([`ipc::KernelIpc`]) -- typed message envelopes over
//!   the existing `MessageBus`.
//! - **Capabilities** ([`capability::AgentCapabilities`]) -- permission
//!   model for agent processes.
//! - **Health monitoring** ([`health::HealthSystem`]) -- aggregated
//!   health checks across all services.
//! - **Console** ([`console`]) -- boot event types and output
//!   formatting for the interactive kernel terminal.
//! - **Configuration** ([`config::KernelConfig`]) -- kernel-specific
//!   settings embedded in the root config.
//! - **Containers** ([`container::ContainerManager`]) -- sidecar
//!   container lifecycle and health integration.
//! - **Applications** ([`app::AppManager`]) -- application manifest
//!   parsing, validation, and lifecycle state machine.
//! - **Cluster** ([`cluster::ClusterMembership`]) -- multi-node
//!   cluster membership, peer tracking, and health.
//! - **Environments** ([`environment::EnvironmentManager`]) --
//!   governance-scoped dev/staging/prod environments.
//! - **Governance** ([`governance::GovernanceEngine`]) -- three-branch
//!   constitutional governance with effect algebra scoring.
//! - **Agency** ([`agency::Agency`]) -- agent-first architecture
//!   with roles, spawn permissions, and agent manifests.
//!
//! # Feature Flags
//!
//! - `native` (default) -- enables tokio runtime, native file I/O.
//! - `wasm-sandbox` -- enables WASM tool runner (Phase K3).
//! - `containers` -- enables container manager (Phase K4).
//! - `ecc` -- enables ECC cognitive substrate (Phase K3c).
//!
//! ## Crate Ecosystem
//!
//! WeftOS is built from these crates:
//!
//! | Crate | Role |
//! |-------|------|
//! | [`weftos`](https://crates.io/crates/weftos) | Product facade -- re-exports kernel, core, types |
//! | [`clawft-kernel`](https://crates.io/crates/clawft-kernel) | Kernel: processes, services, governance, mesh, ExoChain |
//! | [`clawft-core`](https://crates.io/crates/clawft-core) | Agent framework: pipeline, context, tools, skills |
//! | [`clawft-types`](https://crates.io/crates/clawft-types) | Shared type definitions |
//! | [`clawft-platform`](https://crates.io/crates/clawft-platform) | Platform abstraction (native/WASM/browser) |
//! | [`clawft-plugin`](https://crates.io/crates/clawft-plugin) | Plugin SDK for tools, channels, and extensions |
//! | [`clawft-llm`](https://crates.io/crates/clawft-llm) | LLM provider abstraction (11 providers + local) |
//! | [`exo-resource-tree`](https://crates.io/crates/exo-resource-tree) | Hierarchical resource namespace with Merkle integrity |
//!
//! Source: <https://github.com/weave-logic-ai/weftos>

// ── ECC cognitive substrate modules (K3c) ────────────────────────
#[cfg(feature = "ecc")]
pub mod artifact_store;
#[cfg(feature = "ecc")]
pub mod calibration;
#[cfg(feature = "ecc")]
pub mod causal;
#[cfg(feature = "ecc")]
pub mod causal_predict;
#[cfg(feature = "ecc")]
pub mod cognitive_tick;
#[cfg(feature = "ecc")]
pub mod crossref;
#[cfg(feature = "ecc")]
pub mod democritus;
#[cfg(feature = "ecc")]
pub mod embedding;
#[cfg(feature = "ecc")]
pub mod embedding_onnx;
#[cfg(feature = "ecc")]
pub mod eml_coherence;
#[cfg(feature = "ecc")]
pub mod eml_kernel;
#[cfg(feature = "ecc")]
pub mod eml_persistence;
#[cfg(feature = "ecc")]
pub mod hnsw_eml;
#[cfg(feature = "ecc")]
pub mod hnsw_service;
#[cfg(feature = "ecc")]
pub mod impulse;
#[cfg(feature = "ecc")]
pub mod persistence;
#[cfg(feature = "ecc")]
pub mod profile_store;
#[cfg(feature = "ecc")]
pub mod quantum_backend;
#[cfg(all(feature = "ecc", feature = "quantum-braket"))]
pub mod quantum_braket;
#[cfg(all(feature = "ecc", feature = "quantum-pasqal"))]
pub mod quantum_pasqal;
#[cfg(feature = "ecc")]
pub mod quantum_register;
#[cfg(feature = "ecc")]
pub mod quantum_state;
#[cfg(feature = "ecc")]
pub mod vector_backend;
#[cfg(feature = "ecc")]
pub mod vector_diskann;
#[cfg(feature = "ecc")]
pub mod vector_hnsw;
#[cfg(feature = "ecc")]
pub mod vector_hybrid;
#[cfg(feature = "ecc")]
pub mod vector_quantization;
#[cfg(feature = "ecc")]
pub mod weaver;

#[cfg(feature = "sensor")]
pub mod sensor_graph;

#[cfg(feature = "native")]
pub mod a2a;
pub mod agency;
#[cfg(feature = "native")]
pub mod agent_loop;
#[cfg(feature = "native")]
pub mod agent_registry;
pub mod app;
pub mod assessment;
pub mod boot;
pub mod capability;
#[cfg(feature = "exochain")]
pub mod chain;
pub mod cluster;
pub mod config;
pub mod console;
pub mod container;
pub mod cron;
pub mod environment;
pub mod error;
#[cfg(feature = "exochain")]
pub mod gate;
pub mod governance;
pub mod health;
pub mod heartbeat;
pub mod ipc;
#[cfg(feature = "native")]
pub mod node_registry;
pub mod process;
pub mod revocation;
pub mod service;
#[cfg(all(feature = "native", feature = "exochain"))]
pub mod stream_anchor;
#[cfg(feature = "native")]
pub mod substrate_service;
pub mod supervisor;
pub mod topic;
#[cfg(feature = "exochain")]
pub mod tree_manager;

// ── Self-healing & process management modules (08a) ─────────────
#[cfg(feature = "os-patterns")]
pub mod monitor;
#[cfg(feature = "os-patterns")]
pub mod reconciler;
#[allow(clippy::new_without_default)]
pub mod wasm_runner;

// ── Reliable IPC & observability modules (08b) ──────────────────
#[cfg(feature = "os-patterns")]
pub mod dead_letter;
#[cfg(feature = "os-patterns")]
pub mod log_service;
#[cfg(feature = "os-patterns")]
pub mod metrics;
#[cfg(feature = "os-patterns")]
pub mod named_pipe;
#[cfg(feature = "os-patterns")]
pub mod reliable_queue;
#[cfg(feature = "os-patterns")]
pub mod timer;

// ── Content integrity & operational services (08c) ───────────────
pub mod auth_service;
pub mod config_service;
#[cfg(feature = "http-api")]
pub mod http_api;
pub mod tree_view;

// ── Mesh networking modules (K6) ──────────────────────────────
#[cfg(feature = "mesh")]
pub mod mesh;
#[cfg(feature = "mesh")]
pub mod mesh_artifact;
#[cfg(feature = "mesh")]
pub mod mesh_assess;
#[cfg(feature = "mesh")]
pub mod mesh_bootstrap;
#[cfg(feature = "mesh")]
pub mod mesh_chain;
#[cfg(feature = "mesh")]
pub mod mesh_dedup;
#[cfg(feature = "mesh")]
pub mod mesh_discovery;
#[cfg(feature = "mesh")]
pub mod mesh_framing;
#[cfg(feature = "mesh")]
pub mod mesh_heartbeat;
#[cfg(feature = "mesh")]
pub mod mesh_ipc;
#[cfg(feature = "mesh")]
pub mod mesh_kad;
#[cfg(feature = "mesh")]
pub mod mesh_listener;
#[cfg(feature = "mesh")]
pub mod mesh_log;
#[cfg(feature = "mesh")]
pub mod mesh_mdns;
#[cfg(feature = "mesh")]
pub mod mesh_noise;
#[cfg(feature = "mesh")]
pub mod mesh_process;
#[cfg(feature = "mesh")]
pub mod mesh_runtime;
#[cfg(feature = "mesh")]
pub mod mesh_service;
#[cfg(feature = "mesh")]
pub mod mesh_service_adv;
#[cfg(feature = "mesh")]
pub mod mesh_tcp;
#[cfg(feature = "mesh")]
pub mod mesh_tree;
#[cfg(feature = "mesh")]
pub mod mesh_ws;

// Re-export key types at the crate level for convenience.
#[cfg(feature = "native")]
pub use a2a::A2ARouter;
pub use agency::{
    Agency, AgentHealth, AgentInterface, AgentManifest, AgentPriority, AgentResources,
    AgentRestartPolicy, AgentRole, InterfaceProtocol, ResponseMode,
};
#[cfg(feature = "native")]
pub use agent_registry::{
    AgentRegistry, RegisteredAgent, publish_payload, register_payload, subscribe_payload,
};
pub use app::{
    AgentSpec, AppCapabilities, AppError, AppHooks, AppManager, AppManifest, AppState,
    InstalledApp, ServiceSpec, ToolSource, ToolSpec,
};
#[cfg(feature = "ecc")]
pub use artifact_store::{ArtifactBackend, ArtifactStore, ArtifactType, StoredArtifact};
pub use assessment::{
    AnalysisContext, Analyzer, AnalyzerRegistry, AssessmentDiff, AssessmentReport,
    AssessmentService, AssessmentSummary, ComparisonReport, Finding,
    PeerInfo as AssessmentPeerInfo,
};
#[cfg(feature = "os-patterns")]
pub use auth_service::{
    AuditEntry, AuthService, AuthToken, CredentialGrant, CredentialRequest, CredentialType,
    HashedCredential, IssuedToken, StoredCredential as AuthStoredCredential,
};
pub use boot::{Kernel, KernelState};
#[cfg(feature = "ecc")]
pub use calibration::{EccCalibration, EccCalibrationConfig};
pub use capability::{
    AgentCapabilities, CapabilityChecker, CapabilityElevationRequest, ElevationResult, IpcScope,
    ResourceLimits, ResourceType, SandboxPolicy, ToolPermissions,
};
#[cfg(feature = "ecc")]
pub use causal::{
    CausalEdge, CausalEdgeType, CausalGraph, CausalNode, ChangeEvent, ChangePrediction,
    CouplingPair, SpectralResult,
};
#[cfg(feature = "ecc")]
pub use causal_predict::{
    CausalCollapseModel, CausalRankRequest, CausalRankResponse, CoherenceTracker, CollapseFeatures,
    ConversationState, EvidenceRanking, detect_conversation_cycle, predict_delta_lambda2,
    rank_evidence_by_impact,
};
#[cfg(feature = "exochain")]
pub use chain::{
    AnchorReceipt, ChainAnchor, ChainCheckpoint, ChainEvent, ChainLoggable, ChainManager,
    ChainStatus, ChainVerifyResult, CustodyAttestation, GovernanceDecisionEvent,
    IpcDeadLetterEvent, MockAnchor, RestartEvent,
};
pub use clawft_types::config::{
    ClusterNetworkConfig, KernelConfig, PairingConfig, ProfilesConfig,
    VectorBackendKind as VectorBackendKindConfig, VectorConfig, VectorDiskAnnConfig,
    VectorEvictionPolicy, VectorHnswConfig, VectorHybridConfig,
};
#[cfg(feature = "cluster")]
pub use cluster::ClusterService;
#[cfg(feature = "ecc")]
pub use cluster::NodeEccCapability;
#[cfg(any(feature = "mesh", feature = "exochain"))]
pub use cluster::NodeIdentity;
pub use cluster::{
    ClusterConfig, ClusterError, ClusterMembership, NodeId, NodePlatform, NodeState, PairedHost,
    PairedHostsFile, PairingGate, PairingWindowResult, PeerNode,
};
#[cfg(feature = "ecc")]
pub use cognitive_tick::{CognitiveTick, CognitiveTickConfig, CognitiveTickStats};
pub use config::KernelConfigExt;
#[cfg(feature = "os-patterns")]
pub use config_service::{ConfigChange, ConfigEntry, ConfigService, ConfigValue, SecretRef};
pub use console::{BootEvent, BootLog, BootPhase, KernelEventLog, LogLevel};
pub use container::{
    ContainerConfig, ContainerError, ContainerManager, ContainerService, ContainerState,
    ManagedContainer, PortMapping, RestartPolicy, VolumeMount,
};
pub use cron::{CronError, CronService};
#[cfg(feature = "ecc")]
pub use crossref::{CrossRef, CrossRefStore, CrossRefType, StructureTag, UniversalNodeId};
#[cfg(feature = "ecc")]
pub use democritus::{DemocritusConfig, DemocritusLoop, DemocritusTickResult};
#[cfg(feature = "ecc")]
pub use embedding::{
    EmbeddingError, EmbeddingProvider, LlmEmbeddingConfig, LlmEmbeddingProvider,
    MockEmbeddingProvider, select_embedding_provider,
};
#[cfg(feature = "ecc")]
pub use embedding_onnx::{
    AstEmbeddingProvider, OnnxEmbeddingProvider, RustCodeFeatures, SentenceTransformerProvider,
    extract_rust_features, preprocess_markdown, split_sentences,
};
pub use environment::{
    AuditLevel, Environment, EnvironmentClass, EnvironmentError, EnvironmentManager,
    GovernanceBranches, GovernanceScope, LearningMode,
};
pub use error::{KernelError, KernelResult};
#[cfg(feature = "exochain")]
pub use gate::{CapabilityGate, GateBackend, GateDecision, GovernanceGate};
pub use governance::{
    EffectVector, GovernanceBranch, GovernanceDecision, GovernanceEngine, GovernanceRequest,
    GovernanceResult, GovernanceRule, RuleSeverity,
};
pub use health::{HealthStatus, HealthSystem, OverallHealth};
#[cfg(feature = "os-patterns")]
pub use health::{ProbeConfig, ProbeResult, ProbeState};
#[cfg(feature = "ecc")]
pub use hnsw_eml::{
    ArmMetrics, DistanceTrainingPoint, EfPrediction, EfStrategy, EfTrainingPoint, HnswEmlBenchmark,
    HnswEmlConfig, HnswEmlManager, HnswEmlStatus, HnswScalingPoint, PathTrainingPoint, ProbeReport,
    RebuildPrediction, RebuildTrainingPoint, SearchStrategy, SpectrumForm, TriageRecord,
    probe_corpus, run_hnsw_benchmark, triage_strategy,
};
#[cfg(feature = "ecc")]
pub use hnsw_service::{
    HnswSearchResult, HnswService, HnswServiceConfig, MultiKey, MultiKeyConfig, entity_search_keys,
};
#[cfg(feature = "ecc")]
pub use impulse::{ImpulseQueue, ImpulseType};
pub use ipc::{
    ExitReason as SignalExitReason, GlobalPid, KernelIpc, KernelMessage, KernelSignal,
    MessagePayload, MessageTarget, ProcessDown as SignalProcessDown,
};
#[cfg(feature = "mesh")]
pub use mesh::{
    MAX_MESSAGE_SIZE, MeshError, MeshPeer, MeshStream, MeshTransport, TransportListener,
    WeftHandshake,
};
#[cfg(feature = "mesh")]
pub use mesh_artifact::{
    ArtifactAnnouncement, ArtifactExchange, ArtifactRequest, ArtifactResponse,
};
#[cfg(feature = "mesh")]
pub use mesh_assess::{AssessmentEnvelope, AssessmentTransport};
#[cfg(feature = "mesh")]
pub use mesh_bootstrap::{BootstrapDiscovery, PeerExchangeDiscovery};
#[cfg(feature = "mesh")]
pub use mesh_chain::{
    ChainBridgeEvent, ChainForkStatus, ChainSyncRequest, ChainSyncResponse, SyncStateDigest,
};
#[cfg(feature = "mesh")]
pub use mesh_dedup::DedupFilter;
#[cfg(feature = "mesh")]
pub use mesh_discovery::{
    DiscoveredPeer, DiscoveryBackend, DiscoveryCoordinator, DiscoveryError, DiscoverySource,
};
#[cfg(feature = "mesh")]
pub use mesh_framing::{FrameType, MeshFrame};
#[cfg(feature = "mesh")]
pub use mesh_heartbeat::{
    HeartbeatConfig, HeartbeatState, HeartbeatTracker, PeerHeartbeat, PingRequest, PingResponse,
};
#[cfg(feature = "mesh")]
pub use mesh_ipc::{MeshIpcEnvelope, MeshIpcError};
#[cfg(feature = "mesh")]
pub use mesh_kad::{
    ALPHA, DhtEntry, DhtKey, K_BUCKET_SIZE, KEY_BITS, KademliaDiscovery, KademliaTable,
    NamespacedDhtKey, bucket_index, leading_zeros, xor_distance,
};
#[cfg(feature = "mesh")]
pub use mesh_listener::{JoinRequest, JoinResponse, MeshConnectionPool, PeerInfo};
#[cfg(feature = "mesh")]
pub use mesh_log::{LogAggregator, LogQuery as MeshLogQuery, RemoteLogEntry};
#[cfg(feature = "mesh")]
pub use mesh_mdns::{MdnsAnnouncement, MdnsDiscovery, WEFTOS_SERVICE_NAME};
#[cfg(feature = "mesh")]
pub use mesh_noise::{EncryptedChannel, NoiseConfig, NoisePattern};
#[cfg(feature = "mesh")]
pub use mesh_process::{
    ConsensusEntry, ConsensusOp, ConsensusRole, ConsistentHashRing, CrdtGossipState,
    DistributedProcessTable, MetadataConsensus, ProcessAdvertisement, ProcessStatus,
    ResourceSummary,
};
#[cfg(feature = "mesh")]
pub use mesh_runtime::{DiscoveryState, MeshRuntime, PeerConnection};
#[cfg(feature = "mesh")]
pub use mesh_service::{
    RemoteServiceEndpoint, ServiceResolutionCache, ServiceResolveRequest, ServiceResolveResponse,
};
#[cfg(feature = "mesh")]
pub use mesh_service_adv::{ClusterServiceRegistry, ServiceAdvertisement};
#[cfg(feature = "mesh")]
pub use mesh_tcp::TcpTransport;
#[cfg(feature = "mesh")]
pub use mesh_tree::{
    MerkleProof, TreeDiffType, TreeNodeDiff, TreeSyncAction, TreeSyncRequest, TreeSyncResponse,
};
#[cfg(feature = "mesh")]
pub use mesh_ws::WsTransport;
#[cfg(feature = "os-patterns")]
pub use monitor::{ExitReason, MonitorRegistry, ProcessDown, ProcessLink, ProcessMonitor};
#[cfg(feature = "native")]
pub use node_registry::{
    DerivedGrantError, DerivedWriteGrant, GrantScope, MESH_CANONICAL_PREFIX, NodeRegistry,
    RegisteredNode, node_id_from_pubkey, node_publish_payload, path_belongs_to,
    required_path_prefix,
};
#[cfg(feature = "ecc")]
pub use persistence::PersistenceConfig;
pub use process::{Pid, ProcessEntry, ProcessState, ProcessTable, ResourceUsage};
#[cfg(feature = "ecc")]
pub use profile_store::{ProfileEntry, ProfileError, ProfileMeta, ProfileStore};
#[cfg(feature = "ecc")]
pub use quantum_backend::{
    BackendStatus, EvolutionParams, JobHandle, JobStatus, QuantumBackend, QuantumError,
    QuantumResults,
};
#[cfg(all(feature = "ecc", feature = "quantum-braket"))]
pub use quantum_braket::{BraketBackend, BraketConfig, BraketDevice};
#[cfg(all(feature = "ecc", feature = "quantum-pasqal"))]
pub use quantum_pasqal::{PasqalBackend, PasqalConfig, PasqalDevice};
#[cfg(feature = "ecc")]
pub use quantum_register::{RegisterConstraints, build_register};
#[cfg(feature = "ecc")]
pub use quantum_state::{
    Complex, Hypothesis, HypothesisSuperposition, QuantumCognitiveState, QuantumEvidenceRanking,
};
#[cfg(feature = "os-patterns")]
pub use reconciler::{DesiredAgentState, DriftEvent, ReconciliationController};
pub use revocation::{RevocationList, RevokedHost};
pub use service::{
    KernelServiceApi, McpAdapter, ServiceApi, ServiceAuditLevel, ServiceContract, ServiceEndpoint,
    ServiceEntry, ServiceInfo, ServiceRegistry, ServiceType, ShellAdapter, SystemService,
};
#[cfg(all(feature = "native", feature = "exochain"))]
pub use stream_anchor::{StreamWindowAnchor, TopicAnchor, topic_matches};
#[cfg(feature = "native")]
pub use substrate_service::{
    EgressDenied, GateDenied, Sensitivity as SubstrateSensitivity, SubstrateListEntry,
    SubstrateListSnapshot, SubstrateReadSnapshot, SubstrateService,
};
pub use supervisor::{AgentSupervisor, EnclaveConfig, SpawnBackend, SpawnRequest, SpawnResult};
#[cfg(feature = "os-patterns")]
pub use supervisor::{
    ResourceCheckResult, RestartBudget, RestartStrategy, RestartTracker, check_resource_usage,
};
#[cfg(feature = "exochain")]
pub use tree_manager::{TreeManager, TreeStats};
pub use tree_view::{AgentTreeView, TreeScope};
#[cfg(feature = "ecc")]
pub use vector_backend::{
    SearchResult as VectorSearchResult, VectorBackend, VectorBackendKind, VectorError, VectorResult,
};
#[cfg(feature = "ecc")]
pub use vector_diskann::{DiskAnnBackend, DiskAnnConfig};
#[cfg(feature = "ecc")]
pub use vector_hnsw::HnswBackend;
#[cfg(feature = "ecc")]
pub use vector_hybrid::{EvictionPolicy, HybridBackend, HybridConfig};
#[cfg(feature = "ecc")]
pub use weaver::{
    ConfidenceGap, ConfidenceReport, DataSource, ExportedModel, IngestResult, MetaDecisionType,
    MetaLoomEvent, ModelingSession, ModelingSuggestion, StrategyPattern, TickResult, WeaverCommand,
    WeaverEngine, WeaverError, WeaverKnowledgeBase, WeaverResponse,
};
// ── 08b re-exports ──────────────────────────────────────────────
#[cfg(feature = "os-patterns")]
pub use dead_letter::{DeadLetter, DeadLetterQueue, DeadLetterReason};
#[cfg(feature = "os-patterns")]
pub use log_service::{LogEntry, LogQuery, LogService};
#[cfg(feature = "os-patterns")]
pub use metrics::{
    Histogram, METRIC_ACTIVE_AGENTS, METRIC_ACTIVE_SERVICES, METRIC_AGENT_CRASHES,
    METRIC_AGENT_SPAWNS, METRIC_CHAIN_LENGTH, METRIC_GOVERNANCE_EVAL_MS, METRIC_IPC_LATENCY_MS,
    METRIC_MESSAGES_DELIVERED, METRIC_MESSAGES_DROPPED, METRIC_MESSAGES_SENT,
    METRIC_TOOL_EXECUTION_MS, METRIC_TOOL_EXECUTIONS, MetricSnapshot, MetricsRegistry,
};
#[cfg(feature = "os-patterns")]
pub use named_pipe::{NamedPipe, NamedPipeRegistry, PipeInfo};
#[cfg(feature = "os-patterns")]
pub use reliable_queue::{DeliveryResult, PendingDelivery, ReliableConfig, ReliableQueue};
#[cfg(feature = "os-patterns")]
pub use timer::{TimerEntry, TimerInfo, TimerService};
pub use topic::{SubscriberId, SubscriberSink, Subscription, TopicRouter};
#[cfg(feature = "native")]
pub use wasm_runner::AgentSendTool;
pub use wasm_runner::{
    AgentInspectTool, AgentListTool, AgentResumeTool, AgentSpawnTool, AgentStopTool,
    AgentSuspendTool, BackendSelection, BuiltinTool, BuiltinToolSpec, Certificate,
    CompiledModuleCache, DeployedTool, FsCopyTool, FsCreateDirTool, FsExistsTool, FsGlobTool,
    FsMoveTool, FsReadDirTool, FsReadFileTool, FsRemoveTool, FsStatTool, FsWriteFileTool,
    IpcSendTool, IpcSubscribeTool, SandboxConfig, ShellPipeline, SysCronAddTool, SysCronListTool,
    SysCronRemoveTool, SysEnvGetTool, SysServiceHealthTool, SysServiceListTool, ToolCategory,
    ToolError, ToolRegistry, ToolSigningAuthority, ToolVersion, WasiFsScope, WasmError,
    WasmSandboxConfig, WasmTool, WasmToolResult, WasmToolRunner, WasmValidation,
    builtin_tool_catalog, compute_module_hash, verify_tool_signature,
};
#[cfg(feature = "exochain")]
pub use wasm_runner::{SysChainQueryTool, SysChainStatusTool, SysTreeInspectTool, SysTreeReadTool};
