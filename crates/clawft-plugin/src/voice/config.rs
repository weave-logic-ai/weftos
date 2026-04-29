//! Voice-specific configuration helpers.
//!
//! Re-exports `VoiceConfig` from `clawft-types` and provides
//! runtime configuration for the voice pipeline.
//!
//! # Canonical audio config (WEFT-213)
//!
//! Prior to 0.7.0 the in-process voice scaffold carried three audio
//! config types -- `AudioConfig` in `clawft-types` (the user-facing
//! serialized config), `CaptureConfig` in `voice/capture.rs`, and
//! `PlaybackConfig` in `voice/playback.rs`. Three types, three default
//! impls, three sets of fields drifting independently.
//!
//! [`VoiceAudioConfig`] is the canonical replacement. It folds capture
//! and playback into a single document with optional sub-specs:
//!
//! ```text
//! VoiceAudioConfig {
//!     capture: Option<VoiceCaptureSpec>,
//!     playback: Option<VoicePlaybackSpec>,
//! }
//! ```
//!
//! `Option` lets a deployment opt out of either side (a sensor-only
//! node has `playback: None`; a TTS-only node has `capture: None`).
//! The legacy `CaptureConfig` / `PlaybackConfig` types remain as
//! thin aliases with `From`/`Into` to the new specs so 0.7.0 can ship
//! without churning every call site; both are scheduled for removal
//! in 0.8.x.

use serde::{Deserialize, Serialize};

/// Runtime configuration for the voice pipeline.
/// Wraps the serializable VoiceConfig with runtime state.
#[derive(Debug, Clone)]
pub struct VoicePipelineConfig {
    /// Model cache directory path.
    pub model_cache_dir: std::path::PathBuf,
    /// Whether voice pipeline is active.
    pub active: bool,
}

impl Default for VoicePipelineConfig {
    fn default() -> Self {
        Self {
            model_cache_dir: default_model_cache_dir(),
            active: false,
        }
    }
}

fn default_model_cache_dir() -> std::path::PathBuf {
    // Use ~/.clawft/models/voice/ as default
    if let Some(home) = dirs_fallback() {
        home.join(".clawft").join("models").join("voice")
    } else {
        std::path::PathBuf::from(".clawft/models/voice")
    }
}

fn dirs_fallback() -> Option<std::path::PathBuf> {
    std::env::var("HOME").ok().map(std::path::PathBuf::from)
}

// ---------------------------------------------------------------------
// VoiceAudioConfig (canonical, WEFT-213)
// ---------------------------------------------------------------------

/// Canonical audio configuration for the in-process voice backend.
///
/// Replaces the older trio (`AudioConfig` in `clawft-types`,
/// `CaptureConfig` and `PlaybackConfig` in this crate). Capture and
/// playback are independent `Option`s so a deployment can run
/// capture-only (sensor node) or playback-only (TTS-only node)
/// configurations without lying about default sample rates for the
/// disabled side.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct VoiceAudioConfig {
    /// Microphone capture spec. `None` disables capture entirely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture: Option<VoiceCaptureSpec>,

    /// Speaker playback spec. `None` disables playback entirely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playback: Option<VoicePlaybackSpec>,
}

impl VoiceAudioConfig {
    /// Convenience constructor: capture-only.
    pub fn capture_only(capture: VoiceCaptureSpec) -> Self {
        Self {
            capture: Some(capture),
            playback: None,
        }
    }

    /// Convenience constructor: playback-only.
    pub fn playback_only(playback: VoicePlaybackSpec) -> Self {
        Self {
            capture: None,
            playback: Some(playback),
        }
    }

    /// Convenience constructor: both sides enabled with defaults.
    pub fn duplex_default() -> Self {
        Self {
            capture: Some(VoiceCaptureSpec::default()),
            playback: Some(VoicePlaybackSpec::default()),
        }
    }
}

