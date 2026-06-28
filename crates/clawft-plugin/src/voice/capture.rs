//! Audio capture (microphone input) via cpal.
//!
//! Provides `AudioCapture` for streaming PCM audio from the default
//! input device at 16 kHz, 16-bit, mono.
//!
//! ## Privacy indicator (WEFT-207 / SC-1)
//!
//! Every start/stop transition fires the SC-1 mic privacy indicator
//! through [`crate::voice::privacy_indicator::emit_indicator`]. This
//! covers the current stub today and extends, unchanged, to the
//! 0.8.x in-process `cpal::Stream::new` path: that future code path
//! MUST funnel through `AudioCapture::start` / `AudioCapture::stop`
//! (or be invoked exclusively by code that does), so the indicator
//! cannot regress.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::config::{VoiceAudioConfig, VoiceCaptureSpec};
use super::privacy_indicator::{
    IndicatorPayload, IndicatorPublisher, IndicatorState, NoopIndicatorPublisher, emit_indicator,
};

/// Audio capture configuration.
///
/// **Deprecated**: prefer [`VoiceAudioConfig`] / [`VoiceCaptureSpec`].
/// Kept as a thin alias for back-compat through 0.7.x; will be removed
/// in 0.8.x once all in-tree callers migrate. Per WEFT-213 the canonical
/// audio config is [`VoiceAudioConfig`] which expresses capture +
/// playback as a single document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureConfig {
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Number of channels (1 = mono).
    pub channels: u16,
    /// Audio chunk size in samples.
    pub chunk_size: u32,
    /// Optional capture device name; `None` = system default.
    pub device_name: Option<String>,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            sample_rate: 16000,
            channels: 1,
            chunk_size: 512,
            device_name: None,
        }
    }
}

impl CaptureConfig {
    /// Promote a legacy `CaptureConfig` to a [`VoiceCaptureSpec`].
    /// Mirrors `From<CaptureConfig> for VoiceCaptureSpec`; provided as
    /// an explicit method for call sites that prefer it.
    pub fn into_spec(self) -> VoiceCaptureSpec {
        self.into()
    }
}

impl From<CaptureConfig> for VoiceCaptureSpec {
    fn from(value: CaptureConfig) -> Self {
        VoiceCaptureSpec {
            sample_rate: value.sample_rate,
            channels: value.channels,
            chunk_size: value.chunk_size,
            device_name: value.device_name,
        }
    }
}

impl From<VoiceCaptureSpec> for CaptureConfig {
    fn from(value: VoiceCaptureSpec) -> Self {
        CaptureConfig {
            sample_rate: value.sample_rate,
            channels: value.channels,
            chunk_size: value.chunk_size,
            device_name: value.device_name,
        }
    }
}

/// Microphone audio capture stream.
///
/// Wraps cpal input stream. Currently a stub -- real cpal integration
/// is deferred to the 0.8.x in-process voice backend (see ADR-053).
/// 0.7.0 ships with substrate-side capture + transcription, so this
/// scaffolding is a placeholder for the second `SttBackend` implementor.
///
/// Construction defaults the privacy-indicator publisher to a no-op
/// (see [`crate::voice::privacy_indicator`]). Production code paths
/// must call [`AudioCapture::with_indicator_publisher`] (or the full
/// [`AudioCapture::new_with_publisher`] constructor) so the SC-1
/// substrate topic actually surfaces. The tracing-target side of the
/// indicator fires regardless.
pub struct AudioCapture {
    config: CaptureConfig,
    active: bool,
    indicator: Arc<dyn IndicatorPublisher>,
}

impl AudioCapture {
    /// Create a new audio capture from a legacy [`CaptureConfig`].
    ///
    /// Defaults the indicator publisher to
    /// [`NoopIndicatorPublisher`]; the tracing target still fires.
    /// Production callers should use [`Self::new_with_publisher`].
    pub fn new(config: CaptureConfig) -> Self {
        Self {
            config,
            active: false,
            indicator: Arc::new(NoopIndicatorPublisher),
        }
    }

    /// Create a new audio capture with an explicit privacy-indicator
    /// publisher. The substrate-aware daemon wires the real publisher
    /// here; tests pass an
    /// [`crate::voice::privacy_indicator::InMemoryIndicatorPublisher`].
    pub fn new_with_publisher(
        config: CaptureConfig,
        indicator: Arc<dyn IndicatorPublisher>,
    ) -> Self {
        Self {
            config,
            active: false,
            indicator,
        }
    }

    /// Replace the indicator publisher on an existing handle. Useful
    /// when `from_voice` was used to construct the capture but the
    /// real publisher only becomes available later in boot.
    pub fn with_indicator_publisher(mut self, indicator: Arc<dyn IndicatorPublisher>) -> Self {
        self.indicator = indicator;
        self
    }

    /// Create a new audio capture from a unified [`VoiceAudioConfig`].
    /// Returns `None` if the config has no capture spec attached.
    pub fn from_voice(cfg: &VoiceAudioConfig) -> Option<Self> {
        cfg.capture.clone().map(|spec| Self::new(spec.into()))
    }

