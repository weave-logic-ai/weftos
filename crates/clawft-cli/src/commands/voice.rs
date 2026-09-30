//! Voice CLI commands: setup, test-mic, test-speak, talk, wake.
//!
//! Provides `weft voice <subcommand>` for managing the voice pipeline.
//! All commands are gated behind the `voice` feature flag.

use clap::{Args, Subcommand};

use super::voice_daemon_brain::BrainMode;

#[derive(Debug, Args)]
pub struct VoiceArgs {
    #[command(subcommand)]
    pub command: VoiceCommand,
}

#[derive(Debug, Subcommand)]
pub enum VoiceCommand {
    /// Set up voice pipeline (download models, test audio).
    Setup,

    /// Test microphone input.
    TestMic {
        /// Duration in seconds.
        #[arg(short, long, default_value = "5")]
        duration: u32,
    },

    /// Test speaker output.
    TestSpeak {
        /// Text to speak.
        #[arg(short, long, default_value = "Hello, I am ClawFT.")]
        text: String,
    },

    /// Start Talk Mode (continuous voice conversation).
    Talk {
        /// Which brain speaks the reply (WEFT-614 lite): `auto` prefers a
        /// reachable kernel daemon's own §W2.1 voice-loop reply, falling
        /// back to the local bare-Hermes brain; `daemon` forces it (errors
        /// if unreachable); `local` forces the pre-WEFT-614 local brain.
        #[arg(long, value_enum, default_value = "auto")]
        brain: BrainMode,
    },

    /// Listen-only mode (§W1.4): record + decompose + classify every turn WITHOUT
    /// the LLM brain or audio out, rendering the live process stream (partials,
    /// endpoint fire, finalized turn + its decomposition) as it happens. Turns
    /// anchor via `agent.turn.record`, so `weft voice watch` lights up in parallel.
    Listen,

    /// Watch the live voice process for a conversation (§W1.4): committed turns,
    /// their labels, and the per-utterance voice decomposition, rendered as they
    /// land via the ~1 Hz `conversation.graph` poll. Read-only — no mic, no
    /// brain; run it alongside `weft voice talk` or an `agent.turn.record` feed.
    Watch {
        /// Conversation id to watch (the Talk-Mode conv, e.g. `weft-talk`).
        #[arg(default_value = "weft-talk")]
        conv_id: String,
        /// Emit each newly-committed node's full record as JSON (one per line).
        #[arg(long)]
        json: bool,
        /// Committed-state poll cadence in milliseconds (ADR-067 D2 ~1 Hz).
        #[arg(long, default_value = "1000")]
        interval: u64,
    },

    /// Start the wake word daemon (listen for "Hey Weft").
    Wake,

    /// Install the wake word daemon as a system service.
    InstallService {
        /// Service manager to use (auto-detected if not specified).
        /// Supported values: "systemd", "launchd", "schtasks".
        #[arg(long)]
        manager: Option<String>,
    },
}

pub async fn handle_voice(args: VoiceArgs) -> anyhow::Result<()> {
    match args.command {
        VoiceCommand::Setup => {
            handle_setup().await?;
        }
        VoiceCommand::TestMic { duration } => {
            handle_test_mic(duration).await?;
        }
        VoiceCommand::TestSpeak { text } => {
            println!("Speaker test not yet implemented (requires sherpa-rs TTS)");
            println!("Would speak: \"{}\"", text);
        }
        VoiceCommand::Talk { brain } => {
            handle_talk(brain).await?;
        }
        VoiceCommand::Listen => {
            handle_listen().await?;
        }
        VoiceCommand::Watch {
            conv_id,
            json,
            interval,
        } => {
            super::voice_watch::handle_watch(conv_id, json, interval).await?;
        }
        VoiceCommand::Wake => {
            handle_wake().await?;
        }
        VoiceCommand::InstallService { manager } => {
            handle_install_service(manager).await?;
        }
    }
    Ok(())
}

