//! Kernel daemon — persistent kernel process with Unix socket RPC.
//!
//! The daemon boots a [`Kernel`], then listens on a Unix domain socket
//! for JSON-RPC requests. This is the native transport layer; the
//! kernel itself is platform-agnostic and could be wrapped in
//! WebSocket, TCP, or `postMessage` for other environments.

use std::sync::{Arc, OnceLock};

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, UnixListener, UnixStream};
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

use crate::control::{ControlFlags, ControlIntent, ControlKind};

/// Daemon-wide control state shared between the boot wiring (which
/// registers flags as services come up) and the `control.*` RPC
/// handlers (which read + flip them). Set once at daemon boot.
struct DaemonControlState {
    /// The daemon's own node-id, used as the authority that owns
    /// every control intent published by this daemon.
    daemon_node_id: String,
    /// Per-target enable flags, shared with services that consult
    /// them in their main loops.
    flags: ControlFlags,
}

static DAEMON_CONTROL: OnceLock<Arc<DaemonControlState>> = OnceLock::new();

fn daemon_control() -> Option<Arc<DaemonControlState>> {
    DAEMON_CONTROL.get().cloned()
}

/// Daemon-wide handle to the LLM HTTP client. Set at boot if the
/// service spawns successfully; `None` otherwise. The `llm.prompt`
/// handler reads this and returns a clean error when unset rather
/// than panicking.
static DAEMON_LLM: OnceLock<Arc<clawft_service_llm::LlmClient>> = OnceLock::new();

fn daemon_llm() -> Option<Arc<clawft_service_llm::LlmClient>> {
    DAEMON_LLM.get().cloned()
}

/// Daemon-wide handle to the PTY-backed terminal manager. Set at boot;
/// the four `terminal.*` handlers read this. We don't register a
/// control flag for terminal — sessions are user-initiated (no
/// background traffic to gate) and the GUI's "close session" button is
/// the natural off-switch. If we ever grow auto-spawned sessions, the
/// control flag lives here next to `DAEMON_LLM`.
static DAEMON_TERMINAL: OnceLock<Arc<clawft_service_terminal::TerminalManager>> = OnceLock::new();

fn daemon_terminal() -> Option<Arc<clawft_service_terminal::TerminalManager>> {
    DAEMON_TERMINAL.get().cloned()
}

use clawft_kernel::{Kernel, KernelState};
use clawft_platform::NativePlatform;
use clawft_types::config::{Config, KernelConfig};

use crate::protocol::{
    self, AgentInspectResult, AgentSendParams, AgentSpawnParams, AgentSpawnResult, AgentStopParams,
    AgentRestartParams, ClusterJoinParams, ClusterLeaveParams, ClusterNodeInfo,
    ClusterStatusResult, CronAddParams, CronJobInfo, CronRemoveParams, IpcPublishParams,
    IpcSubscribeParams, IpcTopicInfo, KernelStatusResult, LogEntry, LogsParams, ProcessInfo,
    Request, Response, ServiceInfo,
};
#[cfg(feature = "exochain")]
use crate::protocol::{
    ChainEventInfo, ChainExportParams, ChainLocalParams, ChainStatusResult, ChainVerifyResult,
    ResourceInspectParams, ResourceNodeInfo, ResourceRankEntry, ResourceRankParams,
    ResourceScoreParams, ResourceScoreResult, ResourceStatsResult,
};

/// Fork the daemon into the background.
///
/// Spawns `weaver kernel start --foreground` as a detached child process,
/// redirecting stdout/stderr to the kernel log file. Writes the child PID
/// to the PID file. The parent process exits immediately after confirming
/// the daemon started.
pub fn daemonize(config_override: Option<&str>) -> anyhow::Result<()> {
    use std::process::Command;

    let runtime_dir = protocol::runtime_dir();
    std::fs::create_dir_all(&runtime_dir)?;

    let log_path = protocol::log_path();
    let pid_path = protocol::pid_path();

    // Check if already running
    if pid_path.exists()
        && let Ok(pid_str) = std::fs::read_to_string(&pid_path)
    {
        if let Ok(pid) = pid_str.trim().parse::<u32>() {
            // Check if process is alive
            let check = Command::new("kill").args(["-0", &pid.to_string()]).output();
            if check.map(|o| o.status.success()).unwrap_or(false) {
                anyhow::bail!("kernel already running (pid {pid})");
            }
        }
        // Stale PID file
        let _ = std::fs::remove_file(&pid_path);
    }

    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let log_err = log_file.try_clone()?;

    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.args(["kernel", "start", "--foreground"]);
    if let Some(cfg) = config_override {
        cmd.args(["--config", cfg]);
    }

    let child = cmd
        .stdout(log_file)
        .stderr(log_err)
        .stdin(std::process::Stdio::null())
        .spawn()?;

    let pid = child.id();
    std::fs::write(&pid_path, pid.to_string())?;

    println!("WeftOS kernel started (pid {pid})");
    println!("  Socket: {}", protocol::socket_path().display());
    println!("  Log:    {}", log_path.display());
    println!("  PID:    {}", pid_path.display());
    println!();
    println!("Use 'weaver kernel status' to check, 'weaver kernel attach' to view logs.");
    println!("Use 'weaver kernel stop' to shut down.");

    Ok(())
}

