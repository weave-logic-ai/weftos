//! Mic source selection and the whisper / classify pipeline it feeds.
//!
//! Node ids are derived from each node's key (ADR-103 D11), so no node id
//! can be baked in as a default, and anything that can `node.register` and
//! publish can claim to be a mic. Selection therefore never picks by
//! ordering:
//!
//! 1. The operator pin is primary: `WHISPER_INPUT_NODE_ID`, else the
//!    `voice.mic_node_id` config field.
//! 2. Without a pin, auto-select ONLY when exactly one registered node has
//!    published under `sensor/mic`. With two or more, refuse and warn,
//!    naming the candidates and the pin settings.
//! 3. Once selected the supervisor keeps re-checking: a second candidate
//!    appearing is a warning, a chosen node that stops being registered is
//!    dropped and selection starts over.
//!
//! Follow-up: `node.register` carries no capabilities or roles today, so a
//! declared mic capability cannot be required for auto-select. Add one to
//! the registration payload and gate `candidates` on it.

use std::sync::Arc;
use std::time::{Duration, Instant};

use clawft_kernel::{Kernel, NodeRegistry, SubstrateService};
use clawft_platform::NativePlatform;
use tokio::sync::{RwLock, watch};
use tracing::{info, warn};

use crate::control::{ControlFlags, ControlIntent, ControlKind};

/// Env var that pins the mic node id (overrides `voice.mic_node_id`).
pub const MIC_NODE_ENV: &str = "WHISPER_INPUT_NODE_ID";

const POLL: Duration = Duration::from_secs(5);
const WARN_EVERY: Duration = Duration::from_secs(60);

type SharedKernel = Arc<RwLock<Kernel<NativePlatform>>>;

/// Outcome of one selection pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Operator pin. `registered` is false when no such node is registered.
    Pinned { id: String, registered: bool },
    /// Exactly one registered node publishes `sensor/mic`.
    Single(String),
    /// No registered node publishes `sensor/mic`.
    NoCandidate,
    /// Two or more candidates and no pin: refuse to choose.
    Ambiguous(Vec<String>),
}

impl Decision {
    /// Text for a WARN when this decision is not a clean, running choice.
    pub fn warning(&self) -> Option<String> {
        match self {
            Decision::Pinned { id, registered: false } => Some(format!(
                "mic node pinned to {id} ({MIC_NODE_ENV} / voice.mic_node_id) but no registered node has that id; \
                 the pipeline subscribes anyway, check the pin"
            )),
            Decision::NoCandidate => Some(format!(
                "no registered node is publishing sensor/mic; whisper and classify are NOT running. \
                 Set {MIC_NODE_ENV}=<node-id> (or voice.mic_node_id) to pin one"
            )),
            Decision::Ambiguous(c) => Some(format!(
                "{} registered nodes publish sensor/mic ({}); refusing to pick one. \
                 Pin the mic with {MIC_NODE_ENV}=<node-id> (or voice.mic_node_id)",
                c.len(),
                c.join(", ")
            )),
            _ => None,
        }
    }
}

/// Registered nodes that have published anything under `sensor/mic`,
/// sorted for display only (never used to choose).
///
/// Uses the daemon-internal [`SubstrateService::has_published_under`], so
/// it acts as no node principal and reads no data.
pub fn candidates(registry: &NodeRegistry, substrate: &SubstrateService) -> Vec<String> {
    let mut ids: Vec<String> = registry
        .list()
        .into_iter()
        .map(|n| n.node_id)
        .filter(|id| substrate.has_published_under(&format!("substrate/{id}/sensor/mic")))
        .collect();
    ids.sort();
    ids
}

/// One selection pass. The pin always wins; otherwise only a unique
/// candidate is chosen.
pub fn decide(
    pin: Option<&str>,
    registry: &NodeRegistry,
    substrate: &SubstrateService,
) -> Decision {
    if let Some(id) = pin.map(str::trim).filter(|s| !s.is_empty()) {
        return Decision::Pinned {
            registered: registry.contains(id),
            id: id.to_string(),
        };
    }
    let mut c = candidates(registry, substrate);
    match c.len() {
        0 => Decision::NoCandidate,
        1 => Decision::Single(c.remove(0)),
        _ => Decision::Ambiguous(c),
    }
}

