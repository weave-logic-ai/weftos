//! Agent supervisor for process lifecycle management.
//!
//! The [`AgentSupervisor`] manages the full lifecycle of kernel-managed
//! agents: spawn, stop, restart, inspect, and watch. It wraps the
//! existing `AgentLoop` spawn mechanism without replacing it, adding
//! capability enforcement, resource tracking, and process table integration.

use std::collections::HashMap;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(any(feature = "native", feature = "os-patterns"))]
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::process::CancellationToken;

use clawft_platform::Platform;

use crate::capability::AgentCapabilities;
use crate::error::{KernelError, KernelResult};
use crate::ipc::KernelIpc;
use crate::process::{Pid, ProcessEntry, ProcessState, ProcessTable, ResourceUsage};

// ── K1-G1: Restart strategies (os-patterns) ─────────────────────

/// Supervisor restart strategy (Erlang-inspired).
///
/// Determines what happens to sibling agents when one agent fails.
/// Configured per AppManifest or per supervisor instance.
#[non_exhaustive]
#[cfg(feature = "os-patterns")]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RestartStrategy {
    /// Restart only the failed child.
    #[default]
    OneForOne,
    /// Restart all children if one fails.
    OneForAll,
    /// Restart the failed child and all children started after it.
    RestForOne,
    /// Do not restart -- let the agent stay dead.
    Permanent,
    /// Restart only if the agent exited abnormally (non-zero exit code).
    Transient,
}

/// Restart budget: max N restarts within M seconds.
///
/// When the budget is exceeded, the supervisor escalates (stops itself
/// and notifies its parent). Prevents infinite restart loops.
#[cfg(feature = "os-patterns")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestartBudget {
    /// Maximum restarts allowed in the time window.
    pub max_restarts: u32,
    /// Time window in seconds.
    pub within_secs: u64,
}

#[cfg(feature = "os-patterns")]
impl Default for RestartBudget {
    fn default() -> Self {
        Self {
            max_restarts: 5,
            within_secs: 60,
        }
    }
}

/// Restart state tracking per supervised agent.
#[cfg(feature = "os-patterns")]
pub struct RestartTracker {
    /// Number of restarts in the current window.
    pub restart_count: u32,
    /// When the current budget window started.
    pub window_start: std::time::Instant,
    /// When the last restart occurred.
    pub last_restart: Option<std::time::Instant>,
    /// Current backoff delay in milliseconds (exponential: 100ms -> 30s max).
    pub backoff_ms: u64,
}

#[cfg(feature = "os-patterns")]
impl RestartTracker {
    /// Create a new tracker with the window starting now.
    pub fn new() -> Self {
        Self {
            restart_count: 0,
            window_start: std::time::Instant::now(),
            last_restart: None,
            backoff_ms: 0,
        }
    }

    /// Calculate the next backoff delay in milliseconds.
    ///
    /// Exponential backoff: 100ms * 2^(restart_count - 1), capped at 30s.
    ///
    /// Preserved as the hardcoded fallback. See
    /// [`Self::next_backoff_ms_with_model`] for the EML-aware variant
    /// used by the supervisor. The fallback remains reachable so
    /// callers without a model (and every existing test) behave
    /// identically.
    pub fn next_backoff_ms(&self) -> u64 {
        let base: u64 = 100;
        let exponent = self.restart_count.saturating_sub(1);
        let delay = base.saturating_mul(1u64 << exponent.min(20));
        delay.min(30_000)
    }

    /// Calculate the next backoff delay, consulting an optional
    /// [`RestartStrategyModel`](crate::eml_kernel::RestartStrategyModel).
    ///
    /// When `model` is `None` or untrained, this returns the same
    /// exponential-backoff value as [`Self::next_backoff_ms`] (the
    /// model's `predict` falls back to the identical formula when
    /// `!is_trained`). When trained, the model's learned delay is used
    /// instead.
    ///
    /// `failure_type` is the error-kind ordinal (0 = unknown / default
    /// when the caller doesn't track failure kinds), `uptime_secs` is
    /// the agent uptime before the failure, and `system_load` is a
    /// normalised 0.0-1.0 load signal.
    ///
    /// NOTE(eml-swap): wired — Finding #5 (RestartStrategyModel).
    pub fn next_backoff_ms_with_model(
        &self,
        model: Option<&crate::eml_kernel::RestartStrategyModel>,
        failure_type: u32,
        uptime_secs: f64,
        system_load: f64,
    ) -> u64 {
        match model {
            Some(m) => {
                let (delay, _should_retry) =
                    m.predict(self.restart_count, failure_type, uptime_secs, system_load);
                delay
            }
            None => self.next_backoff_ms(),
        }
    }

    /// Check if the restart budget window has expired and reset if so.
    pub fn check_window(&mut self, budget: &RestartBudget) {
        let now = std::time::Instant::now();
        if now.duration_since(self.window_start).as_secs() > budget.within_secs {
            self.restart_count = 0;
            self.window_start = now;
        }
    }

    /// Record a restart attempt. Returns `true` if within budget,
    /// `false` if budget exceeded.
    pub fn record_restart(&mut self, budget: &RestartBudget) -> bool {
        self.check_window(budget);
        self.restart_count += 1;
        self.backoff_ms = self.next_backoff_ms();
        self.last_restart = Some(std::time::Instant::now());
        self.restart_count <= budget.max_restarts
    }

    /// Record a restart attempt with an optional learned
    /// [`RestartStrategyModel`](crate::eml_kernel::RestartStrategyModel).
    ///
    /// Behaviour is identical to [`Self::record_restart`] when `model`
    /// is `None` or untrained; when trained, the computed
    /// `backoff_ms` is sourced from the model.
    ///
    /// NOTE(eml-swap): wired — Finding #5 (RestartStrategyModel).
    pub fn record_restart_with_model(
        &mut self,
        budget: &RestartBudget,
        model: Option<&crate::eml_kernel::RestartStrategyModel>,
        failure_type: u32,
        uptime_secs: f64,
        system_load: f64,
    ) -> bool {
        self.check_window(budget);
        self.restart_count += 1;
        self.backoff_ms =
            self.next_backoff_ms_with_model(model, failure_type, uptime_secs, system_load);
        self.last_restart = Some(std::time::Instant::now());
        self.restart_count <= budget.max_restarts
    }

    /// Check whether the restart budget is exhausted.
    pub fn is_exhausted(&self, budget: &RestartBudget) -> bool {
        // If the window hasn't expired, check the count.
        let now = std::time::Instant::now();
        if now.duration_since(self.window_start).as_secs() > budget.within_secs {
            // Window expired -- budget would reset on next record_restart.
            return false;
        }
        self.restart_count >= budget.max_restarts
    }

    /// Number of restarts remaining in the current window.
    ///
    /// Returns 0 if the budget is already exhausted. Returns
    /// `max_restarts` if the window has expired (it would reset
    /// on the next `record_restart`).
    pub fn remaining(&self, budget: &RestartBudget) -> u32 {
        let now = std::time::Instant::now();
        if now.duration_since(self.window_start).as_secs() > budget.within_secs {
            return budget.max_restarts;
        }
        budget.max_restarts.saturating_sub(self.restart_count)
    }

    /// Determine whether a process should be restarted based on the
    /// strategy and exit code.
    pub fn should_restart(strategy: &RestartStrategy, exit_code: i32) -> bool {
        match strategy {
            RestartStrategy::Permanent => false,
            RestartStrategy::Transient => exit_code != 0,
            // OneForOne, OneForAll, RestForOne always restart
            _ => true,
        }
    }
}

#[cfg(feature = "os-patterns")]
impl Default for RestartTracker {
    fn default() -> Self {
        Self::new()
    }
}

// ── K1-G3: Resource enforcement types ───────────────────────────

/// Result of a resource limit check.
#[non_exhaustive]
#[cfg(feature = "os-patterns")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceCheckResult {
    /// Resource usage is within safe limits.
    Ok,
    /// Resource usage is at 80-99% of the limit.
    Warning {
        resource: String,
        current: u64,
        limit: u64,
    },
    /// Resource usage has reached or exceeded the limit.
    Exceeded {
        resource: String,
        current: u64,
        limit: u64,
    },
}

/// Check resource usage against limits and return the worst result.
#[cfg(feature = "os-patterns")]
pub fn check_resource_usage(
    usage: &ResourceUsage,
    limits: &crate::capability::ResourceLimits,
) -> Vec<ResourceCheckResult> {
    let mut results = Vec::new();

    let checks: &[(&str, u64, u64)] = &[
        ("memory", usage.memory_bytes, limits.max_memory_bytes),
        ("cpu_time", usage.cpu_time_ms, limits.max_cpu_time_ms),
        ("messages", usage.messages_sent, limits.max_messages),
        ("tool_calls", usage.tool_calls, limits.max_tool_calls),
    ];

    for &(name, current, limit) in checks {
        if limit == 0 {
            continue; // unlimited
        }
        let ratio = current as f64 / limit as f64;
        if ratio >= 1.0 {
            results.push(ResourceCheckResult::Exceeded {
                resource: name.to_owned(),
                current,
                limit,
            });
        } else if ratio >= 0.8 {
            results.push(ResourceCheckResult::Warning {
                resource: name.to_owned(),
                current,
                limit,
            });
        }
    }

    results
}

/// Execution backend for spawning an agent process.
///
/// Determines how the agent's work is executed at runtime. Only `Native`
/// is implemented in K0-K2; other variants are defined to crystallize the
/// API surface (see Symposium decisions D2, D3, C1, C8) and will return
/// [`KernelError::BackendNotAvailable`] until their respective K-phases.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SpawnBackend {
    /// Tokio task with agent_loop (K0-K2, default).
    Native,
    /// WASM sandbox via Wasmtime (K3).
    Wasm {
        /// Path to the compiled WASM module.
        module: PathBuf,
    },
    /// Docker/Podman container (K4).
    Container {
        /// Container image reference (e.g. "ghcr.io/org/agent:latest").
        image: String,
    },
    /// Trusted Execution Environment -- SGX, TrustZone, SEV (K6+).
    Tee {
        /// Enclave configuration.
        enclave: EnclaveConfig,
    },
    /// Delegate to a remote node in the cluster (K6).
    Remote {
        /// Cluster node identifier.
        node_id: String,
    },
}