/// Run the kernel daemon in the foreground.
///
/// Boots the kernel, binds to a Unix socket, and serves requests
/// until shutdown is requested (via `kernel.shutdown` RPC or signal).
pub async fn run(config: Config, kernel_config: KernelConfig) -> anyhow::Result<()> {
    let socket_path = protocol::socket_path();

    // Clean up stale socket file
    if socket_path.exists() {
        // Try connecting to see if a daemon is already running
        if tokio::net::UnixStream::connect(&socket_path)
            .await
            .is_ok()
        {
            anyhow::bail!(
                "daemon already running (socket exists and is accepting connections: {})",
                socket_path.display()
            );
        }
        // Stale socket — remove it
        std::fs::remove_file(&socket_path)?;
        debug!("removed stale socket file");
    }

    // Ensure parent directory exists
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Boot kernel
    let platform = NativePlatform::new();
    let kernel = Kernel::boot(config, kernel_config, Arc::new(platform)).await?;
    let kernel = Arc::new(tokio::sync::RwLock::new(kernel));

    // Bootstrap daemon node identity. Loads `<runtime>/node.key`
    // (generates on first run, persists with 0600). Registers the
    // daemon's pubkey with the kernel's NodeRegistry so the substrate
    // publish gate can verify signatures and enforce the
    // `substrate/<node-id>/...` write prefix.
    let runtime_dir = socket_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let daemon_identity = crate::node_identity::load_or_generate(&runtime_dir)
        .map_err(|e| anyhow::anyhow!("daemon identity bootstrap: {e}"))?;
    {
        let k = kernel.read().await;
        let pubkey: [u8; 32] = daemon_identity.signing_key.verifying_key().to_bytes();
        k.node_registry().register(pubkey, Some("daemon".to_string()));
        info!(node_id = %daemon_identity.node_id, "daemon node registered");
        k.event_log().info(
            "node",
            format!("daemon node registered: {}", daemon_identity.node_id),
        );

        // Issue mesh-canonical write grants for every topic this
        // daemon will produce. Per `.planning/sensors/PIPELINE-PRIMITIVE-JOURNAL.md`
        // §R3.6 each `_derived/<topic>` subtree requires a separate
        // grant — this is the seam where they're stamped. MVP: the
        // daemon grants itself in-process; no signature, no
        // revocation. Future federated grants will require an issuer
        // signature checked at the RPC boundary.
        //
        // Topics included now (some are produced by services landing
        // in parallel work):
        // - `transcript`  — whisper STT output (this branch)
        // - `classify`    — speech/silence/keyword classifier (parallel)
        // - `terminal`    — terminal-output capture pipeline (parallel)
        //
        // Stamping all three here means the parallel branches don't
        // each need to touch this grant-issue path.
        for topic in ["transcript", "classify", "terminal"] {
            match k.node_registry().issue_derived_grant(
                daemon_identity.node_id.clone(),
                topic,
                clawft_kernel::GrantScope::TopicPrefix,
            ) {
                Ok(_) => {
                    info!(
                        node_id = %daemon_identity.node_id,
                        topic = %topic,
                        "derived-write grant issued"
                    );
                    k.event_log().info(
                        "node",
                        format!(
                            "derived-write grant: node={} topic={}",
                            daemon_identity.node_id, topic
                        ),
                    );
                }
                Err(e) => {
                    // Topic strings above are constants so this can't
                    // realistically fire — log + continue rather than
                    // abort daemon boot.
                    warn!(error = %e, topic = %topic, "derived-write grant issue failed");
                }
            }
        }
    }

    // Stash daemon-wide control state. Set once; the `control.*`
    // RPC handlers and the service-spawn path read it. Idempotent
    // re-set is impossible (OnceLock) — this must run exactly once
    // per daemon process.
    let control_flags = ControlFlags::new();
    {
        let _ = DAEMON_CONTROL.set(Arc::new(DaemonControlState {
            daemon_node_id: daemon_identity.node_id.clone(),
            flags: control_flags.clone(),
        }));
    }

    // Spawn the whisper STT service. Subscribes to the configured
    // ESP32-side mic pcm_chunk path, transcribes via the local
    // whisper.cpp HTTP service, publishes transcripts under the
    // daemon's own node prefix.
    //
    // Pre-register control flags before spawn so the service holds
    // shared `Arc<AtomicBool>` handles to flags the RPC handler can
    // flip.
    let source_node_id = std::env::var("WHISPER_INPUT_NODE_ID")
        .unwrap_or_else(|_| "n-bfc4cd".to_string());
    let pcm_chunk_target = format!("{source_node_id}/mic/pcm_chunk");
    let rms_target = format!("{source_node_id}/mic/rms");
    let whisper_service_flag =
        control_flags.register(ControlKind::Service, "whisper", true);
    let whisper_source_flag =
        control_flags.register(ControlKind::Sensor, &pcm_chunk_target, true);
    // RMS sensor isn't consumed by anything in-process today; the
    // flag still lives here so toggling it from the GUI publishes
    // the intent that the firmware will eventually subscribe to.
    let _rms_sensor_flag = control_flags.register(ControlKind::Sensor, &rms_target, true);

    // The classifier publishes one `Classification` per pcm_chunk
    // under the daemon's prefix. We compute its path here so the
    // whisper service can subscribe to it for its gate. Mesh-canonical
    // `_derived/...` is the eventual home (R3.0 / R3.2); for now we
    // single-tier under the daemon prefix and the mesh-gate agent
    // will move all derived paths together at integration time.
    let classify_output_path = format!(
        "substrate/{daemon}/derived/classify/{source}/mic",
        daemon = daemon_identity.node_id,
        source = source_node_id,
    );
    let classify_service_flag =
        control_flags.register(ControlKind::Service, "classify", true);

    let _whisper_handle: Option<clawft_service_whisper::WhisperService> = {
        let whisper_url = std::env::var(clawft_service_whisper::WHISPER_SERVICE_URL_ENV)
            .unwrap_or_else(|_| "http://127.0.0.1:8123".to_string());
        let input_path = format!(
            "substrate/{source_node_id}/sensor/mic/pcm_chunk"
        );
        // Mesh-canonical transcript path (R3.2). Source node is part
        // of the path so subscribers see one stable subtree across
        // leader handoff. The daemon issued itself a `transcript`
        // grant above; the gate consults the registry handed to the
        // service via config.
        let output_path_derived = format!(
            "substrate/_derived/transcript/{source_node_id}/mic",
        );
        // REMOVE AFTER PHASE 4: dual-publish for migration.
        // Old node-private path stays alive for one release so
        // existing in-tree subscribers (the Explorer's substrate
        // walk) keep working while consumers migrate to the
        // canonical path.
        let output_path_legacy = format!(
            "substrate/{daemon}/derived/transcript/{source}/mic",
            daemon = daemon_identity.node_id,
            source = source_node_id,
        );
        let node_registry = {
            let k = kernel.read().await;
            k.node_registry().clone()
        };
        let cfg = clawft_service_whisper::WhisperServiceConfig {
            window_ms: 2_000,
            retry_backoff: std::time::Duration::from_millis(500),
            node_id: daemon_identity.node_id.clone(),
            input_path: input_path.clone(),
            output_path_derived: output_path_derived.clone(),
            output_path_legacy: Some(output_path_legacy.clone()),
            service_enabled: Arc::clone(&whisper_service_flag),
            source_enabled: Arc::clone(&whisper_source_flag),
            node_registry,
            // Gate whisper on the classifier's output. The classifier
            // is spawned just below; we point the subscription at the
            // path the classifier will publish to. If the classifier
            // fails to spawn (or hasn't published yet), the gate
            // stays closed and no chunks are transcribed — that's
            // the safe default for a "speech detected" filter.
            classifier_input: Some(classify_output_path.clone()),
            gate_window_ms: 1_500,
        };
        let client_cfg = clawft_service_whisper::WhisperConfig {
            base_url: whisper_url.clone(),
            ..clawft_service_whisper::WhisperConfig::default()
        };
        let client = match clawft_service_whisper::WhisperClient::new(client_cfg) {
            Ok(c) => c,
            Err(e) => {
                warn!(
                    error = %e,
                    "whisper client init failed (continuing without STT)"
                );
                return Err(anyhow::anyhow!("whisper client init: {e}"));
            }
        };
        let substrate = {
            let k = kernel.read().await;
            k.substrate_service().clone()
        };
        match clawft_service_whisper::WhisperService::spawn(substrate, client, cfg) {
            Ok(svc) => {
                info!(
                    input = %input_path,
                    output = %output_path_derived,
                    legacy_output = %output_path_legacy,
                    whisper_url = %whisper_url,
                    "whisper service spawned (dual-publish: canonical + legacy)"
                );
                Some(svc)
            }
            Err(e) => {
                warn!(error = %e, "whisper service failed to spawn (continuing without STT)");
                None
            }
        }
    };

    // Spawn the audio-classifier Stage. Subscribes to the same
    // ESP32-side mic pcm_chunk path the whisper service consumes,
    // runs each window through an `EnergyClassifier` (RMS-threshold
    // VAD), and republishes a `Classification` value under the
    // daemon's prefix at `classify_output_path`. The whisper service
    // (configured above) subscribes to that path and uses it as a
    // speech-vs-silence gate so inference only runs on speech.
    //
    // The `ClassifierBackend` trait is the seam for the future
    // llama.cpp-hosted multi-class classifier (music / noise /
    // speech / silence / ...) — swapping the backend doesn't change
    // the wire shape, so neither the whisper gate nor any GUI
    // subscriber needs a code change.
    let _classify_handle: Option<clawft_service_classify::ClassifierService> = {
        let input_path = format!(
            "substrate/{source_node_id}/sensor/mic/pcm_chunk"
        );
        let cfg = clawft_service_classify::ClassifierServiceConfig {
            node_id: daemon_identity.node_id.clone(),
            source_node: source_node_id.clone(),
            input_path: input_path.clone(),
            output_path: classify_output_path.clone(),
            service_enabled: Arc::clone(&classify_service_flag),
            // Reuse the whisper-side source flag — the user's mental
            // model is "the mic source"; toggling that off should
            // disable both the classifier and the transcription path
            // since they consume the same source.
            source_enabled: Arc::clone(&whisper_source_flag),
        };
        let backend: Arc<dyn clawft_service_classify::ClassifierBackend> =
            Arc::new(clawft_service_classify::EnergyClassifier::from_env());
        let substrate = {
            let k = kernel.read().await;
            k.substrate_service().clone()
        };
        match clawft_service_classify::ClassifierService::spawn(substrate, backend, cfg) {
            Ok(svc) => {
                info!(
                    input = %input_path,
                    output = %classify_output_path,
                    "classifier service spawned (energy VAD)"
                );
                Some(svc)
            }
            Err(e) => {
                warn!(
                    error = %e,
                    "classifier service failed to spawn (whisper gate will \
                     stay closed and transcription will not run until the \
                     classifier publishes)"
                );
                None
            }
        }
    };

    // Spawn the LLM service handle. Unlike whisper this is a
    // request/response client — there's no background tokio task to
    // hold open, so we just construct the client (and one-shot
    // health probe in the background so a cold cache logs cleanly
    // without blocking boot).
    //
    // Pre-register the control flag so `control.set_enabled
    // {kind:"service", target:"llm"}` works the first time the
    // user toggles it from the GUI.
    let _llm_service_flag =
        control_flags.register(ControlKind::Service, "llm", true);
    {
        let llm_url = std::env::var(clawft_service_llm::LLM_SERVICE_URL_ENV)
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                clawft_service_llm::DEFAULT_LLM_SERVICE_URL.to_string()
            });
        let cfg = clawft_service_llm::LlmConfig {
            base_url: llm_url.clone(),
            ..clawft_service_llm::LlmConfig::default()
        };
        match clawft_service_llm::LlmClient::new(cfg) {
            Ok(client) => {
                let arc = Arc::new(client);
                // Background health probe — surfaces "llama-server is
                // down" in the log without making the daemon refuse to
                // boot when the local model service hasn't been
                // started yet.
                let probe = Arc::clone(&arc);
                tokio::spawn(async move {
                    if probe.wait_for_healthy().await {
                        info!(
                            url = %probe.config().base_url,
                            "llm service: healthy"
                        );
                    } else {
                        warn!(
                            url = %probe.config().base_url,
                            "llm service: health probe failed at boot \
                             (RPC will return a clean error per call)"
                        );
                    }
                });
                let _ = DAEMON_LLM.set(arc);
                info!(url = %llm_url, "llm service handle wired");
            }
            Err(e) => {
                warn!(
                    error = %e,
                    "llm client init failed — llm.prompt RPC will \
                     return 'service unavailable'"
                );
            }
        }
    }

    // Spawn the terminal manager. Empty registry — sessions appear on
    // demand via `terminal.spawn`. Construction is infallible (a
    // `DashMap::new()` under the hood); we still match on the result so
    // future fallible variants land cleanly.
    {
        let mgr = Arc::new(clawft_service_terminal::TerminalManager::new());
        let _ = DAEMON_TERMINAL.set(mgr);
        info!("terminal service wired (PTY-backed sessions hosted in daemon)");
    }

    // Publish the top-level UI sentinel for the terminal panel. Lives
    // at `substrate/<daemon-node>/ui/terminal` — the egui Explorer's
    // terminal viewer shape-matches on `{ "kind": "terminal" }`. By
    // convention, every top-level surface (chat-window agent picks a
    // sibling path) publishes its sentinel under
    // `substrate/<daemon-node>/ui/<name>` so the Explorer tree
    // surfaces them as siblings without further coordination.
    {
        let k = kernel.read().await;
        let substrate = k.substrate_service();
        let path = format!(
            "substrate/{}/ui/terminal",
            daemon_identity.node_id
        );
        let value = serde_json::json!({
            "kind": "terminal",
            "label": "Terminal",
            "updated_at_ms": crate::control::now_ms(),
        });
        if let Err(e) = substrate.publish_gated(
            Some(&daemon_identity.node_id),
            &path,
            value,
        ) {
            warn!(error = %e, path = %path, "terminal: ui sentinel publish failed");
        } else {
            debug!(path = %path, "terminal: ui sentinel published");
        }
    }

    // Publish initial control intents now that the daemon node is
    // registered and services are wired. The intents live under the
    // daemon's own prefix so `publish_gated` accepts them.
    {
        let k = kernel.read().await;
        let substrate = k.substrate_service();
        let initial = [
            (ControlKind::Service, "whisper".to_string(), "Whisper STT"),
            (ControlKind::Service, "llm".to_string(), "Local LLM"),
            (ControlKind::Service, "classify".to_string(), "Audio classifier"),
            (ControlKind::Sensor, pcm_chunk_target.clone(), "Mic PCM chunks"),
            (ControlKind::Sensor, rms_target.clone(), "Mic RMS summary"),
        ];
        for (kind, target, label) in &initial {
            let intent = ControlIntent {
                enabled: control_flags
                    .get(*kind, target)
                    .map(|f| f.load(std::sync::atomic::Ordering::SeqCst))
                    .unwrap_or(true),
                kind: *kind,
                target: target.clone(),
                label: (*label).to_string(),
                updated_at_ms: crate::control::now_ms(),
            };
            let path = crate::control::intent_path(
                &daemon_identity.node_id,
                *kind,
                target,
            );
            if let Err(e) = substrate.publish_gated(
                Some(&daemon_identity.node_id),
                &path,
                intent.to_value(),
            ) {
                warn!(error = %e, path = %path, "control: initial intent publish failed");
            } else {
                debug!(path = %path, "control: initial intent published");
            }
        }

        // Publish the chat-window sentinel so the GUI Explorer's chat
        // panel has a stable substrate mount point. Shape:
        // `{ "kind": "chat", "model": "<llama-server model name>" }`.
        // The model field is informational — the daemon's `llm.prompt`
        // handler picks the actual model server-side; this just makes
        // the choice visible in the Explorer's tree label and chat
        // header.
        let chat_path = format!(
            "substrate/{}/ui/chat",
            daemon_identity.node_id,
        );
        let model_name = daemon_llm()
            .map(|c| c.config().model.clone())
            .unwrap_or_else(|| {
                clawft_service_llm::DEFAULT_LLM_MODEL.to_string()
            });
        let chat_sentinel = serde_json::json!({
            "kind": "chat",
            "model": model_name,
        });
        if let Err(e) = substrate.publish_gated(
            Some(&daemon_identity.node_id),
            &chat_path,
            chat_sentinel,
        ) {
            warn!(error = %e, path = %chat_path, "ui: chat sentinel publish failed");
        } else {
            debug!(path = %chat_path, "ui: chat sentinel published");
        }
    }

    // Print boot banner. Lead with the build-id so every run makes
    // which binary is executing visible in the first stdout line —
    // disambiguates "is this the freshly-rebuilt daemon?" without
    // needing to grep `weaver --version` separately.
    {
        let k = kernel.read().await;
        println!(
            "weaver {} · git {} · built {}",
            env!("CARGO_PKG_VERSION"),
            env!("BUILD_GIT_HASH"),
            env!("BUILD_TIMESTAMP"),
        );
        info!(
            version = env!("CARGO_PKG_VERSION"),
            git = env!("BUILD_GIT_HASH"),
            built = env!("BUILD_TIMESTAMP"),
            "weaver build identity"
        );
        print!("{}", clawft_kernel::console::boot_banner());
        print!("{}", k.boot_log().format_all());
    }

    // Bind socket
    let listener = UnixListener::bind(&socket_path)?;
    info!(path = %socket_path.display(), "daemon listening");
    println!("Daemon listening on {}", socket_path.display());

    // Log daemon start to kernel event log
    {
        let k = kernel.read().await;
        k.event_log()
            .info("daemon", format!("listening on {}", socket_path.display()));
    }

    // Stream-window anchor: start configured topic anchors so every
    // `window_secs` window emits a `stream.window_commit` chain event
    // summarising traffic for audit. Each anchor subscribes via the
    // a2a topic router and holds its own BLAKE3 + counters.
    #[cfg(feature = "exochain")]
    let mut stream_anchors: Vec<clawft_kernel::TopicAnchor> = Vec::new();
    #[cfg(feature = "exochain")]
    {
        let k = kernel.read().await;
        if let Some(cfg) = k.kernel_config().anchor.as_ref()
            && cfg.enabled && !cfg.topics.is_empty() {
                let window = std::time::Duration::from_secs(cfg.window_secs.max(1));
                let a2a = k.a2a_router().clone();
                let chain = k.chain_manager().cloned();
                for pattern in &cfg.topics {
                    // Exact topics are wired directly; wildcard patterns
                    // like "sensor.*" are not expanded here — the
                    // config-writer provides the exact topic prefix
                    // they want to anchor, and the anchor currently
                    // subscribes by literal name. A wildcard anchor
                    // watchdog is a follow-up (M1.6+).
                    let topic = pattern.clone();
                    let anchor = clawft_kernel::StreamWindowAnchor::start_topic(
                        Arc::clone(&a2a),
                        chain.clone(),
                        topic.clone(),
                        window,
                    );
                    stream_anchors.push(anchor);
                }
                info!(
                    anchors = stream_anchors.len(),
                    window_ms = window.as_millis() as u64,
                    "stream-window anchors started"
                );
            }
    }

    // Shutdown signal
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Cron tick loop — fires overdue jobs every second
    let cron_kernel = Arc::clone(&kernel);
    let mut cron_shutdown_rx = shutdown_rx.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let k = cron_kernel.read().await;
                    let cron = k.cron_service();
                    let tick_result = cron.tick();

                    // Dispatch fired jobs via chain-logged A2ARouter
                    for job_id in &tick_result.fired {
                        if let Some(job) = cron.job_snapshot(job_id)
                            && let Some(target_pid) = job.target_pid
                        {
                            // Log cron.fire event first (records scheduling intent)
                            #[cfg(feature = "exochain")]
                            if let Some(cm) = k.chain_manager() {
                                cm.append(
                                    "cron",
                                    "cron.fire",
                                    Some(serde_json::json!({
                                        "job_id": job.id,
                                        "name": job.name,
                                        "fire_count": job.fire_count,
                                        "target_pid": job.target_pid,
                                    })),
                                );
                            }

                            // Dispatch via send_checked (logs ipc.send in chain)
                            let msg = clawft_kernel::KernelMessage::new(
                                0,
                                clawft_kernel::MessageTarget::Process(target_pid),
                                clawft_kernel::MessagePayload::Json(serde_json::json!({
                                    "cmd": job.command,
                                    "cron_job_id": job.id,
                                    "cron_job_name": job.name,
                                })),
                            );
                            let a2a = k.a2a_router().clone();

                            #[cfg(feature = "exochain")]
                            {
                                let chain = k.chain_manager();
                                if let Err(e) = a2a.send_checked(msg, chain.map(|c| c.as_ref())).await {
                                    warn!(job_id = %job.id, error = %e, "cron: failed to send to target");
                                }
                            }
                            #[cfg(not(feature = "exochain"))]
                            {
                                if let Err(e) = a2a.send(msg).await {
                                    warn!(job_id = %job.id, error = %e, "cron: failed to send to target");
                                }
                            }
                        }
                    }
                }
                _ = cron_shutdown_rx.changed() => {
                    if *cron_shutdown_rx.borrow() {
                        debug!("cron tick loop shutting down");
                        break;
                    }
                }
            }
        }
    });

    // Chain event bridge — drains non-kernel chain events and forwards to ChainManager
    #[cfg(feature = "exochain")]
    {
        let bridge_kernel = Arc::clone(&kernel);
        let mut bridge_shutdown_rx = shutdown_rx.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let pending = clawft_core::chain_event::drain_pending_chain_events();
                        if pending.is_empty() {
                            continue;
                        }
                        let k = bridge_kernel.read().await;
                        if let Some(cm) = k.chain_manager() {
                            for evt in pending {
                                cm.append(&evt.source, &evt.kind, evt.payload);
                            }
                        }
                    }
                    _ = bridge_shutdown_rx.changed() => {
                        if *bridge_shutdown_rx.borrow() {
                            // Final drain before shutdown
                            let pending = clawft_core::chain_event::drain_pending_chain_events();
                            if !pending.is_empty() {
                                let k = bridge_kernel.read().await;
                                if let Some(cm) = k.chain_manager() {
                                    for evt in pending {
                                        cm.append(&evt.source, &evt.kind, evt.payload);
                                    }
                                }
                            }
                            debug!("chain event bridge shutting down");
                            break;
                        }
                    }
                }
            }
        });
    }

    // Health monitor loop — periodic aggregate() with chain logging
    //
    // We access the kernel inside the loop but drop the RwLock guard before
    // any await on service health checks to avoid Send issues.
    let health_kernel = Arc::clone(&kernel);
    let mut health_shutdown_rx = shutdown_rx.clone();
    tokio::spawn(async move {
        let interval_secs = {
            let k = health_kernel.read().await;
            k.kernel_config().health_check_interval_secs
        };
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    // Snapshot services as concrete Vec (releases DashMap refs)
                    let (services_snapshot, event_log) = {
                        let k = health_kernel.read().await;
                        let snapshot = k.services().snapshot();
                        let el = k.event_log().clone();
                        (snapshot, el)
                    };
                    #[cfg(feature = "exochain")]
                    let chain = {
                        let k = health_kernel.read().await;
                        k.chain_manager().cloned()
                    };

                    // Run health checks outside the lock (no DashMap borrow)
                    let mut results = Vec::new();
                    let mut unhealthy_names = Vec::new();
                    let mut all_unhealthy = true;

                    for (name, svc) in &services_snapshot {
                        let status = svc.health_check().await;
                        match &status {
                            clawft_kernel::HealthStatus::Healthy => {
                                all_unhealthy = false;
                            }
                            clawft_kernel::HealthStatus::Degraded(msg) => {
                                event_log.warn("health", format!("{name}: degraded - {msg}"));
                                unhealthy_names.push(name.clone());
                                all_unhealthy = false;
                            }
                            clawft_kernel::HealthStatus::Unhealthy(msg) => {
                                event_log.error("health", format!("{name}: unhealthy - {msg}"));
                                unhealthy_names.push(name.clone());
                            }
                            clawft_kernel::HealthStatus::Unknown => {
                                unhealthy_names.push(name.clone());
                            }
                            _ => {
                                event_log.warn("health", format!("{name}: unrecognized health status"));
                                unhealthy_names.push(name.clone());
                            }
                        }
                        results.push((name.clone(), status));
                    }

                    let overall = if services_snapshot.is_empty() {
                        clawft_kernel::OverallHealth::Down
                    } else if unhealthy_names.is_empty() {
                        clawft_kernel::OverallHealth::Healthy
                    } else if all_unhealthy {
                        clawft_kernel::OverallHealth::Down
                    } else {
                        clawft_kernel::OverallHealth::Degraded {
                            unhealthy_services: unhealthy_names,
                        }
                    };

                    // Chain event (exochain)
                    #[cfg(feature = "exochain")]
                    if let Some(ref cm) = chain {
                        cm.append("health", "health.check", Some(serde_json::json!({
                            "overall": overall.to_string(),
                            "services": results.len(),
                        })));
                    }
                    let _ = overall; // suppress unused warning when exochain is off
                }
                _ = health_shutdown_rx.changed() => {
                    if *health_shutdown_rx.borrow() {
                        debug!("health monitor loop shutting down");
                        break;
                    }
                }
            }
        }
    });

    // Watchdog loop — sweep finished JoinHandles every 5 seconds
    let watchdog_kernel = Arc::clone(&kernel);
    let mut watchdog_shutdown_rx = shutdown_rx.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let k = watchdog_kernel.read().await;
                    let reaped = k.supervisor().watchdog_sweep().await;
                    for (pid, code) in &reaped {
                        k.event_log().warn("watchdog", format!("reaped PID {pid} (exit code {code})"));
                    }
                }
                _ = watchdog_shutdown_rx.changed() => {
                    if *watchdog_shutdown_rx.borrow() {
                        debug!("watchdog loop shutting down");
                        break;
                    }
                }
            }
        }
    });

    // Accept loop — clone shutdown_tx so the outer scope can still use it for Ctrl+C
    let accept_kernel = Arc::clone(&kernel);
    let rpc_shutdown_tx = shutdown_tx.clone();
    let mut accept_handle = tokio::spawn(async move {
        let mut shutdown_rx = shutdown_rx;
        loop {
            tokio::select! {
                result = listener.accept() => {
                    match result {
                        Ok((stream, _addr)) => {
                            let k = Arc::clone(&accept_kernel);
                            let tx = rpc_shutdown_tx.clone();
                            tokio::spawn(handle_connection(stream, k, tx));
                        }
                        Err(e) => {
                            error!("accept error: {e}");
                        }
                    }
                }
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("shutdown signal received, stopping accept loop");
                        break;
                    }
                }
            }
        }
    });

    // Optional TCP relay. When `[kernel.ipc_tcp]` is enabled, every
    // accepted TCP connection is transparently byte-copied to a fresh
    // connection on the unix socket. All auth / JSON dispatch stays in
    // the unix path — the TCP side is a dumb conduit so cross-boundary
    // callers (Windows side of WSL, remote bridges) can reach the RPC
    // without speaking `AF_UNIX`.
    let ipc_tcp_cfg = {
        let k = kernel.read().await;
        k.kernel_config().ipc_tcp.clone()
    };
    if let Some(cfg) = ipc_tcp_cfg.filter(|c| c.enabled) {
        match TcpListener::bind(&cfg.listen_addr).await {
            Ok(tcp_listener) => {
                info!(addr = %cfg.listen_addr, "ipc tcp relay listening");
                println!("IPC TCP relay listening on {}", cfg.listen_addr);
                let relay_socket_path = socket_path.clone();
                let mut relay_shutdown_rx = shutdown_tx.subscribe();
                tokio::spawn(async move {
                    loop {
                        tokio::select! {
                            result = tcp_listener.accept() => {
                                match result {
                                    Ok((tcp_stream, peer)) => {
                                        info!(peer = %peer, "ipc tcp relay: peer connected");
                                        let sock = relay_socket_path.clone();
                                        tokio::spawn(async move {
                                            match UnixStream::connect(&sock).await {
                                                Ok(mut unix_stream) => {
                                                    let mut tcp_stream = tcp_stream;
                                                    let (a, b) = match tokio::io::copy_bidirectional(
                                                        &mut tcp_stream,
                                                        &mut unix_stream,
                                                    )
                                                    .await
                                                    {
                                                        Ok(pair) => pair,
                                                        Err(e) => {
                                                            debug!(peer = %peer, "ipc tcp relay: copy ended: {e}");
                                                            (0, 0)
                                                        }
                                                    };
                                                    info!(peer = %peer, tx_bytes = a, rx_bytes = b, "ipc tcp relay: peer disconnected");
                                                }
                                                Err(e) => {
                                                    warn!(peer = %peer, "ipc tcp relay: unix connect failed: {e}");
                                                }
                                            }
                                        });
                                    }
                                    Err(e) => {
                                        error!("ipc tcp accept error: {e}");
                                    }
                                }
                            }
                            _ = relay_shutdown_rx.changed() => {
                                if *relay_shutdown_rx.borrow() {
                                    debug!("ipc tcp relay shutting down");
                                    break;
                                }
                            }
                        }
                    }
                });
            }
            Err(e) => {
                warn!(addr = %cfg.listen_addr, "ipc tcp relay bind failed: {e}");
            }
        }
    }

    // Wait for shutdown signal (SIGINT, SIGTERM, SIGHUP) or RPC shutdown.
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut sighup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())?;

    let restart_requested = tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("SIGINT received, shutting down daemon");
            let _ = shutdown_tx.send(true);
            false
        }
        _ = sigterm.recv() => {
            info!("SIGTERM received, shutting down daemon");
            let _ = shutdown_tx.send(true);
            false
        }
        _ = sighup.recv() => {
            info!("SIGHUP received — will restart after shutdown");
            let _ = shutdown_tx.send(true);
            true
        }
        _ = &mut accept_handle => {
            // Accept loop finished (shutdown requested via RPC)
            info!("accept loop finished (RPC shutdown)");
            false
        }
    };

    // If a signal triggered shutdown, wait for the accept loop to finish.
    if restart_requested || !accept_handle.is_finished() {
        let _ = accept_handle.await;
    }

    // Stop stream-window anchors so they flush their final windows
    // before the chain manager shuts down.
    #[cfg(feature = "exochain")]
    {
        for anchor in stream_anchors {
            anchor.shutdown();
        }
    }

    // Gracefully shut down running agents before kernel shutdown
    {
        let k = kernel.read().await;
        let results = k.supervisor().shutdown_all(std::time::Duration::from_secs(5)).await;
        for (pid, code) in &results {
            k.event_log().info("shutdown", format!("agent PID {pid} exited with code {code}"));
        }
    }

    // Shut down kernel
    {
        let mut k = kernel.write().await;
        if let Err(e) = k.shutdown().await {
            warn!("kernel shutdown error: {e}");
        }
    }

    // Clean up socket and PID file
    if socket_path.exists() {
        let _ = std::fs::remove_file(&socket_path);
    }
    let pid_path = protocol::pid_path();
    if pid_path.exists() {
        let _ = std::fs::remove_file(&pid_path);
    }

    println!("Daemon stopped.");

    // If SIGHUP requested restart, re-exec the binary (keeps same PID for systemd).
    if restart_requested {
        info!("re-exec for restart");
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new(std::env::current_exe()?)
            .args(["kernel", "start", "--foreground"])
            .exec(); // replaces process; only reached on error
        eprintln!("re-exec failed: {err}");
        std::process::exit(1);
    }

    Ok(())
}