/// The pin from env (first) or config.
pub fn pin_from(config_pin: Option<&str>) -> Option<String> {
    std::env::var(MIC_NODE_ENV)
        .ok()
        .or_else(|| config_pin.map(str::to_string))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

// ── pipeline ────────────────────────────────────────────────────────────

/// Running whisper + classify services for one mic source.
pub struct MicPipeline {
    source: String,
    whisper: Option<clawft_service_whisper::WhisperService>,
    classify: Option<clawft_service_classify::ClassifierService>,
}

impl MicPipeline {
    async fn stop(self) {
        if let Some(w) = self.whisper {
            w.shutdown().await;
        }
        if let Some(c) = self.classify {
            c.shutdown().await;
        }
    }
}

async fn publish_intent(
    kernel: &SharedKernel,
    daemon_node_id: &str,
    kind: ControlKind,
    target: &str,
    label: &str,
    enabled: bool,
) {
    let intent = ControlIntent {
        enabled,
        kind,
        target: target.to_string(),
        label: label.to_string(),
        updated_at_ms: crate::control::now_ms(),
    };
    let path = crate::control::intent_path(daemon_node_id, kind, target);
    let k = kernel.read().await;
    if let Err(e) =
        k.substrate_service()
            .publish_gated(Some(daemon_node_id), &path, intent.to_value())
    {
        warn!(error = %e, path = %path, "control: intent publish failed");
    }
}

/// Start classify and whisper for `source`. Each is independent: a failure
/// in one is logged accurately and does not skip the other. Service flags
/// are registered here, i.e. only once something is actually starting.
async fn start_pipeline(
    kernel: &SharedKernel,
    control_flags: &ControlFlags,
    daemon_node_id: &str,
    source: &str,
) -> MicPipeline {
    let pcm_chunk_target = format!("{source}/mic/pcm_chunk");
    let rms_target = format!("{source}/mic/rms");
    let source_flag = control_flags.register(ControlKind::Sensor, &pcm_chunk_target, true);
    let _rms_flag = control_flags.register(ControlKind::Sensor, &rms_target, true);
    let whisper_flag = control_flags.register(ControlKind::Service, "whisper", true);
    let classify_flag = control_flags.register(ControlKind::Service, "classify", true);

    let classify_output_path =
        format!("substrate/{daemon_node_id}/derived/classify/{source}/mic");
    let input_path = format!("substrate/{source}/sensor/mic/pcm_chunk");
    let (substrate, node_registry) = {
        let k = kernel.read().await;
        (k.substrate_service().clone(), k.node_registry().clone())
    };

    // Audio classifier (energy VAD): the whisper gate consumes its output.
    let classify = {
        let cfg = clawft_service_classify::ClassifierServiceConfig {
            node_id: daemon_node_id.to_string(),
            source_node: source.to_string(),
            input_path: input_path.clone(),
            output_path: classify_output_path.clone(),
            service_enabled: Arc::clone(&classify_flag),
            // "The mic source" toggle disables both consumers.
            source_enabled: Arc::clone(&source_flag),
        };
        let backend: Arc<dyn clawft_service_classify::ClassifierBackend> =
            Arc::new(clawft_service_classify::EnergyClassifier::from_env());
        match clawft_service_classify::ClassifierService::spawn(substrate.clone(), backend, cfg) {
            Ok(svc) => {
                info!(input = %input_path, output = %classify_output_path, "classifier service spawned (energy VAD)");
                Some(svc)
            }
            Err(e) => {
                warn!(error = %e, "classifier service failed to spawn; the whisper gate stays closed");
                None
            }
        }
    };

    // Whisper STT.
    let whisper = {
        let whisper_url = std::env::var(clawft_service_whisper::WHISPER_SERVICE_URL_ENV)
            .unwrap_or_else(|_| "http://127.0.0.1:8123".to_string());
        let output_path_derived = format!("substrate/_derived/transcript/{source}/mic");
        let cfg = clawft_service_whisper::WhisperServiceConfig {
            window_ms: 2_000,
            retry_backoff: Duration::from_millis(500),
            node_id: daemon_node_id.to_string(),
            input_path: input_path.clone(),
            output_path_derived: output_path_derived.clone(),
            service_enabled: Arc::clone(&whisper_flag),
            source_enabled: Arc::clone(&source_flag),
            node_registry,
            classifier_input: Some(classify_output_path.clone()),
            gate_window_ms: 1_500,
            model_id: "whisper-cpp/unverified".to_string(),
            source_node_hint: source.to_string(),
        };
        let client_cfg = clawft_service_whisper::WhisperConfig {
            base_url: whisper_url.clone(),
            ..clawft_service_whisper::WhisperConfig::default()
        };
        match clawft_service_whisper::WhisperClient::new(client_cfg) {
            Err(e) => {
                warn!(error = %e, "whisper client init failed; STT not running (classify unaffected)");
                None
            }
            Ok(client) => match clawft_service_whisper::WhisperService::spawn(
                substrate.clone(),
                client,
                cfg,
            ) {
                Ok(svc) => {
                    info!(input = %input_path, output = %output_path_derived, whisper_url = %whisper_url, "whisper service spawned");
                    Some(svc)
                }
                Err(e) => {
                    warn!(error = %e, "whisper service failed to spawn; STT not running");
                    None
                }
            },
        }
    };

    // Intents reflect what is really running.
    for (kind, target, label, running) in [
        (ControlKind::Sensor, pcm_chunk_target.as_str(), "Mic PCM chunks", true),
        (ControlKind::Sensor, rms_target.as_str(), "Mic RMS summary", true),
        (ControlKind::Service, "whisper", "Whisper STT", whisper.is_some()),
        (ControlKind::Service, "classify", "Audio classifier", classify.is_some()),
    ] {
        if !running {
            // A failed service must not show as enabled.
            let flag = match kind {
                ControlKind::Service if target == "whisper" => Some(&whisper_flag),
                ControlKind::Service => Some(&classify_flag),
                _ => None,
            };
            if let Some(f) = flag {
                f.store(false, std::sync::atomic::Ordering::SeqCst);
            }
        }
        publish_intent(kernel, daemon_node_id, kind, target, label, running).await;
    }

    MicPipeline {
        source: source.to_string(),
        whisper,
        classify,
    }
}

async fn publish_waiting(kernel: &SharedKernel, daemon_node_id: &str) {
    for (target, label) in [
        ("whisper", "Whisper STT (waiting for mic source)"),
        ("classify", "Audio classifier (waiting for mic source)"),
    ] {
        publish_intent(kernel, daemon_node_id, ControlKind::Service, target, label, false).await;
    }
}

struct WarnGate(Option<Instant>);

impl WarnGate {
    fn warn(&mut self, msg: &str) {
        if self.0.is_none_or(|t| t.elapsed() >= WARN_EVERY) {
            warn!("{msg}");
            self.0 = Some(Instant::now());
        }
    }
}

/// Long-running supervisor: selects the mic, runs the pipeline for it, and
/// keeps re-evaluating. Publishes the chosen node id on `mic_node_tx`
/// (`None` while nothing is selected).
pub async fn supervise(
    pin: Option<String>,
    kernel: SharedKernel,
    control_flags: ControlFlags,
    daemon_node_id: String,
    mic_node_tx: watch::Sender<Option<String>>,
) {
    publish_waiting(&kernel, &daemon_node_id).await;
    let mut current: Option<MicPipeline> = None;
    let mut gate = WarnGate(None);
    loop {
        let (decision, still_registered) = {
            let k = kernel.read().await;
            let d = decide(pin.as_deref(), k.node_registry(), k.substrate_service());
            let reg = current
                .as_ref()
                .is_some_and(|c| k.node_registry().contains(&c.source));
            (d, reg)
        };
        if let Some(msg) = decision.warning() {
            gate.warn(&msg);
        }
        // What (if anything) should be running after this pass.
        let want: Option<String> = match &decision {
            Decision::Pinned { id, .. } => Some(id.clone()),
            Decision::Single(id) => Some(id.clone()),
            Decision::Ambiguous(c) => match &current {
                // Keep a running auto-selection, but the warning above fires.
                Some(cur) if pin.is_none() && still_registered && c.contains(&cur.source) => {
                    Some(cur.source.clone())
                }
                _ => None,
            },
            Decision::NoCandidate => None,
        };
        // A selected node that is no longer registered is dropped (auto only).
        let want = match (&want, &current, pin.is_none(), still_registered) {
            (Some(w), Some(cur), true, false) if *w == cur.source => None,
            _ => want,
        };
        match (want, current.take()) {
            (Some(w), Some(cur)) if cur.source == w => current = Some(cur),
            (Some(w), old) => {
                if let Some(old) = old {
                    info!(old = %old.source, new = %w, "mic source changed; restarting pipeline");
                    old.stop().await;
                }
                let p = start_pipeline(&kernel, &control_flags, &daemon_node_id, &w).await;
                info!(node = %w, "mic source selected");
                let _ = mic_node_tx.send(Some(w));
                current = Some(p);
            }
            (None, Some(old)) => {
                warn!(node = %old.source, "mic source no longer valid; stopping whisper and classify");
                old.stop().await;
                let _ = mic_node_tx.send(None);
                publish_waiting(&kernel, &daemon_node_id).await;
            }
            (None, None) => {}
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

    fn publish_mic(sub: &SubstrateService, id: &str) {
        sub.publish(
            Some(id),
            &format!("substrate/{id}/sensor/mic/pcm_chunk"),
            serde_json::json!({"n": 1}),
        );
    }

    #[test]
    fn single_candidate_is_chosen() {
        let reg = NodeRegistry::new();
        let sub = SubstrateService::new();
        let daemon = node(&reg, 1);
        let mic = node(&reg, 2);
        assert!(clawft_kernel::is_node_id(&mic));
        assert_eq!(decide(None, &reg, &sub), Decision::NoCandidate);
        sub.publish(Some(&daemon), &format!("substrate/{daemon}/health"), serde_json::json!({}));
        assert_eq!(decide(None, &reg, &sub), Decision::NoCandidate, "no mic path, no candidate");
        publish_mic(&sub, &mic);
        assert_eq!(decide(None, &reg, &sub), Decision::Single(mic));
    }

    #[test]
    fn two_candidates_are_refused_never_ordered() {
        let reg = NodeRegistry::new();
        let sub = SubstrateService::new();
        let a = node(&reg, 1);
        let b = node(&reg, 2);
        publish_mic(&sub, &a);
        publish_mic(&sub, &b);
        let d = decide(None, &reg, &sub);
        let Decision::Ambiguous(c) = &d else { panic!("expected refusal, got {d:?}") };
        assert_eq!(c.len(), 2);
        let w = d.warning().expect("refusal must warn");
        assert!(w.contains(&a) && w.contains(&b), "names candidates: {w}");
        assert!(w.contains(MIC_NODE_ENV), "names the pin setting: {w}");
    }

    #[test]
    fn pin_overrides_everything() {
        let reg = NodeRegistry::new();
        let sub = SubstrateService::new();
        let a = node(&reg, 1);
        let b = node(&reg, 2);
        publish_mic(&sub, &a);
        publish_mic(&sub, &b);
        assert_eq!(
            decide(Some(&b), &reg, &sub),
            Decision::Pinned { id: b.clone(), registered: true }
        );
        assert_eq!(decide(Some(&b), &reg, &sub).warning(), None);
    }

    #[test]
    fn pinned_but_unregistered_warns() {
        let reg = NodeRegistry::new();
        let sub = SubstrateService::new();
        let ghost = "0".repeat(32);
        let d = decide(Some(&ghost), &reg, &sub);
        assert_eq!(d, Decision::Pinned { id: ghost.clone(), registered: false });
        let w = d.warning().expect("unregistered pin must warn");
        assert!(w.contains(&ghost));
    }

    #[test]
    fn unregistered_publisher_is_not_a_candidate() {
        let reg = NodeRegistry::new();
        let sub = SubstrateService::new();
        let _daemon = node(&reg, 1);
        publish_mic(&sub, &"0".repeat(32));
        assert_eq!(decide(None, &reg, &sub), Decision::NoCandidate);
    }
}