/// Placeholder configuration for TEE enclaves (D14, C8).
///
/// Will be expanded with actual hardware parameters when TEE
/// runtime support is implemented.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnclaveConfig {
    /// Enclave type: "sgx", "trustzone", "sev".
    pub enclave_type: String,
}

/// Request to spawn a new supervised agent process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnRequest {
    /// Unique identifier for the agent.
    pub agent_id: String,

    /// Capabilities to assign. If `None`, the supervisor's default
    /// capabilities are used.
    #[serde(default)]
    pub capabilities: Option<AgentCapabilities>,

    /// PID of the parent process (for tracking spawn lineage).
    #[serde(default)]
    pub parent_pid: Option<Pid>,

    /// Environment variables for the agent.
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Execution backend. `None` defaults to `SpawnBackend::Native`.
    ///
    /// Non-Native backends return [`KernelError::BackendNotAvailable`]
    /// until their respective K-phase implements them.
    #[serde(default)]
    pub backend: Option<SpawnBackend>,
}

/// Result of a successful agent spawn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnResult {
    /// The PID assigned to the new process.
    pub pid: Pid,

    /// The agent identifier.
    pub agent_id: String,
}

/// Manages the lifecycle of kernel-managed agent processes.
///
/// The supervisor sits between the CLI/API surface and the core
/// `AgentLoop`, providing:
///
/// - **Spawn**: creates a process entry, assigns capabilities,
///   allocates a PID, and tracks the agent in the process table.
/// - **Stop**: signals cancellation (graceful) or immediate termination.
/// - **Restart**: stops then re-spawns with the same configuration.
/// - **Inspect**: returns full process entry with capabilities and
///   resource usage.
/// - **Watch**: returns a receiver for process state changes.
///
/// The supervisor does not own the actual `AgentLoop` execution; that
/// remains the responsibility of the caller (kernel boot or CLI).
/// Instead, the supervisor manages the process table entries and
/// provides the cancellation tokens that control agent lifecycle.
pub struct AgentSupervisor<P: Platform> {
    process_table: Arc<ProcessTable>,
    kernel_ipc: Arc<KernelIpc>,
    default_capabilities: AgentCapabilities,
    #[cfg(feature = "native")]
    running_agents: Arc<DashMap<Pid, tokio::task::JoinHandle<()>>>,
    #[cfg(feature = "native")]
    a2a_router: Option<Arc<crate::a2a::A2ARouter>>,
    #[cfg(feature = "native")]
    cron_service: Option<Arc<crate::cron::CronService>>,
    #[cfg(feature = "exochain")]
    tree_manager: Option<Arc<crate::tree_manager::TreeManager>>,
    #[cfg(feature = "exochain")]
    chain_manager: Option<Arc<crate::chain::ChainManager>>,
    /// Monitor registry for process links and monitors (os-patterns).
    #[cfg(feature = "os-patterns")]
    monitor_registry: Arc<crate::monitor::MonitorRegistry>,
    /// Per-supervisor restart strategy (os-patterns).
    #[cfg(feature = "os-patterns")]
    restart_strategy: RestartStrategy,
    /// Per-supervisor restart budget (os-patterns).
    #[cfg(feature = "os-patterns")]
    restart_budget: RestartBudget,
    /// Per-PID restart trackers (os-patterns).
    #[cfg(feature = "os-patterns")]
    restart_trackers: Arc<DashMap<Pid, RestartTracker>>,
    _platform: PhantomData<P>,
}

impl<P: Platform> AgentSupervisor<P> {
    /// Create a new agent supervisor.
    ///
    /// # Arguments
    ///
    /// * `process_table` - Shared process table (also held by Kernel)
    /// * `kernel_ipc` - IPC subsystem for sending lifecycle signals
    /// * `default_capabilities` - Capabilities assigned to agents that
    ///   don't specify their own
    pub fn new(
        process_table: Arc<ProcessTable>,
        kernel_ipc: Arc<KernelIpc>,
        default_capabilities: AgentCapabilities,
    ) -> Self {
        Self {
            process_table,
            kernel_ipc,
            default_capabilities,
            #[cfg(feature = "native")]
            running_agents: Arc::new(DashMap::new()),
            #[cfg(feature = "native")]
            a2a_router: None,
            #[cfg(feature = "native")]
            cron_service: None,
            #[cfg(feature = "exochain")]
            tree_manager: None,
            #[cfg(feature = "exochain")]
            chain_manager: None,
            #[cfg(feature = "os-patterns")]
            monitor_registry: Arc::new(crate::monitor::MonitorRegistry::new()),
            #[cfg(feature = "os-patterns")]
            restart_strategy: RestartStrategy::default(),
            #[cfg(feature = "os-patterns")]
            restart_budget: RestartBudget::default(),
            #[cfg(feature = "os-patterns")]
            restart_trackers: Arc::new(DashMap::new()),
            _platform: PhantomData,
        }
    }

    /// Configure A2A router and cron service.
    ///
    /// When set, `spawn_and_run` will create per-agent inboxes via the
    /// A2ARouter and pass the cron service handle to the agent work loop.
    #[cfg(feature = "native")]
    pub fn with_a2a_router(
        mut self,
        a2a_router: Arc<crate::a2a::A2ARouter>,
        cron_service: Arc<crate::cron::CronService>,
    ) -> Self {
        self.a2a_router = Some(a2a_router);
        self.cron_service = Some(cron_service);
        self
    }

    /// Get the A2A router (if configured).
    #[cfg(feature = "native")]
    pub fn a2a_router(&self) -> Option<&Arc<crate::a2a::A2ARouter>> {
        self.a2a_router.as_ref()
    }

    /// Get the cron service (if configured).
    #[cfg(feature = "native")]
    pub fn cron_service(&self) -> Option<&Arc<crate::cron::CronService>> {
        self.cron_service.as_ref()
    }

    /// Configure exochain integration (tree + chain managers).
    ///
    /// When set, agent spawn/stop/restart events are recorded in
    /// the resource tree and hash chain.
    #[cfg(feature = "exochain")]
    pub fn with_exochain(
        mut self,
        tree_manager: Option<Arc<crate::tree_manager::TreeManager>>,
        chain_manager: Option<Arc<crate::chain::ChainManager>>,
    ) -> Self {
        self.tree_manager = tree_manager;
        self.chain_manager = chain_manager;
        self
    }

    /// Configure the restart strategy and budget for this supervisor.
    #[cfg(feature = "os-patterns")]
    pub fn with_restart_config(mut self, strategy: RestartStrategy, budget: RestartBudget) -> Self {
        self.restart_strategy = strategy;
        self.restart_budget = budget;
        self
    }

    /// Get the monitor registry (os-patterns).
    #[cfg(feature = "os-patterns")]
    pub fn monitor_registry(&self) -> &Arc<crate::monitor::MonitorRegistry> {
        &self.monitor_registry
    }

    /// Get the restart strategy.
    #[cfg(feature = "os-patterns")]
    pub fn restart_strategy(&self) -> &RestartStrategy {
        &self.restart_strategy
    }

    /// Get the restart budget.
    #[cfg(feature = "os-patterns")]
    pub fn restart_budget(&self) -> &RestartBudget {
        &self.restart_budget
    }

    /// Notify the supervisor that a process has exited.
    ///
    /// Processes link/monitor signals via the [`MonitorRegistry`] and
    /// applies the configured [`RestartStrategy`] to determine whether
    /// the exited process (or siblings) should be restarted.
    ///
    /// Returns a list of `(old_pid, new_spawn_result)` for any restarts
    /// that were performed, or an empty vec if no restarts occurred.
    #[cfg(feature = "os-patterns")]
    pub fn handle_exit(&self, pid: Pid, exit_code: i32) -> Vec<(Pid, SpawnResult)> {
        use crate::monitor::ExitReason;

        let reason = if exit_code == 0 {
            ExitReason::Normal
        } else {
            ExitReason::Crash(format!("exit code {exit_code}"))
        };

        // Deliver link/monitor signals
        let (_link_signals, _down_signals) = self.monitor_registry.process_exited(pid, &reason);

        let mut restarts = Vec::new();

        // Determine whether to restart based on strategy
        if !RestartTracker::should_restart(&self.restart_strategy, exit_code) {
            debug!(pid, exit_code, strategy = ?self.restart_strategy, "not restarting per strategy");
            return restarts;
        }

        // Check budget via per-PID tracker
        let mut tracker = self.restart_trackers.entry(pid).or_default();

        let within_budget = tracker.record_restart(&self.restart_budget);
        let backoff_ms = tracker.backoff_ms;
        drop(tracker);

        if !within_budget {
            warn!(pid, "restart budget exhausted for pid, escalating");
            return restarts;
        }

        info!(
            pid,
            backoff_ms,
            strategy = ?self.restart_strategy,
            "scheduling restart after backoff"
        );

        // Log restart event to chain
        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain_manager {
            cm.append(
                "supervisor",
                "agent.self_heal_restart",
                Some(serde_json::json!({
                    "pid": pid,
                    "exit_code": exit_code,
                    "strategy": format!("{:?}", self.restart_strategy),
                    "backoff_ms": backoff_ms,
                })),
            );
        }

        // Apply strategy
        match self.restart_strategy {
            RestartStrategy::OneForOne | RestartStrategy::Transient => {
                // Restart only the failed process
                if let Ok(result) = self.restart(pid) {
                    restarts.push((pid, result));
                }
            }
            RestartStrategy::OneForAll => {
                // Restart the failed process and all siblings
                let siblings: Vec<ProcessEntry> = self
                    .process_table
                    .list()
                    .into_iter()
                    .filter(|e| e.pid != 0 && e.pid != pid && e.state == ProcessState::Running)
                    .collect();

                for sibling in &siblings {
                    if let Ok(result) = self.restart(sibling.pid) {
                        restarts.push((sibling.pid, result));
                    }
                }
                if let Ok(result) = self.restart(pid) {
                    restarts.push((pid, result));
                }
            }
            RestartStrategy::RestForOne => {
                // Restart the failed process and all processes started after it
                let mut all: Vec<ProcessEntry> = self
                    .process_table
                    .list()
                    .into_iter()
                    .filter(|e| e.pid != 0 && e.state == ProcessState::Running)
                    .collect();
                all.sort_by_key(|e| e.pid);

                let after: Vec<Pid> = all.iter().filter(|e| e.pid > pid).map(|e| e.pid).collect();

                for sibling_pid in &after {
                    if let Ok(result) = self.restart(*sibling_pid) {
                        restarts.push((*sibling_pid, result));
                    }
                }
                if let Ok(result) = self.restart(pid) {
                    restarts.push((pid, result));
                }
            }
            RestartStrategy::Permanent => {
                // Never restart -- already handled above via should_restart
            }
        }

        restarts
    }

