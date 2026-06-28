//! Cloud-based text-to-speech providers.
//!
//! Defines the [`CloudTtsProvider`] trait and provides implementations
//! for OpenAI TTS ([`OpenAiTtsProvider`]) and ElevenLabs
//! ([`ElevenLabsTtsProvider`]).

use async_trait::async_trait;

use crate::PluginError;

/// Result from a cloud TTS synthesis.
#[derive(Debug, Clone)]
pub struct CloudTtsResult {
    /// Raw audio data.
    pub audio_data: Vec<u8>,
    /// MIME type of the audio (e.g., "audio/mp3", "audio/mpeg").
    pub mime_type: String,
    /// Duration of the synthesized audio in milliseconds, if known.
    pub duration_ms: Option<u64>,
}

/// Information about an available voice.
#[derive(Debug, Clone, serde::Serialize)]
pub struct VoiceInfo {
    /// Provider-specific voice ID.
    pub id: String,
    /// Human-readable voice name.
    pub name: String,
    /// Primary language code (BCP-47).
    pub language: String,
}

/// Trait for cloud-based text-to-speech providers.
#[async_trait]
pub trait CloudTtsProvider: Send + Sync {
    /// Provider name (e.g., "openai-tts", "elevenlabs").
    fn name(&self) -> &str;

    /// List available voices for this provider.
    fn available_voices(&self) -> Vec<VoiceInfo>;

    /// Synthesize text to audio bytes.
    ///
    /// * `text` - The text to synthesize.
    /// * `voice_id` - Provider-specific voice identifier.
    async fn synthesize(&self, text: &str, voice_id: &str) -> Result<CloudTtsResult, PluginError>;
}

// ---------------------------------------------------------------------------
// OpenAI TTS
// ---------------------------------------------------------------------------

/// OpenAI TTS API implementation.
///
/// Posts to `https://api.openai.com/v1/audio/speech` with model "tts-1".
/// Available voices: alloy, echo, fable, onyx, nova, shimmer.
pub struct OpenAiTtsProvider {
    api_key: String,
    model: String,
    client: reqwest::Client,
}

impl OpenAiTtsProvider {
    /// Create a new OpenAI TTS provider with the given API key.
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            model: "tts-1".to_string(),
            client: reqwest::Client::new(),
        }
    }

    /// Override the TTS model (default: "tts-1").
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }
}

#[async_trait]
impl CloudTtsProvider for OpenAiTtsProvider {
    fn name(&self) -> &str {
        "openai-tts"
    }

    fn available_voices(&self) -> Vec<VoiceInfo> {
        ["alloy", "echo", "fable", "onyx", "nova", "shimmer"]
            .iter()
            .map(|v| VoiceInfo {
                id: v.to_string(),
                name: v.to_string(),
                language: "en".to_string(),
            })
            .collect()
    }

