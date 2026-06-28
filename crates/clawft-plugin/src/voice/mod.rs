//! Voice pipeline components for ClawFT.
//!
//! All types are gated behind the `voice` feature flag.
//! Sub-features (`voice-stt`, `voice-tts`, `voice-vad`, `voice-wake`)
//! control which pipeline components are compiled.

pub mod config;

#[cfg(feature = "voice-vad")]
pub mod capture;
#[cfg(feature = "voice-vad")]
pub mod playback;
#[cfg(feature = "voice-vad")]
pub mod privacy_indicator;
#[cfg(feature = "voice-vad")]
pub mod vad;

#[cfg(feature = "voice-stt")]
pub mod stt;

#[cfg(feature = "voice-tts")]
pub mod tts;

pub mod models;

#[cfg(feature = "voice-wake")]
pub mod wake;
#[cfg(feature = "voice-wake")]
pub mod wake_daemon;

pub mod channel;
pub mod echo;
pub mod events;
pub mod noise;
pub mod quality;
pub mod talk_mode;

pub mod cloud_stt;
pub mod cloud_tts;
pub mod commands;
pub mod fallback;
pub mod transcript_log;

// Re-export key types
pub use channel::{VoiceChannel, VoiceStatus};
pub use config::{VoiceAudioConfig, VoiceCaptureSpec, VoicePipelineConfig, VoicePlaybackSpec};
pub use echo::{EchoCanceller, EchoCancellerConfig};
pub use events::VoiceWsEvent;
pub use models::ModelDownloadManager;
pub use noise::{NoiseSuppressor, NoiseSuppressorConfig};
pub use quality::{AudioMetrics, analyze_frame};
pub use talk_mode::TalkModeController;

#[cfg(feature = "voice-wake")]
pub use wake::{WakeWordConfig, WakeWordDetector, WakeWordEvent};
#[cfg(feature = "voice-wake")]
pub use wake_daemon::WakeDaemon;