    /// Spawn a new supervised agent process.
    ///
    /// This creates a process table entry and returns the assigned PID.
    /// The actual agent execution (AgentLoop) must be started separately
    /// by the caller using the returned `SpawnResult` and the
    /// cancellation token from the process entry.
    ///
    /// # Errors
    ///
    /// Returns `KernelError::ProcessTableFull` if the process table
    /// has reached its maximum capacity.
    pub fn spawn(&self, request: SpawnRequest) -> KernelResult<SpawnResult> {
        // Check backend availability -- only Native is implemented.
        match &request.backend {
            None | Some(SpawnBackend::Native) => { /* supported */ }
            Some(SpawnBackend::Wasm { .. }) => {
                return Err(KernelError::BackendNotAvailable {
                    backend: "wasm".into(),
                    reason: "WASM sandbox requires K3 (Wasmtime integration)".into(),
                });
            }
            Some(SpawnBackend::Container { .. }) => {
                return Err(KernelError::BackendNotAvailable {
                    backend: "container".into(),
                    reason: "container runtime requires K4 (Docker/Podman integration)".into(),
                });
            }
            Some(SpawnBackend::Tee { .. }) => {
                return Err(KernelError::BackendNotAvailable {
                    backend: "tee".into(),
                    reason: "TEE runtime requires K6+ and hardware support".into(),
                });
            }
            Some(SpawnBackend::Remote { .. }) => {
                return Err(KernelError::BackendNotAvailable {
                    backend: "remote".into(),
                    reason: "remote delegation requires K6 (cluster networking)".into(),
                });
            }
        }

        let caps = request
            .capabilities
            .unwrap_or_else(|| self.default_capabilities.clone());

        info!(
            agent_id = %request.agent_id,
            parent_pid = ?request.parent_pid,
            "spawning supervised agent"
        );

        let entry = ProcessEntry {
            pid: 0, // Will be set by insert()
            agent_id: request.agent_id.clone(),
            state: ProcessState::Starting,
            capabilities: caps,
            resource_usage: ResourceUsage::default(),
            cancel_token: CancellationToken::new(),
            parent_pid: request.parent_pid,
        };

        let pid = self.process_table.insert(entry)?;

        debug!(pid, agent_id = %request.agent_id, "agent spawned");

        Ok(SpawnResult {
            pid,
            agent_id: request.agent_id,
        })
    }

    /// Spawn a supervised agent and run its work as a tokio task.
    ///
    /// Unlike `spawn`, this method also:
    /// 1. Transitions the process to `Running`
    /// 2. Registers the agent in the resource tree (if exochain enabled)
    /// 3. Spawns a tokio task to execute the provided work closure
    /// 4. On completion: transitions to `Exited`, unregisters from tree,
    ///    logs chain events, and cleans up the task handle
    ///
    /// The `work` closure receives the assigned PID and a
    /// [`CancellationToken`]; it should return an exit code (0 = success).
    ///
    /// # Errors
    ///
    /// Returns `KernelError::ProcessTableFull` if the process table
    /// has reached its maximum capacity.
    #[cfg(feature = "native")]
    pub fn spawn_and_run<F, Fut>(&self, request: SpawnRequest, work: F) -> KernelResult<SpawnResult>
    where
        F: FnOnce(Pid, CancellationToken) -> Fut,
        Fut: std::future::Future<Output = i32> + Send + 'static,
    {
        // Capture parent_pid before spawn() consumes the request
        #[cfg(feature = "exochain")]
        let parent_pid = request.parent_pid;

        // 1. Create process entry via existing spawn()
        let result = self.spawn(request)?;
        let pid = result.pid;

        let entry = self
            .process_table
            .get(pid)
            .ok_or(KernelError::ProcessNotFound { pid })?;
        let cancel_token = entry.cancel_token.clone();

        // 2. Register in resource tree (exochain)
        #[cfg(feature = "exochain")]
        if let Some(ref tm) = self.tree_manager
            && let Err(e) = tm.register_agent(&result.agent_id, pid, &entry.capabilities)
        {
            warn!(error = %e, pid, "failed to register agent in resource tree");
        }

        // 3. Transition to Running
        let _ = self.process_table.update_state(pid, ProcessState::Running);

        // 3b. Log spawn chain event
        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain_manager {
            cm.append(
                "supervisor",
                "agent.spawn",
                Some(serde_json::json!({
                    "agent_id": result.agent_id,
                    "pid": pid,
                    "parent_pid": parent_pid,
                })),
            );
        }

        // 4. Spawn tokio task
        let process_table = Arc::clone(&self.process_table);
        let running_agents = Arc::clone(&self.running_agents);
        let agent_id = result.agent_id.clone();
        #[cfg(feature = "exochain")]
        let tree_manager = self.tree_manager.clone();
        #[cfg(feature = "exochain")]
        let chain_manager = self.chain_manager.clone();

        let future = work(pid, cancel_token);
        let handle = tokio::spawn(async move {
            let exit_code = future.await;

            // Transition to Exited
            let _ = process_table.update_state(pid, ProcessState::Exited(exit_code));

            // Blend scoring on agent exit — performance observation
            #[cfg(feature = "exochain")]
            if let Some(ref tm) = tree_manager {
                let agent_path = format!("/kernel/agents/{agent_id}");
                let rid = exo_resource_tree::ResourceId::new(&agent_path);

                // Build observation: successful exit boosts trust/reliability,
                // failure reduces them.
                let success = exit_code == 0;
                let observation = exo_resource_tree::NodeScoring {
                    trust: if success { 0.8 } else { 0.2 },
                    performance: if success { 0.7 } else { 0.3 },
                    difficulty: 0.5,
                    reward: if success { 0.6 } else { 0.1 },
                    reliability: if success { 0.9 } else { 0.1 },
                    velocity: 0.5,
                };
                // Blend with alpha=0.3 (30% observation, 70% prior)
                if let Err(e) = tm.blend_scoring(&rid, &observation, 0.3) {
                    debug!(error = %e, pid, "scoring blend skipped (node may be unregistered)");
                }
            }

            // Unregister from tree
            #[cfg(feature = "exochain")]
            if let Some(ref tm) = tree_manager
                && let Err(e) = tm.unregister_agent(&agent_id, pid, exit_code)
            {
                tracing::warn!(error = %e, pid, "failed to unregister agent from tree");
            }

            // Log exit chain event
            #[cfg(feature = "exochain")]
            if let Some(ref cm) = chain_manager {
                cm.append(
                    "supervisor",
                    "agent.exit",
                    Some(serde_json::json!({
                        "agent_id": agent_id,
                        "pid": pid,
                        "exit_code": exit_code,
                    })),
                );
            }

            // Remove from running agents map
            running_agents.remove(&pid);

            info!(pid, exit_code, agent_id = %agent_id, "agent task completed");
        });

        self.running_agents.insert(pid, handle);

        info!(pid, agent_id = %result.agent_id, "agent spawned and running");

        Ok(result)
    }

    /// Stop a supervised agent process.
    ///
    /// If `graceful` is true, the process is moved to `Stopping` state
    /// and its cancellation token is cancelled, allowing the agent to
    /// finish its current work. If `graceful` is false, the process is
    /// immediately moved to `Exited(-1)`.
    ///
    /// Stopping an already-exited process is idempotent and returns `Ok`.
    ///
    /// # Errors
    ///
    /// Returns `KernelError::ProcessNotFound` if the PID is not in
    /// the process table.
    pub fn stop(&self, pid: Pid, graceful: bool) -> KernelResult<()> {
        let entry = self
            .process_table
            .get(pid)
            .ok_or(KernelError::ProcessNotFound { pid })?;

        // Already exited -- idempotent
        if matches!(entry.state, ProcessState::Exited(_)) {
            warn!(pid, "stop called on already-exited process");
            return Ok(());
        }

        if graceful {
            info!(pid, "gracefully stopping agent");
            // Transition to Stopping, then cancel the token.
            // The spawned task (if any) will detect cancellation,
            // exit, and handle tree/chain cleanup.
            let _ = self.process_table.update_state(pid, ProcessState::Stopping);
            entry.cancel_token.cancel();
        } else {
            info!(pid, "force stopping agent");
            entry.cancel_token.cancel();
            let _ = self
                .process_table
                .update_state(pid, ProcessState::Exited(-1));

            // Abort the running task handle (cleanup won't run)
            #[cfg(feature = "native")]
            if let Some((_, handle)) = self.running_agents.remove(&pid) {
                handle.abort();
            }

            // Since the spawned task was aborted, do tree/chain
            // cleanup directly here.
            #[cfg(feature = "exochain")]
            {
                if let Some(ref tm) = self.tree_manager {
                    let _ = tm.unregister_agent(&entry.agent_id, pid, -1);
                }
                if let Some(ref cm) = self.chain_manager {
                    cm.append(
                        "supervisor",
                        "agent.force_stop",
                        Some(serde_json::json!({
                            "agent_id": entry.agent_id,
                            "pid": pid,
                        })),
                    );
                }
            }
        }

        Ok(())
    }

    /// Restart a supervised agent process.
    ///
    /// Stops the existing process (gracefully), then spawns a new one
    /// with the same agent_id and capabilities. The new process gets
    /// a fresh PID; the old entry remains in the table with
    /// `Exited(0)` state.
    ///
    /// The `parent_pid` of the new process is set to the restarted
    /// PID, creating a restart lineage.
    ///
    /// # Errors
    ///
    /// Returns `KernelError::ProcessNotFound` if the PID is not in
    /// the process table.
    pub fn restart(&self, pid: Pid) -> KernelResult<SpawnResult> {
        let old_entry = self
            .process_table
            .get(pid)
            .ok_or(KernelError::ProcessNotFound { pid })?;

        info!(pid, agent_id = %old_entry.agent_id, "restarting agent");

        // Stop the old process
        self.stop(pid, true)?;

        // Mark as cleanly exited if not already
        if !matches!(old_entry.state, ProcessState::Exited(_)) {
            let _ = self
                .process_table
                .update_state(pid, ProcessState::Exited(0));
        }

        // Spawn replacement with same config
        let request = SpawnRequest {
            agent_id: old_entry.agent_id.clone(),
            capabilities: Some(old_entry.capabilities.clone()),
            parent_pid: Some(pid),
            env: HashMap::new(),
            backend: None, // restarts always use Native
        };

        let result = self.spawn(request)?;

        // Log restart chain event linking old PID to new PID
        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain_manager {
            cm.append(
                "supervisor",
                "agent.restart",
                Some(serde_json::json!({
                    "agent_id": result.agent_id,
                    "old_pid": pid,
                    "new_pid": result.pid,
                })),
            );
        }

        Ok(result)
    }

