//! Kernel configuration types.
//!
//! These types are defined in `clawft-types` so they can be embedded
//! in the root [`Config`](super::Config) without creating a circular
//! dependency with `clawft-kernel`.

use serde::{Deserialize, Serialize};

/// Default maximum number of concurrent processes.
fn default_max_processes() -> u32 {
    64
}

/// Default health check interval in seconds.
fn default_health_check_interval_secs() -> u64 {
    30
}

/// Kernel is enabled by default.
fn default_enabled() -> bool {
    true
}

/// Cluster networking configuration for distributed WeftOS nodes.
///
/// Controls the ruvector-powered clustering layer that coordinates
/// native nodes. Browser/edge nodes join via WebSocket to a
/// coordinator and do not need this configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterNetworkConfig {
    /// Number of replica copies for each shard (default: 3).
    #[serde(default = "default_replication_factor", alias = "replicationFactor")]
    pub replication_factor: usize,

    /// Total number of shards in the cluster (default: 64).
    #[serde(default = "default_shard_count", alias = "shardCount")]
    pub shard_count: u32,

    /// Interval between heartbeat checks in seconds (default: 5).
    #[serde(
        default = "default_cluster_heartbeat",
        alias = "heartbeatIntervalSecs"
    )]
    pub heartbeat_interval_secs: u64,

    /// Timeout before marking a node offline in seconds (default: 30).
    #[serde(default = "default_node_timeout", alias = "nodeTimeoutSecs")]
    pub node_timeout_secs: u64,

    /// Whether to enable DAG-based consensus (default: true).
    #[serde(default = "default_enable_consensus", alias = "enableConsensus")]
    pub enable_consensus: bool,

    /// Minimum nodes required for quorum (default: 2).
    #[serde(default = "default_min_quorum", alias = "minQuorumSize")]
    pub min_quorum_size: usize,

    /// Seed node addresses for discovery (coordinator addresses).
    #[serde(default, alias = "seedNodes")]
    pub seed_nodes: Vec<String>,

    /// Human-readable display name for this node.
    #[serde(default, alias = "nodeName")]
    pub node_name: Option<String>,
}

fn default_replication_factor() -> usize {
    3
}
fn default_shard_count() -> u32 {
    64
}
fn default_cluster_heartbeat() -> u64 {
    5
}
fn default_node_timeout() -> u64 {
    30
}
fn default_enable_consensus() -> bool {
    true
}
fn default_min_quorum() -> usize {
    2
}

impl Default for ClusterNetworkConfig {
    fn default() -> Self {
        Self {
            replication_factor: default_replication_factor(),
            shard_count: default_shard_count(),
            heartbeat_interval_secs: default_cluster_heartbeat(),
            node_timeout_secs: default_node_timeout(),
            enable_consensus: default_enable_consensus(),
            min_quorum_size: default_min_quorum(),
            seed_nodes: Vec::new(),
            node_name: None,
        }
    }
}

