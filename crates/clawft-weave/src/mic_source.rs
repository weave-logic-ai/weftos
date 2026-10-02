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
    // `recover`: a flag cleared by an earlier failed start comes back on,
    // unless the operator explicitly switched it off.
    let source_flag = control_flags.recover(ControlKind::Sensor, &pcm_chunk_target);
    let _rms_flag = control_flags.recover(ControlKind::Sensor, &rms_target);
    let whisper_flag = control_flags.recover(ControlKind::Service, "whisper");
    let classify_flag = control_flags.recover(ControlKind::Service, "classify");

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

#[derive(Debug)]
struct WarnGate(Option<Instant>);

impl WarnGate {
    /// Emit `msg` through `sink` at most once per [`WARN_EVERY`].
    fn warn(&mut self, msg: &str, sink: &mut (dyn FnMut(&str) + Send)) {
        if self.0.is_none_or(|t| t.elapsed() >= WARN_EVERY) {
            sink(msg);
            self.0 = Some(Instant::now());
        }
    }
}

/// What the supervisor is currently running and why.
#[derive(Debug, Default)]
pub struct SupState {
    /// `(node id, chosen by operator pin)`.
    current: Option<(String, bool)>,
    /// An auto-selection was stopped because a second candidate appeared.
    /// Sticky: only an operator pin restarts the pipeline.
    halted: bool,
    gate: Option<WarnGate>,
}

impl SupState {
    /// Node id currently selected, if any.
    pub fn current(&self) -> Option<&str> {
        self.current.as_ref().map(|(id, _)| id.as_str())
    }
}

/// Starts and stops the pipeline for a mic source (real or a test fake).
#[async_trait::async_trait]
pub trait PipelineHost: Send {
    /// Start whisper + classify (and the transcript subscription) for `id`.
    async fn start(&mut self, id: &str);
    /// Stop everything started for `id` and show "waiting for mic source".
    async fn stop(&mut self, id: &str);
}

/// One supervisor iteration. Fails closed: once an AUTO selection exists,
/// any second candidate stops the pipeline and it stays stopped until the
/// operator pins a node. Pinned selections are never disturbed by
/// candidates. `warn` receives every warning text.
pub async fn supervise_step(
    host: &mut dyn PipelineHost,
    state: &mut SupState,
    pin: Option<&str>,
    registry: &NodeRegistry,
    substrate: &SubstrateService,
    warn: &mut (dyn FnMut(&str) + Send),
) {
    let decision = decide(pin, registry, substrate);
    let gate = state.gate.get_or_insert(WarnGate(None));

    match &decision {
        Decision::Pinned { id, .. } => {
            state.halted = false;
            if let Some(msg) = decision.warning() {
                gate.warn(&msg, warn);
            }
            match state.current.clone() {
                Some((cur, true)) if cur == *id => {}
                prior => {
                    if let Some((old, _)) = prior {
                        host.stop(&old).await;
                    }
                    host.start(id).await;
                    state.current = Some((id.clone(), true));
                }
            }
        }
        Decision::Ambiguous(c) => {
            let msg = decision.warning().unwrap_or_default();
            if let Some((cur, false)) = state.current.clone() {
                // The auto-selected node may be the attacker: fail closed.
                warn(&format!(
                    "second mic candidate appeared after auto-selecting {cur}; stopping whisper and classify. {msg}"
                ));
                host.stop(&cur).await;
                state.current = None;
                state.halted = true;
            } else if state.halted {
                gate.warn(&format!("mic auto-selection is halted until the operator pins one. {msg}"), warn);
            } else {
                gate.warn(&msg, warn);
            }
            let _ = c;
        }
        Decision::Single(id) => {
            if state.halted {
                gate.warn(
                    &format!(
                        "mic auto-selection is halted (an earlier second candidate); not restarting for {id}. \
                         Pin the mic with {MIC_NODE_ENV}=<node-id> (or voice.mic_node_id)"
                    ),
                    warn,
                );
            } else {
                match state.current.clone() {
                    Some((cur, _)) if cur == *id => {}
                    prior => {
                        // The chosen node stopped being a candidate and a
                        // single new one took its place: re-resolve.
                        if let Some((old, _)) = prior {
                            host.stop(&old).await;
                        }
                        host.start(id).await;
                        state.current = Some((id.clone(), false));
                    }
                }
            }
        }
        Decision::NoCandidate => {
            if let Some((cur, false)) = state.current.clone() {
                warn(&format!("auto-selected mic node {cur} is no longer a candidate; stopping whisper and classify"));
                host.stop(&cur).await;
                state.current = None;
            }
            if let Some(msg) = decision.warning() {
                gate.warn(&msg, warn);
            }
        }
    }
}

/// The real pipeline host: whisper + classify under the daemon node.
struct RealHost {
    kernel: SharedKernel,
    control_flags: ControlFlags,
    daemon_node_id: String,
    mic_node_tx: watch::Sender<Option<String>>,
    pipeline: Option<MicPipeline>,
}

#[async_trait::async_trait]
impl PipelineHost for RealHost {
    async fn start(&mut self, id: &str) {
        let p = start_pipeline(&self.kernel, &self.control_flags, &self.daemon_node_id, id).await;
        info!(node = %id, "mic source selected");
        let _ = self.mic_node_tx.send(Some(id.to_string()));
        self.pipeline = Some(p);
    }

    async fn stop(&mut self, id: &str) {
        if let Some(p) = self.pipeline.take() {
            warn!(node = %id, "stopping whisper and classify (mic source no longer valid)");
            p.stop().await;
        }
        let _ = self.mic_node_tx.send(None);
        publish_waiting(&self.kernel, &self.daemon_node_id).await;
    }
}