/// `weft voice setup` — download canonical STT/TTS/VAD models with SHA-256
/// verify and stderr progress (WEFT-215).
///
/// Cache directory: `~/.clawft/models/voice/`. Models without a real (non-
/// placeholder) SHA-256 pin are **refused** rather than fetched blindly.
/// Verified cache hits are reused with no network.
///
/// Silero VAD (`silero-vad-v5`) is additionally staged to
/// `~/.weftos/models/silero-vad/silero_vad.onnx` for
/// `clawft_voice_onnx::SileroVoiceness` discovery (WEFT-644).
async fn handle_setup() -> anyhow::Result<()> {
    use clawft_plugin::voice::{
        EnsureOutcome, ModelDownloadManager, finish_stderr_progress, is_placeholder_hash,
        stderr_progress_line,
    };

    println!("=== ClawFT Voice Setup ===\n");

    let cache_dir = ModelDownloadManager::default_cache_dir();
    println!("Model cache: {}\n", cache_dir.display());
    let mgr = ModelDownloadManager::new(cache_dir);

    let models = ModelDownloadManager::all_canonical_models();
    if models.is_empty() {
        anyhow::bail!("no canonical voice models registered");
    }

    let mut ready = 0u32;
    let mut skipped = 0u32;
    let mut failed = 0u32;

    for model in &models {
        if is_placeholder_hash(model.sha256_hint.as_deref()) {
            eprintln!(
                "  SKIP {}: missing or placeholder SHA-256 — refusing download \
                 (pin a real 64-hex digest in ModelInfo::sha256_hint)",
                model.id
            );
            skipped += 1;
            continue;
        }

        if mgr.is_cached(model) {
            let path = mgr.model_path(&model.id);
            println!("  CACHED {}: {}", model.id, path.display());
            maybe_stage_silero_vad(&model.id, &path);
            ready += 1;
            continue;
        }

        let mut line = stderr_progress_line(&model.id);
        let progress: &mut dyn FnMut(u64, u64) = &mut line;
        match mgr.ensure_model(model, Some(progress)).await {
            Ok(EnsureOutcome::Downloaded(path)) => {
                finish_stderr_progress();
                println!("  DOWNLOADED {}: {}", model.id, path.display());
                maybe_stage_silero_vad(&model.id, &path);
                ready += 1;
            }
            Ok(EnsureOutcome::Cached(path)) => {
                finish_stderr_progress();
                println!("  CACHED {}: {}", model.id, path.display());
                maybe_stage_silero_vad(&model.id, &path);
                ready += 1;
            }
            Err(e) => {
                finish_stderr_progress();
                eprintln!("  FAIL {}: {e}", model.id);
                failed += 1;
            }
        }
    }

    println!();
    println!("Summary: {ready} ready, {skipped} skipped (no hash), {failed} failed");
    if skipped > 0 && ready == 0 {
        println!(
            "Note: catalog entries still need real SHA-256 pins before production \
             downloads (WEFT-215; SC-7 signed manifests cover runtime bundle verify)."
        );
    }
    if failed > 0 {
        anyhow::bail!("voice setup failed for {failed} model(s)");
    }

    println!("\n=== Voice setup complete ===");
    Ok(())
}

/// WEFT-644: mirror catalog cache → `~/.weftos/models/silero-vad/silero_vad.onnx`
/// so SileroVoiceness auto-discovery finds the weights without an env override.
fn maybe_stage_silero_vad(model_id: &str, cache_path: &std::path::Path) {
    if model_id != "silero-vad-v5" || !cache_path.is_file() {
        return;
    }
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let dest_dir = home.join(".weftos/models/silero-vad");
    let dest = dest_dir.join("silero_vad.onnx");
    if dest.is_file() {
        // Already staged — leave in place (operator may have pinned a variant).
        println!("  STAGED silero-vad (already present): {}", dest.display());
        return;
    }
    if let Err(e) = std::fs::create_dir_all(&dest_dir) {
        eprintln!("  WARN could not create {}: {e}", dest_dir.display());
        return;
    }
    match std::fs::copy(cache_path, &dest) {
        Ok(_) => println!("  STAGED silero-vad → {}", dest.display()),
        Err(e) => eprintln!("  WARN could not stage silero-vad to {}: {e}", dest.display()),
    }
}