/// Kernel subsystem configuration.
///
/// Embedded in the root `Config` under the `kernel` key. All fields
/// have sensible defaults so that existing configuration files parse
/// without errors.
///
/// # Example JSON
///
/// ```json
/// {
///   "kernel": {
///     "enabled": false,
///     "max_processes": 128,
///     "health_check_interval_secs": 15
///   }
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KernelConfig {
    /// Whether the kernel subsystem is enabled.
    ///
    /// When `false`, kernel subsystems do not activate unless explicitly
    /// invoked via `weave kernel` CLI commands. Defaults to `true`.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Maximum number of concurrent processes in the process table.
    #[serde(default = "default_max_processes", alias = "maxProcesses")]
    pub max_processes: u32,

    /// Interval (in seconds) between periodic health checks.
    #[serde(
        default = "default_health_check_interval_secs",
        alias = "healthCheckIntervalSecs"
    )]
    pub health_check_interval_secs: u64,

    /// Cluster networking configuration (native coordinator nodes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster: Option<ClusterNetworkConfig>,

    /// Local chain configuration (exochain feature).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain: Option<ChainConfig>,

    /// Resource tree configuration (exochain feature).
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "resourceTree")]
    pub resource_tree: Option<ResourceTreeConfig>,

    /// Vector search backend configuration (ECC feature).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vector: Option<VectorConfig>,

    /// Per-user profile namespace configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profiles: Option<ProfilesConfig>,

    /// Time-windowed pairing configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pairing: Option<PairingConfig>,

    /// Mesh networking configuration (K6 transport layer).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mesh: Option<MeshConfig>,

    /// Stream-window chain anchor configuration.
    ///
    /// When enabled, the kernel subscribes to every topic matching
    /// one of the configured prefixes/globs and chain-appends a
    /// `stream.window_commit` event every `window_secs` summarising
    /// the window: BLAKE3 of concatenated message bytes, message
    /// count, byte count, first+last tick, and owning agent_id (when
    /// known). This gives verifiers a tamper-evident anchor without
    /// putting raw frames on-chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<AnchorConfig>,

    /// Optional TCP relay for the daemon's JSON-RPC socket.
    ///
    /// When enabled, the daemon also listens on a TCP port and
    /// transparently forwards every accepted connection to the local
    /// unix socket via in-process byte-copy. Clients speak the exact
    /// same line-delimited JSON-RPC protocol. All auth/policy stays
    /// in the unix-socket handler path — the TCP side is a byte
    /// conduit only. Intended for cross-boundary callers (Windows
    /// side of WSL, remote bridges) that cannot open `AF_UNIX`.
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "ipcTcp")]
    pub ipc_tcp: Option<IpcTcpConfig>,
}

impl Default for KernelConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_processes: default_max_processes(),
            health_check_interval_secs: default_health_check_interval_secs(),
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
}

// ── IPC TCP relay configuration ─────────────────────────────────────────

/// Configuration for the optional TCP relay in front of the daemon's
/// unix-socket JSON-RPC.
///
/// Paired with [`crate::config::KernelConfig::ipc_tcp`]. When enabled,
/// the daemon binds `listen_addr` and forwards each accepted TCP
/// connection to the local unix socket via in-process byte-copy. No
/// protocol translation: clients speak the same line-delimited
/// JSON-RPC as unix-socket clients.
///
/// # Security (WEFT-481)
///
/// - `listen_addr` defaults to loopback (`127.0.0.1:9471`). Setting it
///   to `0.0.0.0` exposes the daemon RPC on every interface; the
///   daemon refuses to bind a non-loopback address unless `bearer`
///   is set, so anonymous broadcast can never happen by accident.
/// - `bearer`, when set, gates every TCP connection. The client must
///   send a `Bearer: <token>\n` line as the first line on the wire
///   before any JSON-RPC request. Mismatch closes the connection.
/// - Connections from non-loopback peers are dropped immediately
///   when `bearer` is unset, regardless of `listen_addr`.
///
/// # Example TOML
///
/// ```toml
/// [kernel.ipc_tcp]
/// enabled = true
/// listen_addr = "127.0.0.1:9471"
/// bearer = "deadbeef..."     # required when listen_addr is non-loopback
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcTcpConfig {
    /// Master switch. Default: false.
    #[serde(default)]
    pub enabled: bool,

    /// Address to bind. Loopback-only by default so cross-boundary
    /// callers must explicitly opt into a broader interface.
    #[serde(default = "default_ipc_tcp_listen_addr", alias = "listenAddr")]
    pub listen_addr: String,

    /// Optional shared bearer token. When set, every TCP connection
    /// must send `Bearer: <token>\n` as the first wire line before
    /// any JSON-RPC request, or the connection is closed. Required
    /// when `listen_addr` is non-loopback. WEFT-481.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearer: Option<String>,
}

impl Default for IpcTcpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            listen_addr: default_ipc_tcp_listen_addr(),
            bearer: None,
        }
    }
}

fn default_ipc_tcp_listen_addr() -> String {
    "127.0.0.1:9471".to_string()
}

