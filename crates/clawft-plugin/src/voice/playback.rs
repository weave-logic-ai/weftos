//! Audio playback (speaker output) via cpal.

use serde::{Deserialize, Serialize};

use super::config::{VoiceAudioConfig, VoicePlaybackSpec};

/// Audio playback configuration.
///
/// **Deprecated**: prefer [`VoiceAudioConfig`] / [`VoicePlaybackSpec`].
/// Kept as a thin alias for back-compat through 0.7.x; will be removed
/// in 0.8.x once all in-tree callers migrate. Per WEFT-213 the canonical
/// audio config is [`VoiceAudioConfig`] which expresses capture +
/// playback as a single document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybackConfig {
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Number of channels (1 = mono).
    pub channels: u16,
    /// Optional playback device name; `None` = system default.
    pub device_name: Option<String>,
}

impl Default for PlaybackConfig {
    fn default() -> Self {
        Self {
            sample_rate: 16000,
            channels: 1,
            device_name: None,
        }
    }
}

impl PlaybackConfig {
    /// Promote a legacy `PlaybackConfig` to a [`VoicePlaybackSpec`].
    pub fn into_spec(self) -> VoicePlaybackSpec {
        self.into()
    }
}

impl From<PlaybackConfig> for VoicePlaybackSpec {
    fn from(value: PlaybackConfig) -> Self {
        VoicePlaybackSpec {
            sample_rate: value.sample_rate,
            channels: value.channels,
            device_name: value.device_name,
        }
    }
}

impl From<VoicePlaybackSpec> for PlaybackConfig {
    fn from(value: VoicePlaybackSpec) -> Self {
        PlaybackConfig {
            sample_rate: value.sample_rate,
            channels: value.channels,
            device_name: value.device_name,
        }
    }
}

/// Speaker audio playback stream.
///
/// Wraps cpal output stream. Currently a stub.
pub struct AudioPlayback {
    config: PlaybackConfig,
    active: bool,
}

impl AudioPlayback {
    /// Create a new playback handle from a legacy [`PlaybackConfig`].
    pub fn new(config: PlaybackConfig) -> Self {
        Self {
            config,
            active: false,
        }
    }

    /// Create a new playback handle from a unified [`VoiceAudioConfig`].
    /// Returns `None` if the config has no playback spec attached.
    pub fn from_voice(cfg: &VoiceAudioConfig) -> Option<Self> {
        cfg.playback.clone().map(|spec| Self::new(spec.into()))
    }

    pub fn start(&mut self) -> Result<(), String> {
        self.active = true;
        tracing::info!("Audio playback started (stub)");
        Ok(())
    }

    pub fn stop(&mut self) {
        self.active = false;
        tracing::info!("Audio playback stopped");
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn config(&self) -> &PlaybackConfig {
        &self.config
    }
}