/// Handle a single client connection — accepts both JSON line mode and
/// (when the `rvf-rpc` feature is enabled) the RVF-framed protocol.
///
/// Detects the connection mode by reading the first 4 bytes:
///   - `RVFS` → RVF-framed protocol (content-hash verified segments)
///   - anything else → legacy line-delimited JSON (bytes prepended to first line)
///
/// Exposed `pub` so integration tests can drive a preassembled kernel
/// directly without the signal-handler plumbing in [`run`].
pub async fn handle_connection(
    mut stream: tokio::net::UnixStream,
    kernel: Arc<tokio::sync::RwLock<Kernel<NativePlatform>>>,
    shutdown_tx: watch::Sender<bool>,
) {
    // Read 4-byte header to detect protocol mode.
    let mut header = [0u8; 4];
    if stream.read_exact(&mut header).await.is_err() {
        return; // connection closed immediately
    }

    #[cfg(feature = "rvf-rpc")]
    if &header == b"RVFS" {
        return handle_rvf_connection(stream, kernel, shutdown_tx).await;
    }

    // JSON mode: the 4 header bytes are the start of the first JSON line.
    handle_json_connection(header, stream, kernel, shutdown_tx).await;
}

/// Outcome of dispatching a single JSON-line request.
///
/// Most requests produce a single response and the connection loop
/// continues reading the next line. `ipc.subscribe_stream` and the
/// other `*.subscribe` stream RPCs are different: after the initial
/// ack is written, the write-half is owned by a streaming forwarder
/// task that pushes every matching publish as one JSON line.
enum DispatchOutcome {
    /// Keep reading subsequent requests from the same connection.
    Continue,
    /// Transport error — close the connection.
    Stop,
    /// The request subscribed to a kernel topic; the writer must be
    /// transferred into a streaming forwarder. The `rx` channel holds
    /// the raw JSON lines to flush to the client; on client disconnect
    /// (write error) the caller must call `on_disconnect` to clean up.
    StreamSubscribe {
        /// Topic that was subscribed to (for logging).
        topic: String,
        /// Receiver for serialized topic messages (JSON + trailing `\n`).
        rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
        /// Unsubscribe action to run when the client disconnects.
        on_disconnect: Box<dyn FnOnce() + Send>,
    },
}

/// Dispatch a single JSON line and write the response.
///
/// Returns the next [`DispatchOutcome`] for the connection loop.
async fn dispatch_json_line(
    line: &str,
    kernel: &Arc<tokio::sync::RwLock<Kernel<NativePlatform>>>,
    shutdown_tx: &watch::Sender<bool>,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
) -> DispatchOutcome {
    let line = line.trim();
    if line.is_empty() {
        return DispatchOutcome::Continue;
    }

    let (response, stream_hookup) = match serde_json::from_str::<Request>(line) {
        Ok(req) => {
            let id = req.id.clone();
            // `*.subscribe_stream` methods take over the connection: the
            // daemon registers an external sink with the router and
            // returns a receiver the caller pipes into the socket.
            if req.method == "ipc.subscribe_stream" {
                match handle_ipc_subscribe_stream(req.params, Arc::clone(kernel)).await {
                    Ok((ack, topic, rx, on_disconnect)) => (
                        ack.with_id(id),
                        Some((topic, rx, on_disconnect)),
                    ),
                    Err(msg) => (Response::error(msg).with_id(id), None),
                }
            } else if req.method == "substrate.subscribe" {
                match handle_substrate_subscribe(req.params, Arc::clone(kernel)).await {
                    Ok((ack, path, rx, on_disconnect)) => (
                        ack.with_id(id),
                        Some((path, rx, on_disconnect)),
                    ),
                    Err(msg) => (Response::error(msg).with_id(id), None),
                }
            } else {
                (
                    dispatch(req.method, req.params, Arc::clone(kernel), shutdown_tx.clone())
                        .await
                        .with_id(id),
                    None,
                )
            }
        }
        Err(e) => (Response::error(format!("invalid request: {e}")), None),
    };

    let mut json = serde_json::to_string(&response).unwrap_or_else(|e| {
        serde_json::to_string(&Response::error(format!("serialize error: {e}"))).unwrap()
    });
    json.push('\n');

    if let Err(e) = writer.write_all(json.as_bytes()).await {
        debug!("write error (client disconnected?): {e}");
        if let Some((_, _, on_disconnect)) = stream_hookup {
            on_disconnect();
        }
        return DispatchOutcome::Stop;
    }

    if let Some((topic, rx, on_disconnect)) = stream_hookup {
        return DispatchOutcome::StreamSubscribe {
            topic,
            rx,
            on_disconnect,
        };
    }

    DispatchOutcome::Continue
}

/// Handle a legacy line-delimited JSON connection.
///
/// The `prefix` bytes were consumed during protocol detection and form
/// the beginning of the first JSON line on the wire.
async fn handle_json_connection(
    prefix: [u8; 4],
    stream: tokio::net::UnixStream,
    kernel: Arc<tokio::sync::RwLock<Kernel<NativePlatform>>>,
    shutdown_tx: watch::Sender<bool>,
) {
    let (reader, mut writer) = stream.into_split();
    let mut buf = BufReader::new(reader);

    // Reconstruct the first line: the 4-byte prefix + the rest until '\n'.
    let mut rest_of_first = String::new();
    if buf.read_line(&mut rest_of_first).await.is_err() {
        return;
    }
    let first_line = format!("{}{}", String::from_utf8_lossy(&prefix), rest_of_first);
    match dispatch_json_line(&first_line, &kernel, &shutdown_tx, &mut writer).await {
        DispatchOutcome::Continue => {}
        DispatchOutcome::Stop => return,
        DispatchOutcome::StreamSubscribe {
            topic,
            rx,
            on_disconnect,
        } => {
            run_stream_subscribe(writer, topic, rx, on_disconnect).await;
            return;
        }
    }

    // Process remaining lines normally.
    loop {
        let mut line = String::new();
        match buf.read_line(&mut line).await {
            Ok(0) => break, // EOF
            Ok(_) => {}
            Err(_) => break,
        }
        match dispatch_json_line(&line, &kernel, &shutdown_tx, &mut writer).await {
            DispatchOutcome::Continue => {}
            DispatchOutcome::Stop => break,
            DispatchOutcome::StreamSubscribe {
                topic,
                rx,
                on_disconnect,
            } => {
                run_stream_subscribe(writer, topic, rx, on_disconnect).await;
                return;
            }
        }
    }
}

/// Pump serialized topic messages from `rx` into the client writer.
///
/// Runs until the receiver closes (router-side shutdown) or the
/// writer returns an I/O error (client disconnected). On exit, runs
/// the provided `on_disconnect` closure so the daemon can remove the
/// external subscription from the router.
async fn run_stream_subscribe(
    mut writer: tokio::net::unix::OwnedWriteHalf,
    topic: String,
    mut rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
    on_disconnect: Box<dyn FnOnce() + Send>,
) {
    debug!(topic, "ipc.subscribe_stream: forwarder started");
    while let Some(bytes) = rx.recv().await {
        if let Err(e) = writer.write_all(&bytes).await {
            debug!(topic, error = %e, "ipc.subscribe_stream: write error, closing");
            break;
        }
    }
    debug!(topic, "ipc.subscribe_stream: forwarder exiting");
    on_disconnect();
}

/// Decode a 32-byte or 64-byte key/signature from either hex or
/// standard base64. Permissive on purpose — Python bridges emit hex,
/// JS bridges often emit base64, and both are safe to accept.
fn decode_bytes(s: &str) -> Result<Vec<u8>, String> {
    let trimmed = s.trim();
    // Try hex first (even-length, all hex digits)
    if !trimmed.is_empty()
        && trimmed.len().is_multiple_of(2)
        && trimmed.chars().all(|c| c.is_ascii_hexdigit())
    {
        let mut out = Vec::with_capacity(trimmed.len() / 2);
        for i in (0..trimmed.len()).step_by(2) {
            let byte = u8::from_str_radix(&trimmed[i..i + 2], 16)
                .map_err(|e| format!("hex decode: {e}"))?;
            out.push(byte);
        }
        return Ok(out);
    }
    // Fall back to base64 (permissive: accept both URL-safe and standard).
    base64_decode_permissive(trimmed)
}

fn base64_decode_permissive(s: &str) -> Result<Vec<u8>, String> {
    // Minimal base64 decoder — sufficient for 32/64 byte keys. Accepts
    // both `+/` and `-_` alphabets. Padding `=` optional.
    let mut buf: u32 = 0;
    let mut bits: u8 = 0;
    let mut out = Vec::with_capacity((s.len() * 3) / 4 + 2);
    for c in s.chars() {
        if c == '=' {
            break;
        }
        let v: u32 = match c {
            'A'..='Z' => (c as u32) - ('A' as u32),
            'a'..='z' => (c as u32) - ('a' as u32) + 26,
            '0'..='9' => (c as u32) - ('0' as u32) + 52,
            '+' | '-' => 62,
            '/' | '_' => 63,
            '\n' | '\r' | ' ' | '\t' => continue,
            _ => return Err(format!("invalid base64 char: {c:?}")),
        };
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xFF) as u8);
        }
    }
    Ok(out)
}

/// Verify an Ed25519 signature for a given node_id against the
/// canonical signed payload. Returns Ok on valid, Err with a message
/// otherwise. Always rejects if the node_id is not registered.
///
/// Mirrors [`verify_agent_signature`] but consults the
/// [`clawft_kernel::NodeRegistry`] instead of the agent registry.
fn verify_node_signature(
    kernel: &Kernel<NativePlatform>,
    node_id: &str,
    signature_str: &str,
    payload: &[u8],
) -> Result<(), String> {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};

    let node = kernel
        .node_registry()
        .get(node_id)
        .ok_or_else(|| format!("unknown node_id: {node_id}"))?;
    let sig_bytes = decode_bytes(signature_str).map_err(|e| format!("signature: {e}"))?;
    if sig_bytes.len() != 64 {
        return Err(format!(
            "signature must be 64 bytes, got {}",
            sig_bytes.len()
        ));
    }
    let mut sig_arr = [0u8; 64];
    sig_arr.copy_from_slice(&sig_bytes);
    let sig = Signature::from_bytes(&sig_arr);
    let vk = VerifyingKey::from_bytes(&node.pubkey)
        .map_err(|e| format!("stored pubkey invalid: {e}"))?;
    vk.verify(payload, &sig)
        .map_err(|e| format!("signature verify failed: {e}"))?;
    Ok(())
}

/// Verify an Ed25519 signature for a given agent_id against the
/// canonical signed payload. Returns Ok on valid, Err with a message
/// otherwise. Always rejects if the agent_id is not registered.
fn verify_agent_signature(
    kernel: &Kernel<NativePlatform>,
    actor_id: &str,
    signature_str: &str,
    payload: &[u8],
) -> Result<(), String> {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};

    let agent = kernel
        .agent_registry()
        .get(actor_id)
        .ok_or_else(|| format!("unknown actor_id: {actor_id}"))?;
    let sig_bytes = decode_bytes(signature_str).map_err(|e| format!("signature: {e}"))?;
    if sig_bytes.len() != 64 {
        return Err(format!(
            "signature must be 64 bytes, got {}",
            sig_bytes.len()
        ));
    }
    let mut sig_arr = [0u8; 64];
    sig_arr.copy_from_slice(&sig_bytes);
    let sig = Signature::from_bytes(&sig_arr);
    let vk = VerifyingKey::from_bytes(&agent.pubkey)
        .map_err(|e| format!("stored pubkey invalid: {e}"))?;
    vk.verify(payload, &sig)
        .map_err(|e| format!("signature verify failed: {e}"))?;
    Ok(())
}

/// Handle `agent.register`: verify proof-of-possession, insert the
/// agent into the registry, and chain-append an `agent.registered`
/// event.
async fn handle_agent_register(
    params: serde_json::Value,
    kernel: Arc<tokio::sync::RwLock<Kernel<NativePlatform>>>,
) -> Response {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};

    let p: crate::protocol::AgentRegisterParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(e) => return Response::error(format!("invalid params: {e}")),
    };

    // Decode pubkey + proof.
    let pubkey_bytes = match decode_bytes(&p.pubkey) {
        Ok(b) => b,
        Err(e) => return Response::error(format!("pubkey decode: {e}")),
    };
    if pubkey_bytes.len() != 32 {
        return Response::error(format!(
            "pubkey must be 32 bytes, got {}",
            pubkey_bytes.len()
        ));
    }
    let mut pubkey_arr = [0u8; 32];
    pubkey_arr.copy_from_slice(&pubkey_bytes);

    let proof_bytes = match decode_bytes(&p.proof) {
        Ok(b) => b,
        Err(e) => return Response::error(format!("proof decode: {e}")),
    };
    if proof_bytes.len() != 64 {
        return Response::error(format!(
            "proof must be 64 bytes, got {}",
            proof_bytes.len()
        ));
    }
    let mut proof_arr = [0u8; 64];
    proof_arr.copy_from_slice(&proof_bytes);

    // Verify the proof-of-possession.
    let payload = clawft_kernel::register_payload(&p.name, &pubkey_arr, p.ts);
    let vk = match VerifyingKey::from_bytes(&pubkey_arr) {
        Ok(vk) => vk,
        Err(e) => return Response::error(format!("invalid pubkey: {e}")),
    };
    let sig = Signature::from_bytes(&proof_arr);
    if let Err(e) = vk.verify(&payload, &sig) {
        return Response::error(format!("proof-of-possession verify failed: {e}"));
    }

    let k = kernel.read().await;
    let entry = k.agent_registry().register(p.name.clone(), pubkey_arr);

    #[cfg(feature = "exochain")]
    if let Some(cm) = k.chain_manager() {
        cm.append(
            "agent",
            "agent.registered",
            Some(serde_json::json!({
                "agent_id": entry.agent_id,
                "name": entry.name,
                "pubkey_hex": hex_encode(&entry.pubkey),
                "registered_at": entry.registered_at.to_rfc3339(),
            })),
        );
    }

    let pubkey_prefix = hex_encode(&entry.pubkey)[..16.min(entry.pubkey.len() * 2)].to_string();
    k.event_log().info(
        "agent",
        format!(
            "registered agent '{}' as {} (pubkey prefix {})",
            entry.name, entry.agent_id, pubkey_prefix
        ),
    );
    info!(
        agent_id = %entry.agent_id,
        name = %entry.name,
        pubkey_prefix = %pubkey_prefix,
        "agent.register: authorized"
    );

    Response::success(
        serde_json::to_value(crate::protocol::AgentRegisterResult {
            agent_id: entry.agent_id,
            name: entry.name,
        })
        .unwrap(),
    )
}

/// Handle `node.register`: verify proof-of-possession, insert the
/// node into the registry, and chain-append a `node.registered`
/// event. Returns the deterministic node-id derived from the pubkey.
///
/// Distinct from `agent.register`: a node is a *physical thing in
/// the mesh* (ESP32, daemon, Pi), not an agent/program/user. Nodes
/// sign substrate emissions; agents sign Actions. Both share the
/// proof-of-possession shape but live in disjoint registries.
async fn handle_node_register(
    params: serde_json::Value,
    kernel: Arc<tokio::sync::RwLock<Kernel<NativePlatform>>>,
) -> Response {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};

    let p: crate::protocol::NodeRegisterParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(e) => return Response::error(format!("invalid params: {e}")),
    };

    // Decode pubkey + proof.
    let pubkey_bytes = match decode_bytes(&p.pubkey) {
        Ok(b) => b,
        Err(e) => return Response::error(format!("pubkey decode: {e}")),
    };
    if pubkey_bytes.len() != 32 {
        return Response::error(format!(
            "pubkey must be 32 bytes, got {}",
            pubkey_bytes.len()
        ));
    }
    let mut pubkey_arr = [0u8; 32];
    pubkey_arr.copy_from_slice(&pubkey_bytes);

    let proof_bytes = match decode_bytes(&p.proof) {
        Ok(b) => b,
        Err(e) => return Response::error(format!("proof decode: {e}")),
    };
    if proof_bytes.len() != 64 {
        return Response::error(format!(
            "proof must be 64 bytes, got {}",
            proof_bytes.len()
        ));
    }
    let mut proof_arr = [0u8; 64];
    proof_arr.copy_from_slice(&proof_bytes);

    // Verify proof-of-possession over the canonical payload.
    let payload = clawft_kernel::node_registry::node_register_payload(
        &pubkey_arr,
        p.ts,
        &p.label,
    );
    let vk = match VerifyingKey::from_bytes(&pubkey_arr) {
        Ok(vk) => vk,
        Err(e) => return Response::error(format!("invalid pubkey: {e}")),
    };
    let sig = Signature::from_bytes(&proof_arr);
    if let Err(e) = vk.verify(&payload, &sig) {
        return Response::error(format!("proof-of-possession verify failed: {e}"));
    }

    let k = kernel.read().await;
    let label = if p.label.is_empty() {
        None
    } else {
        Some(p.label.clone())
    };
    let entry = k.node_registry().register(pubkey_arr, label);

    #[cfg(feature = "exochain")]
    if let Some(cm) = k.chain_manager() {
        cm.append(
            "node",
            "node.registered",
            Some(serde_json::json!({
                "node_id": entry.node_id,
                "label": entry.label,
                "pubkey_hex": hex_encode(&entry.pubkey),
                "registered_at": entry.registered_at.to_rfc3339(),
            })),
        );
    }

    k.event_log().info(
        "node",
        format!(
            "registered node {} (label={:?})",
            entry.node_id, entry.label
        ),
    );
    info!(
        node_id = %entry.node_id,
        label = ?entry.label,
        "node.register: authorized"
    );

    Response::success(
        serde_json::to_value(crate::protocol::NodeRegisterResult {
            node_id: entry.node_id,
            label: entry.label.unwrap_or_default(),
        })
        .unwrap(),
    )
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Handle `ipc.subscribe_stream`: register an external sink with the
/// topic router and return the ack + receiver for the streaming loop.
async fn handle_ipc_subscribe_stream(
    params: serde_json::Value,
    kernel: Arc<tokio::sync::RwLock<Kernel<NativePlatform>>>,
) -> Result<
    (
        Response,
        String,
        tokio::sync::mpsc::Receiver<Vec<u8>>,
        Box<dyn FnOnce() + Send>,
    ),
    String,