impl IpcTcpConfig {
    /// Returns true when `listen_addr` parses to a non-loopback
    /// address (any-interface or routable). WEFT-481 uses this to
    /// refuse to bind without a bearer token.
    pub fn is_non_loopback(&self) -> bool {
        use std::net::SocketAddr;
        match self.listen_addr.parse::<SocketAddr>() {
            Ok(addr) => !addr.ip().is_loopback(),
            // If we can't parse it, treat as non-loopback so the
            // daemon refuses to bind rather than guessing.
            Err(_) => true,
        }
    }
}

// ── Stream-window anchor configuration ──────────────────────────────────

/// Configuration for the kernel's stream-window chain anchor.
///
/// Paired with [`crate::config::KernelConfig::anchor`]. When enabled,
/// every window_secs seconds the anchor emits a `stream.window_commit`
/// chain event summarising all traffic on topics matching one of
/// `topics`.
///
/// # Example TOML
///
/// ```toml
/// [kernel.anchor]
/// enabled = true
/// topics = ["sensor.*"]
/// window_secs = 2
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnchorConfig {
    /// Master switch. Default: false.
    #[serde(default)]
    pub enabled: bool,

    /// Topic patterns to anchor. Each entry is either an exact topic
    /// name or a single-segment wildcard like `"sensor.*"` which
    /// matches any topic sharing the literal `"sensor."` prefix.
    #[serde(default)]
    pub topics: Vec<String>,

    /// Rolling window duration in seconds. Default: 2.
    #[serde(default = "default_anchor_window_secs")]
    pub window_secs: u64,
}

fn default_anchor_window_secs() -> u64 {
    2
}

impl Default for AnchorConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            topics: Vec::new(),
            window_secs: default_anchor_window_secs(),
        }
    }
}

// ── Mesh networking configuration ──────────────────────────────────────

/// Configuration for the K6 mesh transport layer.
///
/// Controls whether the mesh listener is started, what transport to use,
/// and where to bind. When enabled, the kernel spawns a `MeshRuntime`
/// that accepts peer connections and wires them into the A2A router.
///
/// # Example TOML
///
/// ```toml
/// [kernel.mesh]
/// enabled = true
/// transport = "tcp"
/// listen_addr = "0.0.0.0:9470"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshConfig {
    /// Whether the mesh transport is active. Default: false.
    #[serde(default)]
    pub enabled: bool,

    /// Transport backend: "tcp" (default) or "ws" (WebSocket).
    /// QUIC planned for future.
    #[serde(default = "default_mesh_transport")]
    pub transport: String,

    /// Address to bind the mesh listener on.
    #[serde(default = "default_mesh_listen_addr")]
    pub listen_addr: String,

    /// Enable peer discovery via Kademlia DHT.
    #[serde(default)]
    pub discovery: bool,

    /// Seed peers to connect to on startup.
    #[serde(default)]
    pub seed_peers: Vec<String>,

    /// Enable Noise Protocol encryption on mesh connections.
    /// When true, all peer connections use Noise XX handshake
    /// (Noise_XX_25519_ChaChaPoly_SHA256). Default: false.
    #[serde(default)]
    pub noise: bool,

    /// Path to Ed25519 private key for Noise handshake.
    /// If absent, a ephemeral key is generated at boot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub noise_key_path: Option<String>,
}

fn default_mesh_transport() -> String {
    "tcp".to_owned()
}

fn default_mesh_listen_addr() -> String {
    "0.0.0.0:9470".to_owned()
}

impl Default for MeshConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            transport: default_mesh_transport(),
            listen_addr: default_mesh_listen_addr(),
            discovery: false,
            seed_peers: vec![],
            noise: false,
            noise_key_path: None,
        }
    }
}

// ── Profile namespace configuration ─────────────────────────────────────

/// Per-user profile namespace configuration.
///
/// When enabled, each profile gets its own isolated vector storage
/// directory under `storage_path`.
///
/// # Example TOML
///
/// ```toml
/// [kernel.profiles]
/// enabled = true
/// storage_path = ".weftos/profiles"
/// default_profile = "default"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfilesConfig {
    /// Whether profile namespaces are enabled.
    #[serde(default = "default_profiles_enabled")]
    pub enabled: bool,

    /// Base directory for profile data.
    #[serde(default = "default_profiles_storage_path")]
    pub storage_path: String,

    /// Default profile to activate on boot.
    #[serde(default = "default_profile_name")]
    pub default_profile: String,
}

