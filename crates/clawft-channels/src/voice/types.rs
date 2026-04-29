//! Voice channel configuration and error types.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Default substrate STT endpoint (whisper.cpp HTTP server).
///
/// 8112 is the WeftOS reservation for the substrate-side whisper service.
/// `clawft-service-whisper` itself talks to whisper.cpp on 8080, but the
/// voice channel goes through the substrate-fronted port (configurable).
pub const DEFAULT_WHISPER_ENDPOINT: &str = "http://localhost:8112";

/// Default substrate TTS endpoint.
pub const DEFAULT_TTS_ENDPOINT: &str = "http://localhost:8113";

/// Default STT path on the whisper endpoint. whisper.cpp's HTTP server
/// uses `/inference` by convention; M5's substrate fronting may expose
/// `/transcribe` instead. Make this configurable so both shapes work.
pub const DEFAULT_TRANSCRIBE_PATH: &str = "/inference";

/// Default TTS path on the synthesis endpoint.
pub const DEFAULT_SYNTHESIZE_PATH: &str = "/synthesize";

/// Configuration for the voice channel adapter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceAdapterConfig {
    /// Substrate Whisper STT endpoint (no trailing slash).
    #[serde(default = "default_whisper_endpoint", alias = "whisperEndpoint")]
    pub whisper_endpoint: String,

    /// Path on the whisper endpoint for multipart transcription.
    #[serde(default = "default_transcribe_path", alias = "transcribePath")]
    pub transcribe_path: String,

    /// Substrate TTS endpoint (no trailing slash).
    #[serde(default = "default_tts_endpoint", alias = "ttsEndpoint")]
    pub tts_endpoint: String,

    /// Path on the TTS endpoint for synthesis.
    #[serde(default = "default_synthesize_path", alias = "synthesizePath")]
    pub synthesize_path: String,

    /// Optional cpal device name; `None` = use the system default device.
    #[serde(default, alias = "deviceName")]
    pub device_name: Option<String>,

    /// Capture sample rate in Hz. Whisper expects 16 kHz.
    #[serde(default = "default_sample_rate", alias = "sampleRate")]
    pub sample_rate: u32,

    /// Allowed sender ids. Empty = allow all.
    #[serde(default, alias = "allowedSenders")]
    pub allowed_senders: Vec<String>,

    /// VAD energy threshold in dBFS (RMS). -45 matches
    /// `clawft-service-classify`'s default. More negative = more sensitive.
    #[serde(default = "default_vad_threshold_dbfs", alias = "vadThresholdDbfs")]
    pub vad_threshold_dbfs: f32,

    /// Silence duration (ms) that ends an utterance.
    #[serde(default = "default_silence_ms", alias = "silenceMs")]
    pub silence_ms: u32,

    /// Minimum utterance length (ms). Shorter segments are dropped.
    #[serde(default = "default_min_utterance_ms", alias = "minUtteranceMs")]
    pub min_utterance_ms: u32,

    /// Maximum utterance length (ms). Forces a flush past this point.
    #[serde(default = "default_max_utterance_ms", alias = "maxUtteranceMs")]
    pub max_utterance_ms: u32,

    /// HTTP request timeout (seconds) for STT / TTS calls.
    #[serde(default = "default_request_timeout_s", alias = "requestTimeoutSecs")]
    pub request_timeout_s: u64,

    /// BCP-47 language hint sent to the whisper endpoint (`en`, `auto`, …).
    #[serde(default = "default_language")]
    pub language: String,

    /// Sender id stamped on inbound transcript messages, used for the
    /// `ChannelAdapterHost` allow-list and metadata.
    #[serde(default = "default_sender_id", alias = "senderId")]
    pub sender_id: String,

    /// Chat id stamped on inbound messages (a stable correlation key
    /// scoped to this channel instance).
    #[serde(default = "default_chat_id", alias = "chatId")]
    pub chat_id: String,
}

