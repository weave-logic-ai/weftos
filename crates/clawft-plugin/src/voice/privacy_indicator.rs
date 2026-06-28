//! Mic privacy indicator (WEFT-207 / SC-1).
//!
//! When the sensor capture path opens (or closes) a microphone stream,
//! the user MUST be told and the system MUST be able to attest. This
//! module is the single seam every capture path -- the in-tree
//! `AudioCapture` stub today, real `cpal::Stream::new` calls in 0.8.x,
//! and any trait-injected fake -- routes its start/stop through.
//!
//! ## Two surfaces
//!
//! 1. **System-level**: a structured `tracing` event on target
//!    [`INDICATOR_TARGET`] (`"voice.privacy.indicator"`) with the
//!    fields `state` (`"capturing" | "idle"`), `device`, `sample_rate`,
//!    `channels`, and `ts_unix_micros`. Audit consumers (chain layer,
//!    syslog, `tracing-subscriber` filters) subscribe by target and
//!    forward to whatever durable sink they own. This mirrors the
//!    `voice.audit` target from `clawft-service-whisper::audit` so
//!    operators only learn one tracing convention for voice.
//! 2. **User-visible**: a publish on the substrate topic
//!    [`INDICATOR_TOPIC`] (`"weftos.voice.indicator.v1"`) with the
//!    same payload. The future GUI (`clawft-gui-egui`) and any web
//!    UI subscribe and render a real indicator (red dot, mic icon,
//!    notification toast) on state transitions. The capture path
//!    does not depend on `clawft-substrate` directly -- it routes
//!    through the [`IndicatorPublisher`] trait so the substrate-aware
//!    wrapper lives at a higher layer.
//!
//! ## Why a trait, not a direct substrate call
//!
//! `clawft-plugin` is a leaf crate; pulling in `clawft-substrate` would
//! invert the dependency graph. The publisher trait keeps the capture
//! layer ignorant of how the topic actually surfaces, while still
//! letting tests assert on the published payload via
//! [`InMemoryIndicatorPublisher`].
//!
//! ## Why both surfaces
//!
//! The tracing target is the chain-level audit record (tamper-evident,
//! replayable, machine-grep-able). The substrate topic is the live
//! UI signal (pub-sub, low-latency, ephemeral). Wiring both at one
//! seam means a UI that ignores the audit chain still sees the red
//! dot, and an audit trail still lands even when no GUI is running.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

/// Tracing target emitted on every indicator state transition. The
/// chain-event layer and any `tracing-subscriber` filtering on this
/// target receives the structured fields below.
pub const INDICATOR_TARGET: &str = "voice.privacy.indicator";

/// Substrate topic the GUI / external consumers subscribe to for the
/// live mic-state stream. Versioned so the payload schema can evolve
/// without breaking subscribers.
pub const INDICATOR_TOPIC: &str = "weftos.voice.indicator.v1";

/// Indicator state. Distinct from `is_active` flags scattered across
/// capture stubs -- this enum is the load-bearing wire payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IndicatorState {
    /// A capture stream is open and frames may be flowing.
    Capturing,
    /// No capture stream is open.
    Idle,
}

impl IndicatorState {
    /// Stable string used in tracing fields and JSON payloads.
    pub fn as_str(self) -> &'static str {
        match self {
            IndicatorState::Capturing => "capturing",
            IndicatorState::Idle => "idle",
        }
    }
}

/// Payload published on [`INDICATOR_TOPIC`] AND mirrored as structured
/// fields on the [`INDICATOR_TARGET`] tracing event.
///
/// `device` is the configured device name (`None` = system default);
/// `sample_rate` and `channels` come from the capture spec; the
/// timestamp is wall-clock μs since the Unix epoch (parallels
/// `voice.audit` rows so operators can correlate).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndicatorPayload {
    /// `"capturing"` or `"idle"`.
    pub state: String,
    /// Configured device name (`None` = system default).
    pub device: Option<String>,
    /// Sample rate in Hz the capture spec asked for.
    pub sample_rate: u32,
    /// Channel count the capture spec asked for.
    pub channels: u16,
    /// Wall-clock μs since the Unix epoch.
    pub ts_unix_micros: i128,
}

impl IndicatorPayload {
    /// Build a payload for a state transition. `device` is taken
    /// verbatim from the capture spec so subscribers can render it
    /// in the UI.
    pub fn new(
        state: IndicatorState,
        device: Option<String>,
        sample_rate: u32,
        channels: u16,
    ) -> Self {
        Self {
            state: state.as_str().into(),
            device,
            sample_rate,
            channels,
            ts_unix_micros: now_unix_micros(),
        }
    }
}

