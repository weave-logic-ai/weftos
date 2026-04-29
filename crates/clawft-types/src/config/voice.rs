//! Voice pipeline configuration types (W-VOICE workstream).
//!
//! Defines the full voice pipeline config: audio capture, STT, TTS,
//! voice activity detection, wake word, and cloud fallback.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::personality::VoicePersonality;

/// Voice pipeline configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VoiceConfig {
    /// Enable voice features globally.
    #[serde(default)]
    pub enabled: bool,

    /// Audio capture settings.
    #[serde(default)]
    pub audio: AudioConfig,

    /// Speech-to-text settings.
    #[serde(default)]
    pub stt: SttConfig,

    /// Text-to-speech settings.
    #[serde(default)]
    pub tts: TtsConfig,

    /// Voice activity detection settings.
    #[serde(default)]
    pub vad: VadConfig,

    /// Wake word detection settings.
    #[serde(default)]
    pub wake: WakeConfig,

    /// Cloud fallback settings.
    #[serde(default, alias = "cloudFallback")]
    pub cloud_fallback: CloudFallbackConfig,

    /// Per-agent voice personality map.
    ///
    /// Keys are agent names/IDs, values are their voice personalities.
    /// Agents not in this map use the default personality.
    #[serde(default, alias = "personalities")]
    pub personalities: HashMap<String, VoicePersonality>,

    /// Voice transcript consumer configuration (WEFT-555 / M5-W).
    ///
    /// Controls the daemon-side bridge that subscribes to the substrate
    /// transcript topic published by `clawft-service-whisper`, then
    /// routes each transcript into either:
    ///
    /// - the configured agent's `agent.chat` conversation, or
    /// - the daemon's RPC dispatch, when the transcript starts with
    ///   the configured `command_prefix`.
    ///
    /// Disabled by default; voice features must be opted into.
    #[serde(default)]
    pub consumer: VoiceConsumerConfig,
}

/// Voice transcript consumer configuration.
///
/// The consumer subscribes to a single substrate path emitted by
/// `clawft-service-whisper` (typically the mesh-canonical
/// `substrate/_derived/transcript/<source-node>/mic`) and bridges
/// transcripts into the daemon's agent / command surfaces.
///
/// Defaults match the daemon's ESP32-source whisper wiring at boot
/// (`WHISPER_INPUT_NODE_ID` env var, falling back to `n-bfc4cd`). The
/// `enabled` flag defaults to `false` — voice routing is opt-in until
/// the 5 P0 voice security controls (WEFT-207/208/209/210/211) ship.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceConsumerConfig {
    /// Master enable toggle. When `false` the daemon does not subscribe
    /// to the transcript topic at all; whisper still publishes
    /// transcripts but no consumer reads them.
    #[serde(default)]
    pub enabled: bool,

    /// Substrate path to subscribe to for transcripts. Defaults to the
    /// mesh-canonical path produced by `clawft-service-whisper` for the
    /// daemon's configured input source node.
    #[serde(default = "default_transcript_topic", alias = "transcriptTopic")]
    pub transcript_topic: String,

    /// Agent identifier whose conversation receives non-command
    /// transcripts. Defaults to the daemon's single concierge principal
    /// (`concierge-bot`); per-agent fan-out is a future phase.
    #[serde(default = "default_chat_target_agent", alias = "chatTargetAgent")]
    pub chat_target_agent: String,

    /// Conversation id used when synthesizing the agent.chat dispatch.
    /// Stable across transcripts so the agent loop's per-conv state
    /// (mutex, sink, heartbeat) accumulates one continuous voice
    /// session.
    #[serde(default = "default_voice_conv_id", alias = "convId")]
    pub conv_id: String,

    /// Prefix indicating a transcript should be parsed as a command and
    /// routed through the daemon's RPC dispatch instead of the agent
    /// conversation. The prefix (including any trailing whitespace) is
    /// stripped before parsing; the remainder is whitespace-split into
    /// `<method> <args...>`.
    ///
    /// Empty string disables command routing — every transcript falls
    /// through to chat.
    #[serde(default = "default_command_prefix", alias = "commandPrefix")]
    pub command_prefix: String,
}

fn default_transcript_topic() -> String {
    "substrate/_derived/transcript/n-bfc4cd/mic".into()
}

fn default_chat_target_agent() -> String {
    "concierge-bot".into()
}

fn default_voice_conv_id() -> String {
    "voice-default".into()
}

fn default_command_prefix() -> String {
    "weft ".into()
}

impl Default for VoiceConsumerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            transcript_topic: default_transcript_topic(),
            chat_target_agent: default_chat_target_agent(),
            conv_id: default_voice_conv_id(),
            command_prefix: default_command_prefix(),
        }
    }
}