/// Run Talk Mode — the native ECC graph-walk voice conversation (ADR-062).
///
/// Constructs the full `weft talk` assembly via `clawft_voice_talk` — the P2
/// Talk-Mode loop orchestrator + the concrete native stack (parakeet STT,
/// smart-turn endpoint, ECAPA speaker, Kokoro+Orpheus TTS, LocalProvider→Hermes
/// brain) over a cpal mic+speaker — and runs the spoken loop until Ctrl+C.
///
/// The graph constructs cleanly with no weights staged (each native component
/// degrades gracefully); real transcription/synthesis needs the staged ONNX
/// models and the `voice-onnx` build feature (`weft` built with `--features
/// voice-onnx`) plus live Hermes at `:8090`.
async fn handle_talk(brain: BrainMode) -> anyhow::Result<()> {
    use clawft_voice_talk::{TalkConfig, live::run_live_observed_with_llm};
    use tokio_util::sync::CancellationToken;

    println!("=== ClawFT Talk Mode (native ECC graph-walk) ===");
    println!("Mic + speaker via cpal; Hermes brain at :8090.");
    println!("Press Ctrl+C to exit.\n");

    let config = TalkConfig {
        conv_id: "weft-talk".into(),
        base_system: "You are a terse, friendly voice assistant.".into(),
        ..TalkConfig::default()
    };

    // WEFT-614 lite: which brain generates the spoken reply, and whether the
    // recorder should ALSO mirror it (daemon-brain mode already anchors the
    // reply through the daemon's own sink — mirroring again would double it).
    let (llm_override, mirror_assistant) =
        super::voice_daemon_brain::resolve_brain(brain, &config.conv_id).await?;

    // Mirror the user turn (always) — and the assistant reply, unless the
    // daemon brain already anchored it — to the kernel daemon's
    // `agent.turn.record` RPC so the exchange lands on the substrate JSONL
    // and anchors to the witness chain / HNSW / causal graph per
    // `[kernel.agent]`. Best-effort: without a running daemon the
    // conversation still works, unanchored.
    let recorder = spawn_turn_recorder(config.conv_id.clone(), mirror_assistant).await;
    match &recorder {
        Some(_) => println!("Turn anchoring: ON (daemon connected — turns recorded on chain)"),
        None => println!(
            "Turn anchoring: OFF (no kernel daemon — start one with `weaver kernel start` \
             to record turns on the chain)"
        ),
    }

    let cancel = CancellationToken::new();
    let cancel_for_signal = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            println!("\nReceived Ctrl+C, shutting down Talk Mode...");
            cancel_for_signal.cancel();
        }
    });

    run_live_observed_with_llm(config, None, cancel, recorder, llm_override)
        .await
        .map_err(|e| anyhow::anyhow!("talk mode: {e}"))?;

    println!("Talk Mode ended.");
    Ok(())
}

/// Run Listen-only Mode (§W1.4): the native capture + decomposition + classify
/// path with the LLM brain and audio-out disabled (`TalkConfig::listen_only`).
/// Renders the live process stream via [`LiveRenderObserver`] and mirrors each
/// finalized turn to `agent.turn.record` (fanned out with the recorder), so the
/// forest fills and `weft voice watch` lights up in parallel — no reply, no TTS.
async fn handle_listen() -> anyhow::Result<()> {
    use clawft_voice_talk::{ConversationObserver, TalkConfig, live::run_live_observed};
    use tokio_util::sync::CancellationToken;

    use super::voice_watch::{FanOutObserver, LiveRenderObserver};

    println!("=== ClawFT Listen Mode (observe-only — no brain, no audio out) ===");
    println!("Records + decomposes + classifies every turn and renders it live.");
    println!("Press Ctrl+C to exit.\n");

    let config = TalkConfig {
        conv_id: "weft-talk".into(),
        listen_only: true,
        ..TalkConfig::default()
    };

    // Anchor finalized turns to the daemon (best-effort) AND render them
    // live. Listen mode always mirrors both roles (unaffected by WEFT-614's
    // daemon-brain mirror suppression, which only applies to `weft voice talk`).
    let recorder = spawn_turn_recorder(config.conv_id.clone(), true).await;
    match &recorder {
        Some(_) => println!("Turn anchoring: ON (turns recorded on the chain)"),
        None => println!(
            "Turn anchoring: OFF (no kernel daemon — start one with `weaver kernel start` \
             to record turns; the live stream still renders)"
        ),
    }

    // `run_live_observed` takes ONE extra observer; fan out recorder + renderer.
    let mut observers: Vec<std::sync::Arc<dyn ConversationObserver>> =
        vec![std::sync::Arc::new(LiveRenderObserver)];
    if let Some(recorder) = recorder {
        observers.push(recorder);
    }
    let observer: std::sync::Arc<dyn ConversationObserver> =
        std::sync::Arc::new(FanOutObserver(observers));

    let cancel = CancellationToken::new();
    let cancel_for_signal = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            println!("\nReceived Ctrl+C, shutting down Listen Mode...");
            cancel_for_signal.cancel();
        }
    });

    run_live_observed(config, None, cancel, Some(observer))
        .await
        .map_err(|e| anyhow::anyhow!("listen mode: {e}"))?;

    println!("Listen Mode ended.");
    Ok(())
}