    async fn synthesize(&self, text: &str, voice_id: &str) -> Result<CloudTtsResult, PluginError> {
        let body = serde_json::json!({
            "model": self.model,
            "input": text,
            "voice": voice_id,
            "response_format": "mp3",
        });

        let resp = self
            .client
            .post("https://api.openai.com/v1/audio/speech")
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| PluginError::ExecutionFailed(format!("OpenAI TTS request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_body = resp.text().await.unwrap_or_default();
            return Err(PluginError::ExecutionFailed(format!(
                "OpenAI TTS returned {status}: {err_body}"
            )));
        }

        let audio_data = resp
            .bytes()
            .await
            .map_err(|e| PluginError::ExecutionFailed(format!("TTS response read error: {e}")))?
            .to_vec();

        Ok(CloudTtsResult {
            audio_data,
            mime_type: "audio/mp3".to_string(),
            duration_ms: None,
        })
    }
}

// ---------------------------------------------------------------------------
// ElevenLabs TTS
// ---------------------------------------------------------------------------

/// ElevenLabs TTS API implementation.
///
/// Posts to `https://api.elevenlabs.io/v1/text-to-speech/{voice_id}`
/// with `xi-api-key` header authentication.
pub struct ElevenLabsTtsProvider {
    api_key: String,
    client: reqwest::Client,
}

impl ElevenLabsTtsProvider {
    /// Create a new ElevenLabs TTS provider with the given API key.
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl CloudTtsProvider for ElevenLabsTtsProvider {
    fn name(&self) -> &str {
        "elevenlabs"
    }

    fn available_voices(&self) -> Vec<VoiceInfo> {
        vec![
            VoiceInfo {
                id: "21m00Tcm4TlvDq8ikWAM".into(),
                name: "Rachel".into(),
                language: "en".into(),
            },
            VoiceInfo {
                id: "AZnzlk1XvdvUeBnXmlld".into(),
                name: "Domi".into(),
                language: "en".into(),
            },
            VoiceInfo {
                id: "EXAVITQu4vr4xnSDxMaL".into(),
                name: "Bella".into(),
                language: "en".into(),
            },
            VoiceInfo {
                id: "ErXwobaYiN019PkySvjV".into(),
                name: "Antoni".into(),
                language: "en".into(),
            },
        ]
    }

    async fn synthesize(&self, text: &str, voice_id: &str) -> Result<CloudTtsResult, PluginError> {
        let url = format!("https://api.elevenlabs.io/v1/text-to-speech/{voice_id}");
        let body = serde_json::json!({
            "text": text,
            "model_id": "eleven_monolingual_v1",
            "voice_settings": {
                "stability": 0.5,
                "similarity_boost": 0.75,
            },
        });

        let resp = self
            .client
            .post(&url)
            .header("xi-api-key", &self.api_key)
            .header("Content-Type", "application/json")
            .header("Accept", "audio/mpeg")
            .json(&body)
            .send()
            .await
            .map_err(|e| PluginError::ExecutionFailed(format!("ElevenLabs request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_body = resp.text().await.unwrap_or_default();
            return Err(PluginError::ExecutionFailed(format!(
                "ElevenLabs returned {status}: {err_body}"
            )));
        }

        let audio_data = resp
            .bytes()
            .await
            .map_err(|e| PluginError::ExecutionFailed(format!("ElevenLabs read error: {e}")))?
            .to_vec();

        Ok(CloudTtsResult {
            audio_data,
            mime_type: "audio/mpeg".to_string(),
            duration_ms: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- OpenAI TTS tests --

    #[test]
    fn openai_tts_provider_name() {
        let provider = OpenAiTtsProvider::new("test-key".into());
        assert_eq!(provider.name(), "openai-tts");
    }

    #[test]
    fn openai_tts_available_voices() {
        let provider = OpenAiTtsProvider::new("test-key".into());
        let voices = provider.available_voices();
        assert_eq!(voices.len(), 6);
        let ids: Vec<&str> = voices.iter().map(|v| v.id.as_str()).collect();
        assert!(ids.contains(&"alloy"));
        assert!(ids.contains(&"echo"));
        assert!(ids.contains(&"fable"));
        assert!(ids.contains(&"onyx"));
        assert!(ids.contains(&"nova"));
        assert!(ids.contains(&"shimmer"));
    }

    #[test]
    fn openai_tts_with_model_builder() {
        let provider = OpenAiTtsProvider::new("test-key".into()).with_model("tts-1-hd");
        assert_eq!(provider.model, "tts-1-hd");
    }

    #[tokio::test]
    async fn openai_tts_synthesize_invalid_key_errors() {
        let provider = OpenAiTtsProvider::new("invalid-key".into());
        let result = provider.synthesize("hello", "alloy").await;
        assert!(result.is_err());
    }

    // -- ElevenLabs TTS tests --

    #[test]
    fn elevenlabs_provider_name() {
        let provider = ElevenLabsTtsProvider::new("test-key".into());
        assert_eq!(provider.name(), "elevenlabs");
    }

    #[test]
    fn elevenlabs_available_voices() {
        let provider = ElevenLabsTtsProvider::new("test-key".into());
        let voices = provider.available_voices();
        assert_eq!(voices.len(), 4);
        let names: Vec<&str> = voices.iter().map(|v| v.name.as_str()).collect();
        assert!(names.contains(&"Rachel"));
        assert!(names.contains(&"Domi"));
        assert!(names.contains(&"Bella"));
        assert!(names.contains(&"Antoni"));
    }

    #[test]
    fn cloud_tts_result_fields() {
        let result = CloudTtsResult {
            audio_data: vec![1, 2, 3],
            mime_type: "audio/mp3".into(),
            duration_ms: Some(1500),
        };
        assert_eq!(result.audio_data, vec![1, 2, 3]);
        assert_eq!(result.mime_type, "audio/mp3");
        assert_eq!(result.duration_ms, Some(1500));
    }

    #[tokio::test]
    async fn elevenlabs_synthesize_invalid_key_errors() {
        let provider = ElevenLabsTtsProvider::new("invalid-key".into());
        let result = provider.synthesize("hello", "21m00Tcm4TlvDq8ikWAM").await;
        assert!(result.is_err());
    }

    #[test]
    fn voice_info_serializable() {
        let info = VoiceInfo {
            id: "alloy".into(),
            name: "Alloy".into(),
            language: "en".into(),
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("\"id\":\"alloy\""));
        assert!(json.contains("\"name\":\"Alloy\""));
        assert!(json.contains("\"language\":\"en\""));
    }
}