/// Sink that forwards the indicator payload onto the substrate topic
/// (or any equivalent UI bus). Implemented by a substrate-aware
/// wrapper at the daemon layer; defaulted to a no-op so the plugin
/// crate stays substrate-free.
pub trait IndicatorPublisher: Send + Sync {
    /// Publish the payload. Errors are intentionally swallowed by
    /// [`emit_indicator`] -- the indicator is best-effort and MUST
    /// NOT block capture start/stop.
    fn publish(&self, payload: &IndicatorPayload);
}

/// No-op publisher. Used when the daemon has not (yet) wired a real
/// substrate publisher; the tracing surface still fires.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopIndicatorPublisher;

impl IndicatorPublisher for NoopIndicatorPublisher {
    fn publish(&self, _payload: &IndicatorPayload) {
        // intentional no-op
    }
}

/// In-memory publisher used by tests to assert on the exact payload
/// sequence. Thread-safe; clones share the same backing buffer.
#[derive(Debug, Default, Clone)]
pub struct InMemoryIndicatorPublisher {
    inner: Arc<Mutex<Vec<IndicatorPayload>>>,
}

impl InMemoryIndicatorPublisher {
    /// Create an empty publisher.
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot the published payloads in order.
    pub fn snapshot(&self) -> Vec<IndicatorPayload> {
        self.inner
            .lock()
            .expect("indicator buffer poisoned")
            .clone()
    }

    /// Number of published payloads.
    pub fn len(&self) -> usize {
        self.inner.lock().expect("indicator buffer poisoned").len()
    }

    /// True iff no payloads have been published.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl IndicatorPublisher for InMemoryIndicatorPublisher {
    fn publish(&self, payload: &IndicatorPayload) {
        self.inner
            .lock()
            .expect("indicator buffer poisoned")
            .push(payload.clone());
    }
}

/// Emit both indicator surfaces atomically: a structured tracing event
/// on [`INDICATOR_TARGET`] *and* a publish on the supplied publisher
/// (which fronts [`INDICATOR_TOPIC`]). Calling this from anywhere
/// outside a capture start/stop transition is a contract violation.
pub fn emit_indicator(publisher: &dyn IndicatorPublisher, payload: &IndicatorPayload) {
    tracing::info!(
        target: INDICATOR_TARGET,
        state = %payload.state,
        device = payload.device.as_deref().unwrap_or("default"),
        sample_rate = payload.sample_rate,
        channels = payload.channels,
        ts_unix_micros = payload.ts_unix_micros as i64,
        topic = INDICATOR_TOPIC,
        "voice privacy indicator"
    );
    publisher.publish(payload);
}

fn now_unix_micros() -> i128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as i128)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_state_strings_are_stable() {
        // The substrate-topic payload is part of the public wire
        // contract; subscribers (future GUI) match on these strings.
        assert_eq!(IndicatorState::Capturing.as_str(), "capturing");
        assert_eq!(IndicatorState::Idle.as_str(), "idle");
    }

    #[test]
    fn payload_round_trips_through_json() {
        // Substrate topics carry JSON; assert the schema survives a
        // round trip so the GUI can decode without a custom parser.
        let p = IndicatorPayload::new(IndicatorState::Capturing, Some("USB Mic".into()), 16_000, 1);
        let s = serde_json::to_string(&p).unwrap();
        let back: IndicatorPayload = serde_json::from_str(&s).unwrap();
        assert_eq!(back.state, "capturing");
        assert_eq!(back.device.as_deref(), Some("USB Mic"));
        assert_eq!(back.sample_rate, 16_000);
        assert_eq!(back.channels, 1);
    }

    #[test]
    fn topic_and_target_constants_are_stable() {
        // External subscribers depend on these; treat as wire contract.
        assert_eq!(INDICATOR_TOPIC, "weftos.voice.indicator.v1");
        assert_eq!(INDICATOR_TARGET, "voice.privacy.indicator");
    }

    #[test]
    fn in_memory_publisher_records_payloads_in_order() {
        let pub_ = InMemoryIndicatorPublisher::new();
        emit_indicator(
            &pub_,
            &IndicatorPayload::new(IndicatorState::Capturing, None, 16_000, 1),
        );
        emit_indicator(
            &pub_,
            &IndicatorPayload::new(IndicatorState::Idle, None, 16_000, 1),
        );
        let snap = pub_.snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].state, "capturing");
        assert_eq!(snap[1].state, "idle");
    }

    #[test]
    fn noop_publisher_does_not_panic() {
        let pub_ = NoopIndicatorPublisher;
        emit_indicator(
            &pub_,
            &IndicatorPayload::new(IndicatorState::Capturing, None, 16_000, 1),
        );
    }
}
