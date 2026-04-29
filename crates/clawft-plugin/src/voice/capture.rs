//! Audio capture (microphone input) via cpal.
//!
//! Provides `AudioCapture` for streaming PCM audio from the default
//! input device at 16 kHz, 16-bit, mono.

use serde::{Deserialize, Serialize};

use super::config::{VoiceAudioConfig, VoiceCaptureSpec};

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
pub struct AudioCapture {
    config: CaptureConfig,
    active: bool,
}

impl AudioCapture {
    /// Create a new audio capture from a legacy [`CaptureConfig`].
    pub fn new(config: CaptureConfig) -> Self {
        Self { config, active: false }
    }

    /// Create a new audio capture from a unified [`VoiceAudioConfig`].
    /// Returns `None` if the config has no capture spec attached.
    pub fn from_voice(cfg: &VoiceAudioConfig) -> Option<Self> {
        cfg.capture.clone().map(|spec| Self::new(spec.into()))
    }

    /// Start capturing audio.
    pub fn start(&mut self) -> Result<(), String> {
        // Stub: real cpal stream creation deferred to 0.8.x in-process
        // backend (see ADR-053).
        self.active = true;
        tracing::info!(
            sample_rate = self.config.sample_rate,
            channels = self.config.channels,
            "Audio capture started (stub)"
        );
        Ok(())
    }

    /// Stop capturing audio.
    pub fn stop(&mut self) {
        self.active = false;
        tracing::info!("Audio capture stopped");
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