    /// Start capturing audio.
    ///
    /// Fires the SC-1 mic privacy indicator (`state: "capturing"`) on
    /// every successful start. Calling `start` while already active
    /// is a no-op and does NOT re-fire the indicator -- subscribers
    /// see one `capturing` per active session, not one per call.
    pub fn start(&mut self) -> Result<(), String> {
        // Stub: real cpal stream creation deferred to 0.8.x in-process
        // backend (see ADR-053). The indicator wiring below is what
        // makes this code path safe to extend with real cpal calls
        // without a separate audit -- the privacy contract is met
        // here, before any real `cpal::Stream::new` lands.
        if self.active {
            return Ok(());
        }
        self.active = true;
        tracing::info!(
            sample_rate = self.config.sample_rate,
            channels = self.config.channels,
            "Audio capture started (stub)"
        );
        let payload = IndicatorPayload::new(
            IndicatorState::Capturing,
            self.config.device_name.clone(),
            self.config.sample_rate,
            self.config.channels,
        );
        emit_indicator(self.indicator.as_ref(), &payload);
        Ok(())
    }

    /// Stop capturing audio.
    ///
    /// Fires the SC-1 mic privacy indicator (`state: "idle"`) iff a
    /// `capturing` event was previously fired. Idempotent: stopping
    /// an inactive capture is a no-op.
    pub fn stop(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        tracing::info!("Audio capture stopped");
        let payload = IndicatorPayload::new(
            IndicatorState::Idle,
            self.config.device_name.clone(),
            self.config.sample_rate,
            self.config.channels,
        );
        emit_indicator(self.indicator.as_ref(), &payload);
    }

    /// Check if capture is active.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Get the capture configuration.
    pub fn config(&self) -> &CaptureConfig {
        &self.config
    }
}

impl Drop for AudioCapture {
    /// Failsafe: if the handle is dropped while still active, fire the
    /// `idle` indicator so subscribers don't see a permanently-stuck
    /// `capturing` state. This catches panics, early-returns, and
    /// anywhere the explicit `stop` was missed.
    fn drop(&mut self) {
        if self.active {
            self.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::privacy_indicator::InMemoryIndicatorPublisher;

    fn cfg() -> CaptureConfig {
        CaptureConfig {
            sample_rate: 16_000,
            channels: 1,
            chunk_size: 512,
            device_name: Some("test-mic".into()),
        }
    }

    #[test]
    fn start_emits_capturing_indicator() {
        let pub_ = InMemoryIndicatorPublisher::new();
        let mut cap = AudioCapture::new_with_publisher(cfg(), Arc::new(pub_.clone()));
        cap.start().unwrap();
        let snap = pub_.snapshot();
        assert_eq!(snap.len(), 1, "exactly one indicator on start");
        assert_eq!(snap[0].state, "capturing");
        assert_eq!(snap[0].device.as_deref(), Some("test-mic"));
        assert_eq!(snap[0].sample_rate, 16_000);
        assert_eq!(snap[0].channels, 1);
    }

    #[test]
    fn stop_emits_idle_indicator() {
        let pub_ = InMemoryIndicatorPublisher::new();
        let mut cap = AudioCapture::new_with_publisher(cfg(), Arc::new(pub_.clone()));
        cap.start().unwrap();
        cap.stop();
        let snap = pub_.snapshot();
        assert_eq!(snap.len(), 2, "capturing + idle");
        assert_eq!(snap[0].state, "capturing");
        assert_eq!(snap[1].state, "idle");
    }

    #[test]
    fn no_indicator_when_capture_never_started() {
        // Construct + drop without start: no spurious indicator events.
        // Audit consumers should never see an `idle` without a prior
        // `capturing`.
        let pub_ = InMemoryIndicatorPublisher::new();
        let cap = AudioCapture::new_with_publisher(cfg(), Arc::new(pub_.clone()));
        drop(cap);
        assert!(
            pub_.is_empty(),
            "no indicator events for never-started capture"
        );
    }

    #[test]
    fn double_start_emits_one_capturing() {
        let pub_ = InMemoryIndicatorPublisher::new();
        let mut cap = AudioCapture::new_with_publisher(cfg(), Arc::new(pub_.clone()));
        cap.start().unwrap();
        cap.start().unwrap();
        assert_eq!(pub_.len(), 1, "second start is a no-op");
    }

    #[test]
    fn double_stop_emits_one_idle() {
        let pub_ = InMemoryIndicatorPublisher::new();
        let mut cap = AudioCapture::new_with_publisher(cfg(), Arc::new(pub_.clone()));
        cap.start().unwrap();
        cap.stop();
        cap.stop();
        let snap = pub_.snapshot();
        assert_eq!(snap.len(), 2, "second stop is a no-op");
        assert_eq!(snap[1].state, "idle");
    }

    #[test]
    fn drop_while_active_emits_idle_failsafe() {
        // Drop without an explicit stop must still surface `idle`,
        // otherwise a panic in the surrounding code would leave the
        // mic indicator stuck on `capturing` forever from the UI's
        // perspective.
        let pub_ = InMemoryIndicatorPublisher::new();
        {
            let mut cap = AudioCapture::new_with_publisher(cfg(), Arc::new(pub_.clone()));
            cap.start().unwrap();
        }
        let snap = pub_.snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[1].state, "idle");
    }

    #[test]
    fn default_publisher_does_not_panic_on_start_stop() {
        // The legacy `new` constructor uses the no-op publisher; the
        // tracing-target side still fires but the substrate-topic side
        // is silent. Calling start/stop must not panic.
        let mut cap = AudioCapture::new(cfg());
        cap.start().unwrap();
        cap.stop();
    }
}