/// Probe the microphone the live Talk-Mode capture would open: list input
/// devices, record for `duration` seconds, and print a per-half-second level
/// meter plus a verdict against the Talk-Mode VAD threshold (-45 dBFS).
async fn handle_test_mic(duration: u32) -> anyhow::Result<()> {
    use clawft_voice_talk::live::{list_input_devices, mic_probe};
    use clawft_voice_talk::NoiseFloor;

    /// Talk-Mode's adaptive-gate onset margin over the tracked floor
    /// (`VAD_NOISE_MARGIN_DB` in clawft-channels::voice::talkmode). Kept in sync:
    /// the live gate also AND-gates spectral voiceness, so a window here reading
    /// over-threshold on energy alone may still be silence at the mic if it's
    /// broadband — the summary line notes that.
    const VAD_NOISE_MARGIN_DB: f32 = 4.0;

    println!("Input devices:");
    for name in list_input_devices() {
        println!("  - {name}");
    }
    println!("\nRecording {duration}s from the default device — SPEAK NOW...\n");

    let report = tokio::task::spawn_blocking(move || mic_probe(None, duration))
        .await?
        .map_err(|e| anyhow::anyhow!("mic probe: {e}"))?;

    println!(
        "Opened: {} ({} Hz, {} ch)\n",
        report.device, report.native_rate, report.channels
    );

    // Replay the ACTUAL Talk-Mode voiced gate over the 500 ms probe windows so
    // the columns are exactly what the live loop would decide at this gain:
    // startup calibration seeds the floor from the first window (assumed
    // non-speech), then each window prints its RMS, the tracked floor, and the
    // gate verdict. This is the round-7 self-diagnosis lever.
    let mut gate = NoiseFloor::new(VAD_NOISE_MARGIN_DB, report.native_rate);
    let window_len = u64::from(report.native_rate) / 2; // ~500 ms per probe window
    println!("   time     rms      floor    gate");
    for (i, db) in report.windows_dbfs.iter().enumerate() {
        let voiced = gate.classify(*db, window_len);
        let floor = gate.floor_dbfs();
        let bar_len = ((db + 90.0).max(0.0) / 2.0) as usize;
        let verdict = if i == 0 {
            "calibrating"
        } else if voiced {
            "<< VOICE"
        } else {
            "silence"
        };
        println!(
            "  {:>4.1}s  {:>7.1}  {:>7.1}   {:<11} {}",
            (i as f32 + 1.0) * 0.5,
            db,
            floor,
            verdict,
            "█".repeat(bar_len.min(30)),
        );
    }
    println!();
    if report.windows_dbfs.is_empty() {
        println!("NO AUDIO DELIVERED — the device produced no samples at all.");
        println!("Check System Settings → Privacy & Security → Microphone for your terminal.");
        return Ok(());
    }

    let final_floor = gate.floor_dbfs();
    let onset = final_floor + VAD_NOISE_MARGIN_DB;
    let headroom = report.peak_dbfs - onset;
    println!(
        "Calibrated floor {final_floor:.1} dBFS → voice onset ~{onset:.1} dBFS. \
         Loudest window {:.1} dBFS ({headroom:+.1} dB vs onset).",
        report.peak_dbfs
    );
    if headroom < 3.0 {
        println!(
            "Speech barely clears the gate — RAISE input volume in System Settings → \
             Sound → Input (or move closer). Marginal gain also hurts speaker ID."
        );
    } else if final_floor > -35.0 {
        println!(
            "Floor is high ({final_floor:.1} dBFS) — the room is loud or gain is hot. \
             The gate calibrated to it and speech clears it, but consider lowering gain \
             slightly if background sounds trip the gate."
        );
    } else {
        println!("Gain looks healthy: speech clears the adaptive gate with margin to spare.");
    }
    Ok(())
}

/// One queued post from the recorder observer: a durable turn for
/// `agent.turn.record`, or a surface-only process event for
/// `voice.trace.append` (the client half of the `weft voice watch` trace).
enum RecorderPost {
    Turn {
        role: &'static str,
        content: String,
        voice_analysis: Option<serde_json::Value>,
    },
    Trace {
        kind: &'static str,
        detail: serde_json::Value,
    },
}

/// Non-blocking [`ConversationObserver`] that forwards user / committed-
/// assistant turns into an unbounded queue; a background poster task posts
/// each to the daemon's `agent.turn.record` RPC. Process events (cue tones,
/// TTS render timing, gates, interrupts) ride the same queue to
/// `voice.trace.append`, so `weft voice watch` shows the client half of the
/// turn alongside the daemon's own decision trace.
///
/// `mirror_assistant` gates the `CommittedReply` arm (WEFT-614 lite): in
/// daemon-brain mode the reply IS the daemon's own voice-loop output, which
/// the daemon already anchored through its own sink when it committed the
/// node — mirroring it again here would double-anchor the same turn. The
/// user turn is always mirrored regardless.
struct TurnRecordObserver {
    tx: tokio::sync::mpsc::UnboundedSender<RecorderPost>,
    mirror_assistant: bool,
}