/// Long-running supervisor: selects the mic, runs the pipeline for it, and
/// keeps re-evaluating (see [`supervise_step`]). Publishes the chosen node
/// id on `mic_node_tx` (`None` while nothing is selected).
pub async fn supervise(
    pin: Option<String>,
    kernel: SharedKernel,
    control_flags: ControlFlags,
    daemon_node_id: String,
    mic_node_tx: watch::Sender<Option<String>>,
) {
    publish_waiting(&kernel, &daemon_node_id).await;
    let mut host = RealHost {
        kernel: kernel.clone(),
        control_flags,
        daemon_node_id,
        mic_node_tx,
        pipeline: None,
    };
    let mut state = SupState::default();
    loop {
        let (registry, substrate) = {
            let k = kernel.read().await;
            (k.node_registry().clone(), k.substrate_service().clone())
        };
        supervise_step(
            &mut host,
            &mut state,
            pin.as_deref(),
            &registry,
            &substrate,
            &mut |m| warn!("{m}"),
        )
        .await;
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

    // ── supervise_step ────────────────────────────────────────────────

    #[derive(Default)]
    struct FakeHost {
        running: Option<String>,
        log: Vec<String>,
    }

    #[async_trait::async_trait]
    impl PipelineHost for FakeHost {
        async fn start(&mut self, id: &str) {
            self.running = Some(id.to_string());
            self.log.push(format!("start {id}"));
        }
        async fn stop(&mut self, id: &str) {
            self.running = None;
            self.log.push(format!("stop {id}"));
        }
    }

    async fn step(
        host: &mut FakeHost,
        st: &mut SupState,
        pin: Option<&str>,
        reg: &NodeRegistry,
        sub: &SubstrateService,
    ) -> Vec<String> {
        let mut warns = Vec::new();
        supervise_step(host, st, pin, reg, sub, &mut |m| warns.push(m.to_string())).await;
        warns
    }

    #[tokio::test]
    async fn attacker_first_then_real_mic_stops_the_pipeline_and_stays_stopped() {
        let reg = NodeRegistry::new();
        let sub = SubstrateService::new();
        let (mut host, mut st) = (FakeHost::default(), SupState::default());
        let attacker = node(&reg, 1);
        publish_mic(&sub, &attacker);
        step(&mut host, &mut st, None, &reg, &sub).await;
        assert_eq!(host.running.as_deref(), Some(attacker.as_str()), "single candidate runs");

        let real = node(&reg, 2);
        publish_mic(&sub, &real);
        let warns = step(&mut host, &mut st, None, &reg, &sub).await;
        assert_eq!(host.running, None, "second candidate must stop the pipeline");
        let all = warns.join("\n");
        assert!(all.contains(&attacker) && all.contains(&real), "names all candidates: {all}");
        assert!(all.contains(MIC_NODE_ENV));

        // Stays stopped, even if the attacker later vanishes from the picture.
        for _ in 0..3 {
            step(&mut host, &mut st, None, &reg, &sub).await;
        }
        assert_eq!(host.running, None);
        assert_eq!(host.log, vec![format!("start {attacker}"), format!("stop {attacker}")]);

        // Only an operator pin restarts it.
        step(&mut host, &mut st, Some(&real), &reg, &sub).await;
        assert_eq!(host.running.as_deref(), Some(real.as_str()));
    }

    #[tokio::test]
    async fn single_candidate_keeps_running() {
        let reg = NodeRegistry::new();
        let sub = SubstrateService::new();
        let (mut host, mut st) = (FakeHost::default(), SupState::default());
        let mic = node(&reg, 1);
        publish_mic(&sub, &mic);
        for _ in 0..3 {
            step(&mut host, &mut st, None, &reg, &sub).await;
        }
        assert_eq!(host.running.as_deref(), Some(mic.as_str()));
        assert_eq!(host.log, vec![format!("start {mic}")], "started exactly once");
    }

    #[tokio::test]
    async fn pinned_id_stays_on_the_pin_when_a_second_candidate_appears() {
        let reg = NodeRegistry::new();
        let sub = SubstrateService::new();
        let (mut host, mut st) = (FakeHost::default(), SupState::default());
        let pinned = node(&reg, 1);
        publish_mic(&sub, &pinned);
        step(&mut host, &mut st, Some(&pinned), &reg, &sub).await;
        let other = node(&reg, 2);
        publish_mic(&sub, &other);
        for _ in 0..3 {
            step(&mut host, &mut st, Some(&pinned), &reg, &sub).await;
        }
        assert_eq!(host.running.as_deref(), Some(pinned.as_str()));
        assert_eq!(host.log, vec![format!("start {pinned}")]);
    }

    #[tokio::test]
    async fn chosen_node_no_longer_a_candidate_is_dropped() {
        let reg = NodeRegistry::new();
        let sub = SubstrateService::new();
        let (mut host, mut st) = (FakeHost::default(), SupState::default());
        let a = node(&reg, 1);
        publish_mic(&sub, &a);
        step(&mut host, &mut st, None, &reg, &sub).await;
        assert_eq!(st.current(), Some(a.as_str()));
        // A different registry view where `a` is gone and `b` is the only mic.
        let reg2 = NodeRegistry::new();
        let b = node(&reg2, 2);
        publish_mic(&sub, &b);
        step(&mut host, &mut st, None, &reg2, &sub).await;
        assert_eq!(host.running.as_deref(), Some(b.as_str()));
        assert_eq!(host.log, vec![format!("start {a}"), format!("stop {a}"), format!("start {b}")]);
    }
}