fn default_whisper_endpoint() -> String {
    DEFAULT_WHISPER_ENDPOINT.into()
}
fn default_transcribe_path() -> String {
    DEFAULT_TRANSCRIBE_PATH.into()
}
fn default_tts_endpoint() -> String {
    DEFAULT_TTS_ENDPOINT.into()
}
fn default_synthesize_path() -> String {
    DEFAULT_SYNTHESIZE_PATH.into()
}
fn default_sample_rate() -> u32 {
    16_000
}
fn default_vad_threshold_dbfs() -> f32 {
    -45.0
}
fn default_silence_ms() -> u32 {
    700
}
fn default_min_utterance_ms() -> u32 {
    300
}
fn default_max_utterance_ms() -> u32 {
    20_000
}
fn default_request_timeout_s() -> u64 {
    30
}
fn default_language() -> String {
    "en".into()
}
fn default_sender_id() -> String {
    "voice-local".into()
}
fn default_chat_id() -> String {
    "voice".into()
}

impl Default for VoiceAdapterConfig {
    fn default() -> Self {
        Self {
            whisper_endpoint: default_whisper_endpoint(),
            transcribe_path: default_transcribe_path(),
            tts_endpoint: default_tts_endpoint(),
            synthesize_path: default_synthesize_path(),
            device_name: None,
            sample_rate: default_sample_rate(),
            allowed_senders: Vec::new(),
            vad_threshold_dbfs: default_vad_threshold_dbfs(),
            silence_ms: default_silence_ms(),
            min_utterance_ms: default_min_utterance_ms(),
            max_utterance_ms: default_max_utterance_ms(),
            request_timeout_s: default_request_timeout_s(),
            language: default_language(),
            sender_id: default_sender_id(),
            chat_id: default_chat_id(),
        }
    }
}

impl VoiceAdapterConfig {
    /// Validate the config; returns a human-readable error string on failure.
    pub fn validate(&self) -> Result<(), String> {
        if self.whisper_endpoint.is_empty() {
            return Err("voice adapter: whisper_endpoint is required".into());
        }
        if !self.whisper_endpoint.starts_with("http://")
            && !self.whisper_endpoint.starts_with("https://")
        {
            return Err(format!(
                "voice adapter: whisper_endpoint must be http(s)://, got {:?}",
                self.whisper_endpoint
            ));
        }
        if self.tts_endpoint.is_empty() {
            return Err("voice adapter: tts_endpoint is required".into());
        }
        if !self.tts_endpoint.starts_with("http://") && !self.tts_endpoint.starts_with("https://") {
            return Err(format!(
                "voice adapter: tts_endpoint must be http(s)://, got {:?}",
                self.tts_endpoint
            ));
        }
        if self.sample_rate < 8_000 || self.sample_rate > 48_000 {
            return Err(format!(
                "voice adapter: sample_rate must be 8000..=48000, got {}",
                self.sample_rate
            ));
        }
        if self.silence_ms < 50 {
            return Err("voice adapter: silence_ms must be >= 50".into());
        }
        if self.min_utterance_ms == 0 {
            return Err("voice adapter: min_utterance_ms must be > 0".into());
        }
        if self.max_utterance_ms <= self.min_utterance_ms {
            return Err(
                "voice adapter: max_utterance_ms must be > min_utterance_ms".into(),
            );
        }
        Ok(())
    }

    /// Full transcribe URL (no trailing slash logic, paths concat as-is).
    pub fn transcribe_url(&self) -> String {
        format!(
            "{}{}",
            self.whisper_endpoint.trim_end_matches('/'),
            normalize_path(&self.transcribe_path)
        )
    }

    /// Full synthesize URL.
    pub fn synthesize_url(&self) -> String {
        format!(
            "{}{}",
            self.tts_endpoint.trim_end_matches('/'),
            normalize_path(&self.synthesize_path)
        )
    }
}

fn normalize_path(p: &str) -> String {
    if p.starts_with('/') {
        p.to_string()
    } else {
        format!("/{p}")
    }
}