impl clawft_voice_talk::ConversationObserver for TurnRecordObserver {
    fn observe(&self, event: clawft_voice_talk::ConversationEvent) {
        use clawft_voice_talk::ConversationEvent;
        let post = match event {
            ConversationEvent::UserTurn {
                text,
                voice_analysis,
                ..
            } => {
                // Wave 1 §W1.2: serialize the per-utterance decomposition onto
                // the wire so the daemon's `index_turn` stores it as a sibling
                // key and merges its emotion axis (tier:"voice"). Best-effort —
                // a serialization failure drops only the record, never the turn.
                let va = voice_analysis
                    .as_deref()
                    .and_then(|v| serde_json::to_value(v).ok());
                RecorderPost::Turn {
                    role: "user",
                    content: text,
                    voice_analysis: va,
                }
            }
            ConversationEvent::CommittedReply { answer } if self.mirror_assistant => {
                RecorderPost::Turn {
                    role: "assistant",
                    content: answer,
                    voice_analysis: None,
                }
            }
            // Client-side process events → the watch trace (surface-only,
            // never anchored).
            ConversationEvent::CueTone { kind } => RecorderPost::Trace {
                kind: "cue",
                detail: serde_json::json!({ "kind": kind.label() }),
            },
            ConversationEvent::TtsRendered { ms, chunks } => RecorderPost::Trace {
                kind: "tts",
                detail: serde_json::json!({ "ms": ms, "chunks": chunks }),
            },
            ConversationEvent::TurnGated { ref reason } => RecorderPost::Trace {
                kind: "gate",
                detail: serde_json::json!({ "reason": reason }),
            },
            ConversationEvent::Interrupted => RecorderPost::Trace {
                kind: "interrupt",
                detail: serde_json::json!({}),
            },
            // Speculative acks are superseded by the committed reply; a
            // committed reply with mirroring suppressed is skipped — the
            // daemon already anchored it. Per-frame events (partials, level
            // meter) are too chatty for the trace.
            _ => return,
        };
        // Receiver gone (daemon died and the poster exited) — drop silently;
        // anchoring is best-effort by design.
        let _ = self.tx.send(post);
    }
}

/// Connect to the kernel daemon and spawn the poster task. Returns `None`
/// (anchoring disabled) when no daemon is reachable. `mirror_assistant` see
/// [`TurnRecordObserver`].
async fn spawn_turn_recorder(
    conv_id: String,
    mirror_assistant: bool,
) -> Option<std::sync::Arc<dyn clawft_voice_talk::ConversationObserver>> {
    use clawft_rpc::{DaemonClient, Request};

    let mut client = crate::commands::daemon_conn::connect_opt().await?;
    // Warn once if this binary and the daemon were built from different trees
    // (covers `weft voice talk` and `weft voice listen`, which both anchor
    // turns through this recorder).
    super::daemon_guard::warn_on_build_mismatch(&mut client).await;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<RecorderPost>();

    tokio::spawn(async move {
        while let Some(post) = rx.recv().await {
            let (method, params) = match post {
                RecorderPost::Turn {
                    role,
                    content,
                    voice_analysis,
                } => {
                    let mut turn = serde_json::json!({ "role": role, "content": content });
                    if let Some(va) = voice_analysis {
                        turn["voice_analysis"] = va;
                    }
                    (
                        "agent.turn.record",
                        serde_json::json!({
                            "conv_id": conv_id,
                            "channel": "voice.talk",
                            "turns": [turn],
                        }),
                    )
                }
                RecorderPost::Trace { kind, detail } => (
                    "voice.trace.append",
                    serde_json::json!({
                        "conv_id": conv_id,
                        "kind": kind,
                        "detail": detail,
                    }),
                ),
            };
            // One reconnect attempt per post: a dropped socket loses no
            // event, a stopped daemon ends anchoring for the session.
            let mut posted = false;
            for attempt in 0..2 {
                let request = Request::with_params(method, params.clone());
                match client.call(request).await {
                    Ok(resp) => {
                        if let Err(e) = resp.into_result() {
                            tracing::warn!(error = %e, method, "voice recorder post rejected");
                        }
                        posted = true;
                        break;
                    }
                    Err(e) if attempt == 0 => {
                        tracing::debug!(error = %e, method, "voice recorder transport error; reconnecting");
                        match crate::commands::daemon_conn::connect_opt().await {
                            Some(c) => client = c,
                            None => break,
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, method, "voice recorder transport error after reconnect");
                    }
                }
            }
            if !posted && crate::commands::daemon_conn::connect_opt().await.is_none() {
                tracing::warn!("kernel daemon gone — voice turn anchoring stopped for this session");
                return;
            }
        }
    });

    Some(std::sync::Arc::new(TurnRecordObserver {
        tx,
        mirror_assistant,
    }))
}

