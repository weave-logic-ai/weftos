//! Per-agent voice personality configuration.
//!
//! [`VoicePersonality`] defines the voice characteristics for an agent
//! in a multi-agent setup, allowing users to distinguish agents by
//! their distinct voice, speed, pitch, and language.

use serde::{Deserialize, Serialize};

/// Voice personality configuration for an agent.
///
/// Each agent in a multi-agent setup can have a distinct voice,
/// allowing users to distinguish agents by sound.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoicePersonality {
    /// Voice model or voice ID to use for TTS.
    ///
    /// For local TTS: model name (e.g., "en_US-amy-medium").
    /// For cloud TTS: provider-specific voice ID (e.g., "nova",
    /// "EXAVITQu4vr4xnSDxMaL").
    pub voice_id: String,

    /// Preferred TTS provider ("local", "openai", "elevenlabs").
    #[serde(default = "default_provider")]
    pub provider: String,

    /// Speech rate multiplier (0.5 = half speed, 2.0 = double speed).
    #[serde(default = "default_speed")]
    pub speed: f32,

    /// Pitch adjustment (-1.0 to 1.0, 0.0 = default).
    #[serde(default)]
    pub pitch: f32,

    /// Optional spoken name prefix (e.g., "This is Agent Alpha.").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub greeting_prefix: Option<String>,

    /// Language code for this agent's voice (BCP-47).
    #[serde(default = "default_language")]
    pub language: String,
}

fn default_provider() -> String {
    "local".to_string()
}
fn default_speed() -> f32 {
    1.0
}
fn default_language() -> String {
    "en".to_string()
}

impl Default for VoicePersonality {
    fn default() -> Self {
        Self {
            voice_id: "default".to_string(),
            provider: default_provider(),
            speed: default_speed(),
            pitch: 0.0,
            greeting_prefix: None,
            language: default_language(),
        }
    }
}

impl VoicePersonality {
    /// Validate the personality configuration.
    ///
    /// Returns an error message if any field is out of range.
    pub fn validate(&self) -> Result<(), String> {
        if self.speed < 0.5 || self.speed > 2.0 {
            return Err(format!(
                "speed must be between 0.5 and 2.0, got {}",
                self.speed
            ));
        }
        if self.pitch < -1.0 || self.pitch > 1.0 {
            return Err(format!(
                "pitch must be between -1.0 and 1.0, got {}",
                self.pitch
            ));
        }
        if self.voice_id.is_empty() {
            return Err("voice_id must not be empty".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_personality() {
        let p = VoicePersonality::default();
        assert_eq!(p.voice_id, "default");
        assert_eq!(p.provider, "local");
        assert!((p.speed - 1.0).abs() < f32::EPSILON);
        assert!((p.pitch - 0.0).abs() < f32::EPSILON);
        assert!(p.greeting_prefix.is_none());
        assert_eq!(p.language, "en");
    }

    #[test]
    fn serde_roundtrip() {
        let p = VoicePersonality {
            voice_id: "nova".into(),
            provider: "openai".into(),
            speed: 1.2,
            pitch: -0.5,
            greeting_prefix: Some("I am Agent Alpha.".into()),
            language: "en-US".into(),
        };
        let json = serde_json::to_string(&p).unwrap();
        let restored: VoicePersonality = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.voice_id, "nova");
        assert_eq!(restored.provider, "openai");
        assert!((restored.speed - 1.2).abs() < f32::EPSILON);
        assert!((restored.pitch - (-0.5)).abs() < f32::EPSILON);
        assert_eq!(
            restored.greeting_prefix.as_deref(),
            Some("I am Agent Alpha.")
        );
        assert_eq!(restored.language, "en-US");
    }

    #[test]
    fn serde_defaults_applied() {
        let json = r#"{"voice_id": "alloy"}"#;
        let p: VoicePersonality = serde_json::from_str(json).unwrap();
        assert_eq!(p.voice_id, "alloy");
        assert_eq!(p.provider, "local");
        assert!((p.speed - 1.0).abs() < f32::EPSILON);
        assert!((p.pitch - 0.0).abs() < f32::EPSILON);
        assert_eq!(p.language, "en");
    }

    #[test]
    fn greeting_prefix_omitted_when_none() {
        let p = VoicePersonality::default();
        let json = serde_json::to_string(&p).unwrap();
        assert!(!json.contains("greeting_prefix"));
    }

    #[test]
    fn validate_ok() {
        let p = VoicePersonality::default();
        assert!(p.validate().is_ok());
    }

    #[test]
    fn validate_speed_out_of_range() {
        let mut p = VoicePersonality::default();
        p.speed = 3.0;
        assert!(p.validate().is_err());
        assert!(p.validate().unwrap_err().contains("speed"));
    }

    #[test]
    fn validate_pitch_out_of_range() {
        let mut p = VoicePersonality::default();
        p.pitch = -1.5;
        assert!(p.validate().is_err());
        assert!(p.validate().unwrap_err().contains("pitch"));
    }

    #[test]
    fn validate_empty_voice_id() {
        let mut p = VoicePersonality::default();
        p.voice_id = "".into();
        assert!(p.validate().is_err());
        assert!(p.validate().unwrap_err().contains("voice_id"));
    }
}