fn default_profiles_enabled() -> bool {
    true
}

fn default_profiles_storage_path() -> String {
    ".weftos/profiles".to_owned()
}

fn default_profile_name() -> String {
    "default".to_owned()
}

impl Default for ProfilesConfig {
    fn default() -> Self {
        Self {
            enabled: default_profiles_enabled(),
            storage_path: default_profiles_storage_path(),
            default_profile: default_profile_name(),
        }
    }
}

// ── Time-windowed pairing configuration ─────────────────────────────────

/// Configuration for time-windowed mesh pairing.
///
/// Controls where paired host data is persisted and the default
/// enrollment window duration.
///
/// # Example TOML
///
/// ```toml
/// [kernel.pairing]
/// persist_path = ".weftos/runtime/paired_hosts.json"
/// default_window_secs = 30
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingConfig {
    /// Path to the paired hosts persistence file.
    #[serde(default = "default_pairing_persist_path")]
    pub persist_path: String,

    /// Default enrollment window duration in seconds.
    #[serde(default = "default_pairing_window_secs")]
    pub default_window_secs: u64,
}

fn default_pairing_persist_path() -> String {
    ".weftos/runtime/paired_hosts.json".to_owned()
}

fn default_pairing_window_secs() -> u64 {
    30
}

impl Default for PairingConfig {
    fn default() -> Self {
        Self {
            persist_path: default_pairing_persist_path(),
            default_window_secs: default_pairing_window_secs(),
        }
    }
}

/// Local chain configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainConfig {
    /// Whether the local chain is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Maximum events before auto-checkpoint.
    #[serde(default = "default_checkpoint_interval", alias = "checkpointInterval")]
    pub checkpoint_interval: u64,

    /// Chain ID (0 = local node chain).
    #[serde(default)]
    pub chain_id: u32,

    /// Path to the chain checkpoint file for persistence across restarts.
    /// If `None`, defaults to `~/.clawft/chain/local.json`.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "checkpointPath"
    )]
    pub checkpoint_path: Option<String>,
}

fn default_true() -> bool {
    true
}
fn default_checkpoint_interval() -> u64 {
    1000
}

impl Default for ChainConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            checkpoint_interval: default_checkpoint_interval(),
            chain_id: 0,
            checkpoint_path: None,
        }
    }
}

impl ChainConfig {
    /// Returns the effective checkpoint path.
    ///
    /// If `checkpoint_path` is set, returns it. Otherwise falls back to
    /// `~/.clawft/chain.json` (requires the `native` feature for `dirs`).
    pub fn effective_checkpoint_path(&self) -> Option<String> {
        if self.checkpoint_path.is_some() {
            return self.checkpoint_path.clone();
        }
        #[cfg(feature = "native")]
        {
            dirs::home_dir().map(|h| {
                h.join(".clawft")
                    .join("chain.json")
                    .to_string_lossy()
                    .into_owned()
            })
        }
        #[cfg(not(feature = "native"))]
        {
            None
        }
    }
}

/// Resource tree configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceTreeConfig {
    /// Whether the resource tree is enabled.
    #[serde(default = "default_true_rt")]
    pub enabled: bool,

    /// Path to checkpoint file (None = in-memory only).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "checkpointPath"
    )]
    pub checkpoint_path: Option<String>,
}

fn default_true_rt() -> bool {
    true
}

impl Default for ResourceTreeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            checkpoint_path: None,
        }
    }
}

// ── Vector search backend configuration ──────────────────────────────────

/// Which vector search backend to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum VectorBackendKind {
    /// In-memory HNSW (default, fast, suitable for <1M vectors).
    #[default]
    Hnsw,
    /// SSD-backed DiskANN (large scale, 1M+ vectors).
    DiskAnn,
    /// Hot HNSW cache + cold DiskANN store.
    Hybrid,
}