/// Run the wake word daemon -- continuously listen for "Hey Weft".
///
/// Creates a [`WakeDaemon`] and runs until Ctrl+C.
///
/// **WEFT-671:** sole live external caller of `clawft_plugin::voice`
/// (wake only). Talk Mode itself is `clawft_voice_talk` — see
/// `handle_talk`.
///
/// **WEFT-216:** detector backend is **stub**. rustpotter is blocked
/// (candle-core); no model is shipped; no mic capture in the daemon.
/// Detection will not fire. Concrete alternative (future): OpenWakeWord
/// ONNX via `clawft-voice-onnx`. See
/// `docs/plans/wave-0k-WEFT-216-result.md`.
async fn handle_wake() -> anyhow::Result<()> {
    use clawft_plugin::traits::CancellationToken;
    use clawft_plugin::voice::{WakeDaemon, WakeWordConfig};

    println!("=== ClawFT Wake Word Daemon ===");
    println!("Backend: stub (WEFT-216 — rustpotter blocked; no model shipped)");
    println!("No microphone capture; process_frame never reports detection.");
    println!("Press Ctrl+C to exit.\n");

    let config = WakeWordConfig::default();
    let mut daemon = WakeDaemon::new(config)?;
    let cancel = CancellationToken::new();

    // Handle Ctrl+C for graceful shutdown.
    let cancel_for_signal = cancel.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
        println!("\nReceived Ctrl+C, shutting down wake word daemon...");
        cancel_for_signal.cancel();
    });

    daemon.run(cancel).await?;

    println!("Wake word daemon stopped.");
    Ok(())
}

/// Install the wake word daemon as a platform service.
///
/// Auto-detects the platform (Linux/macOS/Windows) and installs the
/// appropriate service definition:
/// - Linux: systemd user unit (`scripts/clawft-wake.service`)
/// - macOS: launchd plist (`scripts/com.clawft.wake.plist`)
/// - Windows: Task Scheduler via `schtasks` (WEFT-220 — final route)
///
/// Windows does not use a Windows Service (SCM). The supported path is a
/// per-user logon task so the wake daemon can access the microphone in the
/// interactive session. See `docs/guides/voice.md` § Wake word service.
#[cfg(not(target_arch = "wasm32"))]
async fn handle_install_service(manager: Option<String>) -> anyhow::Result<()> {
    let detected = manager.unwrap_or_else(detect_service_manager);

    match detected.as_str() {
        "systemd" => install_systemd_service().await,
        "launchd" => install_launchd_service().await,
        "schtasks" => install_schtasks_service().await,
        other => {
            println!("Unsupported service manager: {}", other);
            println!("Supported managers: systemd (Linux), launchd (macOS), schtasks (Windows).");
            println!();
            print_windows_manual_route(&resolve_weft_binary());
            Ok(())
        }
    }
}

#[cfg(target_arch = "wasm32")]
async fn handle_install_service(_manager: Option<String>) -> anyhow::Result<()> {
    println!("Service installation is not available on WASM targets.");
    Ok(())
}

/// Task Scheduler task name for the wake word daemon (Windows, WEFT-220).
const SCHTASKS_TASK_NAME: &str = "ClawftWake";

/// Detect the service manager for the current platform.
fn detect_service_manager() -> String {
    if cfg!(target_os = "macos") {
        "launchd".to_string()
    } else if cfg!(target_os = "linux") {
        "systemd".to_string()
    } else if cfg!(target_os = "windows") {
        "schtasks".to_string()
    } else {
        "unsupported".to_string()
    }
}

/// Resolve the `weft` binary path for service install (prefer this process).
fn resolve_weft_binary() -> std::path::PathBuf {
    std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("weft"))
}