/// Microphone capture sub-config of [`VoiceAudioConfig`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VoiceCaptureSpec {
    /// Sample rate in Hz.
    #[serde(default = "default_sample_rate", alias = "sampleRate")]
    pub sample_rate: u32,
    /// Number of channels (1 = mono).
    #[serde(default = "default_audio_channels")]
    pub channels: u16,
    /// Audio chunk size in samples.
    #[serde(default = "default_chunk_size", alias = "chunkSize")]
    pub chunk_size: u32,
    /// Optional capture device name; `None` = system default.
    #[serde(default, alias = "deviceName")]
    pub device_name: Option<String>,
}

impl Default for VoiceCaptureSpec {
    fn default() -> Self {
        Self {
            sample_rate: default_sample_rate(),
            channels: default_audio_channels(),
            chunk_size: default_chunk_size(),
            device_name: None,
        }
    }
}

/// Speaker playback sub-config of [`VoiceAudioConfig`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VoicePlaybackSpec {
    /// Sample rate in Hz.
    #[serde(default = "default_sample_rate", alias = "sampleRate")]
    pub sample_rate: u32,
    /// Number of channels (1 = mono).
    #[serde(default = "default_audio_channels")]
    pub channels: u16,
    /// Optional playback device name; `None` = system default.
    #[serde(default, alias = "deviceName")]
    pub device_name: Option<String>,
}

impl Default for VoicePlaybackSpec {
    fn default() -> Self {
        Self {
            sample_rate: default_sample_rate(),
            channels: default_audio_channels(),
            device_name: None,
        }
    }
}

fn default_sample_rate() -> u32 {
    16_000
}
fn default_audio_channels() -> u16 {
    1
}
fn default_chunk_size() -> u32 {
    512
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_audio_config_serde_roundtrip_duplex() {
        let original = VoiceAudioConfig::duplex_default();
        let json = serde_json::to_string(&original).unwrap();
        let parsed: VoiceAudioConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(original, parsed);
        // duplex includes both keys
        assert!(json.contains("\"capture\""));
        assert!(json.contains("\"playback\""));
    }

    #[test]
    fn voice_audio_config_serde_roundtrip_capture_only() {
        let original = VoiceAudioConfig::capture_only(VoiceCaptureSpec {
            sample_rate: 48_000,
            channels: 2,
            chunk_size: 1024,
            device_name: Some("HyperX".into()),
        });
        let json = serde_json::to_string(&original).unwrap();
        let parsed: VoiceAudioConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(original, parsed);
        // playback elided per skip_serializing_if
        assert!(!json.contains("\"playback\""));
    }

    #[test]
    fn voice_audio_config_serde_roundtrip_playback_only() {
        let original = VoiceAudioConfig::playback_only(VoicePlaybackSpec {
            sample_rate: 22_050,
            channels: 1,
            device_name: None,
        });
        let json = serde_json::to_string(&original).unwrap();
        let parsed: VoiceAudioConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(original, parsed);
        assert!(!json.contains("\"capture\""));
    }

    #[test]
    fn voice_audio_config_default_is_empty() {
        let cfg: VoiceAudioConfig = Default::default();
        assert!(cfg.capture.is_none());
        assert!(cfg.playback.is_none());
    }

    #[test]
    fn voice_audio_config_camelcase_aliases() {
        // Accept the JS-style camelCase keys produced by the GUI.
        let json = r#"{
            "capture": {"sampleRate": 16000, "chunkSize": 256},
            "playback": {"sampleRate": 24000, "deviceName": "Speakers"}
        }"#;
        let parsed: VoiceAudioConfig = serde_json::from_str(json).unwrap();
        let cap = parsed.capture.unwrap();
        assert_eq!(cap.sample_rate, 16_000);
        assert_eq!(cap.chunk_size, 256);
        let play = parsed.playback.unwrap();
        assert_eq!(play.sample_rate, 24_000);
        assert_eq!(play.device_name.as_deref(), Some("Speakers"));
    }
}