/// Audio capture/playback configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioConfig {
    /// Sample rate in Hz.
    #[serde(default = "default_sample_rate", alias = "sampleRate")]
    pub sample_rate: u32,

    /// Audio chunk size in samples.
    #[serde(default = "default_chunk_size", alias = "chunkSize")]
    pub chunk_size: u32,

    /// Number of audio channels (1 = mono).
    #[serde(default = "default_audio_channels")]
    pub channels: u16,

    /// Input device name (None = system default).
    #[serde(default, alias = "inputDevice")]
    pub input_device: Option<String>,

    /// Output device name (None = system default).
    #[serde(default, alias = "outputDevice")]
    pub output_device: Option<String>,
}

fn default_sample_rate() -> u32 {
    16000
}
fn default_chunk_size() -> u32 {
    512
}
fn default_audio_channels() -> u16 {
    1
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            sample_rate: default_sample_rate(),
            chunk_size: default_chunk_size(),
            channels: default_audio_channels(),
            input_device: None,
            output_device: None,
        }
    }
}

/// Speech-to-text configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SttConfig {
    /// Enable STT.
    #[serde(default = "super::default_true")]
    pub enabled: bool,

    /// STT model name or path.
    #[serde(default = "default_stt_model")]
    pub model: String,

    /// Language code (e.g. "en", "zh", "es"). Empty = auto-detect.
    #[serde(default)]
    pub language: String,
}

fn default_stt_model() -> String {
    "sherpa-onnx-streaming-zipformer-en-20M".into()
}

impl Default for SttConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            model: default_stt_model(),
            language: String::new(),
        }
    }
}

/// Text-to-speech configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsConfig {
    /// Enable TTS.
    #[serde(default = "super::default_true")]
    pub enabled: bool,

    /// TTS provider: "browser" (Web Speech API), "openai", or "elevenlabs".
    #[serde(default = "default_tts_provider")]
    pub provider: String,

    /// TTS model name or path.
    #[serde(default = "default_tts_model")]
    pub model: String,

    /// TTS voice ID.
    #[serde(default)]
    pub voice: String,

    /// Speaking speed multiplier (1.0 = normal).
    #[serde(default = "default_speed")]
    pub speed: f32,
}

fn default_tts_provider() -> String {
    "browser".into()
}
fn default_tts_model() -> String {
    "vits-piper-en_US-amy-medium".into()
}
fn default_speed() -> f32 {
    1.0
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            provider: default_tts_provider(),
            model: default_tts_model(),
            voice: String::new(),
            speed: default_speed(),
        }
    }
}

/// Voice activity detection configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VadConfig {
    /// VAD activation threshold (0.0-1.0).
    #[serde(default = "default_vad_threshold")]
    pub threshold: f32,

    /// Silence duration in ms before speech end.
    #[serde(default = "default_silence_timeout_ms", alias = "silenceTimeoutMs")]
    pub silence_timeout_ms: u32,

    /// Minimum speech duration in ms to trigger processing.
    #[serde(default = "default_min_speech_ms", alias = "minSpeechMs")]
    pub min_speech_ms: u32,
}

fn default_vad_threshold() -> f32 {
    0.5
}
fn default_silence_timeout_ms() -> u32 {
    1500
}
fn default_min_speech_ms() -> u32 {
    250
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            threshold: default_vad_threshold(),
            silence_timeout_ms: default_silence_timeout_ms(),
            min_speech_ms: default_min_speech_ms(),
        }
    }
}

/// Wake word detection configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WakeConfig {
    /// Enable wake word detection.
    #[serde(default)]
    pub enabled: bool,

    /// Wake word phrase (e.g. "hey weft").
    #[serde(default = "default_wake_phrase")]
    pub phrase: String,

    /// Detection sensitivity (0.0-1.0).
    #[serde(default = "default_wake_sensitivity")]
    pub sensitivity: f32,

    /// Custom wake word model path.
    #[serde(default, alias = "modelPath")]
    pub model_path: Option<String>,
}

fn default_wake_phrase() -> String {
    "hey weft".into()
}
fn default_wake_sensitivity() -> f32 {
    0.5
}

impl Default for WakeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            phrase: default_wake_phrase(),
            sensitivity: default_wake_sensitivity(),
            model_path: None,
        }
    }
}

/// Cloud STT/TTS fallback configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CloudFallbackConfig {
    /// Enable cloud fallback when local models fail.
    #[serde(default)]
    pub enabled: bool,

    /// Cloud STT provider ("whisper" for OpenAI Whisper API).
    #[serde(default, alias = "sttProvider")]
    pub stt_provider: String,

    /// Cloud TTS provider ("elevenlabs" or "openai").
    #[serde(default, alias = "ttsProvider")]
    pub tts_provider: String,
}