    /// Inspect a supervised agent process.
    ///
    /// Returns a clone of the full [`ProcessEntry`] including
    /// capabilities and resource usage.
    ///
    /// # Errors
    ///
    /// Returns `KernelError::ProcessNotFound` if the PID is not in
    /// the process table.
    pub fn inspect(&self, pid: Pid) -> KernelResult<ProcessEntry> {
        self.process_table
            .get(pid)
            .ok_or(KernelError::ProcessNotFound { pid })
    }

    /// List processes filtered by state.
    pub fn list_by_state(&self, state: ProcessState) -> Vec<ProcessEntry> {
        self.process_table
            .list()
            .into_iter()
            .filter(|e| e.state == state)
            .collect()
    }

    /// List all running agent processes (excludes kernel PID 0).
    pub fn list_agents(&self) -> Vec<ProcessEntry> {
        self.process_table
            .list()
            .into_iter()
            .filter(|e| e.pid != 0)
            .collect()
    }

    /// Get a reference to the shared process table.
    pub fn process_table(&self) -> &Arc<ProcessTable> {
        &self.process_table
    }

    /// Get a reference to the IPC subsystem.
    pub fn ipc(&self) -> &Arc<KernelIpc> {
        &self.kernel_ipc
    }

    /// Get the default capabilities assigned to new agents.
    pub fn default_capabilities(&self) -> &AgentCapabilities {
        &self.default_capabilities
    }

    /// Count running processes (excluding kernel PID 0).
    pub fn running_count(&self) -> usize {
        self.process_table
            .list()
            .iter()
            .filter(|e| e.pid != 0 && e.state == ProcessState::Running)
            .count()
    }

    /// Get the number of actively tracked running agent tasks.
    #[cfg(feature = "native")]
    pub fn running_task_count(&self) -> usize {
        self.running_agents.len()
    }

    /// Abort all running agent tasks (used during forced shutdown).
    #[cfg(feature = "native")]
    pub fn abort_all(&self) {
        for entry in self.running_agents.iter() {
            entry.value().abort();
        }
        self.running_agents.clear();
    }

    /// Sweep finished agent handles that were not cleaned up normally.
    ///
    /// Iterates `running_agents`, checks `is_finished()` on each
    /// `JoinHandle`, and for any that are finished:
    /// 1. Removes the handle from the map
    /// 2. If the process table still shows `Running`, transitions to
    ///    `Exited(-2)` (watchdog reap) or `Exited(-3)` (panic reap)
    /// 3. Logs a chain event (when exochain is enabled)
    ///
    /// Returns a list of (pid, exit_code) for all reaped processes.
    #[cfg(feature = "native")]
    pub async fn watchdog_sweep(&self) -> Vec<(Pid, i32)> {
        let mut reaped = Vec::new();

        // Collect finished PIDs first to avoid holding DashMap refs across await
        let finished_pids: Vec<Pid> = self
            .running_agents
            .iter()
            .filter(|entry| entry.value().is_finished())
            .map(|entry| *entry.key())
            .collect();

        for pid in finished_pids {
            if let Some((_, handle)) = self.running_agents.remove(&pid) {
                // Check if the task panicked
                let exit_code = match handle.await {
                    Ok(()) => -2, // Watchdog reap (task finished but cleanup didn't remove from map)
                    Err(e) if e.is_panic() => -3, // Panic reap
                    Err(_) => -2, // Cancelled or other
                };

                // Only transition if process table still shows Running
                if let Some(entry) = self.process_table.get(pid)
                    && entry.state == ProcessState::Running
                {
                    let _ = self
                        .process_table
                        .update_state(pid, ProcessState::Exited(exit_code));

                    #[cfg(feature = "exochain")]
                    if let Some(ref cm) = self.chain_manager {
                        cm.append(
                            "watchdog",
                            "agent.watchdog_reap",
                            Some(serde_json::json!({
                                "pid": pid,
                                "exit_code": exit_code,
                                "agent_id": entry.agent_id,
                            })),
                        );
                    }

                    reaped.push((pid, exit_code));
                    info!(pid, exit_code, agent_id = %entry.agent_id, "watchdog reaped stale agent");
                }
            }
        }

        reaped
    }

    /// Gracefully shut down all running agents with a timeout.
    ///
    /// 1. Cancels all agent cancellation tokens via the process table
    /// 2. Drains all JoinHandles from `running_agents`
    /// 3. Waits for all tasks to complete, with a timeout
    /// 4. On timeout, aborts any remaining tasks
    ///
    /// Returns a list of (pid, exit_code) for all agents.
    #[cfg(feature = "native")]
    pub async fn shutdown_all(&self, timeout: std::time::Duration) -> Vec<(Pid, i32)> {
        // 1. Cancel all agent tokens
        for entry in self.process_table.list() {
            if entry.pid == 0 {
                continue; // Don't cancel the kernel process
            }
            entry.cancel_token.cancel();
        }

        // 2. Drain all handles from running_agents
        let handles: Vec<(Pid, tokio::task::JoinHandle<()>)> = {
            let pids: Vec<Pid> = self.running_agents.iter().map(|e| *e.key()).collect();
            let mut collected = Vec::with_capacity(pids.len());
            for pid in pids {
                if let Some((pid, handle)) = self.running_agents.remove(&pid) {
                    collected.push((pid, handle));
                }
            }
            collected
        };

        if handles.is_empty() {
            return Vec::new();
        }

        let process_table = &self.process_table;

        // 3. Wait for all handles concurrently with timeout.
        //    Use futures::future::join_all-style: wrap each handle in a
        //    tokio::time::timeout so no single stuck handle blocks the rest.
        let mut results = Vec::with_capacity(handles.len());

        match tokio::time::timeout(
            timeout,
            futures::future::join_all(
                handles
                    .into_iter()
                    .map(|(pid, handle)| async move { (pid, handle.await) }),
            ),
        )
        .await
        {
            Ok(join_results) => {
                // All handles completed within timeout
                for (pid, join_result) in join_results {
                    let exit_code = match join_result {
                        Ok(()) => process_table
                            .get(pid)
                            .and_then(|e| match e.state {
                                ProcessState::Exited(code) => Some(code),
                                _ => None,
                            })
                            .unwrap_or(0),
                        Err(e) if e.is_panic() => -3,
                        Err(_) => -1,
                    };
                    results.push((pid, exit_code));
                }
            }
            Err(_elapsed) => {
                info!("shutdown timeout reached, aborting remaining agents");
                // Timeout expired. Any handles still alive need to be aborted.
                // Since we moved the handles into join_all, they're already being
                // awaited. The timeout drop aborts them. Record all remaining
                // agents from the running_agents map.
                let remaining: Vec<Pid> = self.running_agents.iter().map(|e| *e.key()).collect();
                for pid in remaining {
                    if let Some((pid, handle)) = self.running_agents.remove(&pid) {
                        handle.abort();
                        let _ = process_table.update_state(pid, ProcessState::Exited(-1));
                        results.push((pid, -1));
                    }
                }

                // If no remaining handles were in the map (handles were consumed by
                // join_all), check the process table for any non-exited agents.
                if results.is_empty() {
                    for entry in process_table.list() {
                        if entry.pid != 0 && !matches!(entry.state, ProcessState::Exited(_)) {
                            let _ = process_table.update_state(entry.pid, ProcessState::Exited(-1));
                            results.push((entry.pid, -1));
                        }
                    }
                }
            }
        }

        results
    }

    /// Get the tree manager (when exochain feature is enabled).
    #[cfg(feature = "exochain")]
    pub fn tree_manager(&self) -> Option<&Arc<crate::tree_manager::TreeManager>> {
        self.tree_manager.as_ref()
    }