> {
    let p: crate::protocol::IpcSubscribeStreamParams =
        serde_json::from_value(params).map_err(|e| format!("invalid params: {e}"))?;

    let k = kernel.read().await;

    // If the caller provided an identity, verify the signature.
    // Missing actor_id is accepted at bring-up but logged as a warn;
    // missing signature for a declared actor is unauthorized.
    if let Some(actor_id) = p.actor_id.as_ref() {
        let sig = p
            .signature
            .as_ref()
            .ok_or_else(|| "actor_id provided but signature missing".to_string())?;
        let ts = p.ts.unwrap_or(0);
        let payload = clawft_kernel::subscribe_payload(&p.topic, ts, actor_id);
        verify_agent_signature(&k, actor_id, sig, &payload)
            .map_err(|e| format!("unauthorized: {e}"))?;
    } else {
        tracing::warn!(
            topic = %p.topic,
            "ipc.subscribe_stream with no actor_id — anonymous subscribe accepted (bring-up only)"
        );
    }

    // Bounded channel; a slow external client will see drops rather
    // than backpressuring the kernel's in-process fanout path.
    let (tx, rx) = tokio::sync::mpsc::channel::<Vec<u8>>(256);

    let router = k.a2a_router().topic_router().clone();
    let id = router.subscribe_sink(
        &p.topic,
        clawft_kernel::SubscriberSink::ExternalStream(tx),
    );
    k.event_log().info(
        "ipc",
        format!(
            "external client subscribed to '{}' (sub_id={}, actor_id={:?})",
            p.topic, id.0, p.actor_id
        ),
    );

    let topic = p.topic.clone();
    let unsubscribe_topic = p.topic.clone();
    let router_for_cleanup = router.clone();
    let on_disconnect: Box<dyn FnOnce() + Send> = Box::new(move || {
        router_for_cleanup.unsubscribe_id(&unsubscribe_topic, id);
    });

    let ack = Response::success(serde_json::json!({
        "subscribed": p.topic,
        "subscriber_id": id.0,
        "streaming": true,
    }));
    Ok((ack, topic, rx, on_disconnect))
}

/// Handle an RVF-framed connection.
///
/// Each request/response is an RVF Meta segment with content-hash
/// integrity. Responses carry the SEALED flag; requests do not.
/// Uses the same `dispatch()` function as JSON mode.
#[cfg(feature = "rvf-rpc")]
async fn handle_rvf_connection(
    stream: tokio::net::UnixStream,
    kernel: Arc<tokio::sync::RwLock<Kernel<NativePlatform>>>,
    shutdown_tx: watch::Sender<bool>,
) {
    use crate::rvf_codec::{RvfFrameReader, RvfFrameWriter};
    use crate::rvf_rpc;

    let (reader, writer) = stream.into_split();
    let mut frame_reader = RvfFrameReader::new(reader);
    let mut frame_writer = RvfFrameWriter::new(writer);
    let mut next_id: u64 = 1;

    loop {
        let frame = match frame_reader.read_frame().await {
            Ok(Some(f)) => f,
            Ok(None) => break, // clean EOF
            Err(e) => {
                debug!("RVF read error: {e}");
                break;
            }
        };

        let response = match rvf_rpc::decode_request(&frame) {
            Ok(req) => {
                let id = req.id.clone();
                dispatch(
                    req.method,
                    req.params,
                    Arc::clone(&kernel),
                    shutdown_tx.clone(),
                )
                .await
                .with_id(id)
            }
            Err(e) => Response::error(format!("invalid RVF request: {e}")),
        };

        let (seg_type, payload, flags, segment_id) =
            rvf_rpc::encode_response(&response, next_id);
        next_id += 1;

        if let Err(e) = frame_writer
            .write_frame(seg_type, &payload, flags, segment_id)
            .await
        {
            debug!("RVF write error: {e}");
            break;
        }
    }
}

/// Handle `substrate.subscribe`: streaming subscription over the
/// substrate service. Mirrors [`handle_ipc_subscribe_stream`].
async fn handle_substrate_subscribe(
    params: serde_json::Value,
    kernel: Arc<tokio::sync::RwLock<Kernel<NativePlatform>>>,
) -> Result<
    (
        Response,
        String,
        tokio::sync::mpsc::Receiver<Vec<u8>>,
        Box<dyn FnOnce() + Send>,
    ),
    String,
> {
    let p: crate::protocol::SubstrateSubscribeParams =
        serde_json::from_value(params).map_err(|e| format!("invalid params: {e}"))?;

    let k = kernel.read().await;

    if let Some(actor_id) = p.actor_id.as_ref() {
        let sig = p
            .signature
            .as_ref()
            .ok_or_else(|| "actor_id provided but signature missing".to_string())?;
        let ts = p.ts.unwrap_or(0);
        let payload = clawft_kernel::subscribe_payload(&p.path, ts, actor_id);
        verify_agent_signature(&k, actor_id, sig, &payload)
            .map_err(|e| format!("unauthorized: {e}"))?;
    }

    let substrate = k.substrate_service().clone();
    let (id, rx) = substrate
        .subscribe(p.actor_id.as_deref(), &p.path)
        .map_err(|e| e.to_string())?;
    k.event_log().info(
        "substrate",
        format!(
            "external client subscribed to '{}' (sub_id={}, actor_id={:?})",
            p.path, id.0, p.actor_id
        ),
    );

    let path_for_ack = p.path.clone();
    let path_for_cleanup = p.path.clone();
    let substrate_for_cleanup = substrate.clone();
    let on_disconnect: Box<dyn FnOnce() + Send> = Box::new(move || {
        substrate_for_cleanup.unsubscribe(&path_for_cleanup, id);
    });

    let ack = Response::success(serde_json::json!({
        "subscribed": p.path,
        "subscriber_id": id.0,
        "streaming": true,
    }));
    Ok((ack, path_for_ack, rx, on_disconnect))
}

/// Handle `substrate.read`: synchronous value + tick + sensitivity.
async fn handle_substrate_read(
    params: serde_json::Value,
    kernel: Arc<tokio::sync::RwLock<Kernel<NativePlatform>>>,
) -> Response {
    let p: crate::protocol::SubstrateReadParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(e) => return Response::error(format!("invalid params: {e}")),
    };
    let k = kernel.read().await;
    match k.substrate_service().read(p.actor_id.as_deref(), &p.path) {
        Ok(snap) => Response::success(
            serde_json::to_value(crate::protocol::SubstrateReadResult {
                value: snap.value,
                tick: snap.tick,
                sensitivity: snap.sensitivity.as_str().to_string(),
            })
            .unwrap(),
        ),
        Err(e) => Response::error(format!("unauthorized: {e}")),
    }
}

/// Handle `substrate.list`: enumerate children of a prefix up to `depth`.
///
/// Wire shape (Phase 1 §3.1):
///
/// ```json
/// // request
/// { "prefix": "substrate/sensor", "depth": 1 }
///
/// // response
/// {
///   "children": [
///     { "path": "substrate/sensor/mic", "has_value": true,  "child_count": 0 },
///     { "path": "substrate/sensor/tof", "has_value": true,  "child_count": 0 }
///   ],
///   "tick": 42
/// }
/// ```
///
/// Same egress gating as `substrate.read` — the prefix itself is
/// checked once, and capture-tier descendants are hidden from
/// anonymous callers so path names don't leak.
async fn handle_substrate_list(
    params: serde_json::Value,
    kernel: Arc<tokio::sync::RwLock<Kernel<NativePlatform>>>,
) -> Response {
    let p: crate::protocol::SubstrateListParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(e) => return Response::error(format!("invalid params: {e}")),
    };
    let k = kernel.read().await;
    match k
        .substrate_service()
        .list(p.actor_id.as_deref(), &p.prefix, p.depth)
    {
        Ok(snap) => {
            let result = crate::protocol::SubstrateListResult {
                children: snap
                    .children
                    .into_iter()
                    .map(|c| crate::protocol::SubstrateListChild {
                        path: c.path,
                        has_value: c.has_value,
                        child_count: c.child_count,
                    })
                    .collect(),
                tick: snap.tick,
            };
            Response::success(serde_json::to_value(result).unwrap())
        }
        Err(e) => Response::error(format!("unauthorized: {e}")),
    }
}

/// Handle `substrate.publish`: verify node signature, enforce the
/// `substrate/<node-id>/...` write prefix, Replace the path's value
/// and fan out.
///
/// Every publish must be node-attributed and signed. Unsigned
/// publishes are rejected — there is no anonymous-publish bypass.
/// The actor-side fields (`actor_id` / `signature` / `ts`) are
/// reserved for the Actions pipeline and ignored here.
async fn handle_substrate_publish(
    params: serde_json::Value,
    kernel: Arc<tokio::sync::RwLock<Kernel<NativePlatform>>>,
) -> Response {
    let p: crate::protocol::SubstratePublishParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(e) => return Response::error(format!("invalid params: {e}")),
    };

    // Hard requirement: every write is node-attributed.
    let node_id = match p.node_id.as_ref() {
        Some(n) => n.clone(),
        None => {
            return Response::error(
                "substrate.publish: node_id required (every write must be node-attributed)"
                    .to_string(),
            );
        }
    };
    let signature = match p.node_signature.as_ref() {
        Some(s) => s.clone(),
        None => {
            return Response::error(
                "substrate.publish: node_signature required when node_id is set".to_string(),
            );
        }
    };
    let node_ts = p.node_ts.unwrap_or(0);

    let k = kernel.read().await;

    // Verify the node signature over the canonical payload.
    let value_bytes = serde_json::to_vec(&p.value).unwrap_or_default();
    let value_str = String::from_utf8_lossy(&value_bytes);
    let payload = clawft_kernel::node_publish_payload(&p.path, &value_str, node_ts, &node_id);
    if let Err(e) = verify_node_signature(&k, &node_id, &signature, &payload) {
        return Response::error(format!("unauthorized: {e}"));
    }

    // Run the publish through the node-identity gate. Rejects writes
    // outside `substrate/<node-id>/...` (node-private tier rule).
    // Mesh-canonical writes (`substrate/_derived/...`) require a
    // separate capability path that isn't wired yet — they will fall
    // through this branch and get rejected, which is correct for
    // this phase.
    let tick = match k
        .substrate_service()
        .publish_gated(Some(&node_id), &p.path, p.value)
    {
        Ok(t) => t,
        Err(e) => return Response::error(format!("gate denied: {e}")),
    };
    k.event_log().info(
        "substrate",
        format!("publish {} tick={} node={}", p.path, tick, node_id),
    );
    if tick == 1 {
        info!(
            path = %p.path,
            node = %node_id,
            "substrate.publish: stream started (first window on path)"
        );
    } else {
        debug!(path = %p.path, node = %node_id, tick, "substrate.publish");
    }
    Response::success(serde_json::json!({
        "path": p.path,
        "tick": tick,
    }))
}

/// Handle `node.identity`: report this daemon's own node-id +
/// label + registration timestamp.
///
/// Lets a remote node (the ESP32 firmware) discover the daemon's
/// node-id at runtime instead of hardcoding it. Used to build
/// control-path prefixes:
/// `substrate/<node_identity.node_id>/control/sensors/...`.
async fn handle_node_identity(
    _params: serde_json::Value,
    kernel: Arc<tokio::sync::RwLock<Kernel<NativePlatform>>>,
) -> Response {
    let state = match daemon_control() {
        Some(s) => s,
        None => {
            return Response::error("daemon control state not initialized".to_string());
        }
    };
    // Look up the daemon's NodeRegistry entry to surface label +
    // registered_at — these are the fields the firmware Claude
    // requested in the dialog.
    let k = kernel.read().await;
    let entry = match k.node_registry().get(&state.daemon_node_id) {
        Some(e) => e,
        None => {
            return Response::error(format!(
                "daemon node {} not in registry (boot inconsistency)",
                state.daemon_node_id
            ));
        }
    };
    Response::success(
        serde_json::to_value(crate::protocol::NodeIdentityResult {
            node_id: entry.node_id,
            label: entry.label.unwrap_or_default(),
            registered_at: entry.registered_at.to_rfc3339(),
        })
        .unwrap(),
    )
}

/// Handle `llm.prompt`: synchronous chat completion against the local
/// LLM service.
///
/// This is the V1 shape — one RPC, full completion in the response.
/// Streaming is a deferred follow-up that lands as `llm.prompt_stream`
/// using the same connection-takeover pattern as
/// `substrate.subscribe`, not as a breaking change to this method.
///
/// Behaviour:
/// 1. If the `llm` control flag is `false`, fast-fail with a clear
///    error so the GUI's disable toggle has the same source-cuts
///    semantic as for sensors.
/// 2. If the daemon's LLM client wasn't wired at boot (init error or
///    feature absent), return a clean "service unavailable" instead
///    of panicking.
/// 3. Build a [`ChatMessage`] vector from the params (`messages`
///    wins over `prompt`; optional `system` is prepended only when
///    `messages` doesn't already carry one).
/// 4. Forward to [`LlmClient::complete`]; map errors back to RPC
///    errors with the same string formatting `LlmError::Display`
///    uses, so the wire surface stays diagnosable.
async fn handle_llm_prompt(
    params: serde_json::Value,
    _kernel: Arc<tokio::sync::RwLock<Kernel<NativePlatform>>>,
) -> Response {
    let p: crate::protocol::LlmPromptParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(e) => return Response::error(format!("invalid params: {e}")),
    };

    // Honor the `llm` control flag. Source-cuts: if disabled, we
    // never even reach the HTTP client.
    if let Some(state) = daemon_control()
        && let Some(flag) = state.flags.get(ControlKind::Service, "llm")
        && !flag.load(std::sync::atomic::Ordering::SeqCst)
    {
        return Response::error("llm service is disabled (toggle on via control.set_enabled)");
    }

    let client = match daemon_llm() {
        Some(c) => c,
        None => {
            return Response::error(
                "llm service not initialized (check daemon boot log for 'llm client init failed')",
            );
        }
    };

    let mut messages: Vec<clawft_service_llm::ChatMessage> = match (p.messages, p.prompt.clone()) {
        (Some(msgs), _) => msgs
            .into_iter()
            .map(|m| clawft_service_llm::ChatMessage {
                role: m.role,
                content: m.content,
                // The daemon's `llm.prompt` RPC predates tool-call
                // support and accepts only role+content from clients;
                // tool fields stay None until a future RPC schema bump
                // exposes them.
                tool_calls: None,
                tool_call_id: None,
            })
            .collect(),
        (None, Some(prompt)) => vec![clawft_service_llm::ChatMessage::user(prompt)],
        (None, None) => {
            return Response::error("invalid params: must supply `prompt` or `messages`");
        }
    };
    if messages.is_empty() {
        return Response::error("invalid params: messages was empty");
    }
    // Prepend a system prompt only when the caller didn't already
    // provide one. Avoids stomping a deliberately-set conversation
    // shape.
    if let Some(sys) = p.system
        && !sys.is_empty()
        && !messages.first().map(|m| m.role == "system").unwrap_or(false)
    {
        messages.insert(0, clawft_service_llm::ChatMessage::system(sys));
    }

    match client.complete(messages, p.temperature, p.max_tokens).await {
        Ok(resp) => {
            let first = &resp.choices[0]; // complete() rejects empty choices upstream
            let result = crate::protocol::LlmPromptResult {
                completion: first.message.content.clone(),
                finish_reason: first.finish_reason.clone(),
                prompt_tokens: resp.usage.prompt_tokens,
                completion_tokens: resp.usage.completion_tokens,
                model: resp.model.clone(),
            };
            Response::success(serde_json::to_value(result).unwrap())
        }
        Err(e) => Response::error(format!("llm.prompt: {e}")),
    }
}

// ── Terminal (PTY) RPC handlers ─────────────────────────────────────
//
// All four handlers share two preconditions:
// 1. `DAEMON_TERMINAL` is set (boot wiring did its job).
// 2. The supplied `session_id` resolves to a live session. Spawn is the
//    exception — it allocates the id rather than receiving one.
//
// Output flows the other way: a tokio task started by `terminal.spawn`
// drains the session's `mpsc::UnboundedReceiver<TerminalEvent>` and
// publishes each chunk to
// `substrate/<daemon-node>/derived/terminal/<session_id>` via
// `publish_gated`. Surfaces poll that path through the existing
// `substrate.read` cascade — no separate streaming RPC.