/// Build the Task Scheduler `/TR` string: `"path\to\weft.exe" voice wake`.
///
/// Paths with spaces are double-quoted; `voice wake` are separate program args.
fn schtasks_task_run_string(exe: &std::path::Path) -> String {
    let exe_s = exe.display().to_string();
    if exe_s.contains(' ') {
        format!("\"{exe_s}\" voice wake")
    } else {
        format!("{exe_s} voice wake")
    }
}

/// Final documented Windows install route (manual Task Scheduler / schtasks).
///
/// Printed when automation fails or the platform is unsupported so operators
/// always have a complete, non-provisional procedure (WEFT-220).
fn print_windows_manual_route(exe: &std::path::Path) {
    let tr = schtasks_task_run_string(exe);
    println!("Windows wake service — final supported route (Task Scheduler / schtasks):");
    println!();
    println!("Automated (preferred):");
    println!("  weft voice install-service");
    println!("  weft voice install-service --manager schtasks");
    println!("  # or from a repo checkout:");
    println!("  powershell -ExecutionPolicy Bypass -File scripts/install-clawft-wake-schtasks.ps1");
    println!();
    println!("Manual schtasks (ONLOGON, current user, limited rights):");
    println!(
        "  schtasks /Create /TN \"{SCHTASKS_TASK_NAME}\" /TR \"{tr}\" /SC ONLOGON /RL LIMITED /F"
    );
    println!();
    println!("GUI (Task Scheduler):");
    println!("  1. Open Task Scheduler → Create Task…");
    println!("  2. Name: {SCHTASKS_TASK_NAME}");
    println!("  3. Trigger: At log on (your user)");
    println!("  4. Action: Start a program → {exe}", exe = exe.display());
    println!("     Arguments: voice wake");
    println!("  5. Conditions: allow start on AC or battery as you prefer");
    println!("  6. Settings: restart on failure optional");
    println!();
    println!("Manage:");
    println!("  schtasks /Run    /TN \"{SCHTASKS_TASK_NAME}\"   # start now");
    println!("  schtasks /End    /TN \"{SCHTASKS_TASK_NAME}\"   # stop");
    println!("  schtasks /Query  /TN \"{SCHTASKS_TASK_NAME}\" /V /FO LIST");
    println!("  schtasks /Delete /TN \"{SCHTASKS_TASK_NAME}\" /F");
    println!();
    println!("Notes:");
    println!("  - ONLOGON (user session) is intentional: wake needs the mic in an");
    println!("    interactive session. A Windows Service (SCM) is not supported.");
    println!("  - Ensure `weft` is on PATH or use the absolute path from this binary.");
    println!("  - Mic privacy: grant microphone access to the terminal / weft host.");
}

/// Install a systemd user service for the wake word daemon.
async fn install_systemd_service() -> anyhow::Result<()> {
    use std::path::PathBuf;

    let home =
        std::env::var("HOME").map_err(|_| anyhow::anyhow!("HOME environment variable not set"))?;
    let service_dir = PathBuf::from(&home)
        .join(".config")
        .join("systemd")
        .join("user");

    // Create the service directory if it doesn't exist.
    tokio::fs::create_dir_all(&service_dir).await?;

    let service_path = service_dir.join("clawft-wake.service");
    let service_content = include_str!("../../../../scripts/clawft-wake.service");

    tokio::fs::write(&service_path, service_content).await?;
    println!("Installed service file to: {}", service_path.display());

    // Try to enable and start the service.
    println!("Enabling clawft-wake.service...");
    let enable_result = tokio::process::Command::new("systemctl")
        .args(["--user", "enable", "clawft-wake.service"])
        .status()
        .await;

    match enable_result {
        Ok(status) if status.success() => {
            println!("Service enabled successfully.");
            println!();
            println!("Commands:");
            println!("  systemctl --user start clawft-wake   # Start now");
            println!("  systemctl --user stop clawft-wake    # Stop");
            println!("  systemctl --user status clawft-wake  # Check status");
            println!("  journalctl --user -u clawft-wake     # View logs");
        }
        _ => {
            println!("Could not enable service via systemctl.");
            println!("You may need to enable it manually:");
            println!("  systemctl --user enable clawft-wake.service");
            println!("  systemctl --user start clawft-wake.service");
        }
    }

    Ok(())
}