/// HNSW-specific vector configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorHnswConfig {
    /// ef_construction parameter for index building.
    #[serde(default = "default_ef_construction")]
    pub ef_construction: usize,

    /// Number of bi-directional links per node (M parameter).
    #[serde(default = "default_m")]
    pub m: usize,

    /// Maximum number of elements the index can hold.
    #[serde(default = "default_max_elements")]
    pub max_elements: usize,
}

fn default_ef_construction() -> usize {
    200
}
fn default_m() -> usize {
    16
}
fn default_max_elements() -> usize {
    100_000
}

impl Default for VectorHnswConfig {
    fn default() -> Self {
        Self {
            ef_construction: default_ef_construction(),
            m: default_m(),
            max_elements: default_max_elements(),
        }
    }
}

/// DiskANN-specific vector configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorDiskAnnConfig {
    /// Maximum number of points the index can hold.
    #[serde(default = "default_diskann_max_points")]
    pub max_points: usize,

    /// Vector dimensionality.
    #[serde(default = "default_diskann_dimensions")]
    pub dimensions: usize,

    /// Number of neighbors per node in the DiskANN graph.
    #[serde(default = "default_diskann_num_neighbors")]
    pub num_neighbors: usize,

    /// Size of the search candidate list.
    #[serde(default = "default_diskann_search_list_size")]
    pub search_list_size: usize,

    /// Directory path for SSD-backed data files.
    #[serde(default = "default_diskann_data_path")]
    pub data_path: String,

    /// Whether to use product quantization for compression.
    #[serde(default = "default_diskann_use_pq")]
    pub use_pq: bool,

    /// Number of PQ sub-quantizer chunks.
    #[serde(default = "default_diskann_pq_num_chunks")]
    pub pq_num_chunks: usize,
}

fn default_diskann_max_points() -> usize {
    10_000_000
}
fn default_diskann_dimensions() -> usize {
    384
}
fn default_diskann_num_neighbors() -> usize {
    64
}
fn default_diskann_search_list_size() -> usize {
    100
}
fn default_diskann_data_path() -> String {
    ".weftos/diskann".to_owned()
}
fn default_diskann_use_pq() -> bool {
    true
}
fn default_diskann_pq_num_chunks() -> usize {
    48
}

impl Default for VectorDiskAnnConfig {
    fn default() -> Self {
        Self {
            max_points: default_diskann_max_points(),
            dimensions: default_diskann_dimensions(),
            num_neighbors: default_diskann_num_neighbors(),
            search_list_size: default_diskann_search_list_size(),
            data_path: default_diskann_data_path(),
            use_pq: default_diskann_use_pq(),
            pq_num_chunks: default_diskann_pq_num_chunks(),
        }
    }
}

/// Eviction policy for the hybrid backend's hot tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum VectorEvictionPolicy {
    /// Least Recently Used.
    #[default]
    Lru,
}

/// Hybrid backend-specific configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorHybridConfig {
    /// Maximum number of vectors in the hot (HNSW) tier.
    #[serde(default = "default_hybrid_hot_capacity")]
    pub hot_capacity: usize,

    /// Access count threshold before a cold vector is promoted to hot.
    #[serde(default = "default_hybrid_promotion_threshold")]
    pub promotion_threshold: u32,

    /// Eviction policy when the hot tier is full.
    #[serde(default)]
    pub eviction_policy: VectorEvictionPolicy,
}

fn default_hybrid_hot_capacity() -> usize {
    50_000
}
fn default_hybrid_promotion_threshold() -> u32 {
    3
}

impl Default for VectorHybridConfig {
    fn default() -> Self {
        Self {
            hot_capacity: default_hybrid_hot_capacity(),
            promotion_threshold: default_hybrid_promotion_threshold(),
            eviction_policy: VectorEvictionPolicy::default(),
        }
    }
}