/// Handle `terminal.spawn`: allocate a PTY, spawn a shell, start the
/// substrate publish pump, and return the session id + resolved shell
/// metadata.
async fn handle_terminal_spawn(
    params: serde_json::Value,
    kernel: Arc<tokio::sync::RwLock<Kernel<NativePlatform>>>,
) -> Response {
    let p: crate::protocol::TerminalSpawnParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(e) => return Response::error(format!("invalid params: {e}")),
    };
    let mgr = match daemon_terminal() {
        Some(m) => m,
        None => return Response::error("terminal service not initialized".to_string()),
    };
    let id = match mgr.spawn(p.rows, p.cols, p.shell.clone(), p.cwd.clone()) {
        Ok(id) => id,
        Err(e) => return Response::error(format!("terminal.spawn: {e}")),
    };
    let session = match mgr.session(&id) {
        Some(s) => s,
        None => {
            return Response::error(
                "terminal.spawn: session disappeared between spawn and lookup".to_string(),
            );
        }
    };

    // Drain the session's output channel onto substrate. The reader
    // pump runs on its own OS thread inside the service crate; this
    // tokio task just forwards events.
    //
    // Authority: the daemon's own node-id, so `publish_gated` accepts
    // the write under its node-private prefix.
    let daemon_node_id = match daemon_control() {
        Some(s) => s.daemon_node_id.clone(),
        None => return Response::error("daemon control state not initialized".to_string()),
    };
    let output_path = format!(
        "substrate/{}/derived/terminal/{}",
        daemon_node_id, id
    );
    let resolved_shell = session.shell().to_string();
    let resolved_cwd = session.cwd().to_string();

    if let Some(mut events) = session.take_events() {
        let substrate = {
            let k = kernel.read().await;
            k.substrate_service().clone()
        };
        let publish_path = output_path.clone();
        let publish_node = daemon_node_id.clone();
        let session_id_for_log = id.clone();
        tokio::spawn(async move {
            use base64::Engine;
            let b64 = base64::engine::general_purpose::STANDARD;
            while let Some(ev) = events.recv().await {
                let chunk = match ev {
                    clawft_service_terminal::TerminalEvent::Output(bytes) => {
                        crate::protocol::TerminalChunk {
                            data: b64.encode(&bytes),
                            ts_ms: crate::control::now_ms(),
                            exit: false,
                        }
                    }
                    clawft_service_terminal::TerminalEvent::Exit => {
                        crate::protocol::TerminalChunk {
                            data: String::new(),
                            ts_ms: crate::control::now_ms(),
                            exit: true,
                        }
                    }
                };
                let value = match serde_json::to_value(&chunk) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!(error = %e, "terminal: chunk serialize failed");
                        continue;
                    }
                };
                if let Err(e) = substrate.publish_gated(
                    Some(&publish_node),
                    &publish_path,
                    value,
                ) {
                    warn!(error = %e, path = %publish_path, "terminal: publish_gated failed");
                }
                if chunk.exit {
                    debug!(session_id = %session_id_for_log, "terminal: drain task exiting on Exit chunk");
                    break;
                }
            }
            debug!(session_id = %session_id_for_log, "terminal: drain task ended");
        });
    } else {
        warn!(session_id = %id, "terminal: take_events returned None — output won't be published");
    }

    let result = crate::protocol::TerminalSpawnResult {
        session_id: id.to_string(),
        rows: if p.rows == 0 { clawft_service_terminal::DEFAULT_ROWS } else { p.rows },
        cols: if p.cols == 0 { clawft_service_terminal::DEFAULT_COLS } else { p.cols },
        shell: resolved_shell,
        cwd: resolved_cwd,
        output_path,
    };
    Response::success(serde_json::to_value(result).unwrap())
}

/// Handle `terminal.write`: forward base64-decoded bytes into the PTY.
async fn handle_terminal_write(
    params: serde_json::Value,
    _kernel: Arc<tokio::sync::RwLock<Kernel<NativePlatform>>>,
) -> Response {
    let p: crate::protocol::TerminalWriteParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(e) => return Response::error(format!("invalid params: {e}")),
    };
    let mgr = match daemon_terminal() {
        Some(m) => m,
        None => return Response::error("terminal service not initialized".to_string()),
    };
    use base64::Engine;
    let bytes = match base64::engine::general_purpose::STANDARD.decode(p.data.as_bytes()) {
        Ok(b) => b,
        Err(e) => return Response::error(format!("terminal.write: bad base64: {e}")),
    };
    match mgr.write(
        &clawft_service_terminal::SessionId::from(p.session_id.as_str()),
        &bytes,
    ) {
        Ok(()) => Response::success(
            serde_json::to_value(crate::protocol::TerminalAck { ok: true }).unwrap(),
        ),
        Err(e) => Response::error(format!("terminal.write: {e}")),
    }
}

/// Handle `terminal.resize`: reflow the PTY for an in-shell app.
async fn handle_terminal_resize(
    params: serde_json::Value,
    _kernel: Arc<tokio::sync::RwLock<Kernel<NativePlatform>>>,
) -> Response {
    let p: crate::protocol::TerminalResizeParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(e) => return Response::error(format!("invalid params: {e}")),
    };
    let mgr = match daemon_terminal() {
        Some(m) => m,
        None => return Response::error("terminal service not initialized".to_string()),
    };
    match mgr.resize(
        &clawft_service_terminal::SessionId::from(p.session_id.as_str()),
        p.rows,
        p.cols,
    ) {
        Ok(()) => Response::success(
            serde_json::to_value(crate::protocol::TerminalAck { ok: true }).unwrap(),
        ),
        Err(e) => Response::error(format!("terminal.resize: {e}")),
    }
}

/// Handle `terminal.close`: kill the child shell, drop the PTY, forget
/// the session. Idempotent — closing an unknown session is `ok`.
async fn handle_terminal_close(
    params: serde_json::Value,
    _kernel: Arc<tokio::sync::RwLock<Kernel<NativePlatform>>>,
) -> Response {
    let p: crate::protocol::TerminalCloseParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(e) => return Response::error(format!("invalid params: {e}")),
    };
    let mgr = match daemon_terminal() {
        Some(m) => m,
        None => return Response::error("terminal service not initialized".to_string()),
    };
    match mgr.close(&clawft_service_terminal::SessionId::from(p.session_id.as_str())) {
        Ok(()) => Response::success(
            serde_json::to_value(crate::protocol::TerminalAck { ok: true }).unwrap(),
        ),
        Err(e) => Response::error(format!("terminal.close: {e}")),
    }
}

/// Handle `control.set_enabled`: flip a daemon-side enable flag and
/// republish the substrate control intent under the daemon's own
/// prefix.
///
/// Body: `{ kind: "service" | "sensor", target: <string>, enabled:
/// <bool>, label?: <string> }`. The handler:
///
/// 1. Updates the in-memory `Arc<AtomicBool>` registered for the
///    target. If no flag was registered (typo / unknown target)
///    the call is rejected with `400`-flavored error.
/// 2. Publishes a fresh `ControlIntent` value at
///    `substrate/<daemon-node>/control/<kind>s/<target>` so
///    subscribers (the GUI, future firmware) see the change.
async fn handle_control_set_enabled(
    params: serde_json::Value,
    kernel: Arc<tokio::sync::RwLock<Kernel<NativePlatform>>>,
) -> Response {
    let p: crate::protocol::ControlSetEnabledParams =
        match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => return Response::error(format!("invalid params: {e}")),
        };
    let kind = match ControlKind::parse(&p.kind) {
        Some(k) => k,
        None => {
            return Response::error(format!(
                "unknown kind {:?} (want \"service\" or \"sensor\")",
                p.kind
            ));
        }
    };
    let state = match daemon_control() {
        Some(s) => s,
        None => return Response::error("daemon control state not initialized".to_string()),
    };
    if state.flags.set(kind, &p.target, p.enabled).is_none() {
        return Response::error(format!(
            "no flag registered for kind={:?} target={:?}",
            p.kind, p.target
        ));
    }

    // Publish the new intent under the daemon's own prefix. Failure
    // here doesn't roll back the in-memory flip — the in-memory
    // flag IS the source of truth for daemon-side enforcement, and
    // the substrate value is the eventual-consistency mirror.
    let intent = ControlIntent {
        enabled: p.enabled,
        kind,
        target: p.target.clone(),
        label: p.label.clone().unwrap_or_default(),
        updated_at_ms: crate::control::now_ms(),
    };
    let path = crate::control::intent_path(&state.daemon_node_id, kind, &p.target);
    let publish_result = {
        let k = kernel.read().await;
        k.substrate_service()
            .publish_gated(Some(&state.daemon_node_id), &path, intent.to_value())
    };
    if let Err(e) = publish_result {
        warn!(
            error = %e,
            path = %path,
            "control: substrate mirror publish failed (in-memory flag was still flipped)"
        );
    }
    info!(
        kind = ?kind,
        target = %p.target,
        enabled = p.enabled,
        "control: flag set"
    );

    Response::success(
        serde_json::to_value(crate::protocol::ControlSetEnabledResult {
            path,
            enabled: p.enabled,
        })
        .unwrap(),
    )
}

/// Handle `control.list`: snapshot every registered control flag.
/// Used by the Explorer to populate a "Controls" overview without
/// walking substrate.
async fn handle_control_list(
    _params: serde_json::Value,
    _kernel: Arc<tokio::sync::RwLock<Kernel<NativePlatform>>>,
) -> Response {
    let state = match daemon_control() {
        Some(s) => s,
        None => return Response::error("daemon control state not initialized".to_string()),
    };
    let entries: Vec<_> = state
        .flags
        .list()
        .into_iter()
        .map(|(kind, target, enabled)| crate::protocol::ControlListEntry {
            kind: match kind {
                ControlKind::Service => "service".to_string(),
                ControlKind::Sensor => "sensor".to_string(),
            },
            target,
            enabled,
        })
        .collect();
    Response::success(
        serde_json::to_value(crate::protocol::ControlListResult { entries }).unwrap(),
    )
}

/// Handle `substrate.canonical_publish_payload`: diagnostic — return
/// the exact bytes the daemon would feed to the signature verifier
/// for a hypothetical publish. No signature check, no actual write.
///
/// Lets remote nodes (the ESP32 firmware especially) sanity-check
/// their canonical-payload buffer against what the daemon expects,
/// so byte-drift bugs become a one-shot diff instead of a guessing
/// game.
async fn handle_substrate_canonical_publish_payload(
    params: serde_json::Value,
    _kernel: Arc<tokio::sync::RwLock<Kernel<NativePlatform>>>,
) -> Response {
    let p: crate::protocol::SubstrateCanonicalPublishPayloadParams =
        match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => return Response::error(format!("invalid params: {e}")),
        };

    // Same canonicalization the publish handler does: re-serialize
    // the value through serde_json::Value, which alphabetizes object
    // keys. This is the load-bearing step — the surface the firmware
    // most often gets wrong.
    let value_bytes = match serde_json::to_vec(&p.value) {
        Ok(b) => b,
        Err(e) => return Response::error(format!("value re-serialize: {e}")),
    };
    let canonical_value_json = String::from_utf8_lossy(&value_bytes).into_owned();

    let payload = clawft_kernel::node_publish_payload(
        &p.path,
        &canonical_value_json,
        p.node_ts,
        &p.node_id,
    );

    let payload_hex = hex_encode(&payload);
    let payload_len = payload.len();

    Response::success(
        serde_json::to_value(crate::protocol::SubstrateCanonicalPublishPayloadResult {
            payload_hex,
            payload_len,
            canonical_value_json,
        })
        .unwrap(),
    )
}

/// Handle `substrate.notify`: signal-only pulse, no payload change.
async fn handle_substrate_notify(
    params: serde_json::Value,
    kernel: Arc<tokio::sync::RwLock<Kernel<NativePlatform>>>,
) -> Response {
    let p: crate::protocol::SubstrateNotifyParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(e) => return Response::error(format!("invalid params: {e}")),
    };
    let k = kernel.read().await;
    let tick = k
        .substrate_service()
        .notify(p.actor_id.as_deref(), &p.path);
    Response::success(serde_json::json!({
        "path": p.path,
        "tick": tick,
    }))
}