/// Install a launchd plist for the wake word daemon (macOS).
async fn install_launchd_service() -> anyhow::Result<()> {
    use std::path::PathBuf;

    let home =
        std::env::var("HOME").map_err(|_| anyhow::anyhow!("HOME environment variable not set"))?;
    let agents_dir = PathBuf::from(&home).join("Library").join("LaunchAgents");

    // Create the LaunchAgents directory if it doesn't exist.
    tokio::fs::create_dir_all(&agents_dir).await?;

    let plist_path = agents_dir.join("com.clawft.wake.plist");
    let plist_content = include_str!("../../../../scripts/com.clawft.wake.plist");

    tokio::fs::write(&plist_path, plist_content).await?;
    println!("Installed plist to: {}", plist_path.display());

    // Try to load the service.
    println!("Loading com.clawft.wake...");
    let load_result = tokio::process::Command::new("launchctl")
        .args(["load", &plist_path.to_string_lossy()])
        .status()
        .await;

    match load_result {
        Ok(status) if status.success() => {
            println!("Service loaded successfully.");
            println!();
            println!("Commands:");
            println!("  launchctl start com.clawft.wake   # Start now");
            println!("  launchctl stop com.clawft.wake    # Stop");
            println!("  launchctl list | grep clawft      # Check status");
        }
        _ => {
            println!("Could not load service via launchctl.");
            println!("You may need to load it manually:");
            println!("  launchctl load {}", plist_path.display());
        }
    }

    Ok(())
}

/// Install a Windows Task Scheduler logon task for the wake word daemon.
///
/// Final Windows route for WEFT-220: `schtasks` ONLOGON as the current user
/// (not a Windows Service). Falls back to printing the full manual procedure
/// if `schtasks` is missing or rejects the create.
#[cfg(not(target_arch = "wasm32"))]
async fn install_schtasks_service() -> anyhow::Result<()> {
    let exe = resolve_weft_binary();
    let tr = schtasks_task_run_string(&exe);

    println!("Installing Windows Task Scheduler task: {SCHTASKS_TASK_NAME}");
    println!("  Program: {}", exe.display());
    println!("  Arguments: voice wake");
    println!("  Trigger: ONLOGON (current user, LIMITED)");
    println!();

    // /F overwrites an existing task with the same name (idempotent reinstall).
    let create_result = tokio::process::Command::new("schtasks")
        .args([
            "/Create",
            "/TN",
            SCHTASKS_TASK_NAME,
            "/TR",
            &tr,
            "/SC",
            "ONLOGON",
            "/RL",
            "LIMITED",
            "/F",
        ])
        .output()
        .await;

    match create_result {
        Ok(output) if output.status.success() => {
            println!("Task '{SCHTASKS_TASK_NAME}' created successfully.");
            println!();
            println!("Commands:");
            println!("  schtasks /Run    /TN \"{SCHTASKS_TASK_NAME}\"   # Start now");
            println!("  schtasks /End    /TN \"{SCHTASKS_TASK_NAME}\"   # Stop");
            println!("  schtasks /Query  /TN \"{SCHTASKS_TASK_NAME}\" /V /FO LIST");
            println!("  schtasks /Delete /TN \"{SCHTASKS_TASK_NAME}\" /F");
            println!();
            println!("Optional: scripts/install-clawft-wake-schtasks.ps1 (same schtasks path).");
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            println!("schtasks /Create failed (exit {}).", output.status);
            if !stdout.trim().is_empty() {
                println!("{stdout}");
            }
            if !stderr.trim().is_empty() {
                println!("{stderr}");
            }
            println!();
            print_windows_manual_route(&exe);
        }
        Err(err) => {
            println!("Could not run schtasks ({err}).");
            println!("Is this a Windows host with Task Scheduler available?");
            println!();
            print_windows_manual_route(&exe);
        }
    }

    Ok(())
}

#[cfg(test)]
mod install_service_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn detect_service_manager_matches_host_os() {
        let m = detect_service_manager();
        #[cfg(target_os = "macos")]
        assert_eq!(m, "launchd");
        #[cfg(target_os = "linux")]
        assert_eq!(m, "systemd");
        #[cfg(target_os = "windows")]
        assert_eq!(m, "schtasks");
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        assert_eq!(m, "unsupported");
    }

    #[test]
    fn schtasks_tr_quotes_paths_with_spaces() {
        let p = Path::new(r"C:\Program Files\weft\weft.exe");
        let tr = schtasks_task_run_string(p);
        assert_eq!(tr, r#""C:\Program Files\weft\weft.exe" voice wake"#);
    }

    #[test]
    fn schtasks_tr_unquoted_when_no_spaces() {
        let p = Path::new(r"C:\weft\weft.exe");
        assert_eq!(schtasks_task_run_string(p), r"C:\weft\weft.exe voice wake");
    }

    #[test]
    fn schtasks_task_name_is_stable() {
        assert_eq!(SCHTASKS_TASK_NAME, "ClawftWake");
    }
}