/// Errors emitted internally by the voice channel.
///
/// Public so factory tests / future callers can match shapes; the
/// `ChannelAdapter` trait surface only ever surfaces
/// [`PluginError`](clawft_plugin::error::PluginError) -- conversions are
/// in `channel.rs`.
#[derive(Debug, Error)]
pub enum VoiceError {
    /// Configuration was rejected by [`VoiceAdapterConfig::validate`].
    #[error("voice config: {0}")]
    Config(String),
    /// HTTP transport / wire-level error talking to the substrate.
    #[error("voice transport: {0}")]
    Transport(String),
    /// Whisper / TTS server returned a non-2xx status.
    #[error("voice server {status}: {body}")]
    Server {
        /// HTTP status returned.
        status: u16,
        /// Server response body (truncated to 4 KiB before logging).
        body: String,
    },
    /// JSON or audio bytes were not in the expected shape.
    #[error("voice malformed: {0}")]
    Malformed(String),
    /// Audio I/O failure (cpal / output sink).
    #[error("voice audio: {0}")]
    Audio(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_validate() {
        VoiceAdapterConfig::default().validate().unwrap();
    }

    #[test]
    fn rejects_empty_whisper_endpoint() {
        let cfg = VoiceAdapterConfig {
            whisper_endpoint: String::new(),
            ..Default::default()
        };
        assert!(cfg.validate().unwrap_err().contains("whisper_endpoint"));
    }

    #[test]
    fn rejects_non_http_whisper_endpoint() {
        let cfg = VoiceAdapterConfig {
            whisper_endpoint: "ftp://nope".into(),
            ..Default::default()
        };
        assert!(cfg.validate().unwrap_err().contains("http"));
    }

    #[test]
    fn rejects_empty_tts_endpoint() {
        let cfg = VoiceAdapterConfig {
            tts_endpoint: String::new(),
            ..Default::default()
        };
        assert!(cfg.validate().unwrap_err().contains("tts_endpoint"));
    }

    #[test]
    fn rejects_bad_sample_rate() {
        let cfg = VoiceAdapterConfig {
            sample_rate: 100,
            ..Default::default()
        };
        assert!(cfg.validate().unwrap_err().contains("sample_rate"));
        let cfg = VoiceAdapterConfig {
            sample_rate: 96_000,
            ..Default::default()
        };
        assert!(cfg.validate().unwrap_err().contains("sample_rate"));
    }

    #[test]
    fn rejects_bad_utterance_bounds() {
        let cfg = VoiceAdapterConfig {
            min_utterance_ms: 0,
            ..Default::default()
        };
        assert!(cfg.validate().unwrap_err().contains("min_utterance"));
        let cfg = VoiceAdapterConfig {
            min_utterance_ms: 5_000,
            max_utterance_ms: 1_000,
            ..Default::default()
        };
        assert!(cfg.validate().unwrap_err().contains("max_utterance"));
    }

    #[test]
    fn url_composition() {
        let cfg = VoiceAdapterConfig {
            whisper_endpoint: "http://localhost:8112/".into(),
            transcribe_path: "transcribe".into(),
            tts_endpoint: "http://tts.example.com".into(),
            synthesize_path: "/synthesize".into(),
            ..Default::default()
        };
        assert_eq!(cfg.transcribe_url(), "http://localhost:8112/transcribe");
        assert_eq!(
            cfg.synthesize_url(),
            "http://tts.example.com/synthesize"
        );
    }

    #[test]
    fn serde_camelcase_aliases() {
        let json = serde_json::json!({
            "whisperEndpoint": "http://h:1",
            "ttsEndpoint": "http://t:2",
            "deviceName": "default",
            "sampleRate": 16000,
            "allowedSenders": ["mathew"],
            "vadThresholdDbfs": -42.0,
            "silenceMs": 800,
            "minUtteranceMs": 250,
            "maxUtteranceMs": 15000,
            "requestTimeoutSecs": 20,
            "senderId": "u1",
            "chatId": "c1"
        });
        let cfg: VoiceAdapterConfig = serde_json::from_value(json).unwrap();
        assert_eq!(cfg.whisper_endpoint, "http://h:1");
        assert_eq!(cfg.tts_endpoint, "http://t:2");
        assert_eq!(cfg.device_name.as_deref(), Some("default"));
        assert_eq!(cfg.allowed_senders, vec!["mathew".to_string()]);
        assert_eq!(cfg.vad_threshold_dbfs, -42.0);
        assert_eq!(cfg.silence_ms, 800);
        assert_eq!(cfg.sender_id, "u1");
        assert_eq!(cfg.chat_id, "c1");
    }

    #[test]
    fn voice_error_display() {
        let e = VoiceError::Server {
            status: 503,
            body: "loading".into(),
        };
        assert!(e.to_string().contains("503"));
        let e = VoiceError::Malformed("no text field".into());
        assert!(e.to_string().contains("malformed"));
    }
}