/// Dispatch a request to the appropriate handler.
///
/// Takes owned values to ensure the future is `Send + 'static`
/// for use inside `tokio::spawn`.
async fn dispatch(
    method: String,
    params: serde_json::Value,
    kernel: Arc<tokio::sync::RwLock<Kernel<NativePlatform>>>,
    shutdown_tx: watch::Sender<bool>,
) -> Response {
    match method.as_str() {
        "kernel.status" => {
            let k = kernel.read().await;
            let state_str = match k.state() {
                KernelState::Booting => "booting",
                KernelState::Running => "running",
                KernelState::ShuttingDown => "shutting_down",
                KernelState::Halted => "halted",
                _ => "unknown",
            };
            let result = KernelStatusResult {
                state: state_str.to_owned(),
                uptime_secs: k.uptime().as_secs_f64(),
                process_count: k.process_table().len(),
                service_count: k.services().len(),
                max_processes: k.kernel_config().max_processes,
                health_check_interval_secs: k.kernel_config().health_check_interval_secs,
            };
            Response::success(serde_json::to_value(result).unwrap())
        }
        "kernel.ps" => {
            let k = kernel.read().await;
            let mut entries: Vec<ProcessInfo> = k
                .process_table()
                .list()
                .iter()
                .map(|e| ProcessInfo {
                    pid: e.pid,
                    agent_id: e.agent_id.clone(),
                    state: e.state.to_string(),
                    memory_bytes: e.resource_usage.memory_bytes,
                    cpu_time_ms: e.resource_usage.cpu_time_ms,
                    parent_pid: e.parent_pid,
                })
                .collect();
            entries.sort_by_key(|e| e.pid);
            Response::success(serde_json::to_value(entries).unwrap())
        }
        "kernel.services" => {
            let k = kernel.read().await;
            let services = k.services().list();
            let infos: Vec<ServiceInfo> = services
                .iter()
                .map(|(name, stype)| ServiceInfo {
                    name: name.clone(),
                    service_type: stype.to_string(),
                    health: "registered".into(),
                })
                .collect();
            Response::success(serde_json::to_value(infos).unwrap())
        }
        "kernel.logs" => {
            let log_params: LogsParams = serde_json::from_value(params).unwrap_or(LogsParams {
                count: 50,
                level: None,
            });

            let k = kernel.read().await;
            let event_log = k.event_log();

            let events = if let Some(ref level_str) = log_params.level {
                let level = match level_str.as_str() {
                    "debug" => clawft_kernel::LogLevel::Debug,
                    "warn" | "warning" => clawft_kernel::LogLevel::Warn,
                    "error" => clawft_kernel::LogLevel::Error,
                    _ => clawft_kernel::LogLevel::Info,
                };
                event_log.filter_level(&level, log_params.count)
            } else {
                event_log.tail(log_params.count)
            };

            let entries: Vec<LogEntry> = events
                .iter()
                .map(|e| LogEntry {
                    timestamp: e.timestamp.to_rfc3339(),
                    phase: e.phase.tag().to_owned(),
                    level: format!("{:?}", e.level).to_lowercase(),
                    message: e.message.clone(),
                })
                .collect();

            Response::success(serde_json::to_value(entries).unwrap())
        }
        "kernel.shutdown" => {
            // Log the shutdown event before signaling
            {
                let k = kernel.read().await;
                k.event_log().info("daemon", "shutdown requested via RPC");
            }
            info!("shutdown requested via RPC");
            let _ = shutdown_tx.send(true);
            Response::success(serde_json::json!("shutting down"))
        }
        // ── M1.5.1a: admin-app write verbs (ADR-015 influences) ────
        //
        // These are the two verbs the WeftOS Admin surface declares
        // influence over in `crates/clawft-app/fixtures/weftos-admin.toml`
        // and binds as affordances on the process table + service
        // gauges in `crates/clawft-surface/fixtures/weftos-admin-desktop.toml`.
        //
        // Governance intersection is stubbed at the compositor level
        // (ADR-006 §2 — honest governance lands with M2's active-radar
        // loop). The daemon's own capability check still applies via
        // `k.supervisor()` + `k.services()`.
        "kernel.kill-process" => {
            let pid = match params.get("pid").and_then(|v| v.as_u64()) {
                Some(p) => p,
                None => {
                    return Response::error(
                        "kernel.kill-process: missing or non-integer `pid`".to_string(),
                    );
                }
            };
            let graceful = params
                .get("graceful")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let k = kernel.read().await;
            match k.supervisor().stop(pid, graceful) {
                Ok(()) => {
                    k.event_log().info(
                        "kernel",
                        format!(
                            "kill-process PID {pid} ({})",
                            if graceful { "graceful" } else { "force" }
                        ),
                    );
                    Response::success(serde_json::json!({ "pid": pid, "graceful": graceful }))
                }
                Err(e) => Response::error(format!("kill-process failed: {e}")),
            }
        }
        "kernel.restart-service" => {
            let name = match params.get("name").and_then(|v| v.as_str()) {
                Some(n) if !n.is_empty() => n.to_string(),
                _ => {
                    return Response::error(
                        "kernel.restart-service: missing or empty `name`".to_string(),
                    );
                }
            };
            let k = kernel.read().await;
            // Preferred path: service has an owner agent, restart via
            // supervisor (re-uses the full agent-restart lineage).
            if let Some(pid) = k.services().resolve_target(&name) {
                match k.supervisor().restart(pid) {
                    Ok(result) => {
                        let _ = k.process_table().update_state(
                            result.pid,
                            clawft_kernel::ProcessState::Running,
                        );
                        k.event_log().info(
                            "kernel",
                            format!(
                                "restart-service {name} (PID {pid} -> {})",
                                result.pid
                            ),
                        );
                        return Response::success(serde_json::json!({
                            "name": name,
                            "old_pid": pid,
                            "new_pid": result.pid,
                        }));
                    }
                    Err(e) => {
                        return Response::error(format!("restart-service failed: {e}"));
                    }
                }
            }
            // Fallback path: SystemService without an owner agent (e.g.
            // external / infrastructure services). Stop then start.
            if let Some(svc) = k.services().get(&name) {
                if let Err(e) = svc.stop().await {
                    return Response::error(format!("restart-service stop failed: {e}"));
                }
                if let Err(e) = svc.start().await {
                    return Response::error(format!("restart-service start failed: {e}"));
                }
                k.event_log()
                    .info("kernel", format!("restart-service {name} (no owner pid)"));
                return Response::success(serde_json::json!({ "name": name }));
            }
            Response::error(format!("restart-service: unknown service `{name}`"))
        }
        "cluster.status" => {
            let k = kernel.read().await;
            let membership = k.cluster_membership();
            let active = membership.count_by_state(&clawft_kernel::NodeState::Active);
            let total = membership.len();

            let result = ClusterStatusResult {
                total_nodes: total,
                healthy_nodes: active,
                total_shards: 0,
                active_shards: 0,
                consensus_enabled: false,
            };
            Response::success(serde_json::to_value(result).unwrap())
        }
        "cluster.nodes" => {
            let k = kernel.read().await;
            let membership = k.cluster_membership();
            let peers = membership.list_peers();
            let nodes: Vec<ClusterNodeInfo> = peers
                .iter()
                .map(|(id, state, platform)| {
                    let peer = membership.get_peer(id);
                    ClusterNodeInfo {
                        node_id: id.clone(),
                        name: peer
                            .as_ref()
                            .map(|p| p.name.clone())
                            .unwrap_or_else(|| id.clone()),
                        platform: platform.to_string(),
                        state: state.to_string(),
                        address: peer.and_then(|p| p.address),
                        last_seen: String::new(),
                    }
                })
                .collect();
            Response::success(serde_json::to_value(nodes).unwrap())
        }
        "cluster.join" => {
            let join_params: ClusterJoinParams =
                match serde_json::from_value(params) {
                    Ok(p) => p,
                    Err(e) => return Response::error(format!("invalid params: {e}")),
                };

            let k = kernel.read().await;
            let membership = k.cluster_membership();
            let node_id = uuid::Uuid::new_v4().to_string();
            let platform = match join_params.platform.as_str() {
                "browser" => clawft_kernel::NodePlatform::Browser,
                "edge" => clawft_kernel::NodePlatform::Edge,
                "wasi" => clawft_kernel::NodePlatform::Wasi,
                _ => clawft_kernel::NodePlatform::CloudNative,
            };
            let peer = clawft_kernel::PeerNode {
                id: node_id.clone(),
                name: join_params.name.unwrap_or_else(|| node_id.clone()),
                platform,
                state: clawft_kernel::NodeState::Active,
                address: join_params.address,
                first_seen: chrono::Utc::now(),
                last_heartbeat: chrono::Utc::now(),
                capabilities: Vec::new(),
                labels: std::collections::HashMap::new(),
            };
            match membership.add_peer(peer) {
                Ok(()) => Response::success(serde_json::json!({ "node_id": node_id })),
                Err(e) => Response::error(format!("join failed: {e}")),
            }
        }
        "cluster.leave" => {
            let leave_params: ClusterLeaveParams =
                match serde_json::from_value(params) {
                    Ok(p) => p,
                    Err(e) => return Response::error(format!("invalid params: {e}")),
                };

            let k = kernel.read().await;
            let membership = k.cluster_membership();
            match membership.remove_peer(&leave_params.node_id) {
                Ok(_) => Response::success(serde_json::json!("ok")),
                Err(e) => Response::error(format!("leave failed: {e}")),
            }
        }
        "cluster.health" => {
            let k = kernel.read().await;
            let membership = k.cluster_membership();
            let peers = membership.list_peers();
            let health: Vec<serde_json::Value> = peers
                .iter()
                .map(|(id, state, _)| {
                    serde_json::json!({
                        "node_id": id,
                        "healthy": matches!(state, clawft_kernel::NodeState::Active),
                        "state": state.to_string(),
                    })
                })
                .collect();
            Response::success(serde_json::to_value(health).unwrap())
        }
        "cluster.shards" => {
            // Shards are only available with the cluster feature
            Response::success(serde_json::json!([]))
        }
        "chain.status" => {
            #[cfg(feature = "exochain")]
            {
                let k = kernel.read().await;
                if let Some(cm) = k.chain_manager() {
                    let status = cm.status();
                    let hash_hex: String =
                        status.last_hash.iter().map(|b| format!("{b:02x}")).collect();
                    let result = ChainStatusResult {
                        chain_id: status.chain_id,
                        sequence: status.sequence,
                        event_count: status.event_count,
                        checkpoint_count: status.checkpoint_count,
                        events_since_checkpoint: status.events_since_checkpoint,
                        last_hash: hash_hex,
                    };
                    Response::success(serde_json::to_value(result).unwrap())
                } else {
                    Response::error("chain not enabled")
                }
            }
            #[cfg(not(feature = "exochain"))]
            Response::error("exochain feature not enabled")
        }
        "chain.local" => {
            #[cfg(feature = "exochain")]
            {
                let local_params: ChainLocalParams = serde_json::from_value(params)
                    .unwrap_or(ChainLocalParams { count: 20 });
                let k = kernel.read().await;
                if let Some(cm) = k.chain_manager() {
                    let events = cm.tail(local_params.count);
                    let infos: Vec<ChainEventInfo> = events
                        .iter()
                        .map(|e| {
                            let hash_hex: String =
                                e.hash.iter().map(|b| format!("{b:02x}")).collect();
                            // Build condensed detail from payload
                            let detail = match &e.payload {
                                Some(p) => {
                                    let mut parts = Vec::new();
                                    if let Some(v) = p.get("agent_id").and_then(|v| v.as_str()) {
                                        parts.push(format!("agent={v}"));
                                    }
                                    if let Some(v) = p.get("pid").and_then(|v| v.as_u64()) {
                                        parts.push(format!("pid={v}"));
                                    }
                                    if let Some(v) = p.get("from").and_then(|v| v.as_u64()) {
                                        parts.push(format!("from={v}"));
                                    }
                                    if let Some(v) = p.get("target").and_then(|v| v.as_str()) {
                                        parts.push(format!("to={v}"));
                                    }
                                    if let Some(v) = p.get("exit_code").and_then(|v| v.as_i64()) {
                                        parts.push(format!("exit={v}"));
                                    }
                                    if let Some(v) = p.get("job_name").and_then(|v| v.as_str()) {
                                        parts.push(format!("job={v}"));
                                    }
                                    if let Some(v) = p.get("payload_type").and_then(|v| v.as_str()) {
                                        parts.push(format!("type={v}"));
                                    }
                                    if let Some(v) = p.get("state").and_then(|v| v.as_str()) {
                                        parts.push(format!("state={v}"));
                                    }
                                    if parts.is_empty() {
                                        // Fallback: show first 60 chars of payload
                                        let s = p.to_string();
                                        if s.len() > 60 { format!("{}...", &s[..60]) } else { s }
                                    } else {
                                        parts.join(" ")
                                    }
                                }
                                None => String::new(),
                            };
                            ChainEventInfo {
                                sequence: e.sequence,
                                chain_id: e.chain_id,
                                timestamp: e.timestamp.to_rfc3339(),
                                source: e.source.clone(),
                                kind: e.kind.clone(),
                                hash: hash_hex,
                                detail,
                            }
                        })
                        .collect();
                    Response::success(serde_json::to_value(infos).unwrap())
                } else {
                    Response::error("chain not enabled")
                }
            }
            #[cfg(not(feature = "exochain"))]
            Response::error("exochain feature not enabled")
        }
        "chain.checkpoint" => {
            #[cfg(feature = "exochain")]
            {
                let k = kernel.read().await;
                if let Some(cm) = k.chain_manager() {
                    let cp = cm.checkpoint();
                    Response::success(serde_json::to_value(cp).unwrap())
                } else {
                    Response::error("chain not enabled")
                }
            }
            #[cfg(not(feature = "exochain"))]
            Response::error("exochain feature not enabled")
        }
        "chain.verify" => {
            #[cfg(feature = "exochain")]
            {
                let k = kernel.read().await;
                if let Some(cm) = k.chain_manager() {
                    let result = cm.verify_integrity();

                    // Also verify RVF signature if chain has a signing key.
                    let signature_verified = if let Some(pubkey) = cm.verifying_key() {
                        let cfg = k.kernel_config().chain.clone().unwrap_or_default();
                        if let Some(ref ckpt_path) = cfg.effective_checkpoint_path() {
                            let rvf_path = std::path::PathBuf::from(ckpt_path)
                                .with_extension("rvf");
                            if rvf_path.exists() {
                                match clawft_kernel::ChainManager::verify_rvf_signature(
                                    &rvf_path, &pubkey,
                                ) {
                                    Ok(valid) => Some(valid),
                                    Err(_) => Some(false),
                                }
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    let proto_result = ChainVerifyResult {
                        valid: result.valid,
                        event_count: result.event_count,
                        errors: result.errors,
                        signature_verified,
                    };
                    Response::success(serde_json::to_value(proto_result).unwrap())
                } else {
                    Response::error("chain not enabled")
                }
            }
            #[cfg(not(feature = "exochain"))]
            Response::error("exochain feature not enabled")
        }
        "chain.export" => {
            #[cfg(feature = "exochain")]
            {
                let export_params: ChainExportParams = serde_json::from_value(params)
                    .unwrap_or(ChainExportParams {
                        format: "json".into(),
                        output: None,
                    });
                let k = kernel.read().await;
                if let Some(cm) = k.chain_manager() {
                    match export_params.format.as_str() {
                        "rvf" => {
                            let default_path =
                                protocol::runtime_dir().join("chain").join("export.rvf");
                            let output_path = export_params
                                .output
                                .map(std::path::PathBuf::from)
                                .unwrap_or(default_path);
                            match cm.save_to_rvf(&output_path) {
                                Ok(()) => Response::success(serde_json::json!({
                                    "format": "rvf",
                                    "path": output_path.display().to_string(),
                                })),
                                Err(e) => Response::error(format!("RVF export failed: {e}")),
                            }
                        }
                        _ => {
                            let events = cm.tail(0);
                            let infos: Vec<ChainEventInfo> = events
                                .iter()
                                .map(|e| {
                                    let hash_hex: String =
                                        e.hash.iter().map(|b| format!("{b:02x}")).collect();
                                    ChainEventInfo {
                                        sequence: e.sequence,
                                        chain_id: e.chain_id,
                                        timestamp: e.timestamp.to_rfc3339(),
                                        source: e.source.clone(),
                                        kind: e.kind.clone(),
                                        hash: hash_hex,
                                        detail: String::new(),
                                    }
                                })
                                .collect();
                            Response::success(serde_json::to_value(infos).unwrap())
                        }
                    }
                } else {
                    Response::error("chain not enabled")
                }
            }
            #[cfg(not(feature = "exochain"))]
            Response::error("exochain feature not enabled")
        }
        "resource.tree" => {
            #[cfg(feature = "exochain")]
            {
                let k = kernel.read().await;
                if let Some(tm) = k.tree_manager() {
                    let tree = tm.tree().lock().unwrap();
                    let mut nodes: Vec<ResourceNodeInfo> = tree
                        .iter()
                        .map(|(id, node)| {
                            let hash_hex: String =
                                node.merkle_hash.iter().map(|b| format!("{b:02x}")).collect();
                            ResourceNodeInfo {
                                id: id.to_string(),
                                kind: format!("{:?}", node.kind),
                                parent: node.parent.as_ref().map(|p| p.to_string()),
                                children: node.children.iter().map(|c| c.to_string()).collect(),
                                metadata: serde_json::to_value(&node.metadata).unwrap_or_default(),
                                merkle_hash: hash_hex,
                                scoring: None, // tree listing omits scoring for brevity
                            }
                        })
                        .collect();
                    nodes.sort_by(|a, b| a.id.cmp(&b.id));
                    Response::success(serde_json::to_value(nodes).unwrap())
                } else {
                    Response::error("resource tree not enabled")
                }
            }
            #[cfg(not(feature = "exochain"))]
            Response::error("exochain feature not enabled")
        }
        "resource.inspect" => {
            #[cfg(feature = "exochain")]
            {
                let inspect_params: ResourceInspectParams = match serde_json::from_value(params) {
                    Ok(p) => p,
                    Err(e) => return Response::error(format!("invalid params: {e}")),
                };
                let k = kernel.read().await;
                if let Some(tm) = k.tree_manager() {
                    let rid = exo_resource_tree::ResourceId::new(&inspect_params.path);
                    // Extract node data under lock, then drop lock before get_scoring
                    let node_data = {
                        let tree = tm.tree().lock().unwrap();
                        tree.get(&rid).map(|node| {
                            let hash_hex: String =
                                node.merkle_hash.iter().map(|b| format!("{b:02x}")).collect();
                            (
                                node.id.to_string(),
                                format!("{:?}", node.kind),
                                node.parent.as_ref().map(|p| p.to_string()),
                                node.children.iter().map(|c| c.to_string()).collect::<Vec<_>>(),
                                serde_json::to_value(&node.metadata).unwrap_or_default(),
                                hash_hex,
                            )
                        })
                    }; // tree lock dropped here
                    if let Some((id, kind, parent, children, metadata, hash_hex)) = node_data {
                        // Now safe to call get_scoring (it acquires its own lock)
                        let scoring = tm.get_scoring(&rid).map(|s| {
                            let composite = s.trust * 0.25
                                + s.performance * 0.20
                                + s.reliability * 0.20
                                + s.velocity * 0.15
                                + s.reward * 0.10
                                + (1.0 - s.difficulty) * 0.10;
                            ResourceScoreResult {
                                path: inspect_params.path.clone(),
                                trust: s.trust,
                                performance: s.performance,
                                difficulty: s.difficulty,
                                reward: s.reward,
                                reliability: s.reliability,
                                velocity: s.velocity,
                                composite,
                            }
                        });
                        let info = ResourceNodeInfo {
                            id,
                            kind,
                            parent,
                            children,
                            metadata,
                            merkle_hash: hash_hex,
                            scoring,
                        };
                        Response::success(serde_json::to_value(info).unwrap())
                    } else {
                        Response::error(format!("resource not found: {}", inspect_params.path))
                    }
                } else {
                    Response::error("resource tree not enabled")
                }
            }
            #[cfg(not(feature = "exochain"))]
            Response::error("exochain feature not enabled")
        }
        "resource.stats" => {
            #[cfg(feature = "exochain")]
            {
                let k = kernel.read().await;
                if let Some(tm) = k.tree_manager() {
                    let tree = tm.tree().lock().unwrap();
                    let hash_hex: String =
                        tree.root_hash().iter().map(|b| format!("{b:02x}")).collect();
                    let mut namespaces = 0usize;
                    let mut services = 0usize;
                    let mut agents = 0usize;
                    let mut devices = 0usize;
                    for (_, node) in tree.iter() {
                        match node.kind {
                            exo_resource_tree::ResourceKind::Namespace => namespaces += 1,
                            exo_resource_tree::ResourceKind::Service => services += 1,
                            exo_resource_tree::ResourceKind::Agent => agents += 1,
                            exo_resource_tree::ResourceKind::Device => devices += 1,
                            _ => {}
                        }
                    }
                    let result = ResourceStatsResult {
                        total_nodes: tree.len(),
                        root_hash: hash_hex,
                        namespaces,
                        services,
                        agents,
                        devices,
                    };
                    Response::success(serde_json::to_value(result).unwrap())
                } else {
                    Response::error("resource tree not enabled")
                }
            }
            #[cfg(not(feature = "exochain"))]
            Response::error("exochain feature not enabled")
        }
        "agent.register" => handle_agent_register(params, kernel).await,
        "node.register" => handle_node_register(params, kernel).await,
        "node.identity" => handle_node_identity(params, kernel).await,
        "substrate.read" => handle_substrate_read(params, kernel).await,
        "substrate.list" => handle_substrate_list(params, kernel).await,
        "substrate.publish" => handle_substrate_publish(params, kernel).await,
        "substrate.canonical_publish_payload" => {
            handle_substrate_canonical_publish_payload(params, kernel).await
        }
        "substrate.notify" => handle_substrate_notify(params, kernel).await,
        "control.set_enabled" => handle_control_set_enabled(params, kernel).await,
        "control.list" => handle_control_list(params, kernel).await,
        "llm.prompt" => handle_llm_prompt(params, kernel).await,
        "terminal.spawn" => handle_terminal_spawn(params, kernel).await,
        "terminal.write" => handle_terminal_write(params, kernel).await,
        "terminal.resize" => handle_terminal_resize(params, kernel).await,
        "terminal.close" => handle_terminal_close(params, kernel).await,
        "agent.spawn" => {
            let spawn_params: AgentSpawnParams = match serde_json::from_value(params) {
                Ok(p) => p,
                Err(e) => return Response::error(format!("invalid params: {e}")),
            };
            let k = kernel.read().await;
            let request = clawft_kernel::SpawnRequest {
                agent_id: spawn_params.agent_id,
                capabilities: None,
                parent_pid: spawn_params.parent_pid,
                env: std::collections::HashMap::new(),
                backend: None,
            };

            // Create inbox via A2ARouter before spawning
            let a2a = k.a2a_router().clone();
            let cron = k.cron_service().clone();
            #[cfg(feature = "exochain")]
            let chain = k.chain_manager().cloned();

            // K3: Build ToolRegistry with reference implementations
            let tool_registry: std::sync::Arc<clawft_kernel::ToolRegistry> = {
                let mut registry = clawft_kernel::ToolRegistry::new();
                registry.register(std::sync::Arc::new(clawft_kernel::FsReadFileTool::new()));
                registry.register(std::sync::Arc::new(clawft_kernel::AgentSpawnTool::new(
                    k.process_table().clone(),
                )));
                std::sync::Arc::new(registry)
            };

            // Use spawn_and_run to actually execute the agent work loop
            let process_table = k.process_table().clone();
            match k.supervisor().spawn_and_run(request, {
                let a2a_clone = a2a.clone();
                let cron_clone = cron.clone();
                let pt_clone = process_table;
                let tool_reg_clone = tool_registry.clone();
                #[cfg(feature = "exochain")]
                let chain_clone = chain.clone();
                #[cfg(feature = "exochain")]
                let gate: Option<std::sync::Arc<dyn clawft_kernel::GateBackend>> = {
                    use clawft_kernel::{
                        GovernanceGate, GovernanceBranch, GovernanceRule, RuleSeverity,
                    };
                    let mut g = GovernanceGate::new(0.8, false);
                    if let Some(ref cm) = chain {
                        g = g.with_chain(cm.clone());
                    }
                    g = g
                        .add_rule(GovernanceRule {
                            id: "exec-guard".into(),
                            description: "Block high-risk exec actions".into(),
                            branch: GovernanceBranch::Judicial,
                            severity: RuleSeverity::Blocking,
                            active: true,
                            reference_url: None,
                            sop_category: None,
                        })
                        .add_rule(GovernanceRule {
                            id: "cron-warn".into(),
                            description: "Warn on cron modifications".into(),
                            branch: GovernanceBranch::Executive,
                            severity: RuleSeverity::Warning,
                            active: true,
                            reference_url: None,
                            sop_category: None,
                        });
                    Some(std::sync::Arc::new(g))
                };
                move |pid, cancel| {
                    let inbox = a2a_clone.create_inbox(pid);
                    async move {
                        clawft_kernel::agent_loop::kernel_agent_loop(
                            pid,
                            cancel,
                            inbox,
                            a2a_clone,
                            cron_clone,
                            pt_clone,
                            Some(tool_reg_clone),
                            #[cfg(feature = "exochain")]
                            chain_clone,
                            #[cfg(feature = "exochain")]
                            gate,
                        )
                        .await
                    }
                }
            }) {
                Ok(result) => {
                    k.event_log().info("agent", format!("spawned {} (PID {})", result.agent_id, result.pid));
                    let spawn_result = AgentSpawnResult {
                        pid: result.pid,
                        agent_id: result.agent_id,
                    };
                    Response::success(serde_json::to_value(spawn_result).unwrap())
                }
                Err(e) => Response::error(format!("spawn failed: {e}")),
            }
        }
        "agent.stop" => {
            let stop_params: AgentStopParams = match serde_json::from_value(params) {
                Ok(p) => p,
                Err(e) => return Response::error(format!("invalid params: {e}")),
            };
            let k = kernel.read().await;
            match k.supervisor().stop(stop_params.pid, stop_params.graceful) {
                Ok(()) => {
                    k.event_log().info("agent", format!("stopped PID {}", stop_params.pid));
                    Response::success(serde_json::json!("ok"))
                }
                Err(e) => Response::error(format!("stop failed: {e}")),
            }
        }
        "agent.restart" => {
            let restart_params: AgentRestartParams = match serde_json::from_value(params) {
                Ok(p) => p,
                Err(e) => return Response::error(format!("invalid params: {e}")),
            };
            let k = kernel.read().await;
            match k.supervisor().restart(restart_params.pid) {
                Ok(result) => {
                    let _ = k.process_table().update_state(result.pid, clawft_kernel::ProcessState::Running);
                    k.event_log().info("agent", format!("restarted {} (PID {} -> {})", result.agent_id, restart_params.pid, result.pid));
                    let spawn_result = AgentSpawnResult {
                        pid: result.pid,
                        agent_id: result.agent_id,
                    };
                    Response::success(serde_json::to_value(spawn_result).unwrap())
                }
                Err(e) => Response::error(format!("restart failed: {e}")),
            }
        }
        "agent.inspect" => {
            let pid = params
                .get("pid")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let k = kernel.read().await;
            match k.supervisor().inspect(pid) {
                Ok(entry) => {
                    let topics = k
                        .a2a_router()
                        .topic_router()
                        .topics_for_pid(pid);
                    let result = AgentInspectResult {
                        pid: entry.pid,
                        agent_id: entry.agent_id,
                        state: entry.state.to_string(),
                        memory_bytes: entry.resource_usage.memory_bytes,
                        cpu_time_ms: entry.resource_usage.cpu_time_ms,
                        messages_sent: entry.resource_usage.messages_sent,
                        tool_calls: entry.resource_usage.tool_calls,
                        topics,
                        parent_pid: entry.parent_pid,
                        can_spawn: entry.capabilities.can_spawn,
                        can_ipc: entry.capabilities.can_ipc,
                        can_exec_tools: entry.capabilities.can_exec_tools,
                        can_network: entry.capabilities.can_network,
                    };
                    Response::success(serde_json::to_value(result).unwrap())
                }
                Err(e) => Response::error(format!("inspect failed: {e}")),
            }
        }
        "agent.list" => {
            let k = kernel.read().await;
            let agents = k.supervisor().list_agents();
            let mut infos: Vec<ProcessInfo> = agents
                .iter()
                .map(|e| ProcessInfo {
                    pid: e.pid,
                    agent_id: e.agent_id.clone(),
                    state: e.state.to_string(),
                    memory_bytes: e.resource_usage.memory_bytes,
                    cpu_time_ms: e.resource_usage.cpu_time_ms,
                    parent_pid: e.parent_pid,
                })
                .collect();
            infos.sort_by_key(|e| e.pid);
            Response::success(serde_json::to_value(infos).unwrap())
        }
        "agent.send" => {
            let send_params: AgentSendParams = match serde_json::from_value(params) {
                Ok(p) => p,
                Err(e) => return Response::error(format!("invalid params: {e}")),
            };
            let k = kernel.read().await;

            // Try to parse message as JSON; fall back to text payload
            let payload = match serde_json::from_str::<serde_json::Value>(&send_params.message) {
                Ok(v) => clawft_kernel::MessagePayload::Json(v),
                Err(_) => clawft_kernel::MessagePayload::Text(send_params.message.clone()),
            };
            let msg = clawft_kernel::KernelMessage::new(
                0, // from kernel (PID 0)
                clawft_kernel::MessageTarget::Process(send_params.pid),
                payload,
            );

            // Route through A2ARouter with chain-logged delivery
            let a2a = k.a2a_router().clone();

            #[cfg(feature = "exochain")]
            let send_result = {
                let chain = k.chain_manager();
                a2a.send_checked(msg, chain.map(|c| c.as_ref())).await
            };
            #[cfg(not(feature = "exochain"))]
            let send_result = a2a.send(msg).await;

            match send_result {
                Ok(()) => {
                    k.event_log().info("ipc", format!("message sent to PID {}", send_params.pid));

                    // Wait briefly for a response from the agent
                    // Create a temporary inbox for kernel PID 0 if not already present
                    // (it may already exist from a previous call — A2ARouter replaces it)
                    let mut reply_rx = a2a.create_inbox(0);
                    match tokio::time::timeout(
                        std::time::Duration::from_secs(2),
                        reply_rx.recv(),
                    )
                    .await
                    {
                        Ok(Some(reply)) => {
                            let reply_value = match &reply.payload {
                                clawft_kernel::MessagePayload::Json(v) => v.clone(),
                                clawft_kernel::MessagePayload::Text(t) => {
                                    serde_json::json!({"text": t})
                                }
                                _ => serde_json::json!({"payload": "non-json"}),
                            };
                            Response::success(reply_value)
                        }
                        Ok(None) | Err(_) => {
                            // No reply within timeout — still report send success
                            Response::success(serde_json::json!("sent"))
                        }
                    }
                }
                Err(e) => Response::error(format!("send failed: {e}")),
            }
        }
        "cron.add" => {
            let cron_params: CronAddParams = match serde_json::from_value(params) {
                Ok(p) => p,
                Err(e) => return Response::error(format!("invalid params: {e}")),
            };
            let k = kernel.read().await;
            let job = match k.cron_service().add_job(
                cron_params.name,
                cron_params.interval_secs,
                cron_params.command,
                cron_params.target_pid,
            ) {
                Ok(job) => job,
                Err(e) => return Response::error(format!("cron.add failed: {e}")),
            };

            k.event_log().info("cron", format!("job added: {} ({}s)", job.name, job.interval_secs));
            let info = CronJobInfo {
                id: job.id,
                name: job.name,
                interval_secs: job.interval_secs,
                command: job.command,
                target_pid: job.target_pid,
                enabled: job.enabled,
                fire_count: job.fire_count,
                last_fired: job.last_fired.map(|t| t.to_rfc3339()),
                created_at: job.created_at.to_rfc3339(),
            };
            Response::success(serde_json::to_value(info).unwrap())
        }
        "cron.list" => {
            let k = kernel.read().await;
            let jobs = k.cron_service().list_jobs();
            let infos: Vec<CronJobInfo> = jobs
                .iter()
                .map(|j| CronJobInfo {
                    id: j.id.clone(),
                    name: j.name.clone(),
                    interval_secs: j.interval_secs,
                    command: j.command.clone(),
                    target_pid: j.target_pid,
                    enabled: j.enabled,
                    fire_count: j.fire_count,
                    last_fired: j.last_fired.map(|t| t.to_rfc3339()),
                    created_at: j.created_at.to_rfc3339(),
                })
                .collect();
            Response::success(serde_json::to_value(infos).unwrap())
        }
        "cron.remove" => {
            let remove_params: CronRemoveParams = match serde_json::from_value(params) {
                Ok(p) => p,
                Err(e) => return Response::error(format!("invalid params: {e}")),
            };
            let k = kernel.read().await;
            match k.cron_service().remove_job(&remove_params.id) {
                Ok(Some(job)) => {
                    #[cfg(feature = "exochain")]
                    if let Some(cm) = k.chain_manager() {
                        cm.append(
                            "cron",
                            "cron.remove",
                            Some(serde_json::json!({
                                "job_id": job.id,
                                "name": job.name,
                            })),
                        );
                    }
                    k.event_log().info("cron", format!("job removed: {}", job.name));
                    Response::success(serde_json::json!({"removed": true, "job_id": job.id}))
                }
                Ok(None) => Response::error(format!("cron job not found: {}", remove_params.id)),
                Err(e) => Response::error(format!("cron remove denied: {e}")),
            }
        }
        "ipc.topics" => {
            let k = kernel.read().await;
            let a2a = k.a2a_router();
            let topics = a2a.topic_router().list_topics();
            let infos: Vec<IpcTopicInfo> = topics
                .iter()
                .map(|(topic, _count)| {
                    let subs = a2a.topic_router().subscribers(topic);
                    IpcTopicInfo {
                        topic: topic.clone(),
                        subscriber_count: subs.len(),
                        subscribers: subs,
                    }
                })
                .collect();
            Response::success(serde_json::to_value(infos).unwrap())
        }
        "ipc.subscribe" => {
            let sub_params: IpcSubscribeParams = match serde_json::from_value(params) {
                Ok(p) => p,
                Err(e) => return Response::error(format!("invalid params: {e}")),
            };
            let k = kernel.read().await;

            // Validate that the PID exists and is running
            if k.supervisor().inspect(sub_params.pid).is_err() {
                return Response::error(format!(
                    "PID {} not found — spawn an agent first",
                    sub_params.pid,
                ));
            }

            let a2a = k.a2a_router();
            a2a.topic_router().subscribe(sub_params.pid, &sub_params.topic);
            k.event_log().info(
                "ipc",
                format!("PID {} subscribed to '{}'", sub_params.pid, sub_params.topic),
            );

            #[cfg(feature = "exochain")]
            if let Some(cm) = k.chain_manager() {
                cm.append(
                    "ipc",
                    "ipc.subscribe",
                    Some(serde_json::json!({
                        "pid": sub_params.pid,
                        "topic": sub_params.topic,
                    })),
                );
            }

            Response::success(serde_json::json!("subscribed"))
        }
        "ipc.publish" => {
            let pub_params: IpcPublishParams = match serde_json::from_value(params) {
                Ok(p) => p,
                Err(e) => return Response::error(format!("invalid params: {e}")),
            };
            let k = kernel.read().await;

            // Authenticate the publish if the caller supplied an
            // identity. Missing actor_id is accepted for bring-up
            // parity with existing clients; missing signature given
            // an actor_id is unauthorized.
            if let Some(actor_id) = pub_params.actor_id.as_ref() {
                let sig = match pub_params.signature.as_ref() {
                    Some(s) => s,
                    None => {
                        return Response::error(
                            "actor_id provided but signature missing".to_string(),
                        );
                    }
                };
                let ts = pub_params.ts.unwrap_or(0);
                let payload = clawft_kernel::publish_payload(
                    &pub_params.topic,
                    &pub_params.message,
                    ts,
                    actor_id,
                );
                if let Err(e) = verify_agent_signature(&k, actor_id, sig, &payload) {
                    return Response::error(format!("unauthorized: {e}"));
                }
            } else {
                tracing::warn!(
                    topic = %pub_params.topic,
                    "ipc.publish with no actor_id — anonymous publish accepted (bring-up only)"
                );
            }

            let a2a = k.a2a_router().clone();

            // Count all registered subscribers for reporting
            let subscriber_count = a2a
                .topic_router()
                .subscribers(&pub_params.topic)
                .len();

            // Build message payload
            let payload = match serde_json::from_str::<serde_json::Value>(&pub_params.message) {
                Ok(v) => clawft_kernel::MessagePayload::Json(v),
                Err(_) => clawft_kernel::MessagePayload::Text(pub_params.message.clone()),
            };
            let msg = clawft_kernel::KernelMessage::new(
                0, // from kernel (PID 0)
                clawft_kernel::MessageTarget::Topic(pub_params.topic.clone()),
                payload,
            );

            #[cfg(feature = "exochain")]
            let send_result = {
                let chain = k.chain_manager();
                a2a.send_checked(msg, chain.map(|c| c.as_ref())).await
            };
            #[cfg(not(feature = "exochain"))]
            let send_result = a2a.send(msg).await;

            match send_result {
                Ok(()) => {
                    k.event_log().info(
                        "ipc",
                        format!(
                            "published to topic '{}' ({} subscriber{})",
                            pub_params.topic,
                            subscriber_count,
                            if subscriber_count == 1 { "" } else { "s" },
                        ),
                    );
                    Response::success(serde_json::json!({
                        "topic": pub_params.topic,
                        "subscribers": subscriber_count,
                    }))
                }
                Err(e) => Response::error(format!("publish failed: {e}")),
            }
        }
        "resource.score" => {
            #[cfg(feature = "exochain")]
            {
                let score_params: ResourceScoreParams = match serde_json::from_value(params) {
                    Ok(p) => p,
                    Err(e) => return Response::error(format!("invalid params: {e}")),
                };
                let k = kernel.read().await;
                if let Some(tm) = k.tree_manager() {
                    let rid = exo_resource_tree::ResourceId::new(&score_params.path);
                    if let Some(scoring) = tm.get_scoring(&rid) {
                        let composite = scoring.trust * 0.25
                            + scoring.performance * 0.20
                            + scoring.reliability * 0.20
                            + scoring.velocity * 0.15
                            + scoring.reward * 0.10
                            + (1.0 - scoring.difficulty) * 0.10;
                        let result = ResourceScoreResult {
                            path: score_params.path,
                            trust: scoring.trust,
                            performance: scoring.performance,
                            difficulty: scoring.difficulty,
                            reward: scoring.reward,
                            reliability: scoring.reliability,
                            velocity: scoring.velocity,
                            composite,
                        };
                        Response::success(serde_json::to_value(result).unwrap())
                    } else {
                        Response::error(format!("no scoring for: {}", score_params.path))
                    }
                } else {
                    Response::error("resource tree not enabled")
                }
            }
            #[cfg(not(feature = "exochain"))]
            Response::error("exochain feature not enabled")
        }
        "resource.rank" => {
            #[cfg(feature = "exochain")]
            {
                let rank_params: ResourceRankParams = serde_json::from_value(params)
                    .unwrap_or(ResourceRankParams { count: 10 });
                let k = kernel.read().await;
                if let Some(tm) = k.tree_manager() {
                    // Equal weight across all 6 dimensions
                    let weights = [1.0, 1.0, 0.5, 0.5, 1.0, 0.5];
                    let ranked = tm.rank_by_score(&weights, rank_params.count);
                    let entries: Vec<ResourceRankEntry> = ranked
                        .iter()
                        .map(|(rid, score)| ResourceRankEntry {
                            path: rid.to_string(),
                            score: *score,
                        })
                        .collect();
                    Response::success(serde_json::to_value(entries).unwrap())
                } else {
                    Response::error("resource tree not enabled")
                }
            }
            #[cfg(not(feature = "exochain"))]
            Response::error("exochain feature not enabled")
        }
        // ── Assessment endpoints ──────────────────────────────
        "assess.run" => {
            let run_params: crate::protocol::AssessRunParams =
                match serde_json::from_value(params) {
                    Ok(p) => p,
                    Err(e) => return Response::error(format!("invalid params: {e}")),
                };
            let dir = run_params.dir.as_deref().unwrap_or(".");
            let k = kernel.read().await;

            #[cfg(feature = "exochain")]
            if let Some(cm) = k.chain_manager() {
                cm.append(
                    "assessment",
                    "assess.run",
                    Some(serde_json::json!({
                        "scope": run_params.scope,
                        "format": run_params.format,
                        "dir": dir,
                    })),
                );
            }

            k.event_log().info(
                "assessment",
                format!("assess.run scope={} format={} dir={}", run_params.scope, run_params.format, dir),
            );

            Response::success(serde_json::json!({
                "status": "accepted",
                "scope": run_params.scope,
                "format": run_params.format,
                "dir": dir,
                "message": "Assessment queued. Results will be available via assess.status."
            }))
        }
        "assess.status" => {
            let k = kernel.read().await;

            #[cfg(feature = "exochain")]
            if let Some(cm) = k.chain_manager() {
                cm.append("assessment", "assess.status", None);
            }

            k.event_log().info("assessment", "assess.status queried".to_string());

            // Return stub until AssessmentService is fully wired
            Response::success(serde_json::json!({
                "status": "idle",
                "last_run": null,
                "findings": [],
                "message": "No assessment report available yet."
            }))
        }
        "assess.link" => {
            let link_params: crate::protocol::AssessLinkParams =
                match serde_json::from_value(params) {
                    Ok(p) => p,
                    Err(e) => return Response::error(format!("invalid params: {e}")),
                };
            let k = kernel.read().await;

            #[cfg(feature = "exochain")]
            if let Some(cm) = k.chain_manager() {
                cm.append(
                    "assessment",
                    "assess.link",
                    Some(serde_json::json!({
                        "name": link_params.name,
                        "location": link_params.location,
                    })),
                );
            }

            k.event_log().info(
                "assessment",
                format!("peer linked: {} -> {}", link_params.name, link_params.location),
            );

            Response::success(serde_json::json!({
                "linked": true,
                "name": link_params.name,
                "location": link_params.location
            }))
        }
        "assess.peers" => {
            let k = kernel.read().await;

            #[cfg(feature = "exochain")]
            if let Some(cm) = k.chain_manager() {
                cm.append("assessment", "assess.peers", None);
            }

            k.event_log().info("assessment", "assess.peers queried".to_string());

            // Return empty list until AssessmentService manages peers
            Response::success(serde_json::json!({
                "peers": []
            }))
        }
        "assess.compare" => {
            let cmp_params: crate::protocol::AssessCompareParams =
                match serde_json::from_value(params) {
                    Ok(p) => p,
                    Err(e) => return Response::error(format!("invalid params: {e}")),
                };
            let k = kernel.read().await;

            #[cfg(feature = "exochain")]
            if let Some(cm) = k.chain_manager() {
                cm.append(
                    "assessment",
                    "assess.compare",
                    Some(serde_json::json!({
                        "peer": cmp_params.peer,
                    })),
                );
            }

            k.event_log().info(
                "assessment",
                format!("assess.compare peer={}", cmp_params.peer),
            );

            Response::success(serde_json::json!({
                "status": "accepted",
                "peer": cmp_params.peer,
                "message": "Comparison queued. Results will be available via assess.status."
            }))
        }
        // ── Assessment mesh endpoints ────────────────────────
        "assess.mesh.status" => {
            let k = kernel.read().await;

            #[cfg(feature = "exochain")]
            if let Some(cm) = k.chain_manager() {
                cm.append("assessment", "assess.mesh.status", None);
            }

            k.event_log().info("assessment", "assess.mesh.status queried".to_string());

            {
                let svc = k.assessment_service();
                if let Some(mc) = svc.mesh_coordinator() {
                    let peers: Vec<crate::protocol::AssessMeshPeerInfo> = mc
                        .peer_states()
                        .into_iter()
                        .map(|p| crate::protocol::AssessMeshPeerInfo {
                            node_id: p.node_id,
                            project_name: p.project_name,
                            last_assessment: p.last_assessment,
                            finding_count: p.finding_count,
                            analyzer_count: p.analyzer_count,
                            last_gossip: p.last_gossip,
                        })
                        .collect();
                    let peer_count = peers.len();
                    Response::success(serde_json::to_value(
                        crate::protocol::AssessMeshStatusResult {
                            mesh_enabled: true,
                            node_id: Some(mc.node_id().to_string()),
                            project_name: Some(mc.project_name().to_string()),
                            peer_count,
                            peers,
                        },
                    ).unwrap())
                } else {
                    Response::success(serde_json::to_value(
                        crate::protocol::AssessMeshStatusResult {
                            mesh_enabled: false,
                            node_id: None,
                            project_name: None,
                            peer_count: 0,
                            peers: vec![],
                        },
                    ).unwrap())
                }
            }
        }
        "assess.mesh.gossip" => {
            let k = kernel.read().await;

            #[cfg(feature = "exochain")]
            if let Some(cm) = k.chain_manager() {
                cm.append("assessment", "assess.mesh.gossip", None);
            }

            k.event_log().info("assessment", "assess.mesh.gossip triggered".to_string());

            {
                let svc = k.assessment_service();
                if let Some(mc) = svc.mesh_coordinator() {
                    if let Some(report) = svc.get_latest() {
                        let gossip = mc.build_gossip(&report);
                        mc.set_pending_broadcast(gossip);
                        Response::success(serde_json::to_value(
                            crate::protocol::AssessMeshGossipResult {
                                sent: true,
                                message: "Gossip broadcast queued.".into(),
                            },
                        ).unwrap())
                    } else {
                        Response::success(serde_json::to_value(
                            crate::protocol::AssessMeshGossipResult {
                                sent: false,
                                message: "No assessment report available to gossip.".into(),
                            },
                        ).unwrap())
                    }
                } else {
                    Response::error("mesh coordination not enabled")
                }
            }
        }
        // ── ECC methods ────────────────────────────────────────
        #[cfg(feature = "ecc")]
        "ecc.status" => {
            let k = kernel.read().await;
            let hnsw_count = k.ecc_hnsw().map(|h| h.len()).unwrap_or(0);
            let tick_info = k.ecc_tick().map(|_t| {
                serde_json::json!({
                    "interval_ms": 50,
                    "running": true,
                })
            });
            let causal_stats = k.ecc_causal().map(|g| {
                serde_json::json!({
                    "nodes": g.node_count(),
                    "edges": g.edge_count(),
                })
            });
            let crossref_count = k.ecc_crossrefs().map(|c| c.count()).unwrap_or(0);
            Response::success(serde_json::json!({
                "enabled": k.ecc_hnsw().is_some(),
                "hnsw_entries": hnsw_count,
                "cognitive_tick": tick_info,
                "causal_graph": causal_stats,
                "crossref_count": crossref_count,
            }))
        }
        #[cfg(feature = "ecc")]
        "ecc.calibrate" => {
            let k = kernel.read().await;
            if let Some(cal) = k.ecc_calibration() {
                Response::success(serde_json::to_value(cal).unwrap_or_default())
            } else {
                Response::error("ECC not initialized or calibration not complete")
            }
        }
        #[cfg(feature = "ecc")]
        "ecc.search" => {
            let k = kernel.read().await;
            if let Some(hnsw) = k.ecc_hnsw() {
                Response::success(serde_json::json!({
                    "available": true,
                    "entries": hnsw.len(),
                    "search_count": hnsw.search_count(),
                    "hint": "use agent tools (ecc/search) for semantic queries — vector embedding required"
                }))
            } else {
                Response::error("HNSW not available")
            }
        }
        #[cfg(feature = "ecc")]
        "ecc.causal" => {
            let node_id = params.get("node_id").and_then(|v| v.as_u64());
            let depth = params.get("depth").and_then(|v| v.as_u64()).unwrap_or(3) as usize;
            let k = kernel.read().await;
            if let Some(graph) = k.ecc_causal() {
                if let Some(id) = node_id {
                    let neighbors = graph.traverse_forward(id, depth);
                    Response::success(serde_json::json!({
                        "node_id": id,
                        "depth": depth,
                        "reachable": neighbors.len(),
                        "nodes": neighbors,
                    }))
                } else {
                    Response::success(serde_json::json!({
                        "nodes": graph.node_count(),
                        "edges": graph.edge_count(),
                        "components": graph.connected_components().len(),
                    }))
                }
            } else {
                Response::error("causal graph not available")
            }
        }
        #[cfg(feature = "ecc")]
        "ecc.tick" => {
            let k = kernel.read().await;
            let cal = k.ecc_calibration();
            if k.ecc_tick().is_some() {
                Response::success(serde_json::json!({
                    "running": true,
                    "interval_ms": cal.map(|c| c.tick_interval_ms).unwrap_or(50),
                    "spectral_capable": cal.map(|c| c.spectral_capable).unwrap_or(false),
                }))
            } else {
                Response::success(serde_json::json!({"running": false}))
            }
        }
        #[cfg(feature = "ecc")]
        "ecc.crossrefs" => {
            let k = kernel.read().await;
            if let Some(crossrefs) = k.ecc_crossrefs() {
                Response::success(serde_json::json!({
                    "count": crossrefs.count(),
                }))
            } else {
                Response::error("crossref store not available")
            }
        }
        // ── Custody attestation ───────────────────────────────────────
        "custody.attest" => {
            #[cfg(feature = "exochain")]
            {
                let k = kernel.read().await;
                if let Some(cm) = k.chain_manager() {
                    #[cfg(feature = "ecc")]
                    let (vector_count, epoch, content_hash) = {
                        if let Some(vb) = k.ecc_vector_backend() {
                            let count = vb.len() as u64;
                            let ep = vb.current_epoch();
                            let hash_input = format!("vector-ids:count={count}:epoch={ep}");
                            let hash = blake3::hash(hash_input.as_bytes());
                            (count, ep, hash.to_hex().to_string())
                        } else if let Some(hnsw) = k.ecc_hnsw() {
                            let count = hnsw.len() as u64;
                            let hash_input = format!("hnsw-ids:count={count}");
                            let hash = blake3::hash(hash_input.as_bytes());
                            (count, 0u64, hash.to_hex().to_string())
                        } else {
                            (0u64, 0u64, "none".to_string())
                        }
                    };
                    #[cfg(not(feature = "ecc"))]
                    let (vector_count, epoch, content_hash) = (0u64, 0u64, "none".to_string());

                    match cm.generate_attestation(vector_count, epoch, &content_hash) {
                        Some(att) => {
                            let sig_hex: String = att.signature.iter().map(|b| format!("{b:02x}")).collect();
                            let result = crate::protocol::CustodyAttestResult {
                                device_id: att.device_id,
                                epoch: att.epoch,
                                chain_head: att.chain_head,
                                chain_depth: att.chain_depth,
                                vector_count: att.vector_count,
                                content_hash: att.content_hash,
                                timestamp: att.timestamp,
                                signature: sig_hex,
                            };
                            Response::success(serde_json::to_value(result).unwrap())
                        }
                        None => Response::error("no signing key configured — attestation requires Ed25519 key"),
                    }
                } else {
                    Response::error("chain not enabled")
                }
            }
            #[cfg(not(feature = "exochain"))]
            Response::error("exochain feature not enabled")
        }

        // ── Host revocation ──────────────────────────────────────────
        "mesh.revoke" => {
            let revoke_params: crate::protocol::MeshRevokeParams = match serde_json::from_value(params) {
                Ok(p) => p,
                Err(e) => return Response::error(format!("invalid params: {e}")),
            };
            let k = kernel.read().await;
            let list = k.revocation_list();
            let added = list.revoke_host(&revoke_params.host_id, &revoke_params.reason);

            #[cfg(feature = "exochain")]
            if let Some(cm) = k.chain_manager() {
                cm.append("mesh", "host.revoked", Some(serde_json::json!({
                    "host_id": revoke_params.host_id,
                    "reason": revoke_params.reason,
                })));
            }

            Response::success(serde_json::json!({
                "host_id": revoke_params.host_id,
                "added": added,
            }))
        }
        "mesh.unrevoke" => {
            let unrevoke_params: crate::protocol::MeshUnrevokeParams = match serde_json::from_value(params) {
                Ok(p) => p,
                Err(e) => return Response::error(format!("invalid params: {e}")),
            };
            let k = kernel.read().await;
            let list = k.revocation_list();
            let removed = list.unrevoke_host(&unrevoke_params.host_id);

            #[cfg(feature = "exochain")]
            if let Some(cm) = k.chain_manager() {
                cm.append("mesh", "host.unrevoked", Some(serde_json::json!({
                    "host_id": unrevoke_params.host_id,
                })));
            }

            Response::success(serde_json::json!({
                "host_id": unrevoke_params.host_id,
                "removed": removed,
            }))
        }
        "mesh.revoked" => {
            let k = kernel.read().await;
            let list = k.revocation_list();
            let entries: Vec<crate::protocol::RevokedHostInfo> = list
                .list_revoked()
                .into_iter()
                .map(|h| crate::protocol::RevokedHostInfo {
                    host_id: h.host_id,
                    revoked_at: h.revoked_at,
                    reason: h.reason,
                })
                .collect();
            Response::success(serde_json::to_value(entries).unwrap())
        }

        "ping" => Response::success(serde_json::json!("pong")),

        // ── Workspace methods ─────────────────────────────────────────
        "workspace.create" => {
            use clawft_core::workspace::WorkspaceManager;

            let name = match params["name"].as_str() {
                Some(n) => n,
                None => return Response::error("missing required param: name"),
            };
            let dir = params["dir"]
                .as_str()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into()));

            match WorkspaceManager::new() {
                Ok(mut mgr) => match mgr.create(name, &dir) {
                    Ok(path) => Response::success(serde_json::json!({
                        "name": name,
                        "path": path.display().to_string(),
                    })),
                    Err(e) => Response::error(format!("workspace create failed: {e}")),
                },
                Err(e) => Response::error(format!("workspace manager init failed: {e}")),
            }
        }
        "workspace.list" => {
            use clawft_core::workspace::WorkspaceManager;

            match WorkspaceManager::new() {
                Ok(mgr) => {
                    let entries: Vec<serde_json::Value> = mgr
                        .list()
                        .iter()
                        .map(|e| {
                            let exists = e.path.join(".clawft").is_dir();
                            serde_json::json!({
                                "name": e.name,
                                "path": e.path.display().to_string(),
                                "status": if exists { "ok" } else { "MISSING" },
                                "last_accessed": e.last_accessed
                                    .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                                    .unwrap_or_else(|| "-".into()),
                            })
                        })
                        .collect();
                    Response::success(serde_json::to_value(entries).unwrap())
                }
                Err(e) => Response::error(format!("workspace manager init failed: {e}")),
            }
        }
        "workspace.load" => {
            use clawft_core::workspace::WorkspaceManager;

            let name_or_path = params["name_or_path"]
                .as_str()
                .or_else(|| params["name"].as_str());
            let name_or_path = match name_or_path {
                Some(n) => n,
                None => return Response::error("missing required param: name_or_path"),
            };

            match WorkspaceManager::new() {
                Ok(mut mgr) => match mgr.load(name_or_path) {
                    Ok(path) => Response::success(serde_json::json!({
                        "path": path.display().to_string(),
                    })),
                    Err(e) => Response::error(format!("workspace load failed: {e}")),
                },
                Err(e) => Response::error(format!("workspace manager init failed: {e}")),
            }
        }
        "workspace.status" => {
            use clawft_core::workspace::{WorkspaceManager, discover_workspace};

            let ws_path = match discover_workspace() {
                Some(p) => p,
                None => return Response::error("no workspace found"),
            };

            match WorkspaceManager::new() {
                Ok(mgr) => match mgr.status(&ws_path) {
                    Ok(status) => Response::success(serde_json::json!({
                        "name": status.name,
                        "path": status.path.display().to_string(),
                        "session_count": status.session_count,
                        "has_config": status.has_config,
                        "has_clawft_md": status.has_clawft_md,
                    })),
                    Err(e) => Response::error(format!("workspace status failed: {e}")),
                },
                Err(e) => Response::error(format!("workspace manager init failed: {e}")),
            }
        }
        "workspace.delete" => {
            use clawft_core::workspace::WorkspaceManager;

            let name = match params["name"].as_str() {
                Some(n) => n,
                None => return Response::error("missing required param: name"),
            };

            match WorkspaceManager::new() {
                Ok(mut mgr) => match mgr.delete(name) {
                    Ok(()) => Response::success(serde_json::json!({ "deleted": name })),
                    Err(e) => Response::error(format!("workspace delete failed: {e}")),
                },
                Err(e) => Response::error(format!("workspace manager init failed: {e}")),
            }
        }
        "workspace.config.set" => {
            use clawft_core::workspace::discover_workspace;

            let key = match params["key"].as_str() {
                Some(k) => k,
                None => return Response::error("missing required param: key"),
            };
            let value = match params["value"].as_str() {
                Some(v) => v,
                None => return Response::error("missing required param: value"),
            };

            let ws_path = match discover_workspace() {
                Some(p) => p,
                None => return Response::error("no workspace found"),
            };

            let config_path = ws_path.join(".clawft").join("config.json");
            let mut config: serde_json::Value = if config_path.exists() {
                match std::fs::read_to_string(&config_path) {
                    Ok(content) => serde_json::from_str(&content).unwrap_or(serde_json::json!({})),
                    Err(_) => serde_json::json!({}),
                }
            } else {
                serde_json::json!({})
            };

            // Navigate/create nested keys using dot notation
            let parts: Vec<&str> = key.split('.').collect();
            let mut current = &mut config;
            for part in &parts[..parts.len() - 1] {
                if !current.is_object() {
                    *current = serde_json::json!({});
                }
                current = current
                    .as_object_mut()
                    .unwrap()
                    .entry(*part)
                    .or_insert_with(|| serde_json::json!({}));
            }
            let last = parts[parts.len() - 1];
            // Parse value as JSON primitive
            let json_value = if value == "true" {
                serde_json::Value::Bool(true)
            } else if value == "false" {
                serde_json::Value::Bool(false)
            } else if let Ok(n) = value.parse::<i64>() {
                serde_json::Value::Number(n.into())
            } else {
                serde_json::Value::String(value.into())
            };
            if !current.is_object() {
                *current = serde_json::json!({});
            }
            current.as_object_mut().unwrap().insert(last.into(), json_value);

            match std::fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap()) {
                Ok(()) => Response::success(serde_json::json!({ "key": key, "value": value })),
                Err(e) => Response::error(format!("failed to write config: {e}")),
            }
        }
        "workspace.config.get" => {
            use clawft_core::workspace::discover_workspace;

            let key = match params["key"].as_str() {
                Some(k) => k,
                None => return Response::error("missing required param: key"),
            };

            let ws_path = match discover_workspace() {
                Some(p) => p,
                None => return Response::error("no workspace found"),
            };

            let config_path = ws_path.join(".clawft").join("config.json");
            if !config_path.exists() {
                return Response::success(serde_json::Value::Null);
            }

            match std::fs::read_to_string(&config_path) {
                Ok(content) => {
                    let config: serde_json::Value =
                        serde_json::from_str(&content).unwrap_or(serde_json::json!({}));
                    let mut current = &config;
                    for part in key.split('.') {
                        match current.get(part) {
                            Some(v) => current = v,
                            None => return Response::success(serde_json::Value::Null),
                        }
                    }
                    Response::success(current.clone())
                }
                Err(e) => Response::error(format!("failed to read config: {e}")),
            }
        }
        "workspace.config.reset" => {
            use clawft_core::workspace::discover_workspace;

            let ws_path = match discover_workspace() {
                Some(p) => p,
                None => return Response::error("no workspace found"),
            };

            let config_path = ws_path.join(".clawft").join("config.json");
            match std::fs::write(&config_path, "{}\n") {
                Ok(()) => Response::success(serde_json::json!({ "reset": true })),
                Err(e) => Response::error(format!("failed to reset config: {e}")),
            }
        }

        other => Response::error(format!("unknown method: {other}")),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn socket_path_resolves() {
        let path = crate::protocol::socket_path();
        assert!(path.to_string_lossy().ends_with("kernel.sock"));
    }
}