/// Unified vector search backend configuration.
///
/// Controls which backend is used for the ECC cognitive substrate's
/// vector search layer.
///
/// # Example TOML
///
/// ```toml
/// [kernel.vector]
/// backend = "hybrid"
///
/// [kernel.vector.hnsw]
/// ef_construction = 200
/// max_elements = 100000
///
/// [kernel.vector.diskann]
/// max_points = 10000000
/// data_path = ".weftos/diskann"
///
/// [kernel.vector.hybrid]
/// hot_capacity = 50000
/// promotion_threshold = 3
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VectorConfig {
    /// Which backend to use.
    #[serde(default)]
    pub backend: VectorBackendKind,

    /// HNSW-specific settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hnsw: Option<VectorHnswConfig>,

    /// DiskANN-specific settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diskann: Option<VectorDiskAnnConfig>,

    /// Hybrid-specific settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hybrid: Option<VectorHybridConfig>,

    /// Logarithmic quantization settings (KG-011).
    ///
    /// Requires `ruvector-core` with PR #352 merged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_quantized: Option<LogQuantizedStubConfig>,

    /// Unified SIMD distance kernel settings (KG-012).
    ///
    /// Requires `ruvector-core` with PR #352 merged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub simd_distance: Option<SimdDistanceStubConfig>,
}

/// Serializable stub for LogQuantized configuration (KG-011).
///
/// Full implementation lives in `clawft-kernel::vector_quantization`.
/// This type mirrors the essential fields for config-file deserialization
/// in `clawft-types` (which cannot depend on `clawft-kernel`).
///
/// Requires `ruvector-core` with PR #352 merged.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogQuantizedStubConfig {
    /// Whether logarithmic quantization is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Compression ratio (default: 4).
    #[serde(default = "default_log_quantized_compression_ratio")]
    pub compression_ratio: usize,
}

fn default_log_quantized_compression_ratio() -> usize {
    4
}

impl Default for LogQuantizedStubConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            compression_ratio: default_log_quantized_compression_ratio(),
        }
    }
}