    /// Get the chain manager (when exochain feature is enabled).
    #[cfg(feature = "exochain")]
    pub fn chain_manager(&self) -> Option<&Arc<crate::chain::ChainManager>> {
        self.chain_manager.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawft_core::bus::MessageBus;

    fn make_supervisor() -> AgentSupervisor<clawft_platform::NativePlatform> {
        let process_table = Arc::new(ProcessTable::new(16));
        let bus = Arc::new(MessageBus::new());
        let ipc = Arc::new(KernelIpc::new(bus));
        AgentSupervisor::new(process_table, ipc, AgentCapabilities::default())
    }

    fn simple_request(agent_id: &str) -> SpawnRequest {
        SpawnRequest {
            agent_id: agent_id.to_owned(),
            capabilities: None,
            parent_pid: None,
            env: HashMap::new(),
            backend: None,
        }
    }

    #[test]
    fn spawn_creates_process_entry() {
        let sup = make_supervisor();
        let result = sup.spawn(simple_request("agent-1")).unwrap();

        assert!(result.pid > 0);
        assert_eq!(result.agent_id, "agent-1");

        let entry = sup.inspect(result.pid).unwrap();
        assert_eq!(entry.agent_id, "agent-1");
        assert_eq!(entry.state, ProcessState::Starting);
    }

    #[test]
    fn spawn_uses_default_capabilities() {
        let sup = make_supervisor();
        let result = sup.spawn(simple_request("agent-1")).unwrap();

        let entry = sup.inspect(result.pid).unwrap();
        assert!(entry.capabilities.can_spawn);
        assert!(entry.capabilities.can_ipc);
        assert!(entry.capabilities.can_exec_tools);
    }

    #[test]
    fn spawn_uses_custom_capabilities() {
        let sup = make_supervisor();
        let caps = AgentCapabilities {
            can_spawn: false,
            can_ipc: false,
            can_exec_tools: true,
            can_network: true,
            ..Default::default()
        };

        let request = SpawnRequest {
            agent_id: "restricted".to_owned(),
            capabilities: Some(caps.clone()),
            parent_pid: None,
            env: HashMap::new(),
            backend: None,
        };

        let result = sup.spawn(request).unwrap();
        let entry = sup.inspect(result.pid).unwrap();
        assert!(!entry.capabilities.can_spawn);
        assert!(!entry.capabilities.can_ipc);
        assert!(entry.capabilities.can_network);
    }

    #[test]
    fn spawn_with_parent_pid() {
        let sup = make_supervisor();
        let parent = sup.spawn(simple_request("parent")).unwrap();

        let request = SpawnRequest {
            agent_id: "child".to_owned(),
            capabilities: None,
            parent_pid: Some(parent.pid),
            env: HashMap::new(),
            backend: None,
        };

        let result = sup.spawn(request).unwrap();
        let entry = sup.inspect(result.pid).unwrap();
        assert_eq!(entry.parent_pid, Some(parent.pid));
    }

    #[test]
    fn spawn_fails_when_table_full() {
        let process_table = Arc::new(ProcessTable::new(2));
        let bus = Arc::new(MessageBus::new());
        let ipc = Arc::new(KernelIpc::new(bus));
        let sup: AgentSupervisor<clawft_platform::NativePlatform> =
            AgentSupervisor::new(process_table, ipc, AgentCapabilities::default());

        sup.spawn(simple_request("a1")).unwrap();
        sup.spawn(simple_request("a2")).unwrap();
        let result = sup.spawn(simple_request("a3"));
        assert!(result.is_err());
    }

    #[test]
    fn stop_graceful() {
        let sup = make_supervisor();
        let result = sup.spawn(simple_request("agent-1")).unwrap();

        // Move to Running first (Starting -> Running -> Stopping)
        sup.process_table()
            .update_state(result.pid, ProcessState::Running)
            .unwrap();

        sup.stop(result.pid, true).unwrap();

        let entry = sup.inspect(result.pid).unwrap();
        assert_eq!(entry.state, ProcessState::Stopping);
        assert!(entry.cancel_token.is_cancelled());
    }

    #[test]
    fn stop_force() {
        let sup = make_supervisor();
        let result = sup.spawn(simple_request("agent-1")).unwrap();

        // Move to Running first
        sup.process_table()
            .update_state(result.pid, ProcessState::Running)
            .unwrap();

        sup.stop(result.pid, false).unwrap();

        let entry = sup.inspect(result.pid).unwrap();
        assert!(entry.cancel_token.is_cancelled());
    }

    #[test]
    fn stop_already_exited_is_idempotent() {
        let sup = make_supervisor();
        let result = sup.spawn(simple_request("agent-1")).unwrap();

        // Move to exited
        sup.process_table()
            .update_state(result.pid, ProcessState::Exited(0))
            .unwrap();

        // Should succeed without error
        sup.stop(result.pid, true).unwrap();
    }

    #[test]
    fn stop_nonexistent_pid_fails() {
        let sup = make_supervisor();
        let result = sup.stop(999, true);
        assert!(result.is_err());
    }

    #[test]
    fn restart_creates_new_process() {
        let sup = make_supervisor();
        let original = sup.spawn(simple_request("agent-1")).unwrap();

        // Move to Running so it can be stopped
        sup.process_table()
            .update_state(original.pid, ProcessState::Running)
            .unwrap();

        let restarted = sup.restart(original.pid).unwrap();

        // New PID, same agent_id
        assert_ne!(restarted.pid, original.pid);
        assert_eq!(restarted.agent_id, "agent-1");

        // New process has parent_pid pointing to old PID
        let new_entry = sup.inspect(restarted.pid).unwrap();
        assert_eq!(new_entry.parent_pid, Some(original.pid));
    }

    #[test]
    fn restart_preserves_capabilities() {
        let sup = make_supervisor();
        let caps = AgentCapabilities {
            can_spawn: false,
            can_network: true,
            ..Default::default()
        };

        let request = SpawnRequest {
            agent_id: "restricted".to_owned(),
            capabilities: Some(caps),
            parent_pid: None,
            env: HashMap::new(),
            backend: None,
        };

        let original = sup.spawn(request).unwrap();
        sup.process_table()
            .update_state(original.pid, ProcessState::Running)
            .unwrap();

        let restarted = sup.restart(original.pid).unwrap();
        let entry = sup.inspect(restarted.pid).unwrap();
        assert!(!entry.capabilities.can_spawn);
        assert!(entry.capabilities.can_network);
    }

    #[test]
    fn list_by_state() {
        let sup = make_supervisor();
        let r1 = sup.spawn(simple_request("a1")).unwrap();
        let r2 = sup.spawn(simple_request("a2")).unwrap();
        sup.spawn(simple_request("a3")).unwrap();

        // Move first two to Running
        sup.process_table()
            .update_state(r1.pid, ProcessState::Running)
            .unwrap();
        sup.process_table()
            .update_state(r2.pid, ProcessState::Running)
            .unwrap();

        let running = sup.list_by_state(ProcessState::Running);
        assert_eq!(running.len(), 2);

        let starting = sup.list_by_state(ProcessState::Starting);
        assert_eq!(starting.len(), 1);
    }

    #[test]
    fn list_agents_excludes_kernel() {
        let sup = make_supervisor();

        // Insert kernel PID 0
        let kernel_entry = ProcessEntry {
            pid: 0,
            agent_id: "kernel".to_owned(),
            state: ProcessState::Running,
            capabilities: AgentCapabilities::default(),
            resource_usage: ResourceUsage::default(),
            cancel_token: CancellationToken::new(),
            parent_pid: None,
        };
        sup.process_table().insert_with_pid(kernel_entry).unwrap();

        // Spawn an agent
        sup.spawn(simple_request("agent-1")).unwrap();

        let agents = sup.list_agents();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].agent_id, "agent-1");
    }

    #[test]
    fn running_count() {
        let sup = make_supervisor();
        let r1 = sup.spawn(simple_request("a1")).unwrap();
        let r2 = sup.spawn(simple_request("a2")).unwrap();
        sup.spawn(simple_request("a3")).unwrap();

        assert_eq!(sup.running_count(), 0); // All Starting

        sup.process_table()
            .update_state(r1.pid, ProcessState::Running)
            .unwrap();
        assert_eq!(sup.running_count(), 1);

        sup.process_table()
            .update_state(r2.pid, ProcessState::Running)
            .unwrap();
        assert_eq!(sup.running_count(), 2);
    }

    #[test]
    fn default_capabilities_accessor() {
        let sup = make_supervisor();
        let caps = sup.default_capabilities();
        assert!(caps.can_spawn);
        assert!(caps.can_ipc);
        assert!(caps.can_exec_tools);
    }

    #[test]
    fn spawn_request_serde_roundtrip() {
        let request = SpawnRequest {
            agent_id: "test".to_owned(),
            capabilities: Some(AgentCapabilities {
                can_spawn: false,
                ..Default::default()
            }),
            parent_pid: Some(5),
            env: HashMap::from([("KEY".into(), "VALUE".into())]),
            backend: Some(SpawnBackend::Native),
        };

        let json = serde_json::to_string(&request).unwrap();
        let restored: SpawnRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.agent_id, "test");
        assert_eq!(restored.parent_pid, Some(5));
        assert!(!restored.capabilities.unwrap().can_spawn);
    }

    #[test]
    fn spawn_result_serde_roundtrip() {
        let result = SpawnResult {
            pid: 42,
            agent_id: "agent-42".to_owned(),
        };

        let json = serde_json::to_string(&result).unwrap();
        let restored: SpawnResult = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.pid, 42);
        assert_eq!(restored.agent_id, "agent-42");
    }

    #[tokio::test]
    async fn spawn_and_run_executes_work() {
        let sup = make_supervisor();

        let result = sup
            .spawn_and_run(simple_request("runner-1"), |_pid, _cancel| async { 0 })
            .unwrap();

        assert!(result.pid > 0);
        assert_eq!(result.agent_id, "runner-1");

        // Process should be Running immediately after spawn_and_run
        let entry = sup.inspect(result.pid).unwrap();
        assert_eq!(entry.state, ProcessState::Running);

        // Wait for the task to complete
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Process should be Exited after work completes
        let entry = sup.inspect(result.pid).unwrap();
        assert!(matches!(entry.state, ProcessState::Exited(0)));

        // Running task should be cleaned up
        assert_eq!(sup.running_task_count(), 0);
    }

    #[tokio::test]
    async fn spawn_and_run_respects_cancellation() {
        let sup = make_supervisor();

        let result = sup
            .spawn_and_run(simple_request("cancellable"), |_pid, cancel| async move {
                cancel.cancelled().await;
                42
            })
            .unwrap();

        assert_eq!(sup.running_task_count(), 1);

        // Stop the agent
        sup.stop(result.pid, true).unwrap();

        // Wait for the task to detect cancellation and exit
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let entry = sup.inspect(result.pid).unwrap();
        assert!(matches!(entry.state, ProcessState::Exited(42)));
        assert_eq!(sup.running_task_count(), 0);
    }

    #[tokio::test]
    async fn spawn_and_run_force_stop_aborts() {
        let sup = make_supervisor();

        let result = sup
            .spawn_and_run(simple_request("force-me"), |_pid, cancel| async move {
                cancel.cancelled().await;
                0
            })
            .unwrap();

        // Force stop should abort the task immediately
        sup.stop(result.pid, false).unwrap();

        let entry = sup.inspect(result.pid).unwrap();
        assert!(matches!(entry.state, ProcessState::Exited(-1)));
        assert_eq!(sup.running_task_count(), 0);
    }

    #[tokio::test]
    async fn abort_all_clears_running_agents() {
        let sup = make_supervisor();

        sup.spawn_and_run(simple_request("a1"), |_pid, cancel| async move {
            cancel.cancelled().await;
            0
        })
        .unwrap();
        sup.spawn_and_run(simple_request("a2"), |_pid, cancel| async move {
            cancel.cancelled().await;
            0
        })
        .unwrap();

        assert_eq!(sup.running_task_count(), 2);

        sup.abort_all();

        assert_eq!(sup.running_task_count(), 0);
    }

    #[tokio::test]
    async fn watchdog_sweep_reaps_finished_task() {
        let sup = make_supervisor();

        // Spawn a task that completes instantly
        let result = sup
            .spawn_and_run(simple_request("instant"), |_pid, _cancel| async { 0 })
            .unwrap();

        // Give the task time to complete and clean up normally
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // The task should have cleaned itself up. If it did, sweep returns empty.
        // If by race condition it didn't, sweep should reap it.
        let reaped = sup.watchdog_sweep().await;

        // Either the task cleaned up on its own (reaped is empty and state is Exited)
        // or the watchdog reaped it.
        let entry = sup.inspect(result.pid).unwrap();
        assert!(
            matches!(entry.state, ProcessState::Exited(_)),
            "process should be Exited after sweep, got {:?}",
            entry.state
        );

        // Running task count should be 0 either way
        assert_eq!(sup.running_task_count(), 0);

        // If reaped, verify exit code
        for (pid, code) in &reaped {
            assert_eq!(*pid, result.pid);
            assert!(*code == -2 || *code == -3);
        }
    }

    #[tokio::test]
    async fn shutdown_all_graceful() {
        let sup = make_supervisor();

        sup.spawn_and_run(simple_request("g1"), |_pid, cancel| async move {
            cancel.cancelled().await;
            0
        })
        .unwrap();
        sup.spawn_and_run(simple_request("g2"), |_pid, cancel| async move {
            cancel.cancelled().await;
            42
        })
        .unwrap();

        assert_eq!(sup.running_task_count(), 2);

        let results = sup.shutdown_all(std::time::Duration::from_secs(5)).await;

        assert_eq!(results.len(), 2);
        assert_eq!(sup.running_task_count(), 0);
    }

    #[tokio::test]
    async fn shutdown_all_timeout_aborts() {
        let sup = make_supervisor();

        // Spawn a task that ignores cancellation (just sleeps forever)
        sup.spawn_and_run(simple_request("stubborn"), |_pid, _cancel| async move {
            // Ignore cancellation — sleep for a very long time
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            0
        })
        .unwrap();

        assert_eq!(sup.running_task_count(), 1);

        // shutdown_all with a very short timeout
        let results = sup
            .shutdown_all(std::time::Duration::from_millis(100))
            .await;

        // Should have at least 1 result (might be aborted)
        assert!(!results.is_empty());
        assert_eq!(sup.running_task_count(), 0);
    }

    #[cfg(feature = "exochain")]
    #[tokio::test]
    async fn chain_logs_agent_spawn() {
        let process_table = Arc::new(ProcessTable::new(16));
        let bus = Arc::new(MessageBus::new());
        let ipc = Arc::new(KernelIpc::new(bus));
        let cm = Arc::new(crate::chain::ChainManager::new(0, 1000));

        let sup: AgentSupervisor<clawft_platform::NativePlatform> =
            AgentSupervisor::new(process_table, ipc, AgentCapabilities::default())
                .with_exochain(None, Some(cm.clone()));

        let request = SpawnRequest {
            agent_id: "chain-agent".to_owned(),
            capabilities: None,
            parent_pid: Some(99),
            env: HashMap::new(),
            backend: None,
        };

        let result = sup
            .spawn_and_run(request, |_pid, _cancel| async { 0 })
            .unwrap();

        // Wait for the task to complete
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Verify agent.spawn event on chain
        let events = cm.tail(10);
        let spawn_evt = events.iter().find(|e| e.kind == "agent.spawn");

        assert!(spawn_evt.is_some(), "expected agent.spawn event on chain");

        let payload = spawn_evt.unwrap().payload.as_ref().unwrap();
        assert_eq!(payload["agent_id"], "chain-agent");
        assert_eq!(payload["pid"], result.pid);
        assert_eq!(payload["parent_pid"], 99);

        // Should also have agent.exit from task completion
        let exit_evt = events.iter().find(|e| e.kind == "agent.exit");
        assert!(exit_evt.is_some(), "expected agent.exit event on chain");
    }

    // ── SpawnBackend tests (K2.1 T1: C1 + C8) ──────────────────

    #[test]
    fn spawn_native_explicit() {
        let sup = make_supervisor();
        let request = SpawnRequest {
            agent_id: "native-agent".to_owned(),
            capabilities: None,
            parent_pid: None,
            env: HashMap::new(),
            backend: Some(SpawnBackend::Native),
        };
        let result = sup.spawn(request).unwrap();
        assert!(result.pid > 0);
        assert_eq!(result.agent_id, "native-agent");
    }

    #[test]
    fn spawn_backend_none_defaults_to_native() {
        let sup = make_supervisor();
        let request = SpawnRequest {
            agent_id: "default-agent".to_owned(),
            capabilities: None,
            parent_pid: None,
            env: HashMap::new(),
            backend: None,
        };
        let result = sup.spawn(request).unwrap();
        assert!(result.pid > 0);
        assert_eq!(result.agent_id, "default-agent");
    }

    #[test]
    fn spawn_wasm_returns_not_available() {
        let sup = make_supervisor();
        let request = SpawnRequest {
            agent_id: "wasm-agent".to_owned(),
            capabilities: None,
            parent_pid: None,
            env: HashMap::new(),
            backend: Some(SpawnBackend::Wasm {
                module: PathBuf::from("/tmp/agent.wasm"),
            }),
        };
        let result = sup.spawn(request);
        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("wasm"), "error should mention wasm: {msg}");
        assert!(
            msg.contains("not available"),
            "error should say not available: {msg}"
        );
    }

    #[test]
    fn spawn_container_returns_not_available() {
        let sup = make_supervisor();
        let request = SpawnRequest {
            agent_id: "container-agent".to_owned(),
            capabilities: None,
            parent_pid: None,
            env: HashMap::new(),
            backend: Some(SpawnBackend::Container {
                image: "ghcr.io/test/agent:latest".into(),
            }),
        };
        let result = sup.spawn(request);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("container"),
            "error should mention container: {msg}"
        );
    }

    #[test]
    fn spawn_tee_returns_not_available() {
        let sup = make_supervisor();
        let request = SpawnRequest {
            agent_id: "tee-agent".to_owned(),
            capabilities: None,
            parent_pid: None,
            env: HashMap::new(),
            backend: Some(SpawnBackend::Tee {
                enclave: EnclaveConfig {
                    enclave_type: "sgx".into(),
                },
            }),
        };
        let result = sup.spawn(request);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("tee"), "error should mention tee: {msg}");
    }

    #[test]
    fn spawn_remote_returns_not_available() {
        let sup = make_supervisor();
        let request = SpawnRequest {
            agent_id: "remote-agent".to_owned(),
            capabilities: None,
            parent_pid: None,
            env: HashMap::new(),
            backend: Some(SpawnBackend::Remote {
                node_id: "node-42".into(),
            }),
        };
        let result = sup.spawn(request);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("remote"), "error should mention remote: {msg}");
    }

    // ── K1-G1: Restart strategy tests (os-patterns) ─────────────

    #[cfg(feature = "os-patterns")]
    mod restart_tests {
        use super::*;
        use crate::capability::ResourceLimits;
        use crate::supervisor::{
            ResourceCheckResult, RestartBudget, RestartStrategy, RestartTracker,
            check_resource_usage,
        };

        #[test]
        fn restart_strategy_default_is_one_for_one() {
            assert_eq!(RestartStrategy::default(), RestartStrategy::OneForOne);
        }

        #[test]
        fn restart_strategy_serde_roundtrip() {
            let strategies = vec![
                RestartStrategy::OneForOne,
                RestartStrategy::OneForAll,
                RestartStrategy::RestForOne,
            ];
            for strategy in strategies {
                let json = serde_json::to_string(&strategy).unwrap();
                let restored: RestartStrategy = serde_json::from_str(&json).unwrap();
                assert_eq!(restored, strategy);
            }
        }

        #[test]
        fn restart_budget_default() {
            let budget = RestartBudget::default();
            assert_eq!(budget.max_restarts, 5);
            assert_eq!(budget.within_secs, 60);
        }

        #[test]
        fn restart_budget_serde_roundtrip() {
            let budget = RestartBudget {
                max_restarts: 3,
                within_secs: 30,
            };
            let json = serde_json::to_string(&budget).unwrap();
            let restored: RestartBudget = serde_json::from_str(&json).unwrap();
            assert_eq!(restored.max_restarts, 3);
            assert_eq!(restored.within_secs, 30);
        }

        #[test]
        fn tracker_new_starts_at_zero() {
            let tracker = RestartTracker::new();
            assert_eq!(tracker.restart_count, 0);
            assert_eq!(tracker.backoff_ms, 0);
            assert!(tracker.last_restart.is_none());
        }

        #[test]
        fn tracker_backoff_exponential() {
            let mut tracker = RestartTracker::new();
            let budget = RestartBudget {
                max_restarts: 10,
                within_secs: 60,
            };

            // First restart: 100ms
            tracker.record_restart(&budget);
            assert_eq!(tracker.backoff_ms, 100);

            // Second restart: 200ms
            tracker.record_restart(&budget);
            assert_eq!(tracker.backoff_ms, 200);

            // Third restart: 400ms
            tracker.record_restart(&budget);
            assert_eq!(tracker.backoff_ms, 400);

            // Fourth restart: 800ms
            tracker.record_restart(&budget);
            assert_eq!(tracker.backoff_ms, 800);
        }

        #[test]
        fn tracker_backoff_caps_at_30s() {
            let mut tracker = RestartTracker::new();
            tracker.restart_count = 20;
            let delay = tracker.next_backoff_ms();
            assert!(delay <= 30_000, "backoff should cap at 30s, got {delay}");
        }

        #[test]
        fn tracker_budget_exceeded_returns_false() {
            let mut tracker = RestartTracker::new();
            let budget = RestartBudget {
                max_restarts: 2,
                within_secs: 60,
            };

            assert!(tracker.record_restart(&budget)); // 1
            assert!(tracker.record_restart(&budget)); // 2
            assert!(!tracker.record_restart(&budget)); // 3 > 2 = exceeded
        }

        #[test]
        fn tracker_budget_within_returns_true() {
            let mut tracker = RestartTracker::new();
            let budget = RestartBudget {
                max_restarts: 5,
                within_secs: 60,
            };

            for _ in 0..5 {
                assert!(tracker.record_restart(&budget));
            }
        }

        #[test]
        fn tracker_records_last_restart() {
            let mut tracker = RestartTracker::new();
            let budget = RestartBudget::default();
            assert!(tracker.last_restart.is_none());

            tracker.record_restart(&budget);
            assert!(tracker.last_restart.is_some());
        }

        #[test]
        fn tracker_with_untrained_model_matches_hardcoded() {
            // Finding #5: untrained RestartStrategyModel must reproduce
            // the exponential-backoff formula bit-for-bit.
            let model = crate::eml_kernel::RestartStrategyModel::new();
            assert!(!model.is_trained());
            let mut tracker = RestartTracker::new();
            let budget = RestartBudget::default();

            for _ in 0..5 {
                tracker.check_window(&budget);
                tracker.restart_count += 1;
                let hardcoded = tracker.next_backoff_ms();
                let via_model = tracker.next_backoff_ms_with_model(Some(&model), 0, 60.0, 0.5);
                assert_eq!(
                    hardcoded, via_model,
                    "untrained model must reproduce hardcoded backoff at count {}",
                    tracker.restart_count
                );
            }
        }

        #[test]
        fn tracker_with_trained_model_uses_prediction() {
            // Finding #5: with a trained RestartStrategyModel the
            // backoff must come from the model, not the formula. We
            // force the trained flag via JSON patch.
            let model = crate::eml_kernel::RestartStrategyModel::new();
            let mut json = serde_json::to_value(&model).unwrap();
            if let Some(inner) = json.get_mut("inner").and_then(|v| v.as_object_mut()) {
                inner.insert("trained".into(), serde_json::Value::Bool(true));
            }
            let forced: crate::eml_kernel::RestartStrategyModel =
                serde_json::from_value(json).unwrap();
            assert!(forced.is_trained());

            let mut tracker = RestartTracker::new();
            tracker.restart_count = 3; // would give 400ms hardcoded

            let hardcoded = tracker.next_backoff_ms();
            let via_model = tracker.next_backoff_ms_with_model(Some(&forced), 0, 60.0, 0.5);
            assert_eq!(hardcoded, 400);
            // With zero-param trained model the prediction differs
            // from the formula — invariant is that the branch fires.
            assert_ne!(hardcoded, via_model);
        }

        #[test]
        fn tracker_record_with_model_matches_plain_when_untrained() {
            // Finding #5: record_restart_with_model(untrained) must
            // produce the same `backoff_ms` as plain record_restart.
            let model = crate::eml_kernel::RestartStrategyModel::new();
            let budget = RestartBudget::default();

            let mut baseline = RestartTracker::new();
            let mut wired = RestartTracker::new();

            for _ in 0..4 {
                baseline.record_restart(&budget);
                wired.record_restart_with_model(&budget, Some(&model), 0, 60.0, 0.5);
                assert_eq!(baseline.backoff_ms, wired.backoff_ms);
                assert_eq!(baseline.restart_count, wired.restart_count);
            }
        }

        // ── K1-G3: Resource enforcement tests ───────────────────

        #[test]
        fn resource_check_within_limits() {
            let usage = ResourceUsage {
                memory_bytes: 100,
                cpu_time_ms: 100,
                tool_calls: 10,
                messages_sent: 10,
            };
            let limits = ResourceLimits::default(); // 256 MiB, etc.
            let results = check_resource_usage(&usage, &limits);
            assert!(results.is_empty());
        }

        #[test]
        fn resource_check_warning_at_80_percent() {
            let usage = ResourceUsage {
                memory_bytes: 220 * 1024 * 1024, // ~86% of 256 MiB
                cpu_time_ms: 100,
                tool_calls: 10,
                messages_sent: 10,
            };
            let limits = ResourceLimits::default();
            let results = check_resource_usage(&usage, &limits);
            assert_eq!(results.len(), 1);
            assert!(
                matches!(&results[0], ResourceCheckResult::Warning { resource, .. } if resource == "memory")
            );
        }

        #[test]
        fn resource_check_exceeded_at_100_percent() {
            let usage = ResourceUsage {
                memory_bytes: 300 * 1024 * 1024, // >256 MiB
                cpu_time_ms: 100,
                tool_calls: 10,
                messages_sent: 10,
            };
            let limits = ResourceLimits::default();
            let results = check_resource_usage(&usage, &limits);
            assert_eq!(results.len(), 1);
            assert!(
                matches!(&results[0], ResourceCheckResult::Exceeded { resource, .. } if resource == "memory")
            );
        }

        #[test]
        fn resource_check_unlimited_skipped() {
            let usage = ResourceUsage {
                memory_bytes: 999_999_999,
                cpu_time_ms: 999_999_999,
                tool_calls: 999_999_999,
                messages_sent: 999_999_999,
            };
            let limits = ResourceLimits {
                max_memory_bytes: 0,
                max_cpu_time_ms: 0,
                max_tool_calls: 0,
                max_messages: 0,
                ..Default::default()
            };
            let results = check_resource_usage(&usage, &limits);
            assert!(results.is_empty(), "0 = unlimited should skip enforcement");
        }

        #[test]
        fn resource_check_multiple_exceeded() {
            let limits = ResourceLimits {
                max_memory_bytes: 100,
                max_cpu_time_ms: 100,
                max_tool_calls: 10,
                max_messages: 10,
                ..Default::default()
            };
            let usage = ResourceUsage {
                memory_bytes: 200,
                cpu_time_ms: 200,
                tool_calls: 20,
                messages_sent: 20,
            };
            let results = check_resource_usage(&usage, &limits);
            assert_eq!(results.len(), 4);
            for r in &results {
                assert!(matches!(r, ResourceCheckResult::Exceeded { .. }));
            }
        }

        #[test]
        fn resource_check_message_limit() {
            let limits = ResourceLimits {
                max_messages: 100,
                ..Default::default()
            };
            let usage = ResourceUsage {
                messages_sent: 100,
                ..Default::default()
            };
            let results = check_resource_usage(&usage, &limits);
            assert_eq!(results.len(), 1);
            assert!(
                matches!(&results[0], ResourceCheckResult::Exceeded { resource, .. } if resource == "messages")
            );
        }

        #[test]
        fn resource_check_cpu_time_limit() {
            let limits = ResourceLimits {
                max_cpu_time_ms: 1000,
                ..Default::default()
            };
            let usage = ResourceUsage {
                cpu_time_ms: 1000,
                ..Default::default()
            };
            let results = check_resource_usage(&usage, &limits);
            assert_eq!(results.len(), 1);
            assert!(
                matches!(&results[0], ResourceCheckResult::Exceeded { resource, .. } if resource == "cpu_time")
            );
        }

        // ── Sprint 10 W1: Self-healing tests ────────────────────

        #[test]
        fn restart_strategy_permanent_serde_roundtrip() {
            let strategy = RestartStrategy::Permanent;
            let json = serde_json::to_string(&strategy).unwrap();
            let restored: RestartStrategy = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, RestartStrategy::Permanent);
        }

        #[test]
        fn restart_strategy_transient_serde_roundtrip() {
            let strategy = RestartStrategy::Transient;
            let json = serde_json::to_string(&strategy).unwrap();
            let restored: RestartStrategy = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, RestartStrategy::Transient);
        }

        #[test]
        fn should_restart_permanent_never_restarts() {
            assert!(!RestartTracker::should_restart(
                &RestartStrategy::Permanent,
                0
            ));
            assert!(!RestartTracker::should_restart(
                &RestartStrategy::Permanent,
                1
            ));
            assert!(!RestartTracker::should_restart(
                &RestartStrategy::Permanent,
                -1
            ));
        }

        #[test]
        fn should_restart_transient_only_on_abnormal() {
            // Normal exit (0) should NOT restart
            assert!(!RestartTracker::should_restart(
                &RestartStrategy::Transient,
                0
            ));
            // Abnormal exits should restart
            assert!(RestartTracker::should_restart(
                &RestartStrategy::Transient,
                1
            ));
            assert!(RestartTracker::should_restart(
                &RestartStrategy::Transient,
                -1
            ));
            assert!(RestartTracker::should_restart(
                &RestartStrategy::Transient,
                42
            ));
        }

        #[test]
        fn should_restart_one_for_one_always() {
            assert!(RestartTracker::should_restart(
                &RestartStrategy::OneForOne,
                0
            ));
            assert!(RestartTracker::should_restart(
                &RestartStrategy::OneForOne,
                1
            ));
        }

        #[test]
        fn should_restart_one_for_all_always() {
            assert!(RestartTracker::should_restart(
                &RestartStrategy::OneForAll,
                0
            ));
            assert!(RestartTracker::should_restart(
                &RestartStrategy::OneForAll,
                -1
            ));
        }

        #[test]
        fn should_restart_rest_for_one_always() {
            assert!(RestartTracker::should_restart(
                &RestartStrategy::RestForOne,
                0
            ));
            assert!(RestartTracker::should_restart(
                &RestartStrategy::RestForOne,
                1
            ));
        }

        #[test]
        fn tracker_is_exhausted_when_at_max() {
            let mut tracker = RestartTracker::new();
            let budget = RestartBudget {
                max_restarts: 3,
                within_secs: 60,
            };

            assert!(!tracker.is_exhausted(&budget));

            tracker.record_restart(&budget); // 1
            tracker.record_restart(&budget); // 2
            assert!(!tracker.is_exhausted(&budget));

            tracker.record_restart(&budget); // 3 == max
            assert!(tracker.is_exhausted(&budget));
        }

        #[test]
        fn tracker_remaining_decreases() {
            let mut tracker = RestartTracker::new();
            let budget = RestartBudget {
                max_restarts: 5,
                within_secs: 60,
            };

            assert_eq!(tracker.remaining(&budget), 5);
            tracker.record_restart(&budget);
            assert_eq!(tracker.remaining(&budget), 4);
            tracker.record_restart(&budget);
            assert_eq!(tracker.remaining(&budget), 3);
        }

        #[test]
        fn tracker_remaining_saturates_at_zero() {
            let mut tracker = RestartTracker::new();
            let budget = RestartBudget {
                max_restarts: 2,
                within_secs: 60,
            };

            tracker.record_restart(&budget); // 1
            tracker.record_restart(&budget); // 2
            assert_eq!(tracker.remaining(&budget), 0);
            tracker.record_restart(&budget); // 3 (over budget)
            assert_eq!(tracker.remaining(&budget), 0);
        }

        #[test]
        fn tracker_backoff_sequence_matches_spec() {
            // Verify the exact sequence: 100, 200, 400, 800, 1600, 3200, ...
            let mut tracker = RestartTracker::new();
            let budget = RestartBudget {
                max_restarts: 20,
                within_secs: 600,
            };

            let expected = [100, 200, 400, 800, 1600, 3200, 6400, 12800, 25600, 30000];
            for &exp in &expected {
                tracker.record_restart(&budget);
                assert_eq!(
                    tracker.backoff_ms, exp,
                    "restart #{}: expected {}ms, got {}ms",
                    tracker.restart_count, exp, tracker.backoff_ms
                );
            }
        }

        #[test]
        fn handle_exit_permanent_no_restart() {
            let sup =
                make_supervisor_with_strategy(RestartStrategy::Permanent, RestartBudget::default());
            let result = sup.spawn(simple_request("perm-agent")).unwrap();
            sup.process_table()
                .update_state(result.pid, ProcessState::Running)
                .unwrap();

            // Mark as exited
            let _ = sup
                .process_table()
                .update_state(result.pid, ProcessState::Exited(1));

            let restarts = sup.handle_exit(result.pid, 1);
            assert!(
                restarts.is_empty(),
                "Permanent strategy should never restart"
            );
        }

        #[test]
        fn handle_exit_transient_normal_no_restart() {
            let sup =
                make_supervisor_with_strategy(RestartStrategy::Transient, RestartBudget::default());
            let result = sup.spawn(simple_request("trans-agent")).unwrap();
            sup.process_table()
                .update_state(result.pid, ProcessState::Running)
                .unwrap();

            let _ = sup
                .process_table()
                .update_state(result.pid, ProcessState::Exited(0));

            let restarts = sup.handle_exit(result.pid, 0);
            assert!(
                restarts.is_empty(),
                "Transient should not restart on normal exit (code 0)"
            );
        }

        #[test]
        fn handle_exit_transient_abnormal_restarts() {
            let sup = make_supervisor_with_strategy(
                RestartStrategy::Transient,
                RestartBudget {
                    max_restarts: 5,
                    within_secs: 60,
                },
            );
            let result = sup.spawn(simple_request("trans-crash")).unwrap();
            sup.process_table()
                .update_state(result.pid, ProcessState::Running)
                .unwrap();

            let _ = sup
                .process_table()
                .update_state(result.pid, ProcessState::Exited(1));

            let restarts = sup.handle_exit(result.pid, 1);
            assert_eq!(
                restarts.len(),
                1,
                "Transient should restart on abnormal exit"
            );
            assert_eq!(restarts[0].0, result.pid);
        }

        #[test]
        fn handle_exit_one_for_one_restarts_only_failed() {
            let sup = make_supervisor_with_strategy(
                RestartStrategy::OneForOne,
                RestartBudget {
                    max_restarts: 10,
                    within_secs: 60,
                },
            );
            let r1 = sup.spawn(simple_request("ofo-a")).unwrap();
            let r2 = sup.spawn(simple_request("ofo-b")).unwrap();
            sup.process_table()
                .update_state(r1.pid, ProcessState::Running)
                .unwrap();
            sup.process_table()
                .update_state(r2.pid, ProcessState::Running)
                .unwrap();

            let _ = sup
                .process_table()
                .update_state(r1.pid, ProcessState::Exited(1));

            let restarts = sup.handle_exit(r1.pid, 1);
            // Only r1 should be restarted
            assert_eq!(restarts.len(), 1);
            assert_eq!(restarts[0].0, r1.pid);

            // r2 should still be running
            let r2_entry = sup.inspect(r2.pid).unwrap();
            assert_eq!(r2_entry.state, ProcessState::Running);
        }

        #[test]
        fn handle_exit_budget_exhausted_no_restart() {
            let sup = make_supervisor_with_strategy(
                RestartStrategy::OneForOne,
                RestartBudget {
                    max_restarts: 1,
                    within_secs: 60,
                },
            );

            // First agent and restart
            let r1 = sup.spawn(simple_request("budget-a")).unwrap();
            sup.process_table()
                .update_state(r1.pid, ProcessState::Running)
                .unwrap();
            let _ = sup
                .process_table()
                .update_state(r1.pid, ProcessState::Exited(1));

            let restarts1 = sup.handle_exit(r1.pid, 1);
            assert_eq!(restarts1.len(), 1, "first restart should succeed");

            // Second crash of same PID -- budget exhausted
            // Need to mark new process as exited too
            let new_pid = restarts1[0].1.pid;
            sup.process_table()
                .update_state(new_pid, ProcessState::Running)
                .unwrap();
            let _ = sup
                .process_table()
                .update_state(r1.pid, ProcessState::Exited(1));

            let restarts2 = sup.handle_exit(r1.pid, 1);
            assert!(
                restarts2.is_empty(),
                "budget should be exhausted after 1 restart"
            );
        }

        #[test]
        fn handle_exit_links_notify_monitor_registry() {
            let sup = make_supervisor_with_strategy(
                RestartStrategy::Permanent, // Don't restart, just test notification
                RestartBudget::default(),
            );
            let r1 = sup.spawn(simple_request("linked-a")).unwrap();
            let r2 = sup.spawn(simple_request("linked-b")).unwrap();

            // Link r1 and r2
            sup.monitor_registry().link(r1.pid, r2.pid);

            // Also set up a monitor
            let _ref_id = sup.monitor_registry().monitor(r2.pid, r1.pid);

            sup.process_table()
                .update_state(r1.pid, ProcessState::Running)
                .unwrap();
            let _ = sup
                .process_table()
                .update_state(r1.pid, ProcessState::Exited(1));

            // handle_exit should process links and monitors
            let _restarts = sup.handle_exit(r1.pid, 1);

            // After exit, r1 should no longer be linked to r2
            assert!(!sup.monitor_registry().is_linked(r1.pid, r2.pid));
            // Monitor on r1 should be cleaned up
            assert!(sup.monitor_registry().get_monitors(r1.pid).is_empty());
        }

        #[test]
        fn supervisor_with_restart_config_builder() {
            let process_table = Arc::new(ProcessTable::new(16));
            let bus = Arc::new(clawft_core::bus::MessageBus::new());
            let ipc = Arc::new(KernelIpc::new(bus));

            let sup: AgentSupervisor<clawft_platform::NativePlatform> =
                AgentSupervisor::new(process_table, ipc, AgentCapabilities::default())
                    .with_restart_config(
                        RestartStrategy::RestForOne,
                        RestartBudget {
                            max_restarts: 3,
                            within_secs: 30,
                        },
                    );

            assert_eq!(*sup.restart_strategy(), RestartStrategy::RestForOne);
            assert_eq!(sup.restart_budget().max_restarts, 3);
            assert_eq!(sup.restart_budget().within_secs, 30);
        }

        fn make_supervisor_with_strategy(
            strategy: RestartStrategy,
            budget: RestartBudget,
        ) -> AgentSupervisor<clawft_platform::NativePlatform> {
            let process_table = Arc::new(ProcessTable::new(32));
            let bus = Arc::new(clawft_core::bus::MessageBus::new());
            let ipc = Arc::new(KernelIpc::new(bus));
            AgentSupervisor::new(process_table, ipc, AgentCapabilities::default())
                .with_restart_config(strategy, budget)
        }
    }

    // ── Sprint 09a: serde roundtrip tests ────────────────────────

    #[test]
    fn spawn_backend_native_serde_roundtrip() {
        let backend = SpawnBackend::Native;
        let json = serde_json::to_string(&backend).unwrap();
        let _: SpawnBackend = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn spawn_backend_wasm_serde_roundtrip() {
        let backend = SpawnBackend::Wasm {
            module: PathBuf::from("/opt/modules/agent.wasm"),
        };
        let json = serde_json::to_string(&backend).unwrap();
        let restored: SpawnBackend = serde_json::from_str(&json).unwrap();
        assert!(matches!(restored, SpawnBackend::Wasm { .. }));
    }

    #[test]
    fn spawn_backend_container_serde_roundtrip() {
        let backend = SpawnBackend::Container {
            image: "ghcr.io/org/agent:v1".into(),
        };
        let json = serde_json::to_string(&backend).unwrap();
        let restored: SpawnBackend = serde_json::from_str(&json).unwrap();
        assert!(matches!(restored, SpawnBackend::Container { .. }));
    }

    #[test]
    fn spawn_backend_tee_serde_roundtrip() {
        let backend = SpawnBackend::Tee {
            enclave: EnclaveConfig {
                enclave_type: "sgx".into(),
            },
        };
        let json = serde_json::to_string(&backend).unwrap();
        let restored: SpawnBackend = serde_json::from_str(&json).unwrap();
        assert!(matches!(restored, SpawnBackend::Tee { .. }));
    }

    #[test]
    fn spawn_backend_remote_serde_roundtrip() {
        let backend = SpawnBackend::Remote {
            node_id: "node-42".into(),
        };
        let json = serde_json::to_string(&backend).unwrap();
        let restored: SpawnBackend = serde_json::from_str(&json).unwrap();
        assert!(matches!(restored, SpawnBackend::Remote { .. }));
    }

    #[test]
    fn enclave_config_serde_roundtrip() {
        let cfg = EnclaveConfig {
            enclave_type: "trustzone".into(),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let restored: EnclaveConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.enclave_type, "trustzone");
    }

    #[test]
    fn spawn_request_with_backend_serde_roundtrip() {
        let req = SpawnRequest {
            agent_id: "test-agent".into(),
            capabilities: None,
            parent_pid: Some(42),
            env: HashMap::from([("LOG_LEVEL".into(), "debug".into())]),
            backend: Some(SpawnBackend::Native),
        };
        let json = serde_json::to_string(&req).unwrap();
        let restored: SpawnRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.agent_id, "test-agent");
        assert_eq!(restored.parent_pid, Some(42));
        assert!(restored.backend.is_some());
    }

    #[test]
    fn spawn_request_minimal_json_deserializes() {
        let json = r#"{"agent_id":"minimal"}"#;
        let req: SpawnRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.agent_id, "minimal");
        assert!(req.capabilities.is_none());
        assert!(req.parent_pid.is_none());
        assert!(req.env.is_empty());
        assert!(req.backend.is_none());
    }
}