/// Serializable stub for SIMD distance configuration (KG-012).
///
/// Full implementation lives in `clawft-kernel::vector_quantization`.
///
/// Requires `ruvector-core` with PR #352 merged.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SimdDistanceStubConfig {
    /// Whether the unified SIMD distance kernel is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Whether to pad vectors to power-of-two length for alignment.
    /// See shaal's v4 caveat about memory overhead.
    #[serde(default)]
    pub pad_to_power_of_two: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_kernel_config() {
        let cfg = KernelConfig::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.max_processes, 64);
        assert_eq!(cfg.health_check_interval_secs, 30);
    }

    #[test]
    fn deserialize_empty() {
        let cfg: KernelConfig = serde_json::from_str("{}").unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.max_processes, 64);
    }

    #[test]
    fn deserialize_camel_case() {
        let json = r#"{"maxProcesses": 128, "healthCheckIntervalSecs": 15}"#;
        let cfg: KernelConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.max_processes, 128);
        assert_eq!(cfg.health_check_interval_secs, 15);
    }

    #[test]
    fn serde_roundtrip() {
        let cfg = KernelConfig {
            enabled: true,
            max_processes: 256,
            health_check_interval_secs: 10,
            cluster: None,
            chain: None,
            resource_tree: None,
            vector: None,
            profiles: None,
            pairing: None,
            mesh: None,
            anchor: None,
            ipc_tcp: None,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let restored: KernelConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.enabled, cfg.enabled);
        assert_eq!(restored.max_processes, cfg.max_processes);
    }

    #[test]
    fn profiles_config_defaults() {
        let cfg = ProfilesConfig::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.storage_path, ".weftos/profiles");
        assert_eq!(cfg.default_profile, "default");
    }

    #[test]
    fn profiles_config_deserialize() {
        let json = r#"{"enabled": false, "storage_path": "/tmp/profiles", "default_profile": "admin"}"#;
        let cfg: ProfilesConfig = serde_json::from_str(json).unwrap();
        assert!(!cfg.enabled);
        assert_eq!(cfg.storage_path, "/tmp/profiles");
        assert_eq!(cfg.default_profile, "admin");
    }

    #[test]
    fn pairing_config_defaults() {
        let cfg = PairingConfig::default();
        assert_eq!(cfg.persist_path, ".weftos/runtime/paired_hosts.json");
        assert_eq!(cfg.default_window_secs, 30);
    }

    #[test]
    fn pairing_config_deserialize() {
        let json = r#"{"persist_path": "/opt/pairing.json", "default_window_secs": 60}"#;
        let cfg: PairingConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.persist_path, "/opt/pairing.json");
        assert_eq!(cfg.default_window_secs, 60);
    }

    #[test]
    fn kernel_config_with_profiles_and_pairing() {
        let json = r#"{"profiles": {"enabled": true}, "pairing": {"default_window_secs": 45}}"#;
        let cfg: KernelConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.profiles.is_some());
        assert!(cfg.profiles.unwrap().enabled);
        assert!(cfg.pairing.is_some());
        assert_eq!(cfg.pairing.unwrap().default_window_secs, 45);
    }

    #[test]
    fn vector_config_defaults() {
        let cfg = VectorConfig::default();
        assert_eq!(cfg.backend, VectorBackendKind::Hnsw);
        assert!(cfg.hnsw.is_none());
        assert!(cfg.diskann.is_none());
        assert!(cfg.hybrid.is_none());
        assert!(cfg.log_quantized.is_none());
        assert!(cfg.simd_distance.is_none());
    }

    #[test]
    fn vector_config_deserialize_hybrid() {
        let json = r#"{"backend": "hybrid", "hybrid": {"hot_capacity": 1000, "promotion_threshold": 5}}"#;
        let cfg: VectorConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.backend, VectorBackendKind::Hybrid);
        let h = cfg.hybrid.unwrap();
        assert_eq!(h.hot_capacity, 1000);
        assert_eq!(h.promotion_threshold, 5);
    }

    #[test]
    fn vector_config_deserialize_diskann() {
        let json = r#"{"backend": "diskann", "diskann": {"max_points": 5000000}}"#;
        let cfg: VectorConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.backend, VectorBackendKind::DiskAnn);
        let d = cfg.diskann.unwrap();
        assert_eq!(d.max_points, 5_000_000);
    }

    #[test]
    fn kernel_config_with_vector() {
        let json = r#"{"vector": {"backend": "hnsw"}}"#;
        let cfg: KernelConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.vector.is_some());
        assert_eq!(cfg.vector.unwrap().backend, VectorBackendKind::Hnsw);
    }

    #[test]
    fn log_quantized_stub_config_defaults() {
        let cfg = LogQuantizedStubConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.compression_ratio, 4);
    }

    #[test]
    fn log_quantized_stub_config_deserialize() {
        let json = r#"{"enabled": true, "compression_ratio": 8}"#;
        let cfg: LogQuantizedStubConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.compression_ratio, 8);
    }

    #[test]
    fn simd_distance_stub_config_defaults() {
        let cfg = SimdDistanceStubConfig::default();
        assert!(!cfg.enabled);
        assert!(!cfg.pad_to_power_of_two);
    }

    #[test]
    fn simd_distance_stub_config_deserialize() {
        let json = r#"{"enabled": true, "pad_to_power_of_two": true}"#;
        let cfg: SimdDistanceStubConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.enabled);
        assert!(cfg.pad_to_power_of_two);
    }

    #[test]
    fn vector_config_with_shaal_stubs() {
        let json = r#"{
            "backend": "hnsw",
            "log_quantized": {"enabled": true, "compression_ratio": 16},
            "simd_distance": {"enabled": true, "pad_to_power_of_two": true}
        }"#;
        let cfg: VectorConfig = serde_json::from_str(json).unwrap();
        let lq = cfg.log_quantized.unwrap();
        assert!(lq.enabled);
        assert_eq!(lq.compression_ratio, 16);
        let sd = cfg.simd_distance.unwrap();
        assert!(sd.enabled);
        assert!(sd.pad_to_power_of_two);
    }
}
